//! Per-file AcoustID result cache (`<data>/vivi/tagger-cache.json`, owner-only).
//!
//! Fingerprinting and the AcoustID lookup are the slow and rate-limited part of
//! identifying a song, so their answer is kept per file: the key is the file's path, size
//! and modification time, so a retagged or re-encoded file is looked up again. Single-file
//! identify, the automatic identification of untagged songs and folder tagging all read
//! it; answers handed over by the AcoustID package (`vivi-tagger-acoustid`) are stored too.
//! At most [`CACHE_MAX`] files are kept (the oldest answers go first). *Forget cached
//! results* in the Auto-tagger's settings deletes the file.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Most files remembered.
pub const CACHE_MAX: usize = 300;

/// One AcoustID recording match (what the lookup's `meta=recordings+releasegroups`
/// answer says about it).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Match {
    /// MusicBrainz recording id.
    pub id: String,
    /// AcoustID score, 0–100.
    pub score: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds (0 = unknown).
    pub duration: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Item {
    /// Unix seconds the answer was stored.
    t: u64,
    matches: Vec<Match>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Doc {
    version: u32,
    files: HashMap<String, Item>,
}

/// Cache key of a file: "path|size|mtime" (seconds), `None` when it cannot be read.
pub fn file_key(path: &Path) -> Option<String> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    Some(format!("{}|{}|{mtime}", abs.display(), md.len()))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The cache file and its entries.
#[derive(Debug)]
pub struct ResultCache {
    path: PathBuf,
    doc: Doc,
}

impl ResultCache {
    /// Load `path` (empty when it is missing or unreadable).
    pub fn load(path: &Path) -> Self {
        let doc = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Doc>(&s).ok())
            .unwrap_or_default();
        ResultCache {
            path: path.to_path_buf(),
            doc,
        }
    }

    pub fn len(&self) -> usize {
        self.doc.files.len()
    }

    /// The stored matches of `key` (an empty list = AcoustID knew nothing).
    pub fn get(&self, key: &str) -> Option<Vec<Match>> {
        self.doc.files.get(key).map(|i| i.matches.clone())
    }

    /// Remember `matches` for `key` at time `t`, dropping the oldest entries beyond
    /// [`CACHE_MAX`].
    pub fn put(&mut self, key: &str, matches: Vec<Match>, t: u64) {
        self.doc.files.insert(key.to_string(), Item { t, matches });
        if self.doc.files.len() > CACHE_MAX {
            let mut v: Vec<(String, u64)> = self
                .doc
                .files
                .iter()
                .map(|(k, i)| (k.clone(), i.t))
                .collect();
            // Newest first; ties by key so the result does not depend on hash order.
            v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            for (k, _) in v.into_iter().skip(CACHE_MAX) {
                self.doc.files.remove(&k);
            }
        }
    }

    /// Write the file (owner-only, atomic).
    pub fn save(&self) -> std::io::Result<()> {
        let doc = Doc {
            version: 1,
            files: self.doc.files.clone(),
        };
        let s = serde_json::to_string(&doc).map_err(std::io::Error::other)?;
        crate::store::write_private(&self.path, s.as_bytes())
    }

    /// Forget everything and delete the file.
    pub fn clear(&mut self) -> std::io::Result<()> {
        self.doc.files.clear();
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// `<data>/vivi/tagger-cache.json`.
pub fn cache_file() -> PathBuf {
    crate::store::data_dir().join("tagger-cache.json")
}

/// The shared cache (loaded on first use; every thread goes through it).
static CACHE: Mutex<Option<ResultCache>> = Mutex::new(None);

fn with_cache<R>(f: impl FnOnce(&mut ResultCache) -> R) -> R {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let c = g.get_or_insert_with(|| ResultCache::load(&cache_file()));
    f(c)
}

/// Cached matches of the file at `path`, if it was looked up before and is unchanged.
pub fn cached(path: &Path) -> Option<Vec<Match>> {
    let key = file_key(path)?;
    with_cache(|c| c.get(&key))
}

/// Remember the AcoustID answer for the file at `path`.
pub fn remember(path: &Path, matches: &[Match]) {
    let Some(key) = file_key(path) else { return };
    with_cache(|c| {
        c.put(&key, matches.to_vec(), now());
        if let Err(e) = c.save() {
            eprintln!("vivi: auto-tagger: save result cache: {e}");
        }
    });
}

/// Forget every cached answer; the number of files that were cached.
pub fn forget() -> std::io::Result<usize> {
    with_cache(|c| {
        let n = c.len();
        c.clear().map(|_| n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vivi-tagcache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn m(id: &str, score: u32) -> Match {
        Match {
            id: id.into(),
            score,
            title: "T".into(),
            ..Default::default()
        }
    }

    #[test]
    fn key_changes_with_size_and_mtime() {
        let d = tmp("key");
        let f = d.join("a.mp3");
        std::fs::write(&f, b"abc").unwrap();
        let k1 = file_key(&f).unwrap();
        assert!(k1.ends_with(&format!(
            "|3|{}",
            std::fs::metadata(&f)
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        )));
        assert!(k1.starts_with(&f.display().to_string()));
        std::fs::write(&f, b"abcd").unwrap();
        assert_ne!(file_key(&f).unwrap(), k1);
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(file_key(&f).unwrap().ends_with("|4|1000000"));
        assert_eq!(file_key(&d.join("missing.mp3")), None);
        assert_eq!(file_key(&d), None);
    }

    #[test]
    fn save_load_bounded_and_private() {
        let d = tmp("bounded");
        let p = d.join("sub").join("tagger-cache.json");
        let mut c = ResultCache::load(&p);
        assert_eq!(c.len(), 0);
        for i in 0..(CACHE_MAX as u64 + 20) {
            c.put(&format!("f{i}"), vec![m(&format!("r{i}"), 90)], 1000 + i);
        }
        assert_eq!(c.len(), CACHE_MAX);
        // The 20 oldest went.
        assert_eq!(c.get("f0"), None);
        assert_eq!(c.get("f19"), None);
        assert_eq!(c.get("f20").unwrap()[0].id, "r20");
        // "Not found" answers are remembered too.
        c.put("empty", Vec::new(), 5000);
        assert_eq!(c.get("empty"), Some(Vec::new()));
        c.save().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let back = ResultCache::load(&p);
        assert_eq!(back.len(), CACHE_MAX);
        assert_eq!(back.get("empty"), Some(Vec::new()));
        assert_eq!(back.get("f319").unwrap()[0], m("r319", 90));
        let mut back = back;
        back.clear().unwrap();
        assert!(!p.exists());
        assert_eq!(back.len(), 0);
        // Clearing twice is fine; a broken file loads empty.
        back.clear().unwrap();
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(ResultCache::load(&p).len(), 0);
    }
}
