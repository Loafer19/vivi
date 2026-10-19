//! Thumbnails written by plugin packages, read here so the core needs no ffmpeg for them:
//! `<data>/vivi/plugin-data/<plugin>/<key>/chapter-<n>.jpg` (n from 1) by Chapter
//! Thumbnails (#75) or Scene Chapters (#62), and `seek-NN.jpg` (seek-bar previews, any
//! count; `seek.meta` = "frames=<n>" once complete) by Chapter Thumbnails. The key is the
//! packages' hash of "path|size|mtime".

use std::path::{Path, PathBuf};

/// The package's cache key: 32-bit djb2-style hash (h*33 + byte) of "path|size|mtime".
pub fn key(path: &str, size: u64, mtime: i64) -> String {
    let s = format!("{path}|{size}|{mtime}");
    let h = s
        .bytes()
        .fold(5381u64, |h, b| (h * 33 + u64::from(b)) % 4_294_967_296);
    format!("{h:08x}")
}

/// Plugin caches with chapter thumbnails, preferred first.
pub fn cache_roots() -> Vec<PathBuf> {
    let d = crate::store::data_dir().join("plugin-data");
    vec![d.join("chapter-thumbnails"), d.join("scene-chapters")]
}

/// Cache folder of `path` under `root` (None when the file cannot be read).
fn file_dir(root: &Path, path: &str) -> Option<PathBuf> {
    std::fs::metadata(path).ok().map(|m| {
        let mtime = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        root.join(key(path, m.len(), mtime))
    })
}

/// Thumbnail file per chapter (None where none was generated yet), from the first root
/// that has one.
pub fn thumbs(roots: &[PathBuf], path: &str, chapters: usize) -> Vec<Option<PathBuf>> {
    let dirs: Vec<PathBuf> = roots.iter().filter_map(|r| file_dir(r, path)).collect();
    (1..=chapters)
        .map(|i| {
            dirs.iter()
                .map(|d| d.join(format!("chapter-{i}.jpg")))
                .find(|f| f.is_file())
        })
        .collect()
}

/// Seek-bar preview frames `seek-00.jpg`, `seek-01.jpg`, … of `path`: the contiguous run
/// from 00 (the package picks the count, e.g. 25–200), capped at `frames=<n>` from
/// `seek.meta` once a run is complete; None when there are fewer than 10.
pub fn seek_frames(root: &Path, path: &str) -> Option<Vec<PathBuf>> {
    let dir = file_dir(root, path)?;
    let max = std::fs::read_to_string(dir.join("seek.meta"))
        .ok()
        .and_then(|m| meta_frames(&m))
        .unwrap_or(1000);
    let v: Vec<PathBuf> = (0..max)
        .map(|i| dir.join(format!("seek-{i:02}.jpg")))
        .take_while(|f| f.is_file())
        .collect();
    (v.len() >= 10).then_some(v)
}

/// `frames=<n>` of a `seek.meta` file.
fn meta_frames(meta: &str) -> Option<usize> {
    meta.lines()
        .find_map(|l| l.trim().strip_prefix("frames="))
        .and_then(|n| n.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    #[test]
    fn key_matches_lua() {
        // Same as the Lua: h = 5381; h = (h*33 + byte) % 2^32 over "a|1|2".
        let mut h: u64 = 5381;
        for b in "a|1|2".bytes() {
            h = (h * 33 + b as u64) % 4_294_967_296;
        }
        assert_eq!(super::key("a", 1, 2), format!("{h:08x}"));
    }

    #[test]
    fn finds_thumbnails() {
        let d = std::env::temp_dir().join(format!("vivi-scenes-{}", std::process::id()));
        let f = d.join("v.mp4");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(&f, b"xx").unwrap();
        let p = f.to_string_lossy().into_owned();
        let m = std::fs::metadata(&f).unwrap();
        let mt = m
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let cd = d.join("root").join(super::key(&p, 2, mt as i64));
        std::fs::create_dir_all(&cd).unwrap();
        std::fs::write(cd.join("chapter-2.jpg"), b"j").unwrap();
        let other = d.join("other").join(super::key(&p, 2, mt as i64));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("chapter-3.jpg"), b"j").unwrap();
        let t = super::thumbs(&[d.join("root"), d.join("other")], &p, 3);
        assert_eq!(
            t.iter().map(Option::is_some).collect::<Vec<_>>(),
            [false, true, true]
        );
        assert!(super::seek_frames(&d.join("root"), &p).is_none());
        for i in 0..12 {
            std::fs::write(cd.join(format!("seek-{i:02}.jpg")), b"j").unwrap();
        }
        std::fs::write(cd.join("seek-13.jpg"), b"j").unwrap();
        assert_eq!(
            super::seek_frames(&d.join("root"), &p).map(|v| v.len()),
            Some(12)
        );
        // More than 100 frames (3-digit names) and the seek.meta cap.
        for i in 12..150 {
            std::fs::write(cd.join(format!("seek-{i:02}.jpg")), b"j").unwrap();
        }
        assert_eq!(
            super::seek_frames(&d.join("root"), &p).map(|v| v.len()),
            Some(150)
        );
        std::fs::write(cd.join("seek.meta"), "frames=120\n").unwrap();
        assert_eq!(
            super::seek_frames(&d.join("root"), &p).map(|v| v.len()),
            Some(120)
        );
        assert_eq!(super::meta_frames("x=1\nframes=25\n"), Some(25));
        assert_eq!(super::meta_frames("frames=\n"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
