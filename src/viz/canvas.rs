//! Tiny software rasteriser (RGBA8) for the visualizer. Kept deliberately simple: the
//! canvas follows the window's physical size up to a cap (see `canvas_size` in the
//! parent module) and Slint scales it the rest of the way, so per-pixel work stays cheap.
//! Sizes in the modes are written for an ~800 px canvas and multiplied by [`Canvas::k`].

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub f32, pub f32, pub f32);

impl Rgb {
    pub fn lerp(self, o: Rgb, t: f32) -> Rgb {
        Rgb(
            self.0 + (o.0 - self.0) * t,
            self.1 + (o.1 - self.1) * t,
            self.2 + (o.2 - self.2) * t,
        )
    }
    /// Hue rotation keeping the colour's saturation/value (h in turns, 0..1).
    pub fn hsv(h: f32, s: f32, v: f32) -> Rgb {
        let h = h.rem_euclid(1.0) * 6.0;
        let c = v * s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        Rgb(r + m, g + m, b + m)
    }
    pub fn to_hsv(self) -> (f32, f32, f32) {
        let Rgb(r, g, b) = self;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d <= 0.0 {
            0.0
        } else if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } / 6.0;
        (h, if max > 0.0 { d / max } else { 0.0 }, max)
    }
}

/// How the modes colour what they draw (setting *Colors*).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// The theme accent.
    #[default]
    Accent,
    /// The album art's palette (a single cover colour when it has only one).
    Art,
    /// A hue per frequency: bass warm, treble cool.
    Frequency,
}

/// Up to this many palette colours.
pub const STOPS: usize = 5;

/// Colours a mode draws with: accent, background and the gradient stops [`Palette::at`]
/// samples (the album art's palette, or just the accent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub accent: Rgb,
    pub bg: Rgb,
    pub dark: bool,
    pub mode: ColorMode,
    pub stops: [Rgb; STOPS],
    /// Stops in use (1..=STOPS).
    pub n: usize,
}

impl Palette {
    /// A palette with only the accent (Theme accent).
    #[cfg(test)]
    pub fn plain(accent: Rgb, bg: Rgb, dark: bool) -> Self {
        Self {
            accent,
            bg,
            dark,
            mode: ColorMode::Accent,
            stops: [accent; STOPS],
            n: 1,
        }
    }
    /// Accent shifted towards white (dark theme) / black (light theme) by `t`.
    pub fn glow(&self, t: f32) -> Rgb {
        let to = if self.dark {
            Rgb(1.0, 1.0, 1.0)
        } else {
            Rgb(0.0, 0.0, 0.0)
        };
        self.accent.lerp(to, t.clamp(0.0, 1.0))
    }
    /// `c` shifted towards white / black like [`Palette::glow`].
    pub fn lift(&self, c: Rgb, t: f32) -> Rgb {
        let to = if self.dark {
            Rgb(1.0, 1.0, 1.0)
        } else {
            Rgb(0.0, 0.0, 0.0)
        };
        c.lerp(to, t.clamp(0.0, 1.0))
    }
    /// Colour at position `t` (0 = bass / left / bottom, 1 = treble / right / top):
    /// the accent, a hue per frequency, or a gradient through the album art's colours.
    pub fn at(&self, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        match self.mode {
            ColorMode::Accent => self.accent,
            ColorMode::Frequency => {
                // Red-orange bass → yellow → green → blue → violet treble.
                let (s, v) = if self.dark { (0.72, 0.98) } else { (0.8, 0.62) };
                Rgb::hsv(0.02 + t * 0.76, s, v)
            }
            ColorMode::Art => {
                if self.n <= 1 {
                    return self.accent;
                }
                let x = t * (self.n - 1) as f32;
                let i = (x as usize).min(self.n - 2);
                self.stops[i].lerp(self.stops[i + 1], x - i as f32)
            }
        }
    }
}

/// Colour on a multi-stop ramp (`t` 0..1, evenly spaced stops).
pub fn ramp(stops: &[Rgb], t: f32) -> Rgb {
    match stops.len() {
        0 => Rgb(0.0, 0.0, 0.0),
        1 => stops[0],
        n => {
            let x = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (x as usize).min(n - 2);
            stops[i].lerp(stops[i + 1], x - i as f32)
        }
    }
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Rgb>,
    /// Size unit: canvas pixels per pixel of the ~800 px design canvas (≥ 1 on large
    /// or HiDPI windows), so lines and caps keep their weight.
    pub k: f32,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![Rgb(0.0, 0.0, 0.0); w * h],
            k: (w.max(h) as f32 / 800.0).max(0.5),
        }
    }
    pub fn clear(&mut self, c: Rgb) {
        self.px.fill(c);
    }
    /// Move every pixel towards `c` by `t` (motion trails).
    pub fn fade(&mut self, c: Rgb, t: f32) {
        for p in &mut self.px {
            *p = p.lerp(c, t);
        }
    }
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h || a <= 0.0 {
            return;
        }
        let p = &mut self.px[y as usize * self.w + x as usize];
        *p = p.lerp(c, a.min(1.0));
    }
    pub fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgb, a: f32) {
        let (xa, xb) = (
            x0.min(x1).max(0.0) as usize,
            (x0.max(x1).ceil() as usize).min(self.w),
        );
        let (ya, yb) = (
            y0.min(y1).max(0.0) as usize,
            (y0.max(y1).ceil() as usize).min(self.h),
        );
        for y in ya..yb {
            let row = &mut self.px[y * self.w..(y + 1) * self.w];
            for p in &mut row[xa..xb] {
                *p = p.lerp(c, a);
            }
        }
    }
    /// Vertical gradient bar: colour `c0` at the bottom (y1) to `c1` at the top (y0).
    pub fn bar(&mut self, x0: f32, x1: f32, y0: f32, y1: f32, c0: Rgb, c1: Rgb) {
        let (xa, xb) = (x0.max(0.0) as usize, (x1.ceil() as usize).min(self.w));
        let (ya, yb) = (y0.max(0.0) as usize, (y1.ceil() as usize).min(self.h));
        let span = (yb.saturating_sub(ya)).max(1) as f32;
        for y in ya..yb {
            let c = c0.lerp(c1, (yb - y) as f32 / span);
            self.px[y * self.w + xa..y * self.w + xb].fill(c);
        }
    }
    /// Soft filled disc.
    pub fn disc(&mut self, cx: f32, cy: f32, r: f32, c: Rgb, a: f32) {
        let r = r.max(0.5);
        let (x0, x1) = ((cx - r - 1.0) as i32, (cx + r + 1.0) as i32);
        let (y0, y1) = ((cy - r - 1.0) as i32, (cy + r + 1.0) as i32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                let cov = (r + 0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, c, a * cov);
                }
            }
        }
    }
    /// Anti-aliased-ish thick line (stamped discs; fine for the small sizes we draw).
    #[allow(clippy::too_many_arguments)]
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Rgb, a: f32) {
        let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        let steps = (len / (width * 0.5).max(0.7)).ceil().max(1.0) as usize;
        let r = width * 0.5;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            if r <= 0.75 {
                self.blend(x.round() as i32, y.round() as i32, c, a);
            } else {
                self.disc(x, y, r, c, a * 0.6);
            }
        }
    }
    /// Filled area under the polyline `pts` (x ascending) down to `base`, coloured per
    /// row by `grad(t)` with t = 0 at `base` and 1 at `base - height`; alpha `a`.
    pub fn area(
        &mut self,
        pts: &[(f32, f32)],
        base: f32,
        height: f32,
        a: f32,
        grad: impl Fn(f32) -> Rgb,
    ) {
        if pts.len() < 2 || height <= 0.0 {
            return;
        }
        let x_start = pts[0].0.max(0.0) as usize;
        let x_end = (pts[pts.len() - 1].0.ceil() as usize).min(self.w);
        let yb = (base.ceil() as usize).min(self.h);
        // One colour per row, shared by every column.
        let rows: Vec<Rgb> = (0..yb)
            .map(|y| grad(((base - y as f32) / height).clamp(0.0, 1.0)))
            .collect();
        let mut j = 0;
        for x in x_start..x_end {
            let xf = x as f32 + 0.5;
            while j + 2 < pts.len() && pts[j + 1].0 < xf {
                j += 1;
            }
            let (x0, y0) = pts[j];
            let (x1, y1) = pts[j + 1];
            let t = if x1 > x0 {
                ((xf - x0) / (x1 - x0)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let y = y0 + (y1 - y0) * t;
            if y >= base {
                continue;
            }
            let ya = y.max(0.0);
            let yi = ya as usize;
            // Anti-aliased top pixel.
            if yi < yb {
                let cov = 1.0 - (ya - yi as f32);
                let p = &mut self.px[yi * self.w + x];
                *p = p.lerp(rows[yi], a * cov);
            }
            for (yy, &row) in rows.iter().enumerate().take(yb).skip(yi + 1) {
                let p = &mut self.px[yy * self.w + x];
                *p = p.lerp(row, a);
            }
        }
    }
    /// Polyline through `pts`.
    pub fn polyline(&mut self, pts: &[(f32, f32)], width: f32, c: Rgb, a: f32) {
        for w in pts.windows(2) {
            self.line(w[0].0, w[0].1, w[1].0, w[1].1, width, c, a);
        }
    }
    /// Polyline with a cheap glow: two wide faint passes under the line itself.
    pub fn glow_polyline(&mut self, pts: &[(f32, f32)], width: f32, c: Rgb, glow: Rgb, a: f32) {
        self.polyline(pts, width * 4.0, glow, a * 0.07);
        self.polyline(pts, width * 2.2, glow, a * 0.16);
        self.polyline(pts, width, c, a);
    }
    /// Disc with a soft halo (two faint larger discs).
    pub fn glow_disc(&mut self, cx: f32, cy: f32, r: f32, c: Rgb, a: f32) {
        self.disc(cx, cy, r * 2.4, c, a * 0.08);
        self.disc(cx, cy, r * 1.6, c, a * 0.18);
        self.disc(cx, cy, r, c, a);
    }
    /// Anti-aliased ring of thickness `width` centred on radius `r`.
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, width: f32, c: Rgb, a: f32) {
        let ro = r + width * 0.5;
        let ri = (r - width * 0.5).max(0.0);
        let (x0, x1) = ((cx - ro - 1.0) as i32, (cx + ro + 1.0) as i32);
        let (y0, y1) = ((cy - ro - 1.0) as i32, (cy + ro + 1.0) as i32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                let cov = (ro + 0.5 - d).clamp(0.0, 1.0) * (d - ri + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, c, a * cov);
                }
            }
        }
    }
    /// Anti-aliased filled ellipse with radii `rx` (along `ang`) and `ry`, rotated by `ang`.
    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, ang: f32, c: Rgb, a: f32) {
        let (rx, ry) = (rx.max(0.5), ry.max(0.5));
        let (s, co) = ang.sin_cos();
        let r = rx.max(ry) + 1.0;
        let edge = rx.min(ry);
        for y in (cy - r) as i32..=(cy + r) as i32 {
            for x in (cx - r) as i32..=(cx + r) as i32 {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let u = dx * co + dy * s;
                let v = -dx * s + dy * co;
                let d = ((u / rx).powi(2) + (v / ry).powi(2)).sqrt();
                let cov = ((1.0 - d) * edge + 0.5).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, c, a * cov);
                }
            }
        }
    }
    /// Pack into RGBA8 for Slint.
    pub fn write_rgba(&self, out: &mut [slint::Rgba8Pixel]) {
        for (o, p) in out.iter_mut().zip(&self.px) {
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            *o = slint::Rgba8Pixel::new(q(p.0), q(p.1), q(p.2), 255);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_and_palette_positions() {
        let (a, b) = (Rgb(0.0, 0.0, 0.0), Rgb(1.0, 1.0, 1.0));
        assert_eq!(ramp(&[a, b], 0.5), Rgb(0.5, 0.5, 0.5));
        assert_eq!(ramp(&[a, b], 2.0), b);
        let mut p = Palette::plain(Rgb(1.0, 0.0, 0.0), a, true);
        assert_eq!(p.at(0.7), p.accent);
        p.mode = ColorMode::Art;
        p.stops[1] = Rgb(0.0, 0.0, 1.0);
        p.n = 2;
        assert_eq!(p.at(0.0), Rgb(1.0, 0.0, 0.0));
        assert_eq!(p.at(1.0), Rgb(0.0, 0.0, 1.0));
        p.mode = ColorMode::Frequency;
        assert_ne!(p.at(0.0), p.at(1.0));
    }

    #[test]
    fn area_fills_under_the_line_only() {
        let mut c = Canvas::new(10, 10);
        let red = Rgb(1.0, 0.0, 0.0);
        c.area(&[(0.0, 5.0), (10.0, 5.0)], 10.0, 10.0, 1.0, |_| red);
        assert_eq!(c.px[2 * 10 + 4], Rgb(0.0, 0.0, 0.0));
        assert_eq!(c.px[7 * 10 + 4], red);
    }
}
