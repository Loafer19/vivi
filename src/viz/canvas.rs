//! Tiny software rasteriser (RGBA8) for the visualizer. Kept deliberately simple: the
//! canvas is small (≤ 960 px wide, scaled up by Slint), so per-pixel work is cheap.

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

/// Colours a mode draws with: theme accent and background.
#[derive(Clone, Copy)]
pub struct Palette {
    pub accent: Rgb,
    pub bg: Rgb,
    pub dark: bool,
}

impl Palette {
    /// Accent shifted towards white (dark theme) / black (light theme) by `t`.
    pub fn glow(&self, t: f32) -> Rgb {
        let to = if self.dark {
            Rgb(1.0, 1.0, 1.0)
        } else {
            Rgb(0.0, 0.0, 0.0)
        };
        self.accent.lerp(to, t.clamp(0.0, 1.0))
    }
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Rgb>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![Rgb(0.0, 0.0, 0.0); w * h],
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
    /// Pack into RGBA8 for Slint.
    pub fn write_rgba(&self, out: &mut [slint::Rgba8Pixel]) {
        for (o, p) in out.iter_mut().zip(&self.px) {
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            *o = slint::Rgba8Pixel::new(q(p.0), q(p.1), q(p.2), 255);
        }
    }
}
