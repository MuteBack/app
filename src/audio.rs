use std::time::Duration;

pub struct MonoFrameAccumulator {
    channels: usize,
    samples_per_frame: usize,
    frame_duration: Duration,
    pending: Vec<i16>,
}

impl MonoFrameAccumulator {
    pub fn new(sample_rate: u32, channels: usize, frame_ms: u32) -> Self {
        let channels = channels.max(1);
        let samples_per_frame = ((sample_rate as usize * frame_ms as usize) / 1000).max(1);
        let frame_duration = Duration::from_secs_f64(samples_per_frame as f64 / sample_rate as f64);

        Self {
            channels,
            samples_per_frame,
            frame_duration,
            pending: Vec::with_capacity(samples_per_frame),
        }
    }

    pub fn frame_duration(&self) -> Duration {
        self.frame_duration
    }

    pub fn push<T, F, G>(&mut self, input: &[T], mut convert: F, mut on_frame: G)
    where
        T: Copy,
        F: FnMut(T) -> i16,
        G: FnMut(&[i16], Duration),
    {
        for chunk in input.chunks_exact(self.channels) {
            let mixed = chunk
                .iter()
                .copied()
                .map(&mut convert)
                .fold(0_i32, |acc, sample| acc + sample as i32)
                / self.channels as i32;

            self.pending.push(mixed as i16);

            if self.pending.len() == self.samples_per_frame {
                on_frame(&self.pending, self.frame_duration);
                self.pending.clear();
            }
        }
    }
}

pub fn f32_to_i16(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    (clamped * i16::MAX as f32) as i16
}

pub fn u16_to_i16(sample: u16) -> i16 {
    (sample as i32 - 32_768) as i16
}

pub struct StreamingLinearResampler {
    source_rate: u32,
    target_rate: u32,
    step: f64,
    position: f64,
    pending: Vec<f32>,
}

impl StreamingLinearResampler {
    pub fn new(source_rate: u32, target_rate: u32) -> Self {
        let source_rate = source_rate.max(1);
        let target_rate = target_rate.max(1);

        Self {
            source_rate,
            target_rate,
            step: source_rate as f64 / target_rate as f64,
            position: 0.0,
            pending: Vec::new(),
        }
    }

    pub fn source_rate(&self) -> u32 {
        self.source_rate
    }

    pub fn target_rate(&self) -> u32 {
        self.target_rate
    }

    pub fn process_i16(&mut self, input: &[i16]) -> Vec<i16> {
        if self.source_rate == self.target_rate {
            return input.to_vec();
        }

        self.pending
            .extend(input.iter().map(|sample| *sample as f32 / i16::MAX as f32));

        if self.pending.len() < 2 {
            return Vec::new();
        }

        let mut output = Vec::new();

        while self.position + 1.0 < self.pending.len() as f64 {
            let index = self.position.floor() as usize;
            let frac = (self.position - index as f64) as f32;
            let left = self.pending[index];
            let right = self.pending[index + 1];
            let sample = left + (right - left) * frac;
            output.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
            self.position += self.step;
        }

        let consumed = self.position.floor() as usize;
        if consumed > 0 {
            self.pending.drain(..consumed);
            self.position -= consumed as f64;
        }

        output
    }
}
