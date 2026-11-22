use anyhow::{anyhow, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use symphonia::default::{get_codecs, get_probe};

pub struct AudioDecoder {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_rate: u32,
    channels: usize,
    duration_secs: f64,
    sample_buf: Option<SampleBuffer<f32>>,
}

impl AudioDecoder {
    pub fn new<R: MediaSource + 'static>(source: R, hint: Option<&str>) -> Result<Self> {
        let mut probe_hint = Hint::new();
        if let Some(ext) = hint {
            probe_hint.with_extension(ext);
        }

        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let format_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let meta_opts = MetadataOptions::default();

        let probed = get_probe()
            .format(&probe_hint, mss, &format_opts, &meta_opts)
            .map_err(|e| anyhow!("Failed to probe audio format: {}", e))?;

        let reader = probed.format;
        let track = reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| anyhow!("No playable audio track found in media"))?
            .clone();

        let track_id = track.id;
        let decoder_opts = DecoderOptions::default();
        let decoder = get_codecs()
            .make(&track.codec_params, &decoder_opts)
            .map_err(|e| anyhow!("Failed to create codec decoder: {}", e))?;

        let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
        let channels = track
            .codec_params
            .channels
            .map(|c| c.count())
            .unwrap_or(2);

        let duration_secs = track
            .codec_params
            .n_frames
            .and_then(|frames| track.codec_params.sample_rate.map(|sr| frames as f64 / sr as f64))
            .unwrap_or(0.0);

        Ok(Self {
            reader,
            decoder,
            track_id,
            sample_rate,
            channels,
            duration_secs,
            sample_buf: None,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_secs
    }

    /// Read next packet and decode into interleaved f32 PCM samples (stereo or mono)
    pub fn next_packet_samples(&mut self) -> Result<Option<Vec<f32>>> {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => {
                    return Err(anyhow!("Packet read error: {}", e));
                }
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();

                    if self.sample_buf.is_none() {
                        self.sample_buf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
                    }

                    if let Some(buf) = self.sample_buf.as_mut() {
                        buf.copy_interleaved_ref(decoded);
                        return Ok(Some(buf.samples().to_vec()));
                    }
                }
                Err(SymphoniaError::DecodeError(_)) => {
                    // Non-fatal decode error, continue to next packet
                    continue;
                }
                Err(e) => return Err(anyhow!("Decode fatal error: {}", e)),
            }
        }
    }

    /// Seek to position in seconds
    pub fn seek(&mut self, pos_sec: f64) -> Result<()> {
        let time = Time::from(pos_sec);
        self.reader.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time,
                track_id: Some(self.track_id),
            },
        )?;
        self.decoder.reset();
        Ok(())
    }
}
