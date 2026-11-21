//! Visual modes. Each implements [`Visual`]; add a new one by implementing the trait and
//! listing it in [`all`].

use super::analysis::{Features, BANDS};
use super::canvas::{ramp, Canvas, ColorMode, Palette, Rgb};
use std::f32::consts::{PI, TAU};

/// Per-frame drawing options from the plugin settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opts {
    /// Bars across the window (setting *Bar density*).
    pub bars: usize,
    /// Spectrometer scroll in design pixels per frame (setting *Waterfall speed*).
    pub scroll: f32,
    /// Spectrum peak caps as LED blocks (setting *LED peak caps*).
    pub led: bool,
    /// The album cover is shown in this mode's [`Visual::cover`] slot (Slint draws it
    /// sharp on top of the canvas).
    pub cover: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            bars: BANDS,
            scroll: 2.0,
            led: false,
            cover: false,
        }
    }
}

pub trait Visual {
    /// Label shown in the mode dropdown (also the saved / IPC name).
    fn name(&self) -> &'static str;
    /// Draw one frame. The canvas keeps its previous contents (for trails), and may
    /// have been resized (then it is cleared to the background first).
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts);
    /// The mode has a cover slot (the album cover is decoded and shown for it).
    fn shows_cover(&self) -> bool {
        false
    }
    /// Diameter of the centred album-cover slot as a fraction of the window's shorter
    /// side (None: no cover).
    fn cover(&self, _f: &Features) -> Option<f32> {
        None
    }
}

/// Registered modes, in dropdown order (after "Disabled"). Saved choices and IPC use the
/// names; new modes go at the end so indices of the old ones stay put.
pub fn all() -> Vec<Box<dyn Visual>> {
    vec![
        Box::new(Spectrometer::default()),
        Box::new(Scope),
        Box::new(Spectrum::default()),
        Box::new(Envelope::default()),
        Box::new(Radial),
        Box::new(Particles::default()),
        Box::new(Pitch),
        Box::new(Cover),
        Box::new(Mountain::default()),
        Box::new(Classic::default()),
        Box::new(Vu::default()),
        Box::new(Dots),
    ]
}

/// Old mode names still accepted (saved choice, IPC): (old, new).
pub const ALIASES: [(&str, &str); 1] = [("Waveform", "Envelope")];

/// Hue for a pitch class: C = accent hue, then around the colour wheel.
fn pitch_color(p: &Palette, pc: usize, v: f32) -> Rgb {
    let (h, s, val) = p.accent.to_hsv();
    Rgb::hsv(h + pc as f32 / 12.0, s.max(0.55), val.max(0.75) * v)
}

/// `n` levels linearly interpolated from the analyser's bands (low → high).
pub fn resample(b: &[f32; BANDS], n: usize) -> Vec<f32> {
    let n = n.max(2);
    (0..n)
        .map(|i| {
            let x = i as f32 * (BANDS - 1) as f32 / (n - 1) as f32;
            let j = (x as usize).min(BANDS - 2);
            b[j] + (b[j + 1] - b[j]) * (x - j as f32)
        })
        .collect()
}

/// Light smoothing across neighbouring bars (keeps peaks, removes jaggies).
fn smooth3(v: &mut [f32]) {
    if v.len() < 3 {
        return;
    }
    let src = v.to_vec();
    for i in 0..v.len() {
        let l = src[i.saturating_sub(1)];
        let r = src[(i + 1).min(src.len() - 1)];
        v[i] = src[i] * 0.6 + (l + r) * 0.2;
    }
}

/// Catmull-Rom spline through `pts` with `seg` segments per span.
fn spline(pts: &[(f32, f32)], seg: usize) -> Vec<(f32, f32)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut out = Vec::with_capacity(pts.len() * seg + 1);
    for i in 0..pts.len() - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        for s in 0..seg {
            let t = s as f32 / seg as f32;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f32, b: f32, c: f32, d: f32| {
                0.5 * (2.0 * b
                    + (c - a) * t
                    + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
                    + (3.0 * b - a - 3.0 * c + d) * t3)
            };
            out.push((f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1)));
        }
    }
    out.push(pts[pts.len() - 1]);
    out
}

/// Average level of the lowest bands (kick drum, bass).
fn bass(f: &Features) -> f32 {
    f.bands[..8].iter().sum::<f32>() / 8.0
}

/// Scrolling spectrogram (time → x, frequency → y, intensity → a multi-stop colour map).
#[derive(Default)]
struct Spectrometer {
    acc: f32,
    /// Colour of each row in the newest column (new columns blend from it).
    last: Vec<Rgb>,
}
impl Visual for Spectrometer {
    fn name(&self) -> &'static str {
        "Spectrometer"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        self.acc += o.scroll * c.k;
        let dx = self.acc as usize;
        self.acc -= dx as f32;
        let (w, h) = (c.w, c.h);
        if dx == 0 || w <= dx {
            return;
        }
        for y in 0..h {
            c.px.copy_within(y * w + dx..(y + 1) * w, y * w);
        }
        if self.last.len() != h {
            self.last = vec![p.bg; h];
        }
        for y in 0..h {
            // Bands interpolated between rows (no 64 visible steps), raw blended with
            // the smoothed level so it flickers less.
            let ft = (h - 1 - y) as f32 / (h - 1).max(1) as f32;
            let x = ft * (BANDS - 1) as f32;
            let i = (x as usize).min(BANDS - 2);
            let fr = x - i as f32;
            let mix = |a: &[f32; BANDS]| a[i] + (a[i + 1] - a[i]) * fr;
            let v = (mix(&f.raw) * 0.65 + mix(&f.bands) * 0.35)
                .clamp(0.0, 1.0)
                .powf(1.5);
            let base = p.at(ft);
            let stops = [
                p.bg,
                p.bg.lerp(base, 0.4),
                base,
                p.lift(base, 0.45),
                p.lift(base, 0.85),
            ];
            let col = ramp(&stops, v);
            let prev = self.last[y];
            for (n, x) in (w - dx..w).enumerate() {
                c.px[y * w + x] = prev.lerp(col, (n + 1) as f32 / dx as f32);
            }
            self.last[y] = col;
        }
    }
}

/// Oscilloscope: the time-domain window, triggered on a rising zero crossing, with a
/// soft glow. Quiet passages are not blown up to full height.
struct Scope;
impl Visual for Scope {
    fn name(&self) -> &'static str {
        "Scope"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, _o: &Opts) {
        c.fade(p.bg, 0.5);
        let wv = &f.wave;
        let span = wv.len() / 2;
        let start = (1..span)
            .find(|&i| wv[i - 1] <= 0.0 && wv[i] > 0.0)
            .unwrap_or(0);
        let (w, h) = (c.w as f32, c.h as f32);
        let mid = h / 2.0;
        // Milder auto-gain: a higher floor on the running average and a gate that
        // fades the trace out near silence instead of amplifying noise.
        let quiet = (f.level / 0.04).clamp(0.0, 1.0);
        let gain = h * 0.126 / f.level_avg.clamp(0.15, 0.6) * quiet;
        let n = span.min(wv.len() - start);
        let step = (n as f32 / w).max(1.0);
        let dx = 2.0 * c.k;
        let mut pts = Vec::with_capacity((w / dx) as usize + 2);
        let mut x = 0.0;
        while x < w {
            let i = start + ((x * step) as usize).min(n - 1);
            pts.push((x, (mid - wv[i] * gain).clamp(0.0, h - 1.0)));
            x += dx;
        }
        let base = p.at(0.3);
        let core = p.lift(base, 0.25 + f.beat * 0.4);
        c.glow_polyline(&pts, 2.2 * c.k, core, base, 1.0);
    }
}

/// Classic spectrum bars: smoothed, colour by frequency / palette, falling peak caps
/// (thin lines or LED blocks) and a fading mirror.
#[derive(Default)]
struct Spectrum {
    /// Peak cap height and its falling speed per bar.
    peaks: Vec<(f32, f32)>,
}
impl Visual for Spectrum {
    fn name(&self) -> &'static str {
        "Spectrum"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        c.clear(p.bg);
        let mut v = resample(&f.bands, o.bars);
        smooth3(&mut v);
        let n = v.len();
        self.peaks.resize(n, (0.0, 0.0));
        let (w, h, k) = (c.w as f32, c.h as f32, c.k);
        let slot = w / n as f32;
        let gap = (slot * 0.22).max(1.0);
        let base = h * 0.84;
        let span = h * 0.76;
        for (i, &x) in v.iter().enumerate() {
            let t = i as f32 / (n - 1) as f32;
            let col = p.at(t);
            let top = p.lift(col, 0.3 + f.beat * 0.25);
            let bh = x * span;
            let x0 = i as f32 * slot + gap / 2.0;
            let x1 = x0 + slot - gap;
            c.bar(x0, x1, base - bh, base, col.lerp(p.bg, 0.3), top);
            // Mirror: a short reflection fading out downwards.
            let rows = (bh * 0.22).ceil() as usize;
            let y0 = base + 2.0 * k;
            for r in 0..rows {
                let a = 0.3 * (1.0 - r as f32 / rows as f32).powi(2);
                let y = y0 + r as f32;
                c.rect(x0, y, x1, y + 1.0, col, a);
            }
            // Caps hold, then fall faster and faster, so they land with the bars when
            // the music pauses (a linear fall left them floating over empty bars).
            let (pk, fall) = &mut self.peaks[i];
            if x >= *pk {
                (*pk, *fall) = (x, 0.0);
            } else {
                *fall += 0.0008;
                *pk = (*pk - *fall).max(x);
            }
            if o.led {
                let ch = 4.0 * k;
                let py = base - *pk * span - ch - 2.0 * k;
                let led = p.lift(col, 0.55);
                c.rect(x0 - k, py - k, x1 + k, py + ch + k, led, 0.15);
                c.rect(x0, py, x1, py + ch, led, 0.95);
            } else {
                let py = base - *pk * span - 4.0 * k;
                c.rect(x0, py, x1, py + 2.0 * k, top, 0.9);
            }
        }
    }
}

/// Scrolling loudness envelope (mirrored peak and RMS), the newest audio on the right.
/// Called "Waveform" before 0.17.
#[derive(Default)]
struct Envelope {
    hist: Vec<(f32, f32)>,
}
impl Visual for Envelope {
    fn name(&self) -> &'static str {
        "Envelope"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, _o: &Opts) {
        c.clear(p.bg);
        let k = c.k;
        let colw = 3.0 * k;
        let cols = ((c.w as f32 / colw) as usize).max(1);
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
            let x = (off + i) as f32 * colw;
            let age = (off + i) as f32 / cols as f32;
            let a = 0.25 + 0.75 * age;
            let col = p.at(pk);
            c.rect(
                x,
                mid - pk * h * 0.45,
                x + colw - k,
                mid + pk * h * 0.45,
                col,
                a * 0.45,
            );
            let g = p.lift(col, r * 0.6);
            c.rect(
                x,
                mid - r * h * 0.45,
                x + colw - k,
                mid + r * h * 0.45,
                g,
                a,
            );
        }
        c.rect(0.0, mid - 0.5 * k, c.w as f32, mid + 0.5 * k, p.accent, 0.3);
    }
}

/// Core size of [`Radial`] (diameter / shorter side).
fn radial_core(f: &Features) -> f32 {
    0.3 + f.beat * 0.05 + f.level * 0.04
}

/// Circular spectrum around a beat-pulsing core: the album cover when there is one.
struct Radial;
impl Visual for Radial {
    fn name(&self) -> &'static str {
        "Radial"
    }
    fn shows_cover(&self) -> bool {
        true
    }
    fn cover(&self, f: &Features) -> Option<f32> {
        Some(radial_core(f))
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        c.fade(p.bg, 0.35);
        let k = c.k;
        let (cx, cy) = (c.w as f32 / 2.0, c.h as f32 / 2.0);
        let m = cx.min(cy);
        let r0 = if o.cover {
            let r = m * radial_core(f);
            c.ring(
                cx,
                cy,
                r + 3.0 * k,
                2.5 * k,
                p.lift(p.accent, 0.2),
                0.3 + f.beat * 0.5,
            );
            r + 6.0 * k
        } else {
            let r = m * (0.32 + f.beat * 0.06 + f.level * 0.05);
            c.disc(cx, cy, r * 0.92, p.accent, 0.12 + f.beat * 0.25);
            r
        };
        let n = BANDS * 2; // mirrored left/right
        let rot = f.time * 0.05;
        for i in 0..n {
            let b = if i < BANDS { i } else { n - 1 - i };
            let v = f.bands[b];
            let ang = rot + i as f32 / n as f32 * TAU - TAU / 4.0;
            let (s, co) = ang.sin_cos();
            let len = v * m * 0.55;
            let col = p.lift(p.at(b as f32 / (BANDS - 1) as f32), v * 0.5 + f.beat * 0.2);
            c.line(
                cx + co * r0,
                cy + s * r0,
                cx + co * (r0 + len + 1.0),
                cy + s * (r0 + len + 1.0),
                (TAU * r0 / n as f32 * 0.55).clamp(1.0, 6.0 * k),
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
    /// Position on the palette (Album art / Frequency colours).
    t: f32,
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
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, _o: &Opts) {
        c.fade(p.bg, 0.22);
        let k = c.k;
        let (cx, cy) = (c.w as f32 / 2.0, c.h as f32 / 2.0);
        let m = cx.min(cy);
        let spawn = if f.onset {
            (40.0 + f.level * 160.0) as usize
        } else {
            (f.level * 6.0) as usize
        };
        let bass = bass(f);
        for _ in 0..spawn {
            if self.ps.len() >= 1500 {
                break;
            }
            let a = self.rnd() * TAU;
            let sp = (0.6 + self.rnd() * 2.4) * (1.0 + bass * 2.0) * m / 180.0;
            // Stay near the accent; the dominant pitch nudges the hue a little.
            let hue = self.rnd() * 0.08 - 0.04 + (f.pitch as f32 / 12.0 - 0.5) * 0.2 * f.pitch_conf;
            let t = self.rnd();
            self.ps.push(P {
                x: cx,
                y: cy,
                vx: a.cos() * sp,
                vy: a.sin() * sp,
                life: 1.0,
                hue,
                t,
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
            let base = if p.mode == ColorMode::Accent {
                Rgb::hsv(h0 + q.hue, s0, v0.max(0.7))
            } else {
                p.at(q.t)
            };
            let col = base.lerp(p.glow(1.0), q.life * 0.3);
            c.disc(q.x, q.y, (1.0 + q.life * 1.8) * k, col, q.life);
        }
        c.glow_disc(
            cx,
            cy,
            m * (0.04 + f.beat * 0.08),
            p.glow(0.5),
            0.6 * f.beat + 0.2,
        );
    }
}

/// Pitch flower: 12 pitch-class petals; the dominant note's hue tints the whole scene.
struct Pitch;
impl Visual for Pitch {
    fn name(&self) -> &'static str {
        "Pitch"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, _o: &Opts) {
        let tint = pitch_color(p, f.pitch, 1.0);
        c.fade(p.bg.lerp(tint, 0.08 * f.pitch_conf), 0.3);
        let k = c.k;
        let (cx, cy) = (c.w as f32 / 2.0, c.h as f32 / 2.0);
        let m = cx.min(cy);
        let inner = m * 0.1;
        for pc in 0..12 {
            let v = f.chroma[pc];
            let ang = pc as f32 / 12.0 * TAU - TAU / 4.0;
            let (s, co) = ang.sin_cos();
            let len = m * (0.14 + v * 0.42);
            let wid = (len * 0.36).max(3.0 * k);
            let d = inner + len / 2.0;
            let (px, py) = (cx + co * d, cy + s * d);
            let dom = pc == f.pitch;
            let col = pitch_color(p, pc, 0.45 + 0.55 * v);
            if dom {
                let halo = 1.2 + f.beat * 0.25;
                c.ellipse(px, py, len / 2.0 * halo, wid / 2.0 * halo, ang, col, 0.2);
            }
            c.ellipse(px, py, len / 2.0, wid / 2.0, ang, col, 0.5 + 0.45 * v);
            // A bright vein down the middle.
            c.line(
                cx + co * inner,
                cy + s * inner,
                cx + co * (inner + len * 0.9),
                cy + s * (inner + len * 0.9),
                1.2 * k,
                p.lift(col, 0.4),
                0.35 + 0.4 * v,
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

/// Core size of [`Cover`] (diameter / shorter side): pulses on bass and beats.
fn cover_core(f: &Features) -> f32 {
    0.46 + bass(f) * 0.07 + f.beat * 0.035
}

/// The album cover, large and centred, pulsing on the bass, with bars all around.
/// Without a cover a vinyl-like disc takes its place.
struct Cover;
impl Visual for Cover {
    fn name(&self) -> &'static str {
        "Cover"
    }
    fn shows_cover(&self) -> bool {
        true
    }
    fn cover(&self, f: &Features) -> Option<f32> {
        Some(cover_core(f))
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        let k = c.k;
        c.fade(p.bg.lerp(p.at(0.2), 0.03), 0.3);
        let (cx, cy) = (c.w as f32 / 2.0, c.h as f32 / 2.0);
        let m = cx.min(cy);
        let r = m * cover_core(f);
        // Soft halo behind the cover.
        c.disc(cx, cy, r * 1.22, p.at(0.15), 0.05 + f.beat * 0.08);
        if !o.cover {
            c.disc(cx, cy, r, p.lift(p.bg, 0.08), 1.0);
            for g in 1..4 {
                c.ring(
                    cx,
                    cy,
                    r * (0.45 + g as f32 * 0.14),
                    k,
                    p.lift(p.bg, 0.16),
                    0.6,
                );
            }
            c.disc(cx, cy, r * 0.3, p.at(0.2), 0.9);
            c.disc(cx, cy, r * 0.05, p.bg, 1.0);
        }
        c.ring(
            cx,
            cy,
            r + 2.0 * k,
            1.5 * k,
            p.lift(p.accent, 0.3),
            0.5 + f.beat * 0.4,
        );
        let half = (o.bars / 2).clamp(24, 64);
        let v = resample(&f.bands, half);
        let n = half * 2;
        let r0 = r + 6.0 * k;
        let wid = (TAU * r0 / n as f32 * 0.5).clamp(1.5 * k, 6.0 * k);
        for i in 0..n {
            // Bass at the top, mirrored down both sides.
            let b = if i < half { i } else { n - 1 - i };
            let lv = v[b];
            let ang = -TAU / 4.0 + (i as f32 + 0.5) / n as f32 * TAU;
            let (s, co) = ang.sin_cos();
            let len = 2.0 * k + lv * m * 0.42;
            let col = p.lift(p.at(b as f32 / (half - 1) as f32), lv * 0.4 + f.beat * 0.15);
            c.line(
                cx + co * r0,
                cy + s * r0,
                cx + co * (r0 + len),
                cy + s * (r0 + len),
                wid,
                col,
                0.95,
            );
        }
    }
}

/// Smooth filled spectrum: a gradient mountain with a glowing ridge and a slower,
/// fainter range behind it.
#[derive(Default)]
struct Mountain {
    back: Vec<f32>,
}
impl Visual for Mountain {
    fn name(&self) -> &'static str {
        "Mountain"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, _o: &Opts) {
        c.clear(p.bg);
        let k = c.k;
        let (w, h) = (c.w as f32, c.h as f32);
        let base = h * 0.92;
        let span = h * 0.74;
        let mut v = f.bands.to_vec();
        smooth3(&mut v);
        self.back.resize(BANDS, 0.0);
        for (b, &x) in self.back.iter_mut().zip(&v) {
            *b = x.max(*b - 0.006);
        }
        let pts = |lv: &[f32]| {
            let n = lv.len();
            let raw: Vec<(f32, f32)> = lv
                .iter()
                .enumerate()
                .map(|(i, &x)| (i as f32 / (n - 1) as f32 * w, base - x * span))
                .collect();
            spline(&raw, 4)
        };
        let back = pts(&self.back);
        c.area(&back, base, span, 0.22, |t| p.lift(p.at(t), 0.1));
        let front = pts(&v);
        c.area(&front, base, span, 0.9, |t| {
            p.lift(p.at(t), t * 0.25).lerp(p.bg, (1.0 - t) * 0.55)
        });
        let ridge = p.lift(p.at(0.6), 0.35 + f.beat * 0.3);
        c.glow_polyline(&front, 2.0 * k, ridge, p.at(0.5), 1.0);
        c.rect(0.0, base, w, h, p.at(0.1), 0.08);
    }
}

/// Winamp-style LED bars: segmented columns, green → yellow → red, with peak holds.
#[derive(Default)]
struct Classic {
    /// Peak segment (fractional) and hold frames left, per bar.
    peaks: Vec<(f32, u32)>,
}
impl Visual for Classic {
    fn name(&self) -> &'static str {
        "Classic"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        c.clear(p.bg);
        let k = c.k;
        let n = (o.bars / 2).clamp(16, 48);
        let v = resample(&f.bands, n);
        self.peaks.resize(n, (0.0, 0));
        let (w, h) = (c.w as f32, c.h as f32);
        let seg = 6.0 * k;
        let pitch = 8.0 * k;
        let base = h * 0.9;
        let rows = ((h * 0.8 / pitch) as usize).max(4);
        let slot = w / n as f32;
        let gap = (slot * 0.2).max(2.0 * k);
        let classic = [
            Rgb(0.15, 0.85, 0.3),
            Rgb(0.95, 0.85, 0.12),
            Rgb(0.95, 0.22, 0.15),
        ];
        let color = |i: usize, r: usize| {
            let t = r as f32 / (rows - 1) as f32;
            match p.mode {
                ColorMode::Accent => ramp(&classic, t),
                ColorMode::Art => p.at(t),
                ColorMode::Frequency => p.at(i as f32 / (n - 1) as f32),
            }
        };
        for (i, &x) in v.iter().enumerate() {
            let lit = (x * rows as f32).round() as usize;
            let x0 = i as f32 * slot + gap / 2.0;
            let x1 = x0 + slot - gap;
            for r in 0..rows {
                let y1 = base - r as f32 * pitch;
                c.rect(
                    x0,
                    y1 - seg,
                    x1,
                    y1,
                    color(i, r),
                    if r < lit { 1.0 } else { 0.07 },
                );
            }
            let (pk, hold) = &mut self.peaks[i];
            if lit as f32 >= *pk {
                (*pk, *hold) = (lit as f32, 12);
            } else if *hold > 0 {
                *hold -= 1;
            } else {
                *pk = (*pk - 0.35).max(lit as f32);
            }
            let r = (pk.round() as usize).min(rows - 1);
            if r > 0 {
                let y1 = base - r as f32 * pitch;
                c.rect(x0, y1 - seg, x1, y1, p.lift(color(i, r), 0.5), 0.95);
            }
        }
    }
}

/// Retro analogue VU meter: a needle with VU ballistics (~300 ms) on a -20..+3 dB arc
/// with a red zone, and a peak lamp. Mono (the tap measures a mono downmix).
#[derive(Default)]
struct Vu {
    needle: f32,
    lamp: f32,
    t: f32,
}
impl Visual for Vu {
    fn name(&self) -> &'static str {
        "VU Meter"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, _o: &Opts) {
        c.clear(p.bg);
        let k = c.k;
        let dt = (f.time - self.t).clamp(0.0, 0.2);
        self.t = f.time;
        // -20 dB .. +3 dB over the scale; level 1.0 (full scale) sits at 0 VU.
        let db = 20.0 * f.level.max(1e-4).log10();
        let pos = ((db + 20.0) / 23.0).clamp(0.0, 1.0);
        let red_from = 20.0 / 23.0;
        self.needle += (pos - self.needle) * (1.0 - (-dt / 0.3).exp());
        self.lamp = if pos > red_from {
            1.0
        } else {
            (self.lamp - dt * 2.0).max(0.0)
        };
        let (w, h) = (c.w as f32, c.h as f32);
        let face = (w * 0.7).min(h * 1.3);
        let (cx, py) = (w / 2.0, h / 2.0 + face * 0.3);
        let rr = face * 0.55;
        let sweep = 1.75; // radians across the scale
        let ang = |t: f32| -PI / 2.0 + (t - 0.5) * sweep;
        let ink = p.lift(p.bg, 0.6);
        let red = if p.mode == ColorMode::Accent {
            Rgb(0.9, 0.25, 0.2)
        } else {
            p.at(1.0)
        };
        // Scale arc (red zone thicker) and ticks.
        let arc = |a: f32, b: f32| -> Vec<(f32, f32)> {
            (0..=24)
                .map(|i| {
                    let t = a + (b - a) * i as f32 / 24.0;
                    let (s, co) = ang(t).sin_cos();
                    (cx + co * rr * 0.86, py + s * rr * 0.86)
                })
                .collect()
        };
        c.polyline(&arc(0.0, red_from), 2.0 * k, ink, 0.8);
        c.polyline(&arc(red_from, 1.0), 5.0 * k, red, 0.9);
        for i in 0..=23 {
            let t = i as f32 / 23.0;
            let major = i % 5 == 0 || i == 20;
            let (s, co) = ang(t).sin_cos();
            let (r1, r2) = (rr * 0.88, rr * if major { 0.99 } else { 0.94 });
            let col = if t > red_from + 1e-3 { red } else { ink };
            c.line(
                cx + co * r1,
                py + s * r1,
                cx + co * r2,
                py + s * r2,
                if major { 2.4 * k } else { 1.2 * k },
                col,
                0.9,
            );
        }
        // Needle with a soft shadow, pivot cap.
        let (s, co) = ang(self.needle).sin_cos();
        let (nx, ny) = (cx + co * rr * 1.02, py + s * rr * 1.02);
        c.line(
            cx + 2.0 * k,
            py + 3.0 * k,
            nx + 2.0 * k,
            ny + 3.0 * k,
            4.0 * k,
            p.bg.lerp(Rgb(0.0, 0.0, 0.0), 0.5),
            0.35,
        );
        c.line(cx, py, nx, ny, 2.6 * k, p.lift(p.accent, 0.15), 1.0);
        c.glow_disc(cx, py, 9.0 * k, p.accent, 1.0);
        // Peak lamp.
        let (lx, ly) = (cx + rr * 0.62, py - rr * 0.36);
        if self.lamp > 0.0 {
            c.glow_disc(lx, ly, 6.0 * k, red, self.lamp);
        }
        c.ring(lx, ly, 6.0 * k, 1.2 * k, ink, 0.6);
    }
}

/// Minimal: a quiet row of dots that swell with their band.
struct Dots;
impl Visual for Dots {
    fn name(&self) -> &'static str {
        "Dots"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        c.fade(p.bg, 0.35);
        let k = c.k;
        let n = (o.bars / 3).clamp(12, 40);
        let v = resample(&f.bands, n);
        let (w, h) = (c.w as f32, c.h as f32);
        let slot = w / n as f32;
        let cy = h / 2.0;
        let rmax = slot * 0.38;
        let pulse = 1.0 + f.beat * 0.08;
        for (i, &x) in v.iter().enumerate() {
            let cx = (i as f32 + 0.5) * slot;
            let t = i as f32 / (n - 1) as f32;
            c.disc(cx, cy, 1.2 * k, p.lift(p.bg, 0.25), 0.6);
            let r = (rmax * x.powf(1.3) * pulse).max(1.2 * k);
            c.disc(cx, cy, r, p.at(t), 0.25 + 0.75 * x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_the_ends_and_interpolates() {
        let mut b = [0.0; BANDS];
        for (i, v) in b.iter_mut().enumerate() {
            *v = i as f32 / (BANDS - 1) as f32;
        }
        for n in [16, 32, 64, 96] {
            let r = resample(&b, n);
            assert_eq!(r.len(), n);
            assert!(r[0].abs() < 1e-6 && (r[n - 1] - 1.0).abs() < 1e-5);
            assert!(r.windows(2).all(|w| w[1] >= w[0]));
        }
    }

    #[test]
    fn names_are_unique_and_aliases_resolve() {
        let names: Vec<_> = all().iter().map(|m| m.name()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        // Old indices stay put (saved index / dropdown order).
        assert_eq!(
            &names[..7],
            &[
                "Spectrometer",
                "Scope",
                "Spectrum",
                "Envelope",
                "Radial",
                "Particles",
                "Pitch"
            ]
        );
        for (_, new) in ALIASES {
            assert!(names.contains(&new));
        }
    }

    /// Every mode draws on silence, on a loud frame and after a resize without
    /// panicking, in every colour mode.
    #[test]
    fn every_mode_draws() {
        let mut f = super::super::analysis::Analyzer::new(48_000).f;
        let modes = [ColorMode::Accent, ColorMode::Art, ColorMode::Frequency];
        for cm in modes {
            let mut p = Palette::plain(Rgb(0.9, 0.5, 0.1), Rgb(0.05, 0.05, 0.05), true);
            p.mode = cm;
            p.stops[1] = Rgb(0.1, 0.6, 0.9);
            p.n = 2;
            for mut m in all() {
                for (w, h, loud) in [(320, 180, false), (320, 180, true), (97, 211, true)] {
                    let mut c = Canvas::new(w, h);
                    c.clear(p.bg);
                    for b in f.bands.iter_mut().chain(f.raw.iter_mut()) {
                        *b = if loud { 0.9 } else { 0.0 };
                    }
                    f.level = if loud { 0.8 } else { 0.0 };
                    f.beat = if loud { 1.0 } else { 0.0 };
                    f.onset = loud;
                    f.time += 0.033;
                    for cover in [false, true] {
                        let o = Opts {
                            cover,
                            led: loud,
                            ..Opts::default()
                        };
                        m.draw(&mut c, &f, &p, &o);
                    }
                    assert!(c
                        .px
                        .iter()
                        .all(|q| q.0.is_finite() && q.1.is_finite() && q.2.is_finite()));
                }
            }
        }
    }
}
