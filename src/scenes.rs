//! Chapter thumbnails written by the Scene Chapters & Thumbnails package (#62):
//! `<data>/vivi/plugin-data/scene-chapters/<key>/chapter-<n>.jpg` (n from 1), where the
//! key is the package's hash of "path|size|mtime" (see plugins/scene-chapters).

use std::path::{Path, PathBuf};

/// The package's cache key: 32-bit djb2-style hash (h*33 + byte) of "path|size|mtime".
pub fn key(path: &str, size: u64, mtime: i64) -> String {
    let s = format!("{path}|{size}|{mtime}");
    let h = s
        .bytes()
        .fold(5381u64, |h, b| (h * 33 + u64::from(b)) % 4_294_967_296);
    format!("{h:08x}")
}

pub fn cache_root() -> PathBuf {
    crate::store::data_dir().join("plugin-data/scene-chapters")
}

/// Thumbnail file per chapter (None where none was generated yet).
pub fn thumbs(root: &Path, path: &str, chapters: usize) -> Vec<Option<PathBuf>> {
    let dir = std::fs::metadata(path).ok().map(|m| {
        let mtime = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        root.join(key(path, m.len(), mtime))
    });
    (1..=chapters)
        .map(|i| {
            dir.as_ref()
                .map(|d| d.join(format!("chapter-{i}.jpg")))
                .filter(|f| f.is_file())
        })
        .collect()
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
        let t = super::thumbs(&d.join("root"), &p, 3);
        assert_eq!(
            t.iter().map(Option::is_some).collect::<Vec<_>>(),
            [false, true, false]
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
