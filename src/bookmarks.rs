//! Persistent bookmarks: named points in a file, saved in
//! `$XDG_DATA_HOME/vivi/bookmarks.json` keyed like mpv's watch_later (upper-hex MD5 of
//! the path), with an optional video frame in `$XDG_DATA_HOME/vivi/bookmarks/`.
//! Shown as accent pins on the seek bar (`Bookmarks.times`), listed in the side panel's
//! Bookmarks tab (jump, rename + note, delete) and across files in the Library
//! (Playlists → Bookmarks, [`all`]). Ctrl+B / the toolbar button adds one.

use crate::player::Player;
use crate::{AppWindow, BookmarkRow};
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Two bookmarks closer than this (seconds) are the same one.
const SAME: f64 = 0.5;
/// Width of the saved frame, px.
const THUMB_W: u32 = 320;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub time: f64,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// File name in [`thumbs_dir`] ("" = none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thumb: String,
    /// Unix seconds.
    #[serde(default)]
    pub created: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Sorted by time.
    #[serde(default)]
    pub marks: Vec<Mark>,
}

/// Watch-later key → bookmarks of that file.
pub type Db = BTreeMap<String, Entry>;

/// mpv's watch_later key for `path`.
pub fn key(path: &str) -> String {
    format!("{:X}", md5::compute(path.as_bytes()))
}

pub fn db_path() -> PathBuf {
    crate::store::data_dir().join("bookmarks.json")
}

pub fn thumbs_dir() -> PathBuf {
    crate::store::data_dir().join("bookmarks")
}

/// The saved bookmarks; empty when the file is missing or unreadable.
pub fn load_from(path: &Path) -> Db {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_to(path: &Path, db: &Db) -> std::io::Result<()> {
    let s = serde_json::to_string_pretty(db).map_err(std::io::Error::other)?;
    crate::store::write_atomic(path, &s)
}

/// "1:23" / "1:02:03".
pub fn short_time(t: f64) -> String {
    let s = t.max(0.0).floor() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn default_label(t: f64) -> String {
    format!("Bookmark at {}", short_time(t))
}

/// The bookmarks of `path` (empty when none).
pub fn marks<'a>(db: &'a Db, path: &str) -> &'a [Mark] {
    db.get(&key(path)).map_or(&[], |e| e.marks.as_slice())
}

/// Add a bookmark at `time`: `Ok(index)` of the new one, or `Err(index)` of an existing
/// bookmark within half a second (nothing added).
pub fn add(db: &mut Db, path: &str, title: &str, mut m: Mark) -> Result<usize, usize> {
    let e = db.entry(key(path)).or_insert_with(|| Entry {
        path: path.to_string(),
        ..Default::default()
    });
    e.path = path.to_string();
    if !title.is_empty() {
        e.title = title.to_string();
    }
    if let Some(i) = e.marks.iter().position(|x| (x.time - m.time).abs() < SAME) {
        return Err(i);
    }
    if m.label.trim().is_empty() {
        m.label = default_label(m.time);
    }
    let i = e.marks.partition_point(|x| x.time < m.time);
    e.marks.insert(i, m);
    Ok(i)
}

/// Rename bookmark `i` of `path` and set its note; an empty name restores the default.
pub fn rename(db: &mut Db, path: &str, i: usize, label: &str, note: &str) -> bool {
    let Some(m) = db.get_mut(&key(path)).and_then(|e| e.marks.get_mut(i)) else {
        return false;
    };
    let label = label.trim();
    m.label = if label.is_empty() {
        default_label(m.time)
    } else {
        label.to_string()
    };
    m.note = note.trim().to_string();
    true
}

/// Remove bookmark `i` of `path` (dropping the file's entry when it was the last).
pub fn remove(db: &mut Db, path: &str, i: usize) -> Option<Mark> {
    let k = key(path);
    let e = db.get_mut(&k)?;
    if i >= e.marks.len() {
        return None;
    }
    let m = e.marks.remove(i);
    if e.marks.is_empty() {
        db.remove(&k);
    }
    Some(m)
}

/// Every bookmark: (path, file title, mark), by file then time.
pub fn all(db: &Db) -> Vec<(String, String, Mark)> {
    let mut v: Vec<_> = db
        .values()
        .flat_map(|e| {
            e.marks
                .iter()
                .map(move |m| (e.path.clone(), e.title.clone(), m.clone()))
        })
        .collect();
    v.sort_by(|a, b| {
        a.0.cmp(&b.0).then(
            a.2.time
                .partial_cmp(&b.2.time)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    v
}

/// Library item key of a bookmark: "<seconds>\u{1f}<path>".
pub fn lib_key(path: &str, time: f64) -> String {
    format!("{time:.3}\u{1f}{path}")
}

/// (path, seconds) of a [`lib_key`].
pub fn parse_lib_key(key: &str) -> Option<(String, f64)> {
    let (t, path) = key.split_once('\u{1f}')?;
    let t = t
        .parse::<f64>()
        .ok()
        .filter(|t| t.is_finite() && *t >= 0.0)?;
    (!path.is_empty()).then(|| (path.to_string(), t))
}

// ---------------------------------------------------------------------------------
// Runtime: the loaded database, the playing file and the side panel / seek bar.

thread_local! {
    static DB: RefCell<Option<Db>> = const { RefCell::new(None) };
    static CUR: RefCell<String> = const { RefCell::new(String::new()) };
    static APP: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    static PLAYER: RefCell<Option<Rc<Player>>> = const { RefCell::new(None) };
    /// Open-at-time from the Library: (path, seconds), applied on FileLoaded.
    static SEEK: RefCell<Option<(String, f64)>> = const { RefCell::new(None) };
}

fn with_db<R>(f: impl FnOnce(&mut Db) -> R) -> R {
    DB.with(|d| f(d.borrow_mut().get_or_insert_with(|| load_from(&db_path()))))
}

fn save() {
    with_db(|db| {
        if let Err(e) = save_to(&db_path(), db) {
            eprintln!("bookmarks: save failed: {e}");
        }
    });
}

/// Every bookmark (Library view).
pub fn list_all() -> Vec<(String, String, Mark)> {
    with_db(|db| all(db))
}

/// The playing file's bookmarks (IPC).
pub fn current_marks() -> Vec<Mark> {
    let path = current();
    with_db(|db| marks(db, &path).to_vec())
}

fn current() -> String {
    CUR.with(|c| c.borrow().clone())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn app() -> Option<AppWindow> {
    APP.with(|a| a.borrow().as_ref().and_then(|w| w.upgrade()))
}

fn player() -> Option<Rc<Player>> {
    PLAYER.with(|p| p.borrow().clone())
}

/// Rebuild the side-panel list and the seek-bar markers for the playing file.
pub fn refresh() {
    let Some(a) = app() else { return };
    let path = current();
    let ms: Vec<Mark> = with_db(|db| marks(db, &path).to_vec());
    let dir = thumbs_dir();
    let rows: Vec<BookmarkRow> = ms
        .iter()
        .map(|m| {
            let img = (!m.thumb.is_empty())
                .then(|| {
                    crate::images::get(&dir.join(&m.thumb), crate::images::TILE, |_| refresh())
                })
                .flatten();
            BookmarkRow {
                time: m.time as f32,
                label: m.label.clone().into(),
                note: m.note.clone().into(),
                has_thumb: img.is_some(),
                thumb: img.unwrap_or_default(),
            }
        })
        .collect();
    let g = a.global::<crate::Bookmarks>();
    let times: Vec<f32> = ms.iter().map(|m| m.time as f32).collect();
    // Keep an open rename while only thumbnails change.
    if g.get_items().row_count() != rows.len() {
        g.set_editing(-1);
    }
    g.set_times(slint::ModelRc::new(slint::VecModel::from(times)));
    g.set_items(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// The playing file changed: show its bookmarks; apply a pending open-at-time.
pub fn file_loaded(p: &Player) {
    let path = p.get("path").unwrap_or_default();
    CUR.with(|c| *c.borrow_mut() = path.clone());
    if let Some((want, t)) = SEEK.with(|s| s.borrow_mut().take()) {
        if want == path {
            p.seek_abs(t);
        }
    }
    refresh();
}

/// Play `path` from `time` (Library → Bookmarks).
pub fn open_at(p: &Player, path: &str, time: f64) {
    if current() == path && p.get("path").as_deref() == Some(path) {
        p.seek_abs(time);
        p.set("pause", "no");
        return;
    }
    SEEK.with(|s| *s.borrow_mut() = Some((path.to_string(), time)));
    p.open(path);
}

/// Delete the bookmark of `path` at `time` (Library context menu).
pub fn remove_at(path: &str, time: f64) {
    let Some(i) = with_db(|db| {
        marks(db, path)
            .iter()
            .position(|m| (m.time - time).abs() < 0.01)
    }) else {
        return;
    };
    if let Some(m) = with_db(|db| remove(db, path, i)) {
        save();
        drop_thumb(&m);
    }
    refresh();
}

fn drop_thumb(m: &Mark) {
    if !m.thumb.is_empty() {
        let f = thumbs_dir().join(&m.thumb);
        crate::images::forget(&f);
        let _ = std::fs::remove_file(f);
    }
}

/// Bookmark the current position (Ctrl+B, toolbar, side panel +).
pub fn add_current(p: &Player, log: bool) {
    let path = p.get("path").unwrap_or_default();
    if path.is_empty() {
        return;
    }
    let t: f64 = p.mpv.get_property("time-pos").unwrap_or(0.0);
    let title = p.get("media-title").unwrap_or_default();
    let thumb = frame(p, &path, t);
    let m = Mark {
        time: t,
        label: String::new(),
        note: String::new(),
        thumb: thumb.clone().unwrap_or_default(),
        created: now(),
    };
    let r = with_db(|db| add(db, &path, &title, m));
    let a = app();
    match r {
        Ok(_) => {
            save();
            if let Some(a) = &a {
                crate::plugins::toast(a, &format!("Bookmarked {}", short_time(t)));
            }
        }
        Err(_) => {
            if let Some(f) = thumb {
                let _ = std::fs::remove_file(thumbs_dir().join(f));
            }
            if let Some(a) = &a {
                crate::plugins::toast(a, &format!("Already bookmarked at {}", short_time(t)));
            }
        }
    }
    if log {
        eprintln!(
            "bookmark at {t:.2}: {}",
            if r.is_ok() { "added" } else { "exists" }
        );
    }
    refresh();
}

/// Save the current video frame for a bookmark (not for audio or cover art); the
/// downscale runs on a thread and the list refreshes when it is done.
fn frame(p: &Player, path: &str, t: f64) -> Option<String> {
    let video = p.get("vid").is_some_and(|v| v != "no" && !v.is_empty())
        && p.get("current-tracks/video/albumart").as_deref() != Some("yes");
    if !video {
        return None;
    }
    let dir = thumbs_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("{}-{}.jpg", key(path), (t * 1000.0).round() as u64);
    let out = dir.join(&name);
    // mpv's own screenshot of the current frame. It fails while hardware decoding
    // hands frames straight to the GPU (vaapi / nvdec zero-copy): mpv cannot read them
    // back. Local files then get a frame from a short-lived, software-decoding libmpv
    // instance (vo=image); anything else stays without a picture (generic icon).
    let raw = dir.join(format!(".{name}.png"));
    if p.try_cmd("screenshot-to-file", &[&raw.to_string_lossy(), "video"])
        .is_err()
    {
        if !std::path::Path::new(path).is_file() {
            return None;
        }
        let src = path.to_string();
        std::thread::spawn(move || {
            if grab(&src, t, &out).is_some() {
                let _ = slint::invoke_from_event_loop(refresh);
            }
        });
        return Some(name);
    }
    std::thread::spawn(move || {
        let r = image::open(&raw).map(|i| {
            let i = i.thumbnail(THUMB_W, THUMB_W * 9 / 16 + 60).to_rgb8();
            i.save_with_format(&out, image::ImageFormat::Jpeg)
        });
        let _ = std::fs::remove_file(&raw);
        if matches!(r, Ok(Ok(()))) {
            let _ = slint::invoke_from_event_loop(refresh);
        }
    });
    Some(name)
}

/// One frame of local file `src` at `t` seconds, scaled to [`THUMB_W`] px wide, written
/// to `out` (JPEG) by a separate headless libmpv (software decode, no audio, no
/// config or scripts). Gives up after 10 s.
fn grab(src: &str, t: f64, out: &std::path::Path) -> Option<()> {
    let tmp = out.with_extension("d");
    std::fs::create_dir_all(&tmp).ok()?;
    let r = grab_into(src, t, &tmp, out);
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

fn grab_into(src: &str, t: f64, tmp: &std::path::Path, out: &std::path::Path) -> Option<()> {
    use libmpv2::events::{Event, EventContext};
    let mpv = libmpv2::Mpv::with_initializer(|i| {
        i.set_property("config", "no")?;
        i.set_property("load-scripts", "no")?;
        i.set_property("ytdl", "no")?;
        i.set_property("terminal", "no")?;
        i.set_property("idle", "yes")?;
        i.set_property("vo", "image")?;
        i.set_property("vo-image-format", "jpg")?;
        i.set_property("vo-image-outdir", tmp.to_string_lossy().as_ref())?;
        i.set_property("ao", "null")?;
        i.set_property("audio", "no")?;
        i.set_property("sid", "no")?;
        i.set_property("hwdec", "no")?;
        i.set_property("vf", format!("scale=w={THUMB_W}:h=-2").as_str())?;
        i.set_property("start", format!("{t:.3}").as_str())?;
        i.set_property("frames", 1i64)?;
        Ok(())
    })
    .ok()?;
    let mut ev = EventContext::new(mpv.ctx);
    let q = format!("\"{}\"", src.replace('\\', "\\\\").replace('"', "\\\""));
    mpv.command("loadfile", &[&q]).ok()?;
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(10) {
        if let Some(Ok(Event::EndFile(_))) = ev.wait_event(0.2) {
            break;
        }
    }
    let f = std::fs::read_dir(tmp).ok()?.flatten().next()?.path();
    std::fs::rename(f, out).ok()
}

fn index(i: i32) -> Option<usize> {
    usize::try_from(i).ok()
}

pub fn setup(app: &AppWindow, pl: &Rc<Player>, log: bool) {
    APP.with(|a| *a.borrow_mut() = Some(app.as_weak()));
    PLAYER.with(|p| *p.borrow_mut() = Some(pl.clone()));
    let g = app.global::<crate::Bookmarks>();
    g.on_add(move || {
        if let Some(p) = player() {
            add_current(&p, log);
        }
    });
    g.on_jump(|i| {
        let (Some(p), Some(i)) = (player(), index(i)) else {
            return;
        };
        let path = current();
        if let Some(t) = with_db(|db| marks(db, &path).get(i).map(|m| m.time)) {
            p.seek_abs(t);
        }
    });
    g.on_rename(|i, label, note| {
        let Some(i) = index(i) else { return };
        let path = current();
        if with_db(|db| rename(db, &path, i, &label, &note)) {
            save();
        }
        refresh();
    });
    g.on_remove(|i| {
        let Some(i) = index(i) else { return };
        let path = current();
        if let Some(m) = with_db(|db| remove(db, &path, i)) {
            save();
            drop_thumb(&m);
        }
        refresh();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(t: f64) -> Mark {
        Mark {
            time: t,
            ..Default::default()
        }
    }

    #[test]
    fn key_matches_watch_later() {
        // mpv: upper-case hex MD5 of the path.
        assert_eq!(key("/a/b.mkv"), format!("{:X}", md5::compute(b"/a/b.mkv")));
        assert_eq!(key("x").len(), 32);
    }

    #[test]
    fn add_sorts_dedupes_and_labels() {
        let mut db = Db::new();
        assert_eq!(add(&mut db, "/f.mkv", "F", mark(90.0)), Ok(0));
        assert_eq!(add(&mut db, "/f.mkv", "", mark(10.0)), Ok(0));
        assert_eq!(add(&mut db, "/f.mkv", "", mark(3725.0)), Ok(2));
        assert_eq!(add(&mut db, "/f.mkv", "", mark(90.3)), Err(1));
        let m = marks(&db, "/f.mkv");
        assert_eq!(
            m.iter().map(|m| m.time).collect::<Vec<_>>(),
            [10.0, 90.0, 3725.0]
        );
        assert_eq!(m[1].label, "Bookmark at 1:30");
        assert_eq!(m[2].label, "Bookmark at 1:02:05");
        assert_eq!(db[&key("/f.mkv")].title, "F");
        assert!(marks(&db, "/other").is_empty());
    }

    #[test]
    fn rename_and_remove() {
        let mut db = Db::new();
        add(&mut db, "/f", "", mark(5.0)).unwrap();
        add(&mut db, "/f", "", mark(65.0)).unwrap();
        assert!(rename(&mut db, "/f", 1, "  Twist ", " the reveal "));
        assert_eq!(marks(&db, "/f")[1].label, "Twist");
        assert_eq!(marks(&db, "/f")[1].note, "the reveal");
        assert!(rename(&mut db, "/f", 1, "", ""));
        assert_eq!(marks(&db, "/f")[1].label, "Bookmark at 1:05");
        assert!(!rename(&mut db, "/f", 7, "x", ""));
        assert_eq!(remove(&mut db, "/f", 0).map(|m| m.time), Some(5.0));
        assert!(remove(&mut db, "/f", 3).is_none());
        assert!(remove(&mut db, "/f", 0).is_some());
        assert!(db.is_empty(), "the last bookmark drops the file's entry");
    }

    #[test]
    fn all_lists_by_file_then_time() {
        let mut db = Db::new();
        add(&mut db, "/b", "B", mark(20.0)).unwrap();
        add(&mut db, "/a", "A", mark(30.0)).unwrap();
        add(&mut db, "/b", "B", mark(2.0)).unwrap();
        let v: Vec<(String, f64)> = all(&db).into_iter().map(|(p, _, m)| (p, m.time)).collect();
        assert_eq!(
            v,
            [("/a".into(), 30.0), ("/b".into(), 2.0), ("/b".into(), 20.0)]
        );
    }

    #[test]
    fn round_trips_through_json() {
        let dir = std::env::temp_dir().join(format!("vivi-bm-{}", std::process::id()));
        let f = dir.join("bookmarks.json");
        let mut db = Db::new();
        let mut m = mark(12.5);
        m.note = "n".into();
        m.thumb = "x.jpg".into();
        add(&mut db, "/f", "T", m).unwrap();
        add(&mut db, "/f", "T", mark(40.0)).unwrap();
        save_to(&f, &db).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(
            !text.contains("\"note\": \"\""),
            "empty notes are not written"
        );
        assert_eq!(load_from(&f), db);
        assert!(load_from(&dir.join("missing.json")).is_empty());
        std::fs::write(&f, "not json").unwrap();
        assert!(load_from(&f).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn library_keys() {
        let k = lib_key("/m/a b.mkv", 83.25);
        assert_eq!(parse_lib_key(&k), Some(("/m/a b.mkv".into(), 83.25)));
        assert_eq!(parse_lib_key("/m/a.mkv"), None);
        assert_eq!(parse_lib_key("x\u{1f}/m/a.mkv"), None);
        assert_eq!(parse_lib_key("3\u{1f}"), None);
    }

    #[test]
    fn short_times() {
        assert_eq!(short_time(0.0), "0:00");
        assert_eq!(short_time(59.9), "0:59");
        assert_eq!(short_time(3600.0), "1:00:00");
        assert_eq!(short_time(-3.0), "0:00");
    }
}
