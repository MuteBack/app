use std::thread;
use std::time::Duration;

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};

use crate::ducking::{DuckError, Ducker};

pub struct EndpointDucker {
    endpoint: IAudioEndpointVolume,
    original_volume: Option<f32>,
    com_initialized: bool,
}

impl EndpointDucker {
    pub fn new() -> Result<Self, DuckError> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|error| {
                    DuckError::Message(format!("failed to initialize COM: {error}"))
                })?;
        }

        let endpoint = Self::default_endpoint()?;

        Ok(Self {
            endpoint,
            original_volume: None,
            com_initialized: true,
        })
    }

    fn default_endpoint() -> Result<IAudioEndpointVolume, DuckError> {
        let enumerator: IMMDeviceEnumerator = unsafe {
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|error| {
                DuckError::Message(format!("failed to create device enumerator: {error}"))
            })?
        };

        let device = unsafe {
            enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|error| {
                    DuckError::Message(format!("failed to get default render device: {error}"))
                })?
        };

        unsafe {
            device
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                .map_err(|error| {
                    DuckError::Message(format!("failed to activate endpoint volume: {error}"))
                })
        }
    }

    pub fn duck_with_fade(&mut self, level: f32, duration: Duration) -> Result<(), DuckError> {
        if !(0.0..=1.0).contains(&level) {
            return Err(DuckError::InvalidLevel(level));
        }

        let original = self.ensure_original_volume()?;
        let current = self.current_volume()?;
        let target = (original * level).clamp(0.0, 1.0);

        self.fade_to(current, target, duration)
    }

    pub fn restore_with_fade(&mut self, duration: Duration) -> Result<(), DuckError> {
        let Some(original) = self.original_volume else {
            return Ok(());
        };

        let current = self.current_volume()?;
        self.fade_to(current, original, duration)?;
        self.original_volume = None;
        Ok(())
    }

    fn ensure_original_volume(&mut self) -> Result<f32, DuckError> {
        if let Some(original) = self.original_volume {
            return Ok(original);
        }

        let current = self.current_volume()?;
        self.original_volume = Some(current);
        Ok(current)
    }

    fn current_volume(&self) -> Result<f32, DuckError> {
        unsafe {
            self.endpoint.GetMasterVolumeLevelScalar().map_err(|error| {
                DuckError::Message(format!("failed to read current volume: {error}"))
            })
        }
    }

    fn set_volume(&self, volume: f32) -> Result<(), DuckError> {
        unsafe {
            self.endpoint
                .SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0), std::ptr::null())
                .map_err(|error| {
                    DuckError::Message(format!("failed to set endpoint volume: {error}"))
                })
        }
    }

    fn fade_to(&self, from: f32, to: f32, duration: Duration) -> Result<(), DuckError> {
        if duration.is_zero() {
            return self.set_volume(to);
        }

        let steps = (duration.as_millis() / 16).clamp(1, 40) as u32;
        let sleep = duration / steps;

        for step in 1..=steps {
            let progress = step as f32 / steps as f32;
            let eased = progress * progress * (3.0 - 2.0 * progress);
            self.set_volume(from + (to - from) * eased)?;
            thread::sleep(sleep);
        }

        Ok(())
    }
}

impl Ducker for EndpointDucker {
    fn duck(&mut self, level: f32) -> Result<(), DuckError> {
        if !(0.0..=1.0).contains(&level) {
            return Err(DuckError::InvalidLevel(level));
        }

        let original = self.ensure_original_volume()?;
        self.set_volume(original * level)
    }

    fn restore(&mut self) -> Result<(), DuckError> {
        if let Some(original) = self.original_volume.take() {
            unsafe {
                self.endpoint
                    .SetMasterVolumeLevelScalar(original, std::ptr::null())
                    .map_err(|error| {
                        DuckError::Message(format!("failed to restore endpoint volume: {error}"))
                    })?;
            }
        }

        Ok(())
    }

    fn refresh(&mut self) -> Result<(), DuckError> {
        self.endpoint = Self::default_endpoint()?;
        self.original_volume = None;
        Ok(())
    }
}

impl Drop for EndpointDucker {
    fn drop(&mut self) {
        let _ = self.restore();

        if self.com_initialized {
            unsafe {
                CoUninitialize();
            }
        }
    }
}
