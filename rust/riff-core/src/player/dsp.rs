use std::f32::consts::PI;

/// Direct Form II Transposed Biquad Filter
#[derive(Debug, Clone)]
pub struct BiquadFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl BiquadFilter {
    pub fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Peaking EQ filter with gain in dB, frequency in Hz, and Q factor
    pub fn set_peaking_eq(&mut self, sample_rate: f32, freq: f32, gain_db: f32, q: f32) {
        if gain_db.abs() < 0.01 {
            self.b0 = 1.0;
            self.b1 = 0.0;
            self.b2 = 0.0;
            self.a1 = 0.0;
            self.a2 = 0.0;
            return;
        }

        let a = 10.0_f32.powf(gain_db / 40.0);
        let omega = 2.0 * PI * freq / sample_rate;
        let alpha = omega.sin() / (2.0 * q.max(0.1));
        let cos_w = omega.cos();

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha / a;

        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let out = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * out + self.z2;
        self.z2 = self.b2 * input - self.a2 * out;
        out
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// 3-Band Equalizer (Bass, Mid, Treble)
#[derive(Debug, Clone)]
pub struct Equalizer3Band {
    bass: BiquadFilter,
    mid: BiquadFilter,
    treble: BiquadFilter,
}

impl Equalizer3Band {
    pub fn new(sample_rate: f32) -> Self {
        let mut bass = BiquadFilter::new();
        let mut mid = BiquadFilter::new();
        let mut treble = BiquadFilter::new();

        bass.set_peaking_eq(sample_rate, 100.0, 0.0, 0.7);
        mid.set_peaking_eq(sample_rate, 1000.0, 0.0, 0.7);
        treble.set_peaking_eq(sample_rate, 10000.0, 0.0, 0.7);

        Self { bass, mid, treble }
    }

    pub fn update(&mut self, sample_rate: f32, bass_db: f32, mid_db: f32, treble_db: f32) {
        self.bass.set_peaking_eq(sample_rate, 100.0, bass_db, 0.7);
        self.mid.set_peaking_eq(sample_rate, 1000.0, mid_db, 0.7);
        self.treble.set_peaking_eq(sample_rate, 10000.0, treble_db, 0.7);
    }

    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        let s = self.bass.process(sample);
        let s = self.mid.process(s);
        self.treble.process(s)
    }

    pub fn reset(&mut self) {
        self.bass.reset();
        self.mid.reset();
        self.treble.reset();
    }
}

/// Standalone, pure-Rust Radix-2 Cooley-Tukey FFT for real-time spectrum visualization
pub struct SpectrumAnalyzer {
    fft_size: usize,
    window: Vec<f32>,
    real: Vec<f32>,
    imag: Vec<f32>,
    output_bands: usize,
}

impl SpectrumAnalyzer {
    pub fn new(fft_size: usize, output_bands: usize) -> Self {
        assert!(fft_size.is_power_of_two(), "FFT size must be power of two");
        // Hann window pre-calculation
        let window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / (fft_size as f32 - 1.0)).cos()))
            .collect();

        Self {
            fft_size,
            window,
            real: vec![0.0; fft_size],
            imag: vec![0.0; fft_size],
            output_bands,
        }
    }

    /// Compute frequency spectrum bands normalized between 0.0 and 1.0
    pub fn compute(&mut self, samples: &[f32]) -> Vec<f32> {
        let n = self.fft_size;
        let take_len = samples.len().min(n);

        for i in 0..take_len {
            self.real[i] = samples[i] * self.window[i];
            self.imag[i] = 0.0;
        }
        for i in take_len..n {
            self.real[i] = 0.0;
            self.imag[i] = 0.0;
        }

        // Bit-reversal permutation
        let mut j = 0;
        for i in 0..(n - 1) {
            if i < j {
                self.real.swap(i, j);
                self.imag.swap(i, j);
            }
            let mut k = n >> 1;
            while k <= j {
                j -= k;
                k >>= 1;
            }
            j += k;
        }

        // Cooley-Tukey Radix-2 FFT computation
        let mut len = 2;
        while len <= n {
            let half = len >> 1;
            let angle = -2.0 * PI / len as f32;
            let w_step_r = angle.cos();
            let w_step_i = angle.sin();

            let mut i = 0;
            while i < n {
                let mut w_r = 1.0;
                let mut w_i = 0.0;

                for k in 0..half {
                    let u_r = self.real[i + k];
                    let u_i = self.imag[i + k];

                    let v_r = self.real[i + k + half] * w_r - self.imag[i + k + half] * w_i;
                    let v_i = self.real[i + k + half] * w_i + self.imag[i + k + half] * w_r;

                    self.real[i + k] = u_r + v_r;
                    self.imag[i + k] = u_i + v_i;
                    self.real[i + k + half] = u_r - v_r;
                    self.imag[i + k + half] = u_i - v_i;

                    let next_w_r = w_r * w_step_r - w_i * w_step_i;
                    let next_w_i = w_r * w_step_i + w_i * w_step_r;
                    w_r = next_w_r;
                    w_i = next_w_i;
                }
                i += len;
            }
            len <<= 1;
        }

        // Calculate magnitudes for positive frequencies (first N/2 bins)
        let half_n = n / 2;
        let mut magnitudes = vec![0.0f32; half_n];
        for i in 0..half_n {
            let r = self.real[i];
            let im = self.imag[i];
            magnitudes[i] = (r * r + im * im).sqrt() / (n as f32);
        }

        // Group bins logarithmically into output_bands
        let mut bands = vec![0.0f32; self.output_bands];
        let num_bands = self.output_bands;

        for b in 0..num_bands {
            let start_ratio = (b as f32 / num_bands as f32).powf(2.0);
            let end_ratio = ((b + 1) as f32 / num_bands as f32).powf(2.0);

            let start_idx = (start_ratio * half_n as f32).floor() as usize;
            let end_idx = ((end_ratio * half_n as f32).ceil() as usize).max(start_idx + 1).min(half_n);

            let mut sum = 0.0;
            let mut count = 0;
            for idx in start_idx..end_idx {
                sum += magnitudes[idx];
                count += 1;
            }
            let avg = if count > 0 { sum / count as f32 } else { 0.0 };
            // Scale and soft compress for visually pleasant bars
            let scaled = (avg * 8.0).clamp(0.0, 1.0);
            bands[b] = scaled;
        }

        bands
    }
}
