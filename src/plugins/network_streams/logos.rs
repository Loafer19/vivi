//! Station logo cache: download, sniff, store, prune, and the stable cache file names.

use super::*;

/// Largest logo side kept in the cache; logos are shown at 40-96 px.
pub(crate) const LOGO_MAX: u32 = 128;

pub(crate) fn logo_dir() -> PathBuf {
    crate::store::cache_dir().join("stream-logos")
}

/// What downloaded logo bytes are, from their first bytes (servers often send HTML error
/// pages or icons with an image content type).
#[derive(Debug, PartialEq)]
pub(crate) enum LogoKind {
    /// PNG or JPEG: decoded, shrunk to [`LOGO_MAX`] and stored as PNG.
    Raster,
    Svg,
    /// Anything else (HTML, ICO, GIF, WebP, ...): not stored.
    Other,
}

pub(crate) fn sniff_logo(b: &[u8]) -> LogoKind {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") || b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return LogoKind::Raster;
    }
    let head = String::from_utf8_lossy(&b[..b.len().min(1024)]).to_ascii_lowercase();
    if head.contains("<svg") {
        LogoKind::Svg
    } else {
        LogoKind::Other
    }
}

/// Write logo bytes next to `base` (the extension-less cache name) with the extension of
/// their real format; raster logos are shrunk to [`LOGO_MAX`]. `None` = not an image.
pub(crate) fn store_logo(base: &Path, b: &[u8]) -> Option<PathBuf> {
    match sniff_logo(b) {
        LogoKind::Raster => {
            let img = image::load_from_memory(b).ok()?;
            let img = if img.width() > LOGO_MAX || img.height() > LOGO_MAX {
                img.thumbnail(LOGO_MAX, LOGO_MAX)
            } else {
                img
            };
            let out = base.with_extension("png");
            let tmp = base.with_extension("tmp");
            img.to_rgba8()
                .save_with_format(&tmp, image::ImageFormat::Png)
                .ok()?;
            std::fs::rename(&tmp, &out).ok()?;
            Some(out)
        }
        LogoKind::Svg => {
            let out = base.with_extension("svg");
            std::fs::write(&out, b).ok()?;
            Some(out)
        }
        LogoKind::Other => None,
    }
}

/// Station logo already in the cache (no download, no decoding).
pub(crate) fn cached_logo(url: &str) -> Option<PathBuf> {
    cached_with_ext(&logo_dir(), url, &["png", "svg"])
}

/// Older versions saved logos as they came, without an extension, so Slint could not tell
/// their format and every one failed to load. Convert those once (shrinking raster logos)
/// and delete the ones that are not images. Returns how many logos became usable.
pub(crate) fn migrate_logo_cache() -> usize {
    let Ok(rd) = std::fs::read_dir(logo_dir()) else {
        return 0;
    };
    let mut n = 0;
    for p in rd.flatten().map(|e| e.path()) {
        if p.extension().is_some() || !p.is_file() {
            continue;
        }
        if let Ok(b) = std::fs::read(&p) {
            n += usize::from(store_logo(&p, &b).is_some());
        }
        let _ = std::fs::remove_file(&p);
    }
    n
}

thread_local! {
    /// Logos decoded by the image cache: (now-playing card?, key, image), drained in `tick`.
    pub(crate) static READY: std::cell::RefCell<Vec<(bool, String, slint::Image)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The logo at `p` from the image cache, or `None` while it is decoded (it then arrives
/// through [`READY`] under `key`).
pub(crate) fn logo_image(
    p: &Path,
    size: u32,
    now_playing: bool,
    key: String,
) -> Option<slint::Image> {
    crate::images::get(p, size, move |i| {
        READY.with(|r| r.borrow_mut().push((now_playing, key, i)))
    })
}

/// Download a station logo into the cache (once); `None` when it cannot be fetched or
/// is not an image.
pub(crate) fn fetch_logo(url: &str) -> Option<PathBuf> {
    if let Some(p) = cached_logo(url) {
        return Some(p);
    }
    let dir = logo_dir();
    let base = dir.join(hash_name(url));
    let miss = hashed_file(&dir, url, |h| format!("{h}.none"));
    // Broken logo URLs are remembered for a week instead of being retried every time.
    if std::fs::metadata(&miss)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e.as_secs() < 7 * 86400)
    {
        return None;
    }
    std::fs::create_dir_all(&dir).ok()?;
    let stored = match web::get_bytes(url, 2 * 1024 * 1024) {
        Ok(Some(b)) if !b.is_empty() => store_logo(&base, &b),
        _ => None,
    };
    if stored.is_none() {
        let _ = std::fs::write(&miss, b"");
    }
    stored
}

/// Short stable file name for a URL: FNV-1a 64 of its bytes, 16 hex digits. Used for
/// every URL-keyed cache file (station logos, guides, covers, web thumbnails, playlist
/// copies), so it must never change between builds.
pub fn hash_name(url: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The name Vivi 0.12 and older gave `url`: `DefaultHasher` (SipHash-1-3 with zero keys)
/// of `str::hash`, i.e. the bytes and a `0xff` terminator. The standard library may change
/// that hasher in any release, so it is spelled out here to keep finding old cache files.
pub fn legacy_hash_name(url: &str) -> String {
    let mut m = url.as_bytes().to_vec();
    m.push(0xff);
    format!("{:016x}", siphash13(&m))
}

/// SipHash-1-3 with zero keys, as `std::hash::SipHasher13::new()` (length in the last byte).
pub(crate) fn siphash13(m: &[u8]) -> u64 {
    let mut v = [
        0x736f_6d65_7073_6575u64,
        0x646f_7261_6e64_6f6du64,
        0x6c79_6765_6e65_7261u64,
        0x7465_6462_7974_6573u64,
    ];
    fn round(v: &mut [u64; 4]) {
        v[0] = v[0].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(13) ^ v[0];
        v[0] = v[0].rotate_left(32);
        v[2] = v[2].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(16) ^ v[2];
        v[0] = v[0].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(21) ^ v[0];
        v[2] = v[2].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(17) ^ v[2];
        v[2] = v[2].rotate_left(32);
    }
    let (words, rest) = m.as_chunks::<8>();
    for c in words {
        let w = u64::from_le_bytes(*c);
        v[3] ^= w;
        round(&mut v);
        v[0] ^= w;
    }
    let mut last = [0u8; 8];
    last[..rest.len()].copy_from_slice(rest);
    let w = u64::from_le_bytes(last) | ((m.len() as u64 & 0xff) << 56);
    v[3] ^= w;
    round(&mut v);
    v[0] ^= w;
    v[2] ^= 0xff;
    for _ in 0..3 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

/// `dir/name(hash_name(url))`. When only the file of the old name
/// ([`legacy_hash_name`]) exists, it is renamed first, so caches written by Vivi 0.12 and
/// older (logos, guides, covers, thumbnails, last good playlist copies) keep working.
pub fn hashed_file(dir: &Path, url: &str, name: impl Fn(&str) -> String) -> PathBuf {
    let new = dir.join(name(&hash_name(url)));
    if !new.exists() {
        let old = dir.join(name(&legacy_hash_name(url)));
        if old.is_file() {
            let _ = std::fs::rename(&old, &new);
        }
    }
    new
}

/// The first of `dir/<hash>.<ext>` for `exts` that exists (old names migrated).
pub fn cached_with_ext(dir: &Path, url: &str, exts: &[&str]) -> Option<PathBuf> {
    exts.iter()
        .map(|e| hashed_file(dir, url, |h| format!("{h}.{e}")))
        .find(|p| p.is_file())
}

/// Keep the logo cache under `max` bytes (oldest files go first).
pub(crate) fn prune_logos(max: u64) {
    let dir = crate::store::cache_dir().join("stream-logos");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut v: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = v.iter().map(|x| x.1).sum();
    v.sort();
    for (_, n, p) in v {
        if total <= max {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= n;
        }
    }
}
