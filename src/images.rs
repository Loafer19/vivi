//! One image cache for the whole app (Home, Recent, library rows, station logos, Free
//! Music covers, chapter thumbnails, playlist covers).
//!
//! * Every picture is decoded once per size on a worker thread, shrunk to fit `size` px
//!   (vivi's own 320×180 video thumbnails are used as they are).
//! * Decoded images live in an in-memory LRU with a byte budget ([`MEMORY_BUDGET`]).
//! * Shrunk copies are written to `~/.cache/vivi/thumbs/pics/` (PNG), so the next start
//!   reads a small file instead of decoding the original again.
//! * The image caches on disk (thumbnails, station logos, web thumbnails, Free Music
//!   covers) are kept under [`DISK_BUDGET`], oldest files first; Settings → Library can
//!   clear them.
//!
//! [`get`] is the UI-thread entry point: it returns the image when it is in memory, else
//! queues it and calls `done` when it is ready (callers show a type icon meanwhile).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

/// Grid tiles (Home shelves, library grid, Recent tiles, now-playing logo).
pub const TILE: u32 = 256;
/// List rows (station and Free Music rows, side panel).
pub const ROW: u32 = 48;
/// Decoded images kept in memory.
pub const MEMORY_BUDGET: usize = 40 * 1024 * 1024;
/// Image caches on disk.
pub const DISK_BUDGET: u64 = 200 * 1024 * 1024;

/// Least-recently-used map that keeps the summed `bytes` of its entries under a budget.
pub struct Lru<K, V> {
    map: HashMap<K, (V, usize, u64)>,
    bytes: usize,
    budget: usize,
    tick: u64,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    pub fn new(budget: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            budget,
            tick: 0,
        }
    }
    pub fn get(&mut self, k: &K) -> Option<V> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(k).map(|e| {
            e.2 = t;
            e.0.clone()
        })
    }
    /// Insert, then drop least recently used entries until the budget holds (the new
    /// entry stays even when it alone is over budget).
    pub fn insert(&mut self, k: K, v: V, bytes: usize) {
        self.tick += 1;
        if let Some(old) = self.map.insert(k.clone(), (v, bytes, self.tick)) {
            self.bytes -= old.1;
        }
        self.bytes += bytes;
        while self.bytes > self.budget && self.map.len() > 1 {
            let Some(oldest) = self
                .map
                .iter()
                .filter(|(key, _)| **key != k)
                .min_by_key(|(_, e)| e.2)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(e) = self.map.remove(&oldest) {
                self.bytes -= e.1;
            }
        }
    }
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        self.map.retain(|k, _| keep(k));
        self.bytes = self.map.values().map(|e| e.1).sum();
    }
    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
}

/// Cache key: source file and the size it is shown at.
pub type Key = (PathBuf, u32);

/// File of the shrunk copy of `src` at `size` in `pics`.
fn thumb_in(pics: &Path, src: &Path, size: u32) -> PathBuf {
    let h = md5::compute(src.to_string_lossy().as_bytes());
    pics.join(format!("{h:x}-{size}.png"))
}

fn pics_dir() -> PathBuf {
    crate::store::cache_dir().join("thumbs/pics")
}

/// The image cache directories on disk (sources downloaded by vivi and shrunk copies).
pub fn disk_dirs() -> Vec<PathBuf> {
    let c = crate::store::cache_dir();
    [
        "thumbs",
        "thumbs/pics",
        "stream-logos",
        "web-thumbs",
        "free-music-covers",
    ]
    .iter()
    .map(|d| c.join(d))
    .collect()
}

enum Decoded {
    Pixels(u32, u32, Vec<u8>),
    Svg(PathBuf),
}

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Worker side: the shrunk copy when it is newer than the source, else decode the source
/// (PNG / JPEG, or vivi's raw 320×180 RGBA video thumbnails), shrink it and store the copy.
fn decode(src: &Path, size: u32) -> Option<Decoded> {
    decode_in(&pics_dir(), src, size)
}

fn decode_in(pics: &Path, src: &Path, size: u32) -> Option<Decoded> {
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "svg" {
        return src.is_file().then(|| Decoded::Svg(src.to_path_buf()));
    }
    if ext == "rgba" {
        // Video thumbnails made by vivi: already small, used as they are.
        let (w, h) = (crate::library::THUMB_W, crate::library::THUMB_H);
        let b = std::fs::read(src).ok()?;
        return (b.len() == (w * h * 4) as usize).then_some(Decoded::Pixels(w, h, b));
    }
    let t = thumb_in(pics, src, size);
    if let (Some(a), Some(b)) = (mtime(&t), mtime(src)) {
        if a >= b {
            if let Ok(img) = image::open(&t) {
                // Mark as recently used for the disk budget.
                let _ = std::fs::File::options()
                    .append(true)
                    .open(&t)
                    .and_then(|f| f.set_modified(std::time::SystemTime::now()));
                return Some(pixels(img.into_rgba8()));
            }
        }
    }
    let img = image::ImageReader::open(src)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    if img.width() <= size && img.height() <= size {
        // Already small (e.g. station logos): no copy needed.
        return Some(pixels(img.into_rgba8()));
    }
    let small = img.thumbnail(size, size).into_rgba8();
    if let Some(d) = t.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = t.with_extension("tmp");
    if small
        .save_with_format(&tmp, image::ImageFormat::Png)
        .is_ok()
    {
        let _ = std::fs::rename(&tmp, &t);
    }
    Some(pixels(small))
}

fn pixels(img: image::RgbaImage) -> Decoded {
    Decoded::Pixels(img.width(), img.height(), img.into_raw())
}

type Done = Box<dyn FnOnce(slint::Image)>;

struct Cache {
    lru: Lru<Key, slint::Image>,
    /// Requests in flight, with the callers waiting for them.
    pending: HashMap<Key, Vec<Done>>,
    /// Sources that could not be decoded this run (not retried).
    failed: std::collections::HashSet<Key>,
    tx: Option<Sender<Key>>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache {
        lru: Lru::new(MEMORY_BUDGET),
        pending: HashMap::new(),
        failed: Default::default(),
        tx: None,
    });
}

fn worker() -> Sender<Key> {
    let (tx, rx) = channel::<Key>();
    let _ = std::thread::Builder::new()
        .name("vivi-images".into())
        .spawn(move || {
            while let Ok(k) = rx.recv() {
                let d = decode(&k.0, k.1);
                let _ = slint::invoke_from_event_loop(move || finish(k, d));
            }
        });
    tx
}

fn finish(k: Key, d: Option<Decoded>) {
    let img = match d {
        Some(Decoded::Pixels(w, h, px)) => Some((
            slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(&px, w, h)),
            px.len(),
        )),
        Some(Decoded::Svg(p)) => slint::Image::load_from_path(&p)
            .ok()
            .map(|i| (i, (k.1 * k.1 * 4) as usize)),
        None => None,
    };
    let waiting = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let w = c.pending.remove(&k).unwrap_or_default();
        match &img {
            Some((i, bytes)) => c.lru.insert(k.clone(), i.clone(), *bytes),
            None => {
                c.failed.insert(k);
            }
        }
        w
    });
    if let Some((i, _)) = img {
        for f in waiting {
            f(i.clone());
        }
    }
}

/// The picture of `src` shrunk to fit `size` px. Returns it when it is in memory;
/// otherwise queues it for the worker and calls `done` (UI thread) once it is decoded.
/// Files that fail to decode are not retried during this run.
pub fn get(
    src: &Path,
    size: u32,
    done: impl FnOnce(slint::Image) + 'static,
) -> Option<slint::Image> {
    let k: Key = (src.to_path_buf(), size);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(i) = c.lru.get(&k) {
            return Some(i);
        }
        if c.failed.contains(&k) {
            return None;
        }
        let first = !c.pending.contains_key(&k);
        c.pending.entry(k.clone()).or_default().push(Box::new(done));
        if first {
            let tx = c.tx.get_or_insert_with(worker).clone();
            let _ = tx.send(k);
        }
        None
    })
}

/// Drop every size of `src` from memory (call when the file was replaced).
pub fn forget(src: &Path) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.lru.retain(|k| k.0 != src);
        c.failed.retain(|k| k.0 != src);
    });
}

/// Total size of the image caches on disk, in bytes.
pub fn disk_usage() -> u64 {
    files(&disk_dirs()).iter().map(|f| f.1).sum()
}

fn files(dirs: &[PathBuf]) -> Vec<(std::time::SystemTime, u64, PathBuf)> {
    dirs.iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten())
        .filter_map(|e| {
            let m = e.metadata().ok().filter(|m| m.is_file())?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect()
}

/// Delete the oldest cache files until the image caches fit in `max` bytes.
pub fn prune_disk(max: u64) {
    let mut v = files(&disk_dirs());
    let mut total: u64 = v.iter().map(|f| f.1).sum();
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

/// Settings → Clear cache: empty the image caches on disk and in memory.
pub fn clear() {
    prune_disk(0);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.lru.clear();
        c.failed.clear();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_keeps_the_byte_budget_and_evicts_least_recent() {
        let mut l: Lru<(PathBuf, u32), u8> = Lru::new(100);
        let k = |s: &str, n| (PathBuf::from(s), n);
        l.insert(k("a", 48), 1, 40);
        l.insert(k("b", 48), 2, 40);
        assert_eq!(l.get(&k("a", 48)), Some(1)); // a is now newer than b
        l.insert(k("c", 48), 3, 40); // 120 > 100: b goes
        assert_eq!(l.get(&k("b", 48)), None);
        assert_eq!(l.get(&k("a", 48)), Some(1));
        assert_eq!(l.bytes(), 80);
        // Same path, other size: a separate entry.
        l.insert(k("a", 160), 4, 20);
        assert_eq!((l.len(), l.bytes()), (3, 100));
        assert_eq!(l.get(&k("a", 160)), Some(4));
        // Replacing an entry updates the byte count.
        l.insert(k("a", 160), 5, 10);
        assert_eq!(l.bytes(), 90);
        // One entry over budget alone is kept, everything else goes.
        l.insert(k("big", 48), 6, 500);
        assert_eq!((l.len(), l.bytes()), (1, 500));
        l.insert(k("small", 48), 7, 5);
        l.retain(|key| key.0 != Path::new("big"));
        assert_eq!((l.len(), l.bytes()), (1, 5));
        l.clear();
        assert_eq!((l.len(), l.bytes()), (0, 0));
    }

    #[test]
    fn thumb_files_differ_by_source_and_size() {
        let t = |s: &str, n| thumb_in(Path::new("/c"), Path::new(s), n);
        let a = t("/m/a.jpg", ROW);
        assert_ne!(a, t("/m/a.jpg", TILE));
        assert_ne!(a, t("/m/b.jpg", ROW));
        assert_eq!(a, t("/m/a.jpg", ROW));
        assert!(a.to_string_lossy().ends_with("-48.png"));
    }

    #[test]
    fn decode_shrinks_and_reuses_the_disk_copy() {
        let dir = std::env::temp_dir().join(format!("vivi-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("cover.jpg");
        image::RgbImage::from_pixel(400, 200, image::Rgb([10, 200, 10]))
            .save_with_format(&src, image::ImageFormat::Jpeg)
            .unwrap();
        let pics = dir.join("pics");
        let Some(Decoded::Pixels(w, h, px)) = decode_in(&pics, &src, ROW) else {
            panic!("no pixels");
        };
        assert_eq!((w, h, px.len()), (48, 24, 48 * 24 * 4));
        assert!(thumb_in(&pics, &src, ROW).is_file());
        assert!(matches!(
            decode_in(&pics, &src, ROW),
            Some(Decoded::Pixels(48, 24, _))
        ));
        assert!(decode_in(&pics, &dir.join("missing.png"), ROW).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
