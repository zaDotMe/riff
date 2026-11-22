use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleRate, Stream, StreamConfig};

pub struct AudioSink {
    _stream: Stream,
    sample_rate: u32,
    channels: u16,
    buffer: Arc<Mutex<VecDeque<f32>>>,
}

impl AudioSink {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("No default audio output device available"))?;

        let supported_config = device
            .default_output_config()
            .map_err(|e| anyhow!("Failed to query default audio config: {}", e))?;

        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels();

        let config = StreamConfig {
            channels,
            sample_rate: SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let buffer = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(32768)));
        let buf_cb = buffer.clone();

        let err_fn = |err| {
            tracing::warn!("Audio output stream error: {}", err);
        };

        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if let Ok(mut locked) = buf_cb.try_lock() {
                        for sample in data.iter_mut() {
                            *sample = locked.pop_front().unwrap_or(0.0);
                        }
                    } else {
                        // In case of lock contention, output silence to prevent audio glitch
                        for sample in data.iter_mut() {
                            *sample = 0.0;
                        }
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| anyhow!("Failed to build audio output stream: {}", e))?;

        stream.play().map_err(|e| anyhow!("Failed to start audio playback stream: {}", e))?;

        Ok(Self {
            _stream: stream,
            sample_rate,
            channels,
            buffer,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn write_samples(&self, samples: &[f32]) {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.extend(samples.iter().copied());
        }
    }

    pub fn clear(&self) {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }
    }

    pub fn buffered_samples(&self) -> usize {
        self.buffer.lock().map(|b| b.len()).unwrap_or(0)
    }
}
