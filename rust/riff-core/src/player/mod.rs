pub mod decoder;
pub mod dsp;
pub mod sink;

use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{error, warn};

use crate::events::{EventBus, RiffEvent};
use crate::models::Track;
use decoder::AudioDecoder;
use dsp::{AudioEffects, Equalizer3Band, SpectrumAnalyzer};
use sink::AudioSink;

enum PlayerCommand {
    PlayFile { path: PathBuf, track: Option<Track> },
    PlayBytes { data: Vec<u8>, hint: Option<String>, track: Option<Track> },
    Pause,
    Resume,
    Seek(f64),
    Stop,
    SetVolume(f32),
    SetEq { bass: f32, mid: f32, treble: f32 },
    SetSpeed(f32),
    SetEffects { reverb: f32, distortion: f32 },
}

pub struct AudioPlayer {
    cmd_tx: mpsc::Sender<PlayerCommand>,
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>, // f32 stored as u32 bits
    position: Arc<AtomicU64>, // f64 stored as u64 bits
    duration: Arc<AtomicU64>,
    current_track: Arc<Mutex<Option<Track>>>,
}

impl AudioPlayer {
    pub fn new(events: Arc<EventBus>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<PlayerCommand>();
        let is_playing = Arc::new(AtomicBool::new(false));
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let position = Arc::new(AtomicU64::new(0.0f64.to_bits()));
        let duration = Arc::new(AtomicU64::new(0.0f64.to_bits()));
        let current_track = Arc::new(Mutex::new(None));

        let t_playing = is_playing.clone();
        let t_vol = volume.clone();
        let t_pos = position.clone();
        let t_dur = duration.clone();
        let t_track = current_track.clone();
        let t_events = events;

        // Spawn dedicated audio playback engine thread
        std::thread::spawn(move || {
            let mut sink = match AudioSink::new() {
                Ok(s) => Some(s),
                Err(e) => {
                    warn!("Failed to initialize native audio sink: {}", e);
                    None
                }
            };

            let sink_sample_rate = sink.as_ref().map(|s| s.sample_rate()).unwrap_or(44100) as f32;
            let sink_channels = sink.as_ref().map(|s| s.channels()).unwrap_or(2) as usize;

            let mut eq = Equalizer3Band::new(sink_sample_rate);
            let mut fx = AudioEffects::new(sink_sample_rate);
            let mut spectrum = SpectrumAnalyzer::new(512, 16);
            let mut current_decoder: Option<AudioDecoder> = None;
            let mut current_position = 0.0f64;
            let mut current_duration = 0.0f64;
            let mut is_active = false;
            let mut vol_factor = 1.0f32;
            let mut speed_factor = 1.0f32;

            let mut last_spectrum_emit = Instant::now();
            let mut last_state_emit = Instant::now();
            let mut spectrum_samples_acc = Vec::<f32>::with_capacity(1024);

            loop {
                // Check incoming commands
                while let Ok(cmd) = cmd_rx.try_recv() {
                    match cmd {
                        PlayerCommand::PlayFile { path, track } => {
                            match File::open(&path) {
                                Ok(f) => {
                                    let ext = path
                                        .extension()
                                        .and_then(|s| s.to_str())
                                        .map(str::to_ascii_lowercase);
                                    match AudioDecoder::new(f, ext.as_deref()) {
                                        Ok(dec) => {
                                            let dec_dur = dec.duration_secs();
                                            current_duration = if dec_dur > 0.0 {
                                                dec_dur
                                            } else {
                                                track.as_ref().map(|t| t.duration).unwrap_or(0.0)
                                            };
                                            t_dur.store(current_duration.to_bits(), Ordering::SeqCst);
                                            current_position = 0.0;
                                            t_pos.store(0.0f64.to_bits(), Ordering::SeqCst);
                                            current_decoder = Some(dec);
                                            is_active = true;
                                            t_playing.store(true, Ordering::SeqCst);

                                            if let Some(s) = sink.as_mut() {
                                                s.clear();
                                            }
                                            if let Ok(mut cur) = t_track.lock() {
                                                *cur = track.clone();
                                            }

                                            t_events.emit(RiffEvent::TrackChanged { track });
                                            t_events.emit(RiffEvent::PlaybackState {
                                                is_playing: true,
                                                position_sec: 0.0,
                                                duration_sec: current_duration,
                                            });
                                        }
                                        Err(e) => {
                                            error!("Failed to open decoder for {}: {}", path.display(), e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to open file {}: {}", path.display(), e);
                                }
                            }
                        }
                        PlayerCommand::PlayBytes { data, hint, track } => {
                            let cur = std::io::Cursor::new(data);
                            match AudioDecoder::new(cur, hint.as_deref()) {
                                Ok(dec) => {
                                    let dec_dur = dec.duration_secs();
                                    current_duration = if dec_dur > 0.0 {
                                        dec_dur
                                    } else {
                                        track.as_ref().map(|t| t.duration).unwrap_or(0.0)
                                    };
                                    t_dur.store(current_duration.to_bits(), Ordering::SeqCst);
                                    current_position = 0.0;
                                    t_pos.store(0.0f64.to_bits(), Ordering::SeqCst);
                                    current_decoder = Some(dec);
                                    is_active = true;
                                    t_playing.store(true, Ordering::SeqCst);

                                    if let Some(s) = sink.as_mut() {
                                        s.clear();
                                    }
                                    if let Ok(mut cur) = t_track.lock() {
                                        *cur = track.clone();
                                    }

                                    t_events.emit(RiffEvent::TrackChanged { track });
                                    t_events.emit(RiffEvent::PlaybackState {
                                        is_playing: true,
                                        position_sec: 0.0,
                                        duration_sec: current_duration,
                                    });
                                }
                                Err(e) => {
                                    error!("Failed to decode audio bytes: {}", e);
                                }
                            }
                        }
                        PlayerCommand::Pause => {
                            is_active = false;
                            t_playing.store(false, Ordering::SeqCst);
                            t_events.emit(RiffEvent::PlaybackState {
                                is_playing: false,
                                position_sec: current_position,
                                duration_sec: current_duration,
                            });
                        }
                        PlayerCommand::Resume => {
                            if current_decoder.is_some() {
                                is_active = true;
                                t_playing.store(true, Ordering::SeqCst);
                                t_events.emit(RiffEvent::PlaybackState {
                                    is_playing: true,
                                    position_sec: current_position,
                                    duration_sec: current_duration,
                                });
                            }
                        }
                        PlayerCommand::Seek(pos) => {
                            if let Some(dec) = current_decoder.as_mut() {
                                if let Ok(()) = dec.seek(pos) {
                                    current_position = pos;
                                    t_pos.store(pos.to_bits(), Ordering::SeqCst);
                                    if let Some(s) = sink.as_mut() {
                                        s.clear();
                                    }
                                    t_events.emit(RiffEvent::PlaybackState {
                                        is_playing: is_active,
                                        position_sec: current_position,
                                        duration_sec: current_duration,
                                    });
                                }
                            }
                        }
                        PlayerCommand::Stop => {
                            is_active = false;
                            current_decoder = None;
                            t_playing.store(false, Ordering::SeqCst);
                            if let Some(s) = sink.as_mut() {
                                s.clear();
                            }
                            t_events.emit(RiffEvent::PlaybackState {
                                is_playing: false,
                                position_sec: 0.0,
                                duration_sec: 0.0,
                            });
                        }
                        PlayerCommand::SetVolume(v) => {
                            vol_factor = v.clamp(0.0, 1.0);
                            t_vol.store(vol_factor.to_bits(), Ordering::SeqCst);
                            t_events.emit(RiffEvent::VolumeChanged {
                                volume: vol_factor,
                                muted: vol_factor < 0.001,
                            });
                        }
                        PlayerCommand::SetEq { bass, mid, treble } => {
                            eq.update(sink_sample_rate, bass, mid, treble);
                        }
                        PlayerCommand::SetSpeed(spd) => {
                            speed_factor = spd.clamp(0.5, 2.0);
                        }
                        PlayerCommand::SetEffects { reverb, distortion } => {
                            fx.set_params(reverb, distortion);
                        }
                    }
                }

                if !is_active {
                    std::thread::sleep(Duration::from_millis(15));
                    continue;
                }

                // If sink already has plenty of buffered samples, yield CPU
                if let Some(s) = sink.as_ref() {
                    if s.buffered_samples() > 16384 {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                }

                // Decode next audio packet
                let mut reached_eof = false;
                if let Some(dec) = current_decoder.as_mut() {
                    match dec.next_packet_samples() {
                        Ok(Some(raw_samples)) => {
                            let dec_channels = dec.channels();
                            let dec_sr = dec.sample_rate();

                            let mut processed = Vec::with_capacity(raw_samples.len() * 2);
                            let step = dec_channels.max(1);
                            let frames = raw_samples.len() / step;
                            let resample_ratio = speed_factor * (dec_sr as f32 / sink_sample_rate.max(1.0));

                            if (resample_ratio - 1.0).abs() < 0.005 || frames < 2 {
                                // Process samples through EQ, FX & Volume at 1.0x
                                for chunk in raw_samples.chunks(step) {
                                    let left = chunk.get(0).copied().unwrap_or(0.0);
                                    let right = chunk.get(1).copied().unwrap_or(left);

                                    let eq_l = eq.process(left);
                                    let eq_r = eq.process(right);
                                    let (fx_l, fx_r) = fx.process_stereo(eq_l, eq_r);
                                    let out_l = fx_l * vol_factor;
                                    let out_r = fx_r * vol_factor;

                                    processed.push(out_l);
                                    if sink_channels >= 2 {
                                        processed.push(out_r);
                                    }

                                    spectrum_samples_acc.push((out_l + out_r) * 0.5);
                                }
                            } else {
                                // Linear resampling for sample-rate conversion & speed/pitch adjustment
                                let mut pos = 0.0f32;
                                while (pos as usize) + 1 < frames {
                                    let i0 = pos as usize;
                                    let frac = pos - i0 as f32;
                                    let i1 = i0 + 1;

                                    let l0 = raw_samples[i0 * step];
                                    let l1 = raw_samples[i1 * step];
                                    let r0 = if step > 1 { raw_samples[i0 * step + 1] } else { l0 };
                                    let r1 = if step > 1 { raw_samples[i1 * step + 1] } else { l1 };

                                    let left = l0 + (l1 - l0) * frac;
                                    let right = r0 + (r1 - r0) * frac;

                                    let eq_l = eq.process(left);
                                    let eq_r = eq.process(right);
                                    let (fx_l, fx_r) = fx.process_stereo(eq_l, eq_r);
                                    let out_l = fx_l * vol_factor;
                                    let out_r = fx_r * vol_factor;

                                    processed.push(out_l);
                                    if sink_channels >= 2 {
                                        processed.push(out_r);
                                    }

                                    spectrum_samples_acc.push((out_l + out_r) * 0.5);
                                    pos += resample_ratio;
                                }
                            }

                            // Advance tracked position
                            let dt = frames as f64 / dec_sr as f64;
                            current_position += dt;
                            t_pos.store(current_position.to_bits(), Ordering::SeqCst);

                            if let Some(s) = sink.as_ref() {
                                s.write_samples(&processed);
                            }
                        }
                        Ok(None) => {
                            reached_eof = true;
                        }
                        Err(e) => {
                            warn!("Decoder decode error: {}", e);
                            reached_eof = true;
                        }
                    }
                }

                if reached_eof {
                    is_active = false;
                    t_playing.store(false, Ordering::SeqCst);
                    t_events.emit(RiffEvent::PlaybackState {
                        is_playing: false,
                        position_sec: current_duration,
                        duration_sec: current_duration,
                    });
                }

                // Periodically emit Spectrum event (~30 FPS = ~33ms)
                if last_spectrum_emit.elapsed() >= Duration::from_millis(33) {
                    if !spectrum_samples_acc.is_empty() {
                        let bands = spectrum.compute(&spectrum_samples_acc);
                        t_events.emit(RiffEvent::Spectrum { bands });
                        spectrum_samples_acc.clear();
                    }
                    last_spectrum_emit = Instant::now();
                }

                // Periodically emit PlaybackState event (~250ms)
                if last_state_emit.elapsed() >= Duration::from_millis(250) {
                    t_events.emit(RiffEvent::PlaybackState {
                        is_playing: is_active,
                        position_sec: current_position,
                        duration_sec: current_duration,
                    });
                    last_state_emit = Instant::now();
                }
            }
        });

        Self {
            cmd_tx,
            is_playing,
            volume,
            position,
            duration,
            current_track,
        }
    }

    pub fn play_file(&self, path: PathBuf, track: Option<Track>) {
        let _ = self.cmd_tx.send(PlayerCommand::PlayFile { path, track });
    }

    pub fn play_bytes(&self, data: Vec<u8>, hint: Option<String>, track: Option<Track>) {
        let _ = self.cmd_tx.send(PlayerCommand::PlayBytes { data, hint, track });
    }

    pub fn pause(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Pause);
    }

    pub fn resume(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Resume);
    }

    pub fn seek(&self, pos_sec: f64) {
        let _ = self.cmd_tx.send(PlayerCommand::Seek(pos_sec));
    }

    pub fn stop(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Stop);
    }

    pub fn set_volume(&self, vol: f32) {
        let _ = self.cmd_tx.send(PlayerCommand::SetVolume(vol));
    }

    pub fn set_eq(&self, bass: f32, mid: f32, treble: f32) {
        let _ = self.cmd_tx.send(PlayerCommand::SetEq { bass, mid, treble });
    }

    pub fn set_speed(&self, speed: f32) {
        let _ = self.cmd_tx.send(PlayerCommand::SetSpeed(speed));
    }

    pub fn set_effects(&self, reverb: f32, distortion: f32) {
        let _ = self.cmd_tx.send(PlayerCommand::SetEffects { reverb, distortion });
    }

    pub fn is_playing(&self) -> bool {
        self.is_playing.load(Ordering::SeqCst)
    }

    pub fn get_position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::SeqCst))
    }

    pub fn get_duration(&self) -> f64 {
        f64::from_bits(self.duration.load(Ordering::SeqCst))
    }

    pub fn get_volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::SeqCst))
    }

    pub fn get_current_track(&self) -> Option<Track> {
        self.current_track.lock().ok().and_then(|t| t.clone())
    }
}
