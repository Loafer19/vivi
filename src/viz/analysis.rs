//! Per-frame audio features: log-spaced spectrum (FFT), loudness, onset/beat detection
//! (spectral flux with an adaptive threshold) and a 12-bin chroma for the dominant pitch.

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

pub const BANDS: usize = 64;
const F_LO: f32 = 30.0;
const F_HI: f32 = 16_000.0;

/// What the visual modes get every frame.
pub struct Features {
    /// Smoothed band levels, 0..1 (log frequency, low → high).
    pub bands: [f32; BANDS],
    /// Unsmoothed band levels (for the spectrogram).
    pub raw: [f32; BANDS],
    /// Time-domain window (mono, -1..1), oldest first.
    pub wave: Vec<f32>,
    /// RMS loudness 0..1 (smoothed) and its slow average.
    pub level: f32,
    pub level_avg: f32,
    /// True on the frame an onset/beat is detected; `beat` decays 1 → 0 after it.
    pub onset: bool,
    pub beat: f32,
    /// Energy per pitch class (C, C#, … B), normalised to max 1.
    pub chroma: [f32; 12],
    /// Dominant pitch class (0 = C) and its confidence 0..1.
    pub pitch: usize,
    pub pitch_conf: f32,
    /// Seconds since the analyser started (for animation).
    pub time: f32,
}

pub struct Analyzer {
    n: usize,
    rate: u32,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    prev_mag: Vec<f32>,
    flux_hist: [f32; 43],
    flux_i: usize,
    since_onset: f32,
    pub f: Features,
}

impl Analyzer {
    /// Window length for a sample rate: ~40 ms (2048 @ 44.1/48 kHz).
    pub fn window_len(rate: u32) -> usize {
        ((rate as f32 * 0.04) as usize)
            .next_power_of_two()
            .clamp(512, 8192)
    }

    pub fn new(rate: u32) -> Self {
        let n = Self::window_len(rate);
        let fft = FftPlanner::new().plan_fft_forward(n);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        Self {
            n,
            rate,
            window: (0..n)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
                .collect(),
            buf: vec![Complex::default(); n],
            scratch,
            prev_mag: vec![0.0; n / 2],
            flux_hist: [0.0; 43],
            flux_i: 0,
            since_onset: 1.0,
            fft,
            f: Features {
                bands: [0.0; BANDS],
                raw: [0.0; BANDS],
                wave: vec![0.0; n],
                level: 0.0,
                level_avg: 0.1,
                onset: false,
                beat: 0.0,
                chroma: [0.0; 12],
                pitch: 0,
                pitch_conf: 0.0,
                time: 0.0,
            },
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.n
    }

    /// Decay everything towards silence (paused / no data).
    pub fn idle(&mut self, dt: f32) {
        let k = (-dt * 6.0).exp();
        let f = &mut self.f;
        f.bands.iter_mut().for_each(|b| *b *= k);
        f.raw.iter_mut().for_each(|b| *b *= k);
        f.wave.iter_mut().for_each(|w| *w *= k);
        f.level *= k;
        f.beat *= k;
        f.onset = false;
        f.pitch_conf *= k;
        f.time += dt;
    }

    /// Analyse `samples` (len == `self.len()`), `dt` = seconds since the last frame.
    pub fn process(&mut self, samples: &[f32], dt: f32) {
        let n = self.n;
        let f = &mut self.f;
        f.time += dt;
        f.wave.copy_from_slice(samples);
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        let lvl = (rms * 2.5).min(1.0);
        f.level = if lvl > f.level {
            lvl
        } else {
            f.level + (lvl - f.level) * (dt * 8.0).min(1.0)
        };
        f.level_avg += (f.level - f.level_avg) * (dt * 0.5).min(1.0);

        for ((c, s), w) in self.buf.iter_mut().zip(samples).zip(&self.window) {
            *c = Complex::new(s * w, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);
        let norm = 2.0 / (n as f32 * 0.5);
        let hz = self.rate as f32 / n as f32;

        // Spectral flux (positive magnitude increase) for onsets; chroma from 55 Hz–2 kHz.
        let mut flux = 0.0;
        let mut chroma = [0.0f32; 12];
        for k in 1..n / 2 {
            let m = self.buf[k].norm() * norm;
            let d = m - self.prev_mag[k];
            if d > 0.0 && (k as f32 * hz) < 8000.0 {
                flux += d;
            }
            self.prev_mag[k] = m;
            let fr = k as f32 * hz;
            if (55.0..2000.0).contains(&fr) {
                let note = 12.0 * (fr / 440.0).log2() + 69.0; // MIDI number
                let pc = (note.round() as i64).rem_euclid(12) as usize;
                // Favour bins close to the note centre.
                let w = 1.0 - (note - note.round()).abs() * 1.5;
                chroma[pc] += m * m * w.max(0.0);
            }
        }

        // Log-spaced bands, levels in dB mapped to 0..1 (-60 dB .. 0 dB).
        let ratio = (F_HI / F_LO).powf(1.0 / BANDS as f32);
        for b in 0..BANDS {
            let lo = F_LO * ratio.powi(b as i32);
            let hi = lo * ratio;
            let (k0, k1) = (
                (lo / hz) as usize,
                ((hi / hz) as usize).max((lo / hz) as usize + 1),
            );
            let k1 = k1.min(n / 2);
            let mut peak = 0.0f32;
            for k in k0.max(1)..k1.max(k0.max(1) + 1).min(n / 2) {
                peak = peak.max(self.prev_mag[k]);
            }
            // Gentle tilt so highs are not always flat on music (pink spectrum).
            let tilt = 1.0 + b as f32 / BANDS as f32 * 2.0;
            let db = 20.0 * (peak * tilt + 1e-9).log10();
            let v = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            f.raw[b] = v;
            let s = &mut f.bands[b];
            *s = if v > *s {
                *s + (v - *s) * (dt * 30.0).min(1.0)
            } else {
                *s + (v - *s) * (dt * 6.0).min(1.0)
            };
        }

        // Onset: flux above mean + 1.5σ of the last ~1.4 s, at most every 120 ms.
        self.flux_hist[self.flux_i] = flux;
        self.flux_i = (self.flux_i + 1) % self.flux_hist.len();
        let len = self.flux_hist.len() as f32;
        let mean = self.flux_hist.iter().sum::<f32>() / len;
        let var = self
            .flux_hist
            .iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f32>()
            / len;
        self.since_onset += dt;
        f.onset = flux > mean + 1.5 * var.sqrt() && flux > 0.02 && self.since_onset > 0.12;
        if f.onset {
            self.since_onset = 0.0;
            f.beat = 1.0;
        } else {
            f.beat *= (-dt * 5.0).exp();
        }

        let max = chroma.iter().cloned().fold(0.0, f32::max);
        if max > 0.0 {
            let sum: f32 = chroma.iter().sum();
            let (pc, _) =
                chroma
                    .iter()
                    .enumerate()
                    .fold((0, 0.0), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
            for (c, v) in f.chroma.iter_mut().zip(chroma) {
                *c += (v / max - *c) * (dt * 10.0).min(1.0);
            }
            f.pitch = pc;
            f.pitch_conf = (max / sum * 3.0).min(1.0) * (f.level * 4.0).min(1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_440_is_a() {
        let rate = 48_000;
        let mut an = Analyzer::new(rate);
        let n = an.len();
        let s: Vec<f32> = (0..n)
            .map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin())
            .collect();
        for _ in 0..10 {
            an.process(&s, 0.033);
        }
        assert_eq!(an.f.pitch, 9);
        assert!(an.f.level > 0.3);
        // 440 Hz lands in the lower-middle bands, highs stay quiet.
        let peak = (0..BANDS).max_by(|&a, &b| an.f.bands[a].total_cmp(&an.f.bands[b]));
        assert!((20..40).contains(&peak.unwrap()), "{peak:?}");
    }
}
