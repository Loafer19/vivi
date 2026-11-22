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
    /// Spectrum peak caps as LED blocks (setting *LED peak caps*, Bars style only).
    pub led: bool,
    /// The album cover is shown in this mode's [`Visual::cover`] slot (Slint draws it
    /// sharp on top of the canvas).
    pub cover: bool,
    /// Spectrum visual style (Bars / LED / Mountain / Dots).
    pub spectrum: SpectrumStyle,
    /// Radial visual style (Rays / Cover).
    pub radial: RadialStyle,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            bars: BANDS,
            scroll: 2.0,
            led: false,
            cover: false,
            spectrum: SpectrumStyle::Bars,
            radial: RadialStyle::Rays,
        }
    }
}

/// Spectrum styles: the four looks that used to be separate modes (Classic is LED).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpectrumStyle {
    #[default]
    Bars,
    Led,
    Mountain,
    Dots,
}

impl SpectrumStyle {
    pub const NAMES: [&'static str; 4] = ["Bars", "LED", "Mountain", "Dots"];
    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match norm(s).as_str() {
            "bars" | "bar" => Self::Bars,
            "led" | "classic" => Self::Led,
            "mountain" => Self::Mountain,
            "dots" | "dot" => Self::Dots,
            _ => return None,
        })
    }
}

/// Radial styles: Rays is the original circular spectrum; Cover is the large album art.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RadialStyle {
    #[default]
    Rays,
    Cover,
}

impl RadialStyle {
    pub const NAMES: [&'static str; 2] = ["Rays", "Cover"];
    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match norm(s).as_str() {
            "rays" | "ray" | "radial" => Self::Rays,
            "cover" => Self::Cover,
            _ => return None,
        })
    }
}

/// Style names for a mode's picker, or empty when that mode has none.
pub fn styles_for(mode: &str) -> &'static [&'static str] {
    match norm(mode).as_str() {
        "spectrum" => &SpectrumStyle::NAMES,
        "radial" => &RadialStyle::NAMES,
        _ => &[],
    }
}

/// Compare mode / style names ignoring case, spaces and dashes.
pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A saved or IPC name: the current mode, and a style if the name was an old mode
/// (Classic, Cover, …) or an explicit style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub mode: &'static str,
    pub spectrum: Option<SpectrumStyle>,
    pub radial: Option<RadialStyle>,
}

/// Look up `name` as a current mode, an [`ALIASES`] entry, or a style (LED, Mountain,
/// Cover, …) that implies its parent mode.
pub fn resolve(name: &str) -> Option<Resolved> {
    let want = norm(name);
    if want.is_empty() {
        return None;
    }
    for (old, mode, style) in ALIASES {
        if norm(old) == want {
            return Some(with_style(mode, style));
        }
    }
    for &mode in &[
        "Disabled",
        "Spectrometer",
        "Scope",
        "Spectrum",
        "Envelope",
        "Radial",
        "Particles",
        "Pitch",
        "VU Meter",
    ] {
        if norm(mode) == want {
            return Some(Resolved {
                mode,
                spectrum: None,
                radial: None,
            });
        }
    }
    // A bare style name still works: "LED" / "Bars" → Spectrum, "Rays" → Radial.
    // "Cover", "Mountain", "Dots" and "Classic" are already aliases above.
    if let Some(s) = SpectrumStyle::parse(&want) {
        return Some(Resolved {
            mode: "Spectrum",
            spectrum: Some(s),
            radial: None,
        });
    }
    if let Some(s) = RadialStyle::parse(&want) {
        return Some(Resolved {
            mode: "Radial",
            spectrum: None,
            radial: Some(s),
        });
    }
    None
}

fn with_style(mode: &'static str, style: Option<&str>) -> Resolved {
    let mut r = Resolved {
        mode,
        spectrum: None,
        radial: None,
    };
    if let Some(s) = style {
        if mode == "Spectrum" {
            r.spectrum = SpectrumStyle::parse(s);
        } else if mode == "Radial" {
            r.radial = RadialStyle::parse(s);
        }
    }
    r
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
    fn cover(&self, _f: &Features, _o: &Opts) -> Option<f32> {
        None
    }
}

/// Registered modes, in dropdown order (after "Disabled"). The first seven names keep
/// their old indices; VU Meter follows Pitch (Cover / Mountain / Classic / Dots are now
/// styles of Spectrum or Radial).
pub fn all() -> Vec<Box<dyn Visual>> {
    vec![
        Box::new(Spectrometer::default()),
        Box::new(Scope),
        Box::new(Spectrum::default()),
        Box::new(Envelope::default()),
        Box::new(Radial),
        Box::new(Particles::default()),
        Box::new(Pitch),
        Box::new(Vu::default()),
    ]
}

/// Old mode names still accepted (saved choice, IPC): (old name, current mode, style).
pub const ALIASES: [(&str, &str, Option<&str>); 5] = [
    ("Waveform", "Envelope", None),
    ("Classic", "Spectrum", Some("LED")),
    ("Mountain", "Spectrum", Some("Mountain")),
    ("Dots", "Spectrum", Some("Dots")),
    ("Cover", "Radial", Some("Cover")),
];

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
    f.bands[..8].iter().map(|&b| f.shown(b)).sum::<f32>() / 8.0
}

/// Pixel range `[x0, x1)` of column `i` of `n` that tiles `[0, w)` with no gaps
/// or overlaps, even when `w` is not a multiple of `n`.
fn col_span(i: usize, n: usize, w: usize) -> (usize, usize) {
    let n = n.max(1);
    (i * w / n, (i + 1) * w / n)
}

/// Inset a `[x0, x1)` column by `gap` pixels, split left/right, still in integer
/// pixels. A slot narrower than the gap keeps a 1-pixel bar.
fn col_inset(x0: usize, x1: usize, gap: usize) -> (f32, f32) {
    let w = x1.saturating_sub(x0);
    let g = gap.min(w.saturating_sub(1));
    let left = g / 2;
    ((x0 + left) as f32, (x1 - (g - left)) as f32)
}

/// `n` display levels: resampled bands after [`Features::shown`].
fn levels(f: &Features, n: usize) -> Vec<f32> {
    let mut v = resample(&f.bands, n);
    for x in &mut v {
        *x = f.shown(*x);
    }
    v
}

/// Spectrogram intensity: stay near the background until a bin is clearly
/// above the floor, and reach the palette colour only near full scale.
fn spec_v(v: f32) -> f32 {
    let t = ((v - 0.18) / 0.82).clamp(0.0, 1.0);
    t * t
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
            // Untilted `raw` dominates so a loud, dense mix does not light
            // every row; a little smoothing just knocks the flicker down.
            let v = spec_v((mix(&f.raw) * 0.85 + mix(&f.bands) * 0.15).clamp(0.0, 1.0));
            let base = p.at(ft);
            let stops = [
                p.bg,
                p.bg.lerp(base, 0.10),
                p.bg.lerp(base, 0.40),
                base,
                p.lift(base, 0.65),
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
        let n = span.min(wv.len().saturating_sub(start)).max(1);
        // Map the triggered span across the full width, including canvases wider
        // than the window (HiDPI). Clamping the step to 1 left a flat hold of the
        // last sample on the right (~2/3 of a 1024-wide window at 1.5–2×).
        let last = (w - 1.0).max(0.0);
        let dx = (2.0 * c.k).max(1.0);
        let mut pts = Vec::with_capacity((last / dx) as usize + 3);
        let mut x = 0.0;
        loop {
            let t = if last <= 0.0 {
                0.0
            } else {
                (x / last).clamp(0.0, 1.0)
            };
            let i = start + ((t * (n - 1) as f32).round() as usize).min(n - 1);
            pts.push((x, (mid - wv[i] * gain).clamp(0.0, h - 1.0)));
            if x >= last {
                break;
            }
            x = (x + dx).min(last);
        }
        let base = p.at(0.3);
        let core = p.lift(base, 0.25 + f.beat * 0.4);
        c.glow_polyline(&pts, 2.2 * c.k, core, base, 1.0);
    }
}

/// Spectrum mode: Bars (with optional LED caps), LED (Classic Winamp look), Mountain, Dots.
#[derive(Default)]
struct Spectrum {
    /// Peak cap height and its falling speed per bar (Bars style).
    peaks: Vec<(f32, f32)>,
    mountain: Mountain,
    classic: Classic,
}
impl Visual for Spectrum {
    fn name(&self) -> &'static str {
        "Spectrum"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        match o.spectrum {
            SpectrumStyle::Bars => self.draw_bars(c, f, p, o),
            SpectrumStyle::Led => self.classic.draw(c, f, p, o),
            SpectrumStyle::Mountain => self.mountain.draw(c, f, p, o),
            SpectrumStyle::Dots => Dots.draw(c, f, p, o),
        }
    }
}

impl Spectrum {
    fn draw_bars(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        c.clear(p.bg);
        let mut v = levels(f, o.bars);
        smooth3(&mut v);
        let n = v.len();
        self.peaks.resize(n, (0.0, 0.0));
        let (h, k) = (c.h as f32, c.k);
        let base = h * 0.84;
        let span = h * 0.76;
        for (i, &x) in v.iter().enumerate() {
            let t = i as f32 / (n - 1) as f32;
            let col = p.at(t);
            let top = p.lift(col, 0.3 + f.beat * 0.25);
            let bh = x * span;
            let (a, b) = col_span(i, n, c.w);
            let gap = ((((b - a) as f32) * 0.22).round() as usize).max(1);
            let (x0, x1) = col_inset(a, b, gap);
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
/// Called "Waveform" before 0.16.6.
#[derive(Default)]
struct Envelope {
    hist: Vec<(f32, f32)>,
    /// Slow peak of the resynth window, so a dense mix that sums past 1.0
    /// still shows an envelope instead of a solid block.
    ceil: f32,
}
impl Visual for Envelope {
    fn name(&self) -> &'static str {
        "Envelope"
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, _o: &Opts) {
        c.clear(p.bg);
        let k = c.k;
        // Column count from `k` only; integer spans tile `[0, w)` with no
        // rounding gaps (a float `x1 - gap` left uneven dark slivers).
        let cols = ((c.w as f32 / (3.0 * k)).round() as usize).max(1);
        // Short hops of the newest samples, not one peak of the whole ~40 ms
        // window: a loud master always has a near-full-scale peak in that
        // window, which (with `f.level`, already *2.5 and clamped to 1) painted
        // a solid wall of columns. Hops keep the beat-to-beat envelope.
        let hop = (f.wave.len() / 8).max(32);
        let hops = 4;
        let origin = f.wave.len().saturating_sub(hop * hops);
        for h in 0..hops {
            let sl = &f.wave[origin + h * hop..origin + (h + 1) * hop];
            if sl.is_empty() {
                continue;
            }
            let peak = sl.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            let rms = (sl.iter().map(|s| s * s).sum::<f32>() / sl.len() as f32).sqrt();
            self.ceil = if peak > self.ceil {
                peak
            } else {
                self.ceil * 0.992 + peak * 0.008
            };
            let scale = self.ceil.max(0.35);
            self.hist.push((
                (peak / scale).clamp(0.0, 1.0),
                (rms / scale).clamp(0.0, 1.0),
            ));
        }
        if self.hist.len() > cols {
            let d = self.hist.len() - cols;
            self.hist.drain(..d);
        }
        let (w, h) = (c.w, c.h);
        let mid = h as f32 / 2.0;
        // Leave a little headroom so even a full-scale hop is not a top-to-bottom fill.
        let amp = h as f32 * 0.40;
        let pad = cols - self.hist.len();
        for (i, &(pk, r)) in self.hist.iter().enumerate() {
            let (x0, x1) = col_span(pad + i, cols, w);
            let (x0, x1) = (x0 as f32, x1 as f32);
            let age = (pad + i) as f32 / cols as f32;
            let a = 0.25 + 0.75 * age;
            let col = p.at(pk);
            c.rect(x0, mid - pk * amp, x1, mid + pk * amp, col, a * 0.45);
            let g = p.lift(col, r * 0.6);
            c.rect(x0, mid - r * amp, x1, mid + r * amp, g, a);
        }
        c.rect(0.0, mid - 0.5 * k, w as f32, mid + 0.5 * k, p.accent, 0.3);
    }
}

impl Envelope {
    #[cfg(test)]
    fn peak_range(&self) -> (f32, f32) {
        self.hist
            .iter()
            .map(|(pk, _)| *pk)
            .fold((1.0f32, 0.0f32), |(mn, mx), pk| (mn.min(pk), mx.max(pk)))
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
    fn cover(&self, f: &Features, o: &Opts) -> Option<f32> {
        Some(match o.radial {
            RadialStyle::Rays => radial_core(f),
            RadialStyle::Cover => cover_core(f),
        })
    }
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        match o.radial {
            RadialStyle::Rays => self.draw_rays(c, f, p, o),
            RadialStyle::Cover => Cover.draw(c, f, p, o),
        }
    }
}

impl Radial {
    fn draw_rays(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
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
            let v = f.shown(f.bands[b]);
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
impl Cover {
    fn draw(&self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
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
        let v = levels(f, half);
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
impl Mountain {
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, _o: &Opts) {
        c.clear(p.bg);
        let k = c.k;
        let (w, h) = (c.w as f32, c.h as f32);
        let base = h * 0.92;
        let span = h * 0.74;
        let mut v: Vec<f32> = f.bands.iter().map(|&b| f.shown(b)).collect();
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
impl Classic {
    fn draw(&mut self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        c.clear(p.bg);
        let k = c.k;
        let n = (o.bars / 2).clamp(16, 48);
        let v = levels(f, n);
        self.peaks.resize(n, (0.0, 0));
        let h = c.h as f32;
        let seg = 6.0 * k;
        let pitch = 8.0 * k;
        let base = h * 0.9;
        let rows = ((h * 0.8 / pitch) as usize).max(4);
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
            let (a, b) = col_span(i, n, c.w);
            let gap = ((((b - a) as f32) * 0.2).round() as usize).max((2.0 * k).round() as usize);
            let (x0, x1) = col_inset(a, b, gap);
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
impl Dots {
    fn draw(&self, c: &mut Canvas, f: &Features, p: &Palette, o: &Opts) {
        c.fade(p.bg, 0.35);
        let k = c.k;
        let n = (o.bars / 3).clamp(12, 40);
        let v = levels(f, n);
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
        assert_eq!(
            names,
            &[
                "Spectrometer",
                "Scope",
                "Spectrum",
                "Envelope",
                "Radial",
                "Particles",
                "Pitch",
                "VU Meter",
            ]
        );
        for &(_, mode, _) in &ALIASES {
            assert!(names.contains(&mode), "{mode}");
        }
    }

    #[test]
    fn aliases_resolve_to_mode_and_style() {
        let r = resolve("Classic").unwrap();
        assert_eq!(r.mode, "Spectrum");
        assert_eq!(r.spectrum, Some(SpectrumStyle::Led));
        let r = resolve("LED").unwrap();
        assert_eq!(r.mode, "Spectrum");
        assert_eq!(r.spectrum, Some(SpectrumStyle::Led));
        let r = resolve("Mountain").unwrap();
        assert_eq!(
            (r.mode, r.spectrum),
            ("Spectrum", Some(SpectrumStyle::Mountain))
        );
        let r = resolve("Dots").unwrap();
        assert_eq!(
            (r.mode, r.spectrum),
            ("Spectrum", Some(SpectrumStyle::Dots))
        );
        let r = resolve("Cover").unwrap();
        assert_eq!((r.mode, r.radial), ("Radial", Some(RadialStyle::Cover)));
        let r = resolve("Waveform").unwrap();
        assert_eq!((r.mode, r.spectrum, r.radial), ("Envelope", None, None));
        let r = resolve("Bars").unwrap();
        assert_eq!(r.spectrum, Some(SpectrumStyle::Bars));
        let r = resolve("Rays").unwrap();
        assert_eq!((r.mode, r.radial), ("Radial", Some(RadialStyle::Rays)));
        let r = resolve("Spectrum").unwrap();
        assert_eq!((r.mode, r.spectrum, r.radial), ("Spectrum", None, None));
        assert!(resolve("").is_none() && resolve("nope").is_none());
        assert_eq!(styles_for("Spectrum"), &SpectrumStyle::NAMES);
        assert_eq!(styles_for("Radial"), &RadialStyle::NAMES);
        assert!(styles_for("Scope").is_empty());
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
                    let v = if loud { 0.9 } else { 0.0 };
                    f.bands.fill(v);
                    f.raw.fill(v);
                    f.level = if loud { 0.8 } else { 0.0 };
                    f.beat = if loud { 1.0 } else { 0.0 };
                    f.onset = loud;
                    f.time += 0.033;
                    for cover in [false, true] {
                        let styles = styles_for(m.name());
                        let styles: &[&str] = if styles.is_empty() { &[""] } else { styles };
                        for &style in styles {
                            let mut o = Opts {
                                cover,
                                led: loud,
                                ..Opts::default()
                            };
                            if let Some(s) = SpectrumStyle::parse(style) {
                                o.spectrum = s;
                            }
                            if let Some(s) = RadialStyle::parse(style) {
                                o.radial = s;
                            }
                            m.draw(&mut c, &f, &p, &o);
                        }
                    }
                    assert!(c
                        .px
                        .iter()
                        .all(|q| q.0.is_finite() && q.1.is_finite() && q.2.is_finite()));
                }
            }
        }
    }

    fn luma(q: Rgb) -> f32 {
        q.0 * 0.3 + q.1 * 0.59 + q.2 * 0.11
    }

    /// History columns tile the full canvas width, including HiDPI sizes where
    /// `k` is not 1 (a `3*k` vs `3` mismatch left a stale strip on the right).
    #[test]
    fn envelope_history_spans_full_width() {
        let mut f = super::super::analysis::Analyzer::new(48_000).f;
        f.wave.fill(0.5);
        f.level = 0.4;
        let p = Palette::plain(Rgb(0.9, 0.5, 0.1), Rgb(0.05, 0.05, 0.05), true);
        let bg = luma(p.bg);
        for (w, h) in [(1536, 960), (1200, 800), (800, 600)] {
            let mut m = Envelope::default();
            let mut c = Canvas::new(w, h);
            for _ in 0..(w + 80) {
                m.draw(&mut c, &f, &p, &Opts::default());
            }
            // Midline is only a couple of pixels; sample well into the envelope.
            let y = (h / 2).saturating_sub((h / 6).max(8));
            let lit = |x: usize| (luma(c.px[y * w + x]) - bg).abs() > 0.04;
            assert!(lit(1), "left edge empty at {w}×{h}");
            assert!(lit(w / 2), "centre empty at {w}×{h}");
            assert!(lit(w - 2), "right edge empty at {w}×{h}");
        }
    }

    /// Scope maps the triggered window across the full canvas, including widths
    /// wider than the ~1024-sample half-window (the old step ≥ 1 left a flat
    /// hold of the last sample on the right).
    #[test]
    fn scope_spans_full_width_at_several_sizes() {
        let mut f = super::super::analysis::Analyzer::new(48_000).f;
        let n = f.wave.len();
        let span = n / 2;
        // Rising zero at the start of the displayed span. A wide pulse in the
        // last 20% of the span, then a quiet tail, so a one-sample-per-pixel
        // hold would place the pulse at sample x (not the right of a wide canvas)
        // and a too-narrow spike would vanish on small widths.
        f.wave.fill(0.0);
        f.wave[0] = -0.05;
        for (i, v) in f.wave.iter_mut().enumerate().take(span).skip(1) {
            let t = i as f32 / span as f32;
            *v = if (0.80..0.95).contains(&t) { 0.9 } else { 0.05 };
        }
        f.level = 0.5;
        f.level_avg = 0.3;
        let p = Palette::plain(Rgb(0.2, 0.9, 0.3), Rgb(0.05, 0.05, 0.05), true);
        let bg = luma(p.bg);
        for (w, h) in [(97, 63), (800, 450), (1024, 640), (1025, 641), (1536, 960)] {
            let mut c = Canvas::new(w, h);
            c.clear(p.bg);
            Scope.draw(&mut c, &f, &p, &Opts::default());
            let mid = h as i32 / 2;
            // y of the strongest departure from the background in column x.
            let col_y = |x: usize| -> (i32, f32) {
                (0..h)
                    .map(|y| (y as i32, (luma(c.px[y * w + x]) - bg).abs()))
                    .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                    .unwrap()
            };
            let (_y_l, d_l) = col_y(1);
            let (y_r, d_r) = col_y(w - 2);
            assert!(d_l > 0.04, "Scope left edge has no trace at {w}×{h}");
            assert!(
                d_r > 0.04,
                "Scope right edge has no trace at {w}×{h} (Δ={d_r})"
            );
            // The pulse sits in the last fifth of the *canvas*, not at a 1024-sample
            // pixel index. The quiet tail keeps the right edge near the mid-line.
            let mut best_x = 0;
            let mut best_dev = 0i32;
            for x in 0..w {
                let (y, d) = col_y(x);
                let dev = (y - mid).unsigned_abs() as i32;
                if d > 0.04 && dev > best_dev {
                    best_dev = dev;
                    best_x = x;
                }
            }
            assert!(
                best_x > w * 7 / 10,
                "Scope pulse at x={best_x} of {w} (expected the right fifth)"
            );
            assert!(
                (y_r - mid).unsigned_abs() < (h / 6).max(8) as u32,
                "Scope right edge stuck off the mid-line at {w}×{h} (y={y_r}, mid={mid})"
            );
        }
    }

    /// Envelope columns tile every width without holes, stay off the top and
    /// bottom on a loud signal, and still follow amplitude changes.
    #[test]
    fn envelope_spans_and_does_not_saturate_on_loud_signal() {
        let p = Palette::plain(Rgb(0.9, 0.5, 0.1), Rgb(0.05, 0.05, 0.05), true);
        let bg = luma(p.bg);
        for (w, h) in [(97, 63), (800, 450), (1024, 640), (1025, 641), (1536, 960)] {
            let mut f = super::super::analysis::Analyzer::new(48_000).f;
            // Alternate quiet / loud hops in the newest samples so column
            // heights must differ (a 3-cycle |sin| was too mild once ceil settled).
            let n = f.wave.len();
            let hop = (n / 8).max(32);
            for (i, s) in f.wave.iter_mut().enumerate() {
                let block = if i < n.saturating_sub(hop * 4) {
                    0
                } else {
                    (i - (n - hop * 4)) / hop
                };
                let env = if block % 2 == 0 { 0.20 } else { 0.95 };
                *s = env * (i as f32 * 0.2).sin();
            }
            f.level = 1.0;
            f.level_avg = 0.8;
            let mut m = Envelope::default();
            let mut c = Canvas::new(w, h);
            for _ in 0..(w + 40) {
                m.draw(&mut c, &f, &p, &Opts::default());
            }
            let mid = h / 2;
            let y_body = mid.saturating_sub((h / 20).max(2));
            let lit = |x: usize, y: usize| (luma(c.px[y * w + x]) - bg).abs() > 0.04;
            assert!(lit(1, y_body), "Envelope left empty at {w}×{h}");
            assert!(lit(w / 2, y_body), "Envelope centre empty at {w}×{h}");
            assert!(lit(w - 2, y_body), "Envelope right empty at {w}×{h}");
            // No rounding-gap columns: at most a 1 px hole, never a 2 px run.
            let mut hole = 0usize;
            let mut max_hole = 0usize;
            for x in 0..w {
                if lit(x, y_body) {
                    max_hole = max_hole.max(hole);
                    hole = 0;
                } else {
                    hole += 1;
                }
            }
            max_hole = max_hole.max(hole);
            assert!(
                max_hole <= 1,
                "Envelope has a {max_hole}px gap column at {w}×{h}"
            );
            // Not a top-to-bottom fill: a loud hop still leaves the top and
            // bottom bands on the background.
            let top_lit = (0..w).filter(|&x| lit(x, 1)).count();
            let bot_lit = (0..w).filter(|&x| lit(x, h - 2)).count();
            assert!(
                top_lit < w / 5 && bot_lit < w / 5,
                "Envelope saturated at {w}×{h}: top={top_lit} bottom={bot_lit} of {w}"
            );
            let (pk_lo, pk_hi) = m.peak_range();
            assert!(
                pk_hi > pk_lo + 0.4,
                "Envelope hops did not keep a quiet/loud range at {w}×{h} (pk {pk_lo}..{pk_hi})"
            );
            let mut heights = Vec::new();
            let step = (w / 32).max(1);
            for x in (0..w).step_by(step) {
                let mut lo = None;
                let mut hi = None;
                for y in 0..h {
                    if (luma(c.px[y * w + x]) - bg).abs() > 0.12 {
                        lo = Some(lo.map_or(y, |v: usize| v.min(y)));
                        hi = Some(hi.map_or(y, |v: usize| v.max(y)));
                    }
                }
                if let (Some(lo), Some(hi)) = (lo, hi) {
                    heights.push(hi.saturating_sub(lo));
                }
            }
            assert!(
                !heights.is_empty(),
                "Envelope painted no strong columns at {w}×{h}"
            );
            // On a very short canvas a 1 px column is too coarse for a reliable
            // painted-height check; hop peaks already prove the shape there.
            if h >= 200 {
                let min_h = *heights.iter().min().unwrap();
                let max_h = *heights.iter().max().unwrap();
                assert!(
                    max_h > min_h + h / 10,
                    "Envelope has no shape on a loud modulated signal at {w}×{h} (min={min_h} max={max_h})"
                );
            }
        }
    }

    #[test]
    fn col_span_tiles_the_width() {
        for w in [97usize, 1024, 1025, 1536] {
            for n in [1, 7, 32, 64, 96, w, w + 3] {
                let mut last = 0;
                for i in 0..n {
                    let (a, b) = col_span(i, n, w);
                    assert_eq!(a, last, "gap or overlap at {i} of {n} in width {w}");
                    assert!(b > a || w == 0 || n > w, "empty column {i} of {n} in {w}");
                    last = b;
                }
                assert_eq!(last, w);
            }
        }
    }

    /// A loud, flat (music-like) spectrum must not paint every spectrogram row
    /// at full intensity — that looked like a wall of gold vertical stripes.
    #[test]
    fn spectrometer_stays_dark_on_loud_broadband() {
        let mut f = super::super::analysis::Analyzer::new(48_000).f;
        f.bands.fill(0.85);
        f.raw.fill(0.85);
        f.ceil = 0.85;
        let p = Palette::plain(Rgb(0.9, 0.7, 0.2), Rgb(0.05, 0.05, 0.05), true);
        let mut m = Spectrometer::default();
        let (w, h) = (320, 180);
        let mut c = Canvas::new(w, h);
        c.clear(p.bg);
        let o = Opts {
            scroll: 4.0,
            ..Opts::default()
        };
        for _ in 0..100 {
            m.draw(&mut c, &f, &p, &o);
        }
        let x = w - 1;
        let mut hot = 0;
        let mut sum = 0.0;
        for y in 0..h {
            let l = luma(c.px[y * w + x]);
            sum += l;
            // Top of the colour map (lifted gold / near-white), not mid-gold.
            if l > 0.75 {
                hot += 1;
            }
        }
        let mean = sum / h as f32;
        assert!(
            hot * 4 < h && mean < 0.65,
            "spectrogram column saturated: {hot}/{h} rows hot, mean luma {mean:.3}"
        );

        // A single full-scale band still lights its rows, with the rest dark.
        f.bands.fill(0.0);
        f.raw.fill(0.0);
        f.raw[8] = 1.0;
        f.bands[8] = 1.0;
        f.ceil = 1.0;
        let mut m = Spectrometer::default();
        let mut c = Canvas::new(w, h);
        c.clear(p.bg);
        for _ in 0..100 {
            m.draw(&mut c, &f, &p, &o);
        }
        let mut hot = 0;
        let mut dark = 0;
        let bg = luma(p.bg);
        for y in 0..h {
            let l = luma(c.px[y * w + x]);
            if l > 0.35 {
                hot += 1;
            }
            if (l - bg).abs() < 0.08 {
                dark += 1;
            }
        }
        // A single band is only a few rows after the steep intensity curve.
        assert!(hot >= 1, "peaked band did not light any rows: {hot}");
        assert!(
            dark * 2 > h && hot * 4 < h,
            "peaked spectrogram lost contrast: {hot} hot / {dark} dark of {h}"
        );
    }
}
