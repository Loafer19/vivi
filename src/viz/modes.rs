//! Visual modes. Each implements [`Visual`]; add a new one by implementing the trait and
//! listing it in [`all`].

use super::analysis::{Features, BANDS};
use super::canvas::{Canvas, Palette, Rgb};
use std::f32::consts::TAU;

pub trait Visual {
    /// Label shown in the mode dropdown.
    fn name(&self) -> &'static str;
    /// Draw one frame. The canvas keeps its previous contents (for trails), and may
    /// have been resized (then it is cleared to the background first).
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette);
}

/// Registered modes, in dropdown order (after "Disabled").
pub fn all() -> Vec<Box<dyn Visual>> {
    vec![
        Box::new(Spectrometer::default()),
        Box::new(Scope),
        Box::new(Spectrum::default()),
        Box::new(Waveform::default()),
        Box::new(Radial),
        Box::new(Particles::default()),
        Box::new(Pitch),
    ]
}

/// Hue for a pitch class: C = accent hue, then around the colour wheel.
fn pitch_color(p: &Palette, pc: usize, v: f32) -> Rgb {
    let (h, s, val) = p.accent.to_hsv();
    Rgb::hsv(h + pc as f32 / 12.0, s.max(0.55), val.max(0.75) * v)
}

/// Scrolling spectrogram (time → x, frequency → y, intensity → accent).
#[derive(Default)]
struct Spectrometer {
    acc: f32,
}
impl Visual for Spectrometer {
    fn name(&self) -> &'static str {
        "Spectrometer"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette) {
        // Scroll ~2 px per frame.
        self.acc += 2.0;
        let dx = self.acc as usize;
        self.acc -= dx as f32;
        let (w, h) = (c.w, c.h);
        if dx == 0 || w <= dx {
            return;
        }
        for y in 0..h {
            c.px.copy_within(y * w + dx..(y + 1) * w, y * w);
        }
        let hot = p.glow(0.8);
        for y in 0..h {
            let b = ((h - 1 - y) as f32 / h as f32 * BANDS as f32) as usize;
            let v = f.raw[b.min(BANDS - 1)].powf(1.6);
            let col = if v < 0.6 {
                p.bg.lerp(p.accent, v / 0.6)
            } else {
                p.accent.lerp(hot, (v - 0.6) / 0.4)
            };
            for x in w - dx..w {
                c.px[y * w + x] = col;
            }
        }
    }
}

/// Oscilloscope: the time-domain window, triggered on a rising zero crossing.
struct Scope;
impl Visual for Scope {
    fn name(&self) -> &'static str {
        "Scope"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette) {
        c.fade(p.bg, 0.45);
        let wv = &f.wave;
        let span = wv.len() / 2;
        let start = (1..span)
            .find(|&i| wv[i - 1] <= 0.0 && wv[i] > 0.0)
            .unwrap_or(0);
        let (w, h) = (c.w as f32, c.h as f32);
        let mid = h / 2.0;
        let gain = h * 0.42 / f.level_avg.clamp(0.08, 0.6) * 0.3;
        let col = p.glow(f.beat * 0.4);
        let mut prev = None;
        let n = span.min(wv.len() - start);
        let step = (n as f32 / w).max(1.0);
        let mut x = 0.0;
        while x < w {
            let i = start + ((x * step) as usize).min(n - 1);
            let y = (mid - wv[i] * gain).clamp(0.0, h - 1.0);
            if let Some((px, py)) = prev {
                c.line(px, py, x, y, 2.5, col, 1.0);
            }
            prev = Some((x, y));
            x += 2.0;
        }
    }
}

/// Classic spectrum bars with peak caps.
#[derive(Default)]
struct Spectrum {
    peaks: Vec<f32>,
}
impl Visual for Spectrum {
    fn name(&self) -> &'static str {
        "Spectrum"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette) {
        c.clear(p.bg);
        self.peaks.resize(BANDS, 0.0);
        let (w, h) = (c.w as f32, c.h as f32);
        let slot = w / BANDS as f32;
        let gap = (slot * 0.25).max(1.0);
        let base = h * 0.92;
        let top = p.glow(0.35 + f.beat * 0.3);
        for (i, &v) in f.bands.iter().enumerate() {
            let bh = v * h * 0.82;
            let x0 = i as f32 * slot + gap / 2.0;
            c.bar(
                x0,
                x0 + slot - gap,
                base - bh,
                base,
                p.accent.lerp(p.bg, 0.25),
                top,
            );
            // Mirror reflection.
            c.rect(
                x0,
                base + 2.0,
                x0 + slot - gap,
                base + 2.0 + bh * 0.08,
                p.accent,
                0.25,
            );
            let pk = &mut self.peaks[i];
            *pk = if v > *pk { v } else { (*pk - 0.006).max(0.0) };
            let py = base - *pk * h * 0.82 - 4.0;
            c.rect(x0, py, x0 + slot - gap, py + 2.0, top, 0.9);
        }
    }
}

/// Scrolling loudness waveform (mirrored envelope), the newest audio on the right.
#[derive(Default)]
struct Waveform {
    hist: Vec<(f32, f32)>,
}
impl Visual for Waveform {
    fn name(&self) -> &'static str {
        "Waveform"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette) {
        c.clear(p.bg);
        let cols = c.w / 3;
        let peak = f.wave.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        let rms = f.level;
        self.hist.push((peak.min(1.0), rms));
        if self.hist.len() > cols {
            let d = self.hist.len() - cols;
            self.hist.drain(..d);
        }
        let h = c.h as f32;
        let mid = h / 2.0;
        let off = cols - self.hist.len();
        for (i, &(pk, r)) in self.hist.iter().enumerate() {
            let x = ((off + i) * 3) as f32;
            let age = (off + i) as f32 / cols as f32;
            let a = 0.25 + 0.75 * age;
            c.rect(
                x,
                mid - pk * h * 0.45,
                x + 2.0,
                mid + pk * h * 0.45,
                p.accent,
                a * 0.45,
            );
            let g = p.glow(r * 0.6);
            c.rect(x, mid - r * h * 0.45, x + 2.0, mid + r * h * 0.45, g, a);
        }
        c.rect(0.0, mid - 0.5, c.w as f32, mid + 0.5, p.accent, 0.3);
    }
}

/// Circular spectrum around a beat-pulsing core.
struct Radial;
impl Visual for Radial {
    fn name(&self) -> &'static str {
        "Radial"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette) {
        c.fade(p.bg, 0.35);
        let (cx, cy) = (c.w as f32 / 2.0, c.h as f32 / 2.0);
        let m = cx.min(cy);
        let r0 = m * (0.32 + f.beat * 0.06 + f.level * 0.05);
        c.disc(cx, cy, r0 * 0.92, p.accent, 0.12 + f.beat * 0.25);
        let n = BANDS * 2; // mirrored left/right
        let rot = f.time * 0.05;
        for i in 0..n {
            let b = if i < BANDS { i } else { n - 1 - i };
            let v = f.bands[b];
            let ang = rot + i as f32 / n as f32 * TAU - TAU / 4.0;
            let (s, co) = ang.sin_cos();
            let len = v * m * 0.55;
            let col = p.glow(v * 0.5 + f.beat * 0.2);
            c.line(
                cx + co * r0,
                cy + s * r0,
                cx + co * (r0 + len + 1.0),
                cy + s * (r0 + len + 1.0),
                (TAU * r0 / n as f32 * 0.55).clamp(1.0, 6.0),
                col,
                1.0,
            );
        }
    }
}

/// Beat-reactive particles: bursts on onsets, drift speed follows loudness.
#[derive(Default)]
struct Particles {
    ps: Vec<P>,
    seed: u32,
}
struct P {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    life: f32,
    hue: f32,
}
impl Particles {
    fn rnd(&mut self) -> f32 {
        // xorshift: plenty for sparkle.
        let mut s = self.seed.wrapping_add(0x9E37_79B9) | 1;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.seed = s;
        (s >> 8) as f32 / (1u32 << 24) as f32
    }
}
impl Visual for Particles {
    fn name(&self) -> &'static str {
        "Particles"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette) {
        c.fade(p.bg, 0.22);
        let (cx, cy) = (c.w as f32 / 2.0, c.h as f32 / 2.0);
        let m = cx.min(cy);
        let spawn = if f.onset {
            (40.0 + f.level * 160.0) as usize
        } else {
            (f.level * 6.0) as usize
        };
        let bass = f.bands[..8].iter().sum::<f32>() / 8.0;
        for _ in 0..spawn {
            if self.ps.len() >= 1500 {
                break;
            }
            let a = self.rnd() * TAU;
            let sp = (0.6 + self.rnd() * 2.4) * (1.0 + bass * 2.0) * m / 180.0;
            // Stay near the accent; the dominant pitch nudges the hue a little.
            let hue = self.rnd() * 0.08 - 0.04 + (f.pitch as f32 / 12.0 - 0.5) * 0.2 * f.pitch_conf;
            self.ps.push(P {
                x: cx,
                y: cy,
                vx: a.cos() * sp,
                vy: a.sin() * sp,
                life: 1.0,
                hue,
            });
        }
        let speed = 0.6 + f.level * 2.0;
        let (h0, s0, v0) = p.accent.to_hsv();
        let (w, h) = (c.w as f32, c.h as f32);
        self.ps.retain_mut(|q| {
            q.x += q.vx * speed;
            q.y += q.vy * speed;
            q.life -= 0.012;
            q.life > 0.0 && q.x > -4.0 && q.y > -4.0 && q.x < w + 4.0 && q.y < h + 4.0
        });
        for q in &self.ps {
            let col = Rgb::hsv(h0 + q.hue, s0, v0.max(0.7)).lerp(p.glow(1.0), q.life * 0.3);
            c.disc(q.x, q.y, 1.0 + q.life * 1.8, col, q.life);
        }
        c.disc(
            cx,
            cy,
            m * (0.04 + f.beat * 0.08),
            p.glow(0.5),
            0.6 * f.beat + 0.2,
        );
    }
}

/// Pitch wheel: 12 pitch-class petals; the dominant note's hue tints the whole scene.
struct Pitch;
impl Visual for Pitch {
    fn name(&self) -> &'static str {
        "Pitch"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette) {
        let tint = pitch_color(p, f.pitch, 1.0);
        c.fade(p.bg.lerp(tint, 0.08 * f.pitch_conf), 0.3);
        let (cx, cy) = (c.w as f32 / 2.0, c.h as f32 / 2.0);
        let m = cx.min(cy);
        for pc in 0..12 {
            let v = f.chroma[pc];
            let ang = pc as f32 / 12.0 * TAU - TAU / 4.0;
            let (s, co) = ang.sin_cos();
            let r = m * (0.25 + v * 0.5);
            let dom = pc == f.pitch;
            let col = pitch_color(p, pc, 0.45 + 0.55 * v);
            c.line(
                cx,
                cy,
                cx + co * r,
                cy + s * r,
                if dom { 6.0 } else { 3.0 },
                col,
                0.9,
            );
            c.disc(
                cx + co * r,
                cy + s * r,
                3.0 + v * 6.0 + if dom { f.beat * 6.0 } else { 0.0 },
                col,
                1.0,
            );
        }
        c.disc(
            cx,
            cy,
            m * (0.12 + f.level * 0.08 + f.beat * 0.05),
            tint,
            0.25 + 0.6 * f.pitch_conf,
        );
    }
}
