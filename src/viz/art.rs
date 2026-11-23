//! Visualizer colours from the album art (setting *Colors → Album art*): the cover's
//! most common vivid hues (up to five, the main one first), made readable on the theme
//! background. A cover with one clear colour gives a single-colour palette.

use super::canvas::{Rgb, STOPS};
use std::path::Path;

/// Hue buckets of the histogram.
const BUCKETS: usize = 24;

/// The dominant vivid colour of an image file (PNG / JPEG), or None when it has none
/// (greyscale, black and white, mostly dark) or cannot be read. Blocking: run it on a
/// worker thread.
#[cfg(test)]
pub fn dominant(path: &Path) -> Option<Rgb> {
    palette(path).first().copied()
}

/// Up to [`STOPS`] vivid colours of an image file, the most common first; empty when it
/// has none or cannot be read. Blocking: run it on a worker thread.
#[cfg(test)]
pub fn palette(path: &Path) -> Vec<Rgb> {
    open(path).map(|i| palette_of_image(&i)).unwrap_or_default()
}

/// A cover file decoded for the visualizer: the picture (at most 512 px, for the cover
/// slot) and its [`palette`]. None when it cannot be read. Blocking: run it on a worker
/// thread.
pub fn decode(path: &Path) -> Option<(slint::SharedPixelBuffer<slint::Rgba8Pixel>, Vec<Rgb>)> {
    let img = open(path)?;
    let pal = palette_of_image(&img);
    let pic = img.thumbnail(512, 512).to_rgba8();
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        pic.as_raw(),
        pic.width(),
        pic.height(),
    );
    Some((buf, pal))
}

/// A cover file decoded and blurred for the karaoke "Cover art" background.
/// Blocking: run it on a worker thread.
pub fn decode_blurred(path: &Path) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let img = open(path)?;
    let pic = img.thumbnail(256, 256).fast_blur(14.0).to_rgba8();
    Some(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            pic.as_raw(),
            pic.width(),
            pic.height(),
        ),
    )
}

fn open(path: &Path) -> Option<image::DynamicImage> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}

fn palette_of_image(img: &image::DynamicImage) -> Vec<Rgb> {
    let img = img.thumbnail(64, 64).to_rgb8();
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
    palette_of(&px)
}

/// [`palette`]'s first colour on decoded pixels.
#[cfg(test)]
pub fn dominant_of(px: &[Rgb]) -> Option<Rgb> {
    palette_of(px).first().copied()
}

/// [`palette`] on decoded pixels: a hue histogram weighted by saturation and brightness
/// (vivid pixels count most, greys not at all). The best bucket together with its two
/// neighbours wins and its weighted mean colour is the first colour; those buckets are
/// then taken out and the next best group follows, while it still covers a fair part of
/// the picture (≥ 12% of the first group's weight).
pub fn palette_of(px: &[Rgb]) -> Vec<Rgb> {
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
    let mut out = Vec::new();
    let mut first = 0.0f32;
    while out.len() < STOPS {
        let group = |i: usize| near(i).iter().map(|&j| w[j]).sum::<f32>();
        let Some(best) = (0..BUCKETS).max_by(|&a, &b| group(a).total_cmp(&group(b))) else {
            break;
        };
        let tw = group(best);
        if out.is_empty() {
            // At least ~4% of the picture must be clearly coloured.
            if tw <= 0.0 || tw < px.len() as f32 * 0.04 * 0.3 {
                return out;
            }
            first = tw;
        } else if tw <= 0.0 || tw < first * 0.12 {
            break;
        }
        let ch = |k: usize| near(best).iter().map(|&j| sum[j][k]).sum::<f32>() / tw;
        out.push(Rgb(ch(0), ch(1), ch(2)));
        for j in near(best) {
            w[j] = 0.0;
        }
    }
    out
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
    fn palette_keeps_the_main_colours_in_order() {
        let mut px = vec![Rgb(0.86, 0.15, 0.47); 50];
        px.extend(vec![Rgb(0.08, 0.72, 0.65); 30]);
        px.extend(vec![Rgb(0.95, 0.75, 0.1); 15]);
        px.extend(vec![Rgb(0.5, 0.5, 0.5); 5]);
        let p = palette_of(&px);
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(close(p[0], Rgb(0.86, 0.15, 0.47)));
        assert!(close(p[1], Rgb(0.08, 0.72, 0.65)));
        assert!(close(p[2], Rgb(0.95, 0.75, 0.1)));
        // A single clear colour stays a single-colour palette.
        let mut px = vec![Rgb(0.2, 0.4, 0.9); 40];
        px.extend(vec![Rgb(0.1, 0.1, 0.1); 60]);
        assert_eq!(palette_of(&px).len(), 1);
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
        let blurred = decode_blurred(&p).unwrap();
        assert_eq!((blurred.width(), blurred.height()), (256, 256));
        assert!(decode_blurred(Path::new("/no/such/cover.png")).is_none());
        let _ = std::fs::remove_file(p);
    }
}
