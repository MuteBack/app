#[cfg(windows)]
mod windows_runtime {
    use std::error::Error;
    use std::fmt::{Display, Formatter};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{
        FromSample, Sample, SampleFormat, SampleRate, SizedSample, Stream, SupportedStreamConfig,
        SupportedStreamConfigRange, I24,
    };

    use crate::app::{AudioTick, MuteBackApp};
    use crate::audio::{MonoFrameAccumulator, StreamingLinearResampler};
    use crate::config::AppConfig;
    use crate::ducking::{Ducker, NoopDucker};
    use crate::platform::windows::EndpointDucker;
    use crate::session::SessionAction;
    use crate::speaker::{OnnxSpeakerEmbeddingEngine, SpeakerVerifiedVad};
    use crate::vad::{NearFieldVad, ReferenceRejectingVad, SharedReferenceAudio, SileroVadEngine};

    type RuntimeVad = SpeakerVerifiedVad<
        ReferenceRejectingVad<NearFieldVad<SileroVadEngine>>,
        OnnxSpeakerEmbeddingEngine,
    >;

    #[derive(Debug, Clone)]
    pub struct RuntimeInfo {
        pub microphone: String,
        pub input_sample_rate: u32,
        pub input_channels: u16,
        pub input_sample_format: String,
    }

    #[derive(Debug, Clone)]
    pub struct AudioInputDevice {
        pub id: String,
        pub name: String,
        pub is_default: bool,
    }

    #[derive(Debug, Clone)]
    pub enum RuntimeEvent {
        Started(RuntimeInfo),
        Ducked,
        Restored,
        Warning(String),
        Error(String),
        Stopped,
    }

    #[derive(Debug, Clone)]
    pub struct RuntimeError(String);

    impl RuntimeError {
        fn new(message: impl Into<String>) -> Self {
            Self(message.into())
        }
    }

    impl Display for RuntimeError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl Error for RuntimeError {}

    pub fn list_input_devices() -> Result<Vec<AudioInputDevice>, RuntimeError> {
        let host = cpal::default_host();
        let default_name = host
            .default_input_device()
            .and_then(|device| device.name().ok());
        let devices = host
            .input_devices()
            .map_err(|error| RuntimeError::new(format!("failed to list microphones: {error}")))?
            .filter_map(|device| device.name().ok())
            .map(|name| AudioInputDevice {
                id: name.clone(),
                is_default: default_name.as_deref() == Some(name.as_str()),
                name,
            })
            .collect::<Vec<_>>();

        Ok(devices)
    }

    enum RuntimeCommand {
        UpdateConfig(AppConfig),
        Restore,
        Stop,
    }

    pub struct RuntimeHandle {
        command_tx: mpsc::Sender<RuntimeCommand>,
        join: Option<JoinHandle<()>>,
    }

    impl RuntimeHandle {
        pub fn start(
            config: AppConfig,
            event_tx: mpsc::Sender<RuntimeEvent>,
        ) -> Result<Self, RuntimeError> {
            let (command_tx, command_rx) = mpsc::channel();
            let join = thread::Builder::new()
                .name("muteback-runtime".to_string())
                .spawn(move || run(config, command_rx, event_tx))
                .map_err(|error| RuntimeError::new(format!("failed to start runtime: {error}")))?;

            Ok(Self {
                command_tx,
                join: Some(join),
            })
        }

        pub fn update_config(&self, config: AppConfig) -> Result<(), RuntimeError> {
            self.command_tx
                .send(RuntimeCommand::UpdateConfig(config))
                .map_err(|_| RuntimeError::new("runtime is not available"))
        }

        pub fn request_restore(&self) -> Result<(), RuntimeError> {
            self.command_tx
                .send(RuntimeCommand::Restore)
                .map_err(|_| RuntimeError::new("runtime is not available"))
        }

        pub fn stop(&mut self) {
            let _ = self.command_tx.send(RuntimeCommand::Stop);

            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    impl Drop for RuntimeHandle {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn run(
        config: AppConfig,
        command_rx: mpsc::Receiver<RuntimeCommand>,
        event_tx: mpsc::Sender<RuntimeEvent>,
    ) {
        if let Err(error) = run_inner(config, command_rx, &event_tx) {
            let _ = event_tx.send(RuntimeEvent::Error(error.to_string()));
        }

        let _ = event_tx.send(RuntimeEvent::Stopped);
    }

    fn run_inner(
        initial_config: AppConfig,
        command_rx: mpsc::Receiver<RuntimeCommand>,
        event_tx: &mpsc::Sender<RuntimeEvent>,
    ) -> Result<(), Box<dyn Error>> {
        let host = cpal::default_host();
        let input_device = select_input_device(&host, &initial_config, event_tx)?;
        let input_config = select_detector_input_config(&input_device)?;
        let input_sample_rate = input_config.sample_rate().0;
        let detector_sample_rate = 16_000;
        let device_name = input_device
            .name()
            .unwrap_or_else(|_| "Unknown microphone".to_string());

        let (frame_tx, frame_rx) = mpsc::sync_channel::<FrameMessage>(12);
        let (error_tx, error_rx) = mpsc::channel::<String>();
        let stream = build_input_stream(
            input_device,
            input_config.clone(),
            frame_tx,
            error_tx.clone(),
        )?;
        stream.play()?;

        let reference_audio = SharedReferenceAudio::new();
        let reference_stream =
            match build_reference_stream(&host, reference_audio.clone(), error_tx.clone()) {
                Ok(Some(stream)) => match stream.play() {
                    Ok(()) => Some(stream),
                    Err(error) => {
                        let _ = event_tx.send(RuntimeEvent::Warning(format!(
                            "reference audio unavailable; loopback rejection is disabled: {error}"
                        )));
                        None
                    }
                },
                Ok(None) => {
                    let _ = event_tx.send(RuntimeEvent::Warning(
                        "reference audio unavailable; loopback rejection is disabled".to_string(),
                    ));
                    None
                }
                Err(error) => {
                    let _ = event_tx.send(RuntimeEvent::Warning(format!(
                        "reference audio unavailable; loopback rejection is disabled: {error}"
                    )));
                    None
                }
            };

        let silero = SileroVadEngine::new(detector_sample_rate)
            .map_err(|error| RuntimeError::new(format!("failed to start Silero VAD: {error}")))?;
        let near_field = NearFieldVad::new(silero, Duration::from_millis(32));
        let reference_rejecting = ReferenceRejectingVad::new(near_field, reference_audio);
        let speaker_engine = match OnnxSpeakerEmbeddingEngine::new() {
            Ok(engine) => Some(engine),
            Err(error) => {
                let _ = event_tx.send(RuntimeEvent::Warning(format!(
                    "speaker verification unavailable: {error}"
                )));
                None
            }
        };
        let vad = SpeakerVerifiedVad::new(
            reference_rejecting,
            speaker_engine,
            active_speaker_profile(&initial_config),
        );
        let mut resampler = StreamingLinearResampler::new(input_sample_rate, detector_sample_rate);
        let mut detector_frames = MonoFrameAccumulator::new(detector_sample_rate, 1, 32);
        let mut app = MuteBackApp::new(initial_config.clone(), vad, NoopDucker::default());
        let mut config = initial_config;
        let mut ducker = EndpointDucker::new()?;

        let _ = event_tx.send(RuntimeEvent::Started(RuntimeInfo {
            microphone: device_name,
            input_sample_rate: input_config.sample_rate().0,
            input_channels: input_config.channels(),
            input_sample_format: format!("{:?}", input_config.sample_format()),
        }));

        let mut running = true;
        while running {
            running = handle_commands(&command_rx, &mut app, &mut ducker, &mut config, event_tx)?;

            while let Ok(message) = error_rx.try_recv() {
                let _ = event_tx.send(RuntimeEvent::Warning(message));
            }

            match frame_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(frame) => {
                    let resampled = resampler.process_i16(&frame.samples);
                    detector_frames.push(
                        &resampled,
                        |sample| sample,
                        |detector_frame, elapsed| match app.process_audio_frame(
                            detector_frame,
                            AudioTick {
                                elapsed,
                                hotkey_pressed: false,
                                explicit_stop: false,
                            },
                        ) {
                            Ok(update) => {
                                if let Some(action) = update.action {
                                    if let Err(error) =
                                        apply_action(&mut ducker, &config, action, event_tx)
                                    {
                                        let _ = event_tx.send(RuntimeEvent::Error(error));
                                    }
                                }
                            }
                            Err(error) => {
                                let _ = event_tx.send(RuntimeEvent::Error(error.to_string()));
                            }
                        },
                    );
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        ducker.restore()?;
        drop(reference_stream);
        drop(stream);
        Ok(())
    }

    fn handle_commands(
        command_rx: &mpsc::Receiver<RuntimeCommand>,
        app: &mut MuteBackApp<RuntimeVad, NoopDucker>,
        ducker: &mut EndpointDucker,
        config: &mut AppConfig,
        event_tx: &mpsc::Sender<RuntimeEvent>,
    ) -> Result<bool, Box<dyn Error>> {
        while let Ok(command) = command_rx.try_recv() {
            match command {
                RuntimeCommand::UpdateConfig(next_config) => {
                    let enabling_voice_match = active_speaker_profile(config).is_none()
                        && active_speaker_profile(&next_config).is_some();
                    *config = next_config.clone();
                    app.vad_mut()
                        .set_profile(active_speaker_profile(&next_config));
                    app.set_config(next_config);

                    if enabling_voice_match {
                        if let Some(action) = app.force_restore()? {
                            apply_action(ducker, config, action, event_tx)?;
                        }
                    }
                }
                RuntimeCommand::Restore => {
                    if let Some(action) = app.force_restore()? {
                        apply_action(ducker, config, action, event_tx)?;
                    }
                }
                RuntimeCommand::Stop => return Ok(false),
            }
        }

        Ok(true)
    }

    fn apply_action(
        ducker: &mut EndpointDucker,
        config: &AppConfig,
        action: SessionAction,
        event_tx: &mpsc::Sender<RuntimeEvent>,
    ) -> Result<(), String> {
        match action {
            SessionAction::Duck => {
                if config.smooth_ducking {
                    ducker
                        .duck_with_fade(config.normalized_ducking_level(), config.duck_fade)
                        .map_err(|error| error.to_string())?;
                } else {
                    ducker
                        .duck(config.normalized_ducking_level())
                        .map_err(|error| error.to_string())?;
                }
                let _ = event_tx.send(RuntimeEvent::Ducked);
            }
            SessionAction::Restore => {
                if config.smooth_ducking {
                    ducker
                        .restore_with_fade(config.restore_fade)
                        .map_err(|error| error.to_string())?;
                } else {
                    ducker.restore().map_err(|error| error.to_string())?;
                }
                let _ = event_tx.send(RuntimeEvent::Restored);
            }
        }

        Ok(())
    }

    fn active_speaker_profile(config: &AppConfig) -> Option<crate::config::SpeakerProfile> {
        config
            .voice_match_enabled
            .then(|| config.speaker_profile.clone())
            .flatten()
    }

    fn select_input_device(
        host: &cpal::Host,
        config: &AppConfig,
        event_tx: &mpsc::Sender<RuntimeEvent>,
    ) -> Result<cpal::Device, RuntimeError> {
        if let Some(preferred_name) = config
            .microphone_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        {
            match host.input_devices() {
                Ok(devices) => {
                    for device in devices {
                        if device.name().ok().as_deref() == Some(preferred_name) {
                            return Ok(device);
                        }
                    }
                    let _ = event_tx.send(RuntimeEvent::Warning(format!(
                        "selected microphone not found, using default: {preferred_name}"
                    )));
                }
                Err(error) => {
                    let _ = event_tx.send(RuntimeEvent::Warning(format!(
                        "failed to inspect microphones, using default: {error}"
                    )));
                }
            }
        }

        host.default_input_device()
            .ok_or_else(|| RuntimeError::new("no default input device found"))
    }

    fn select_detector_input_config(
        device: &cpal::Device,
    ) -> Result<SupportedStreamConfig, Box<dyn Error>> {
        let preferred_rates = [16_000, 32_000, 48_000, 8_000];
        let mut ranges = device.supported_input_configs()?.collect::<Vec<_>>();

        ranges.sort_by_key(|range| range.channels());

        for rate in preferred_rates {
            if let Some(config) = ranges
                .iter()
                .find_map(|range| choose_rate(range, SampleRate(rate)))
            {
                return Ok(config);
            }
        }

        Ok(device.default_input_config()?)
    }

    fn choose_rate(
        range: &SupportedStreamConfigRange,
        rate: SampleRate,
    ) -> Option<SupportedStreamConfig> {
        if range.min_sample_rate() <= rate && rate <= range.max_sample_rate() {
            Some(range.clone().with_sample_rate(rate))
        } else {
            None
        }
    }

    fn build_reference_stream(
        host: &cpal::Host,
        reference: SharedReferenceAudio,
        error_tx: mpsc::Sender<String>,
    ) -> Result<Option<Stream>, Box<dyn Error>> {
        let Some(output_device) = host.default_output_device() else {
            return Ok(None);
        };

        let output_config = output_device.default_output_config()?;
        build_reference_input_stream(output_device, output_config, reference, error_tx).map(Some)
    }

    #[derive(Debug)]
    struct FrameMessage {
        samples: Vec<i16>,
    }

    fn build_input_stream(
        device: cpal::Device,
        supported_config: SupportedStreamConfig,
        frame_tx: mpsc::SyncSender<FrameMessage>,
        error_tx: mpsc::Sender<String>,
    ) -> Result<Stream, Box<dyn Error>> {
        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;
        let config = supported_config.config();
        let sample_format = supported_config.sample_format();

        let stream = match sample_format {
            SampleFormat::I8 => build_typed_input_stream::<i8>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::I16 => build_typed_input_stream::<i16>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::I24 => build_typed_input_stream::<I24>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::I32 => build_typed_input_stream::<i32>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::I64 => build_typed_input_stream::<i64>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::U8 => build_typed_input_stream::<u8>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::U16 => build_typed_input_stream::<u16>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::U32 => build_typed_input_stream::<u32>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::U64 => build_typed_input_stream::<u64>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::F32 => build_typed_input_stream::<f32>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            SampleFormat::F64 => build_typed_input_stream::<f64>(
                &device,
                &config,
                sample_rate,
                channels,
                frame_tx,
                error_tx,
            )?,
            other => {
                return Err(format!("unsupported sample format: {other:?}").into());
            }
        };

        Ok(stream)
    }

    fn build_reference_input_stream(
        device: cpal::Device,
        supported_config: SupportedStreamConfig,
        reference: SharedReferenceAudio,
        error_tx: mpsc::Sender<String>,
    ) -> Result<Stream, Box<dyn Error>> {
        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;
        let config = supported_config.config();
        let sample_format = supported_config.sample_format();

        let stream = match sample_format {
            SampleFormat::I8 => build_typed_reference_stream::<i8>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::I16 => build_typed_reference_stream::<i16>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::I24 => build_typed_reference_stream::<I24>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::I32 => build_typed_reference_stream::<i32>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::I64 => build_typed_reference_stream::<i64>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::U8 => build_typed_reference_stream::<u8>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::U16 => build_typed_reference_stream::<u16>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::U32 => build_typed_reference_stream::<u32>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::U64 => build_typed_reference_stream::<u64>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::F32 => build_typed_reference_stream::<f32>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            SampleFormat::F64 => build_typed_reference_stream::<f64>(
                &device,
                &config,
                sample_rate,
                channels,
                reference,
                error_tx,
            )?,
            other => {
                return Err(format!("unsupported reference sample format: {other:?}").into());
            }
        };

        Ok(stream)
    }

    fn build_typed_input_stream<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        sample_rate: u32,
        channels: usize,
        frame_tx: mpsc::SyncSender<FrameMessage>,
        error_tx: mpsc::Sender<String>,
    ) -> Result<Stream, cpal::BuildStreamError>
    where
        T: SizedSample,
        i16: FromSample<T>,
    {
        let err_tx = error_tx.clone();
        let mut accumulator = MonoFrameAccumulator::new(sample_rate, channels, 20);

        device.build_input_stream(
            config,
            move |data: &[T], _| {
                accumulator.push(
                    data,
                    |sample| i16::from_sample(sample),
                    |frame, elapsed| {
                        forward_frame(&frame_tx, &error_tx, frame, elapsed);
                    },
                );
            },
            move |error| {
                let _ = err_tx.send(format!("stream error: {error}"));
            },
            None,
        )
    }

    fn build_typed_reference_stream<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        sample_rate: u32,
        channels: usize,
        reference: SharedReferenceAudio,
        error_tx: mpsc::Sender<String>,
    ) -> Result<Stream, cpal::BuildStreamError>
    where
        T: SizedSample,
        i16: FromSample<T>,
    {
        let err_tx = error_tx.clone();
        let mut accumulator = MonoFrameAccumulator::new(sample_rate, channels, 20);

        device.build_input_stream(
            config,
            move |data: &[T], _| {
                accumulator.push(
                    data,
                    |sample| i16::from_sample(sample),
                    |frame, _elapsed| {
                        reference.update_frame(frame);
                    },
                );
            },
            move |error| {
                let _ = err_tx.send(format!("reference audio stream error: {error}"));
            },
            None,
        )
    }

    fn forward_frame(
        frame_tx: &mpsc::SyncSender<FrameMessage>,
        error_tx: &mpsc::Sender<String>,
        frame: &[i16],
        _elapsed: Duration,
    ) {
        // The audio callback should stay responsive, so we drop frames instead of
        // blocking if the processor lags behind.
        if frame_tx
            .try_send(FrameMessage {
                samples: frame.to_vec(),
            })
            .is_err()
        {
            let _ =
                error_tx.send("dropped a microphone frame because processing was busy".to_string());
        }
    }
}

#[cfg(not(windows))]
mod unsupported_runtime {
    use std::error::Error;
    use std::fmt::{Display, Formatter};
    use std::sync::mpsc;

    use crate::config::AppConfig;

    #[derive(Debug, Clone)]
    pub struct RuntimeInfo {
        pub microphone: String,
        pub input_sample_rate: u32,
        pub input_channels: u16,
        pub input_sample_format: String,
    }

    #[derive(Debug, Clone)]
    pub enum RuntimeEvent {
        Started(RuntimeInfo),
        Ducked,
        Restored,
        Warning(String),
        Error(String),
        Stopped,
    }

    #[derive(Debug, Clone)]
    pub struct RuntimeError(String);

    impl Display for RuntimeError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl Error for RuntimeError {}

    pub fn list_input_devices() -> Result<Vec<AudioInputDevice>, RuntimeError> {
        Ok(Vec::new())
    }

    pub struct RuntimeHandle;

    impl RuntimeHandle {
        pub fn start(
            _config: AppConfig,
            _event_tx: mpsc::Sender<RuntimeEvent>,
        ) -> Result<Self, RuntimeError> {
            Err(RuntimeError(
                "background runtime is Windows-only in this MVP".to_string(),
            ))
        }

        pub fn update_config(&self, _config: AppConfig) -> Result<(), RuntimeError> {
            Ok(())
        }

        pub fn request_restore(&self) -> Result<(), RuntimeError> {
            Ok(())
        }

        pub fn stop(&mut self) {}
    }
}

#[cfg(not(windows))]
pub use unsupported_runtime::*;
#[cfg(windows)]
pub use windows_runtime::*;
