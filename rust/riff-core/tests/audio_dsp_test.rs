use std::fs::File;
use std::path::PathBuf;
use riff_core::player::decoder::AudioDecoder;
use riff_core::player::dsp::{Equalizer3Band, SpectrumAnalyzer};

#[test]
fn test_audio_dsp_and_fft() {
    let mut eq = Equalizer3Band::new(44100.0);
    eq.update(44100.0, 4.0, -2.0, 3.0);

    // Process a dummy 440Hz sine wave sample
    let input = 0.5f32;
    let filtered = eq.process(input);
    assert!(!filtered.is_nan(), "DSP output should not be NaN");

    let mut spectrum = SpectrumAnalyzer::new(512, 16);
    let sine_wave: Vec<f32> = (0..512)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 44100.0).sin())
        .collect();

    let bands = spectrum.compute(&sine_wave);
    assert_eq!(bands.len(), 16, "Should produce exactly 16 frequency bands");
    for &b in &bands {
        assert!(b >= 0.0 && b <= 1.0, "Band value {} should be in [0.0, 1.0]", b);
    }
}

#[test]
fn test_audio_decoder_with_real_file() {
    let cand_path = PathBuf::from(r"C:\Users\TEFA RPL\Downloads\science-and-tech-2025-09-27-10-56-52-utc\Main.mp3");
    if cand_path.is_file() {
        let f = File::open(&cand_path).expect("failed to open test mp3");
        let mut dec = AudioDecoder::new(f, Some("mp3")).expect("failed to create decoder");
        assert!(dec.sample_rate() > 0, "sample rate must be > 0");
        assert!(dec.channels() > 0, "channels must be > 0");

        let pcm = dec.next_packet_samples().expect("packet decode failed");
        assert!(pcm.is_some(), "should decode at least one audio packet");
        let samples = pcm.unwrap();
        assert!(!samples.is_empty(), "samples must not be empty");
    }
}
