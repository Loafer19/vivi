//! Persistent bookmarks: named points in a file, saved in
//! `$XDG_DATA_HOME/vivi/bookmarks.json` keyed like mpv's watch_later (upper-hex MD5 of
//! the path), with an optional video frame in `$XDG_DATA_HOME/vivi/bookmarks/`.
//! Shown as accent pins on the seek bar (`Bookmarks.times`), listed in the side panel's
//! Bookmarks tab (jump, rename + note, delete) and across files in the Library
//! (Playlists → Bookmarks, [`all`]). Ctrl+B / the toolbar button adds one.
//!
//! Since 0.15.0 the bookmarks of a file can become chapters: merged into the player's
//! chapter list ([`merge_chapters`], for the chapter list, seek-bar ticks and chapter
//! keys until the file closes) or written as an FFmpeg metadata file ([`ffmetadata`],
//! for `mpv --chapters-file` or `ffmpeg -i in -i x.ffmetadata -map_metadata 1`). Settings
//! → Privacy → *Clear bookmarks* deletes them all ([`clear_all`]).

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

/// Chapter title of a bookmark: its name (the default one when it has none).
fn chapter_title(m: &Mark) -> String {
    if m.label.trim().is_empty() {
        default_label(m.time)
    } else {
        m.label.trim().to_string()
    }
}

/// The file's chapters (title, start) plus its bookmarks, sorted by time. A bookmark
/// within half a second of a chapter start (or another bookmark) adds nothing.
pub fn merge_chapters(chapters: &[(String, f64)], marks: &[Mark]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = chapters.to_vec();
    for m in marks {
        if !out.iter().any(|(_, t)| (t - m.time).abs() < SAME) {
            out.push((chapter_title(m), m.time.max(0.0)));
        }
    }
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    out
}

/// One line value of an FFmpeg metadata file: `=`, `;`, `#`, backslashes and line breaks
/// are escaped with a backslash.
fn ff_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '=' | ';' | '#' | '\\' | '\n') {
            o.push('\\');
        }
        if c != '\r' {
            o.push(c);
        }
    }
    o
}

/// FFmpeg metadata (`;FFMETADATA1`) with one `[CHAPTER]` per entry (milliseconds); a
/// chapter ends where the next one starts, the last one at `duration` (or one second
/// after its start when the length is unknown).
pub fn ffmetadata(title: &str, chapters: &[(String, f64)], duration: Option<f64>) -> String {
    let mut s = String::from(";FFMETADATA1\n");
    if !title.trim().is_empty() {
        s += &format!("title={}\n", ff_escape(title.trim()));
    }
    let ms = |t: f64| (t.max(0.0) * 1000.0).round() as u64;
    for (i, (name, start)) in chapters.iter().enumerate() {
        let end = chapters
            .get(i + 1)
            .map(|c| c.1)
            .or(duration.filter(|d| *d > *start))
            .unwrap_or(start + 1.0);
        s += &format!(
            "\n[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={}\n",
            ms(*start),
            ms(end).max(ms(*start)),
            ff_escape(name)
        );
    }
    s
}

/// Default chapters file of `path`: `<stem>.ffmetadata` next to it.
pub fn chapters_file_for(path: &str) -> PathBuf {
    Path::new(path).with_extension("ffmetadata")
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
            // Only once the file is there: a miss is remembered by the image cache, and
            // the picture is still being captured right after Add.
            let f = dir.join(&m.thumb);
            let img = (!m.thumb.is_empty() && f.is_file())
                .then(|| crate::images::get(&f, crate::images::TILE, |_| refresh()))
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

/// Bookmarks changed: the side panel, the seek bar and the Library's Bookmarks and
/// Playlists pages (when one is showing).
pub fn changed() {
    refresh();
    if let Some(a) = app() {
        let lib = a.global::<crate::Library>();
        if matches!(lib.get_nav().as_str(), "bookmarks" | "playlists") {
            lib.invoke_refresh();
        }
    }
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
    changed();
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
    let _ = add_here(p, "", "", log);
}

/// Bookmark the current position with a name and note ("" = the default name):
/// `Ok(index)` of the new bookmark, `Err(index)` of one already within half a second,
/// `Err(usize::MAX)` when nothing is playing. Toasts the result.
pub fn add_here(p: &Player, label: &str, note: &str, log: bool) -> Result<usize, usize> {
    let path = p.get("path").unwrap_or_default();
    if path.is_empty() {
        return Err(usize::MAX);
    }
    let t: f64 = p.mpv.get_property("time-pos").unwrap_or(0.0);
    let title = p.get("media-title").unwrap_or_default();
    // An existing bookmark: no frame is captured.
    if let Some(i) = with_db(|db| {
        marks(db, &path)
            .iter()
            .position(|x| (x.time - t).abs() < SAME)
    }) {
        if let Some(a) = app() {
            crate::plugins::toast(&a, &format!("Already bookmarked at {}", short_time(t)));
        }
        if log {
            eprintln!("bookmark at {t:.2}: exists");
        }
        return Err(i);
    }
    let thumb = frame(p, &path, t);
    let m = Mark {
        time: t,
        label: label.trim().to_string(),
        note: note.trim().to_string(),
        thumb: thumb.unwrap_or_default(),
        created: now(),
    };
    let r = with_db(|db| add(db, &path, &title, m));
    if r.is_ok() {
        save();
        if let Some(a) = app() {
            crate::plugins::toast(&a, &format!("Bookmarked {}", short_time(t)));
        }
    }
    if log {
        eprintln!("bookmark at {t:.2}: added");
    }
    changed();
    r
}

/// Seek to bookmark `i` of the playing file; false when there is none.
pub fn jump(p: &Player, i: usize) -> bool {
    let path = current();
    match with_db(|db| marks(db, &path).get(i).map(|m| m.time)) {
        Some(t) => {
            p.seek_abs(t);
            true
        }
        None => false,
    }
}

/// Delete bookmark `i` of the playing file (and its frame).
pub fn remove_current(i: usize) -> Option<Mark> {
    let path = current();
    let m = with_db(|db| remove(db, &path, i));
    if let Some(m) = &m {
        save();
        drop_thumb(m);
    }
    changed();
    m
}

/// Number of saved bookmarks (all files).
pub fn count() -> usize {
    with_db(|db| db.values().map(|e| e.marks.len()).sum())
}

/// Delete every bookmark and saved frame (Settings → Privacy); the number deleted.
pub fn clear_all() -> usize {
    let n = count();
    with_db(|db| db.clear());
    save();
    if let Ok(rd) = std::fs::read_dir(thumbs_dir()) {
        for f in rd.flatten().map(|e| e.path()) {
            crate::images::forget(&f);
            let _ = std::fs::remove_file(&f);
        }
    }
    changed();
    n
}

/// The playing file's chapters as mpv has them (untitled ones keep an empty title).
fn raw_chapters(p: &Player) -> Vec<(String, f64)> {
    let n: i64 = p.mpv.get_property("chapter-list/count").unwrap_or(0);
    (0..n)
        .map(|i| {
            let t: f64 = p
                .mpv
                .get_property(&format!("chapter-list/{i}/time"))
                .unwrap_or(0.0);
            (
                p.get(&format!("chapter-list/{i}/title"))
                    .unwrap_or_default(),
                t,
            )
        })
        .collect()
}

/// Merge the playing file's bookmarks into mpv's chapter list (until the file closes):
/// the number of chapters added.
pub fn to_chapters(p: &Player) -> Result<usize, String> {
    let ms = current_marks();
    if ms.is_empty() {
        return Err("This file has no bookmarks".into());
    }
    let before = raw_chapters(p);
    let merged = merge_chapters(&before, &ms);
    let added = merged.len() - before.len();
    if added > 0 {
        set_chapter_list(p, &merged)?;
    }
    Ok(added)
}

/// Write the playing file's chapters and bookmarks as an FFmpeg metadata file:
/// `path`, else a save dialog (default `<video>.ffmetadata` next to the file).
/// `Err("")` when the dialog was cancelled.
pub fn to_chapters_file(p: &Player, path: Option<PathBuf>) -> Result<PathBuf, String> {
    let file = current();
    let ms = current_marks();
    if ms.is_empty() {
        return Err("This file has no bookmarks".into());
    }
    let out = match path {
        Some(p) => p,
        None => {
            let def = chapters_file_for(&file);
            let mut d = rfd::FileDialog::new()
                .set_title("Save bookmarks as chapters")
                .add_filter("FFmpeg metadata", &["ffmetadata", "txt"]);
            if let Some(dir) = def.parent().filter(|d| d.is_dir()) {
                d = d.set_directory(dir);
            }
            if let Some(n) = def.file_name() {
                d = d.set_file_name(n.to_string_lossy());
            }
            d.save_file().ok_or_else(String::new)?
        }
    };
    let chapters = merge_chapters(&raw_chapters(p), &ms);
    let duration: Option<f64> = p
        .mpv
        .get_property::<f64>("duration")
        .ok()
        .filter(|d| d.is_finite() && *d > 0.0);
    let title = p.get("media-title").unwrap_or_default();
    std::fs::write(&out, ffmetadata(&title, &chapters, duration))
        .map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(out)
}

/// Replace mpv's `chapter-list` (an array of {title, time} maps).
fn set_chapter_list(p: &Player, chapters: &[(String, f64)]) -> Result<(), String> {
    use libmpv2_sys as sys;
    use std::ffi::CString;
    let cstr = |s: &str| CString::new(s.replace('\0', "")).unwrap_or_default();
    let (k_title, k_time) = (cstr("title"), cstr("time"));
    let titles: Vec<CString> = chapters.iter().map(|(t, _)| cstr(t)).collect();
    // Per chapter: the [title, time] values and their two keys.
    let mut values: Vec<[sys::mpv_node; 2]> = chapters
        .iter()
        .zip(&titles)
        .map(|((_, t), title)| {
            // SAFETY: mpv_node is plain C data; every union member is valid when zeroed.
            let mut a: sys::mpv_node = unsafe { std::mem::zeroed() };
            a.format = sys::mpv_format_MPV_FORMAT_STRING;
            a.u.string = title.as_ptr() as *mut _;
            // SAFETY: as above.
            let mut b: sys::mpv_node = unsafe { std::mem::zeroed() };
            b.format = sys::mpv_format_MPV_FORMAT_DOUBLE;
            b.u.double_ = *t;
            [a, b]
        })
        .collect();
    let mut keys: Vec<[*mut std::os::raw::c_char; 2]> = chapters
        .iter()
        .map(|_| [k_title.as_ptr() as *mut _, k_time.as_ptr() as *mut _])
        .collect();
    let mut maps: Vec<sys::mpv_node_list> = values
        .iter_mut()
        .zip(keys.iter_mut())
        .map(|(v, k)| sys::mpv_node_list {
            num: 2,
            values: v.as_mut_ptr(),
            keys: k.as_mut_ptr(),
        })
        .collect();
    let mut items: Vec<sys::mpv_node> = maps
        .iter_mut()
        .map(|m| {
            // SAFETY: as above.
            let mut n: sys::mpv_node = unsafe { std::mem::zeroed() };
            n.format = sys::mpv_format_MPV_FORMAT_NODE_MAP;
            n.u.list = m;
            n
        })
        .collect();
    let mut arr = sys::mpv_node_list {
        num: items.len() as _,
        values: items.as_mut_ptr(),
        keys: std::ptr::null_mut(),
    };
    // SAFETY: as above.
    let mut root: sys::mpv_node = unsafe { std::mem::zeroed() };
    root.format = sys::mpv_format_MPV_FORMAT_NODE_ARRAY;
    root.u.list = &mut arr;
    let name = cstr("chapter-list");
    // SAFETY: valid handle owned by `p.mpv`; mpv copies the node during the call and every
    // pointer in it (keys, strings, values, lists) outlives the call.
    let r = unsafe {
        sys::mpv_set_property(
            p.mpv.ctx.as_ptr(),
            name.as_ptr(),
            sys::mpv_format_MPV_FORMAT_NODE,
            &mut root as *mut sys::mpv_node as *mut std::os::raw::c_void,
        )
    };
    if r < 0 {
        return Err(format!(
            "the player refused the chapter list (mpv error {r})"
        ));
    }
    Ok(())
}

/// Side panel buttons / IPC: bookmarks → chapters in the player (`player`) or a
/// chapters file (`file`), with a toast.
pub fn export(p: &Player, to: &str) {
    let msg = match to {
        "file" => match to_chapters_file(p, None) {
            Ok(f) => format!(
                "Chapters saved to {}",
                f.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            Err(e) if e.is_empty() => return,
            Err(e) => e,
        },
        _ => match to_chapters(p) {
            Ok(0) => "Every bookmark is already a chapter".into(),
            Ok(n) => format!(
                "{n} bookmark{} added as chapters",
                if n == 1 { "" } else { "s" }
            ),
            Err(e) => e,
        },
    };
    if let Some(a) = app() {
        crate::plugins::toast(&a, &msg);
    }
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
                let _ = slint::invoke_from_event_loop(changed);
            }
        });
        return Some(name);
    }
    std::thread::spawn(move || {
        let r = image::open(&raw).map(|i| {
            let i = i.thumbnail(THUMB_W, THUMB_W * 9 / 16 + 60).to_rgb8();
            let tmp = out.with_extension("part");
            i.save_with_format(&tmp, image::ImageFormat::Jpeg)
                .and_then(|()| std::fs::rename(&tmp, &out).map_err(Into::into))
        });
        let _ = std::fs::remove_file(&raw);
        if matches!(r, Ok(Ok(()))) {
            let _ = slint::invoke_from_event_loop(changed);
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
        if let (Some(p), Some(i)) = (player(), index(i)) {
            jump(&p, i);
        }
    });
    g.on_export(|to| {
        if let Some(p) = player() {
            export(&p, &to);
        }
    });
    g.on_rename(|i, label, note| {
        let Some(i) = index(i) else { return };
        let path = current();
        if with_db(|db| rename(db, &path, i, &label, &note)) {
            save();
        }
        changed();
    });
    g.on_remove(|i| {
        if let Some(i) = index(i) {
            remove_current(i);
        }
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

    #[test]
    fn bookmarks_merge_into_chapters() {
        let ch = vec![("Intro".to_string(), 0.0), ("Part 2".to_string(), 120.0)];
        let mut named = mark(60.0);
        named.label = "  Fight  ".into();
        // 120.3 is within half a second of "Part 2": nothing added.
        let ms = [mark(300.0), named, mark(120.3)];
        let m = merge_chapters(&ch, &ms);
        assert_eq!(
            m,
            vec![
                ("Intro".to_string(), 0.0),
                ("Fight".to_string(), 60.0),
                ("Part 2".to_string(), 120.0),
                ("Bookmark at 5:00".to_string(), 300.0),
            ]
        );
        assert_eq!(merge_chapters(&[], &[]), vec![]);
    }

    #[test]
    fn ffmetadata_chapters_file() {
        assert_eq!(ff_escape("a=b;c#d\\e\nf\r"), "a\\=b\\;c\\#d\\\\e\\\nf");
        let ch = vec![("One".to_string(), 0.0), ("Two = 2".to_string(), 61.5)];
        assert_eq!(
            ffmetadata(" Film ", &ch, Some(90.0)),
            ";FFMETADATA1\ntitle=Film\n\
             \n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=61500\ntitle=One\n\
             \n[CHAPTER]\nTIMEBASE=1/1000\nSTART=61500\nEND=90000\ntitle=Two \\= 2\n"
        );
        // Unknown length: the last chapter is one second long; no title line.
        let s = ffmetadata("", &ch[1..], None);
        assert!(s.starts_with(";FFMETADATA1\n\n[CHAPTER]"));
        assert!(s.contains("START=61500\nEND=62500\n"));
        assert_eq!(
            chapters_file_for("/v/Film.2024.mkv"),
            PathBuf::from("/v/Film.2024.ffmetadata")
        );
    }
}
