//! Visualizer colours from the album art (setting *Colours → Album art*): the most
//! common vivid hue of the playing song's cover, made readable on the theme background.

use super::canvas::Rgb;
use std::path::Path;

/// Hue buckets of the histogram.
const BUCKETS: usize = 24;

/// The dominant vivid colour of an image file (PNG / JPEG), or None when it has none
/// (greyscale, black and white, mostly dark) or cannot be read. Blocking: run it on a
/// worker thread.
pub fn dominant(path: &Path) -> Option<Rgb> {
    let img = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .thumbnail(64, 64)
        .to_rgb8();
    let px: Vec<Rgb> = img
        .pixels()
        .map(|p| {
            Rgb(
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
            )
        })
        .collect();
    dominant_of(&px)
}

/// [`dominant`] on decoded pixels: a hue histogram weighted by saturation and
/// brightness (vivid pixels count most, greys not at all); the best bucket together
/// with its two neighbours wins and its weighted mean colour is returned.
pub fn dominant_of(px: &[Rgb]) -> Option<Rgb> {
    let mut w = [0.0f32; BUCKETS];
    let mut sum = [[0.0f32; 3]; BUCKETS];
    for &c in px {
        let (h, s, v) = c.to_hsv();
        if s < 0.25 || v < 0.2 {
            continue;
        }
        let k = s * s * v;
        let b = ((h * BUCKETS as f32) as usize).min(BUCKETS - 1);
        w[b] += k;
        sum[b][0] += c.0 * k;
        sum[b][1] += c.1 * k;
        sum[b][2] += c.2 * k;
    }
    let near = |i: usize| [(i + BUCKETS - 1) % BUCKETS, i, (i + 1) % BUCKETS];
    let best = (0..BUCKETS).max_by(|&a, &b| {
        let s = |i: usize| near(i).iter().map(|&j| w[j]).sum::<f32>();
        s(a).total_cmp(&s(b))
    })?;
    let tw: f32 = near(best).iter().map(|&j| w[j]).sum();
    // At least ~4% of the picture must be clearly coloured.
    if tw <= 0.0 || tw < px.len() as f32 * 0.04 * 0.3 {
        return None;
    }
    let ch = |k: usize| near(best).iter().map(|&j| sum[j][k]).sum::<f32>() / tw;
    Some(Rgb(ch(0), ch(1), ch(2)))
}

/// A cover colour adjusted to work as the visualizer accent on the theme background:
/// saturated enough to read as a colour, bright on the dark theme, darker on the light one.
pub fn usable(c: Rgb, dark: bool) -> Rgb {
    let (h, s, v) = c.to_hsv();
    let s = s.clamp(0.45, 0.9);
    let v = if dark {
        v.clamp(0.7, 1.0)
    } else {
        v.clamp(0.45, 0.75)
    };
    Rgb::hsv(h, s, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rgb, b: Rgb) -> bool {
        (a.0 - b.0).abs() < 0.08 && (a.1 - b.1).abs() < 0.08 && (a.2 - b.2).abs() < 0.08
    }

    #[test]
    fn picks_the_main_colour() {
        // 60% pink, 25% teal, the rest grey: pink wins.
        let mut px = vec![Rgb(0.86, 0.15, 0.47); 60];
        px.extend(vec![Rgb(0.08, 0.72, 0.65); 25]);
        px.extend(vec![Rgb(0.5, 0.5, 0.5); 15]);
        assert!(close(dominant_of(&px).unwrap(), Rgb(0.86, 0.15, 0.47)));
        // A small vivid area still beats a large grey / dark one.
        let mut px = vec![Rgb(0.1, 0.1, 0.1); 80];
        px.extend(vec![Rgb(0.2, 0.4, 0.9); 20]);
        assert!(close(dominant_of(&px).unwrap(), Rgb(0.2, 0.4, 0.9)));
    }

    #[test]
    fn greyscale_has_none() {
        let px: Vec<Rgb> = (0..100)
            .map(|i| {
                let v = i as f32 / 100.0;
                Rgb(v, v, v)
            })
            .collect();
        assert_eq!(dominant_of(&px), None);
        assert_eq!(dominant_of(&[]), None);
    }

    #[test]
    fn readable_on_both_themes() {
        let navy = Rgb(0.05, 0.08, 0.3);
        let (_, s, v) = usable(navy, true).to_hsv();
        assert!(v >= 0.69 && s >= 0.44);
        let pastel = Rgb(0.95, 0.85, 0.9);
        let (_, s, v) = usable(pastel, false).to_hsv();
        assert!(v <= 0.76 && s >= 0.44);
    }

    #[test]
    fn decodes_a_file() {
        let p = std::env::temp_dir().join("vivi-viz-art-test.png");
        let img = image::RgbImage::from_fn(32, 32, |x, _| {
            if x < 24 {
                image::Rgb([220, 120, 20])
            } else {
                image::Rgb([30, 30, 30])
            }
        });
        img.save(&p).unwrap();
        let c = dominant(&p).unwrap();
        assert!(
            close(c, Rgb(220.0 / 255.0, 120.0 / 255.0, 20.0 / 255.0)),
            "{c:?}"
        );
        assert_eq!(dominant(Path::new("/no/such/cover.png")), None);
        let _ = std::fs::remove_file(p);
    }
}
