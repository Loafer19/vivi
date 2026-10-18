//! Media library (M6): folder scan into a SQLite cache, tag reading, views and thumbnails.
//!
//! - Scan runs on a background thread (`walkdir` + `lofty`), results in
//!   `$XDG_CACHE_HOME/vivi/library.db`.
//! - Thumbnails are produced lazily by a worker thread with the `ffmpeg` CLI (if installed)
//!   as raw 320×180 RGBA files in `$XDG_CACHE_HOME/vivi/thumbs/`, so no image decoders
//!   are linked into the binary.

use crate::store;
use rusqlite::{params, Connection};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

pub const VIDEO_EXT: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "m4v", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "ogv",
    "3gp",
];
pub const AUDIO_EXT: &[&str] = &[
    "mp3", "flac", "ogg", "opus", "m4a", "aac", "wav", "wma", "alac", "aiff", "ape", "mka",
];
pub const THUMB_W: u32 = 320;
pub const THUMB_H: u32 = 180;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Media {
    pub path: String,
    pub kind: String, // "video" | "audio"
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub track: i64,
    pub duration: f64,
    pub size: i64,
    pub mtime: i64,
}

pub fn kind_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if VIDEO_EXT.contains(&ext.as_str()) {
        Some("video")
    } else if AUDIO_EXT.contains(&ext.as_str()) {
        Some("audio")
    } else {
        None
    }
}

pub fn db_path() -> PathBuf {
    store::cache_dir().join("library.db")
}

pub fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let c = Connection::open(path)?;
    c.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS media(
            path TEXT PRIMARY KEY, kind TEXT, title TEXT, artist TEXT, album TEXT,
            genre TEXT, track INTEGER, duration REAL, size INTEGER, mtime INTEGER);",
    )?;
    Ok(c)
}

fn read_tags(path: &Path, m: &mut Media) {
    use lofty::prelude::*;
    let Ok(tf) = lofty::read_from_path(path) else {
        return;
    };
    m.duration = tf.properties().duration().as_secs_f64();
    if let Some(t) = tf.primary_tag().or_else(|| tf.first_tag()) {
        let s =
            |v: Option<std::borrow::Cow<str>>| v.map(|x| x.trim().to_string()).unwrap_or_default();
        let title = s(t.title());
        if !title.is_empty() {
            m.title = title;
        }
        m.artist = s(t.artist());
        m.album = s(t.album());
        m.genre = s(t.genre());
        m.track = t.track().unwrap_or(0) as i64;
    }
}

/// Scan `roots`, updating changed files and removing vanished ones. Returns the media count.
#[cfg(test)]
pub fn scan(conn: &mut Connection, roots: &[String]) -> rusqlite::Result<usize> {
    scan_changes(conn, roots).map(|(n, _)| n)
}

/// [`scan`] that also returns how many entries were added, updated or removed. Unchanged
/// files (same mtime as in the index) are only stat'ed, not re-read.
pub fn scan_changes(conn: &mut Connection, roots: &[String]) -> rusqlite::Result<(usize, usize)> {
    let known: BTreeMap<String, i64> = {
        let mut st = conn.prepare("SELECT path, mtime FROM media")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.filter_map(Result::ok).collect()
    };
    let tx = conn.transaction()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut changed = 0;
    for root in roots {
        for e in walkdir::WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !e.file_type().is_file() {
                continue;
            }
            let p = e.path();
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            let Some(kind) = kind_of(p) else { continue };
            let Ok(md) = e.metadata() else { continue };
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            // Forward slashes everywhere (mpv accepts them on Windows too).
            let path = p.to_string_lossy().replace('\\', "/");
            seen.insert(path.clone());
            if known.get(&path) == Some(&mtime) {
                continue;
            }
            let mut m = Media {
                title: p
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path,
                kind: kind.into(),
                size: md.len() as i64,
                mtime,
                ..Default::default()
            };
            read_tags(p, &mut m);
            changed += 1;
            tx.execute(
                "INSERT OR REPLACE INTO media VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    m.path, m.kind, m.title, m.artist, m.album, m.genre, m.track, m.duration,
                    m.size, m.mtime
                ],
            )?;
        }
    }
    let in_roots = |p: &str| {
        roots
            .iter()
            .any(|r| p.starts_with(&format!("{}/", r.trim_end_matches('/'))))
    };
    for p in known.keys() {
        if !seen.contains(p) || !in_roots(p) {
            tx.execute("DELETE FROM media WHERE path=?1", [p])?;
            changed += 1;
        }
    }
    tx.commit()?;
    Ok((seen.len(), changed))
}

pub fn all(conn: &Connection) -> Vec<Media> {
    let Ok(mut st) = conn
        .prepare("SELECT path,kind,title,artist,album,genre,track,duration,size,mtime FROM media")
    else {
        return Vec::new();
    };
    st.query_map([], |r| {
        Ok(Media {
            path: r.get(0)?,
            kind: r.get(1)?,
            title: r.get(2)?,
            artist: r.get(3)?,
            album: r.get(4)?,
            genre: r.get(5)?,
            track: r.get(6)?,
            duration: r.get(7)?,
            size: r.get(8)?,
            mtime: r.get(9)?,
        })
    })
    .map(|it| it.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Fraction watched (0..1) from mpv's watch-later data.
pub fn progress(m: &Media) -> f32 {
    match store::resume_position(&m.path) {
        Some(t) if m.duration > 0.0 => (t / m.duration).clamp(0.0, 1.0) as f32,
        Some(_) => 0.05,
        None => 0.0,
    }
}

/// Sort keys (Figma "Sorting Options"): unwatched, watched, newest, oldest, title,
/// title-desc, artist, artist-desc, album, album-desc, track, track-desc, uri, uri-desc,
/// size. Legacy keys: name (= title), date (= newest).
pub fn sort_with(v: &mut [Media], by: &str, watched: &dyn Fn(&Media) -> bool) {
    let (key, desc) = match by.strip_suffix("-desc") {
        Some(k) => (k, true),
        None => (by, false),
    };
    match key {
        "newest" | "date" => v.sort_by_key(|m| std::cmp::Reverse(m.mtime)),
        "oldest" => v.sort_by_key(|m| m.mtime),
        "size" => v.sort_by_key(|m| std::cmp::Reverse(m.size)),
        "unwatched" | "watched" => {
            v.sort_by_cached_key(|m| natural(&m.title));
            let w = key == "watched";
            v.sort_by_key(|m| watched(m) != w);
        }
        "artist" => v.sort_by_cached_key(|m| (natural(&m.artist), natural(&m.title))),
        "album" => v.sort_by_cached_key(|m| (natural(&m.album), m.track)),
        "track" => v.sort_by_key(|m| m.track),
        "uri" => v.sort_by_cached_key(|m| natural(&m.path)),
        _ => v.sort_by_cached_key(|m| natural(&m.title)),
    }
    if desc {
        v.reverse();
    }
}

/// Lowercase key where digit runs are zero-padded, so "Ep 2" < "Ep 10".
fn natural(s: &str) -> String {
    let mut out = String::new();
    let mut num = String::new();
    for c in s.chars().chain(std::iter::once('\0')) {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push_str(&format!("{num:0>10}"));
                num.clear();
            }
            out.extend(c.to_lowercase());
        }
    }
    out
}

pub fn human_size(n: i64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.1} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.0} MB", n / 1e6)
    } else {
        format!("{:.0} KB", n / 1e3)
    }
}

pub fn human_time(t: f64) -> String {
    crate::util::clock(t as i64)
}

// ---------- thumbnails ----------

pub fn thumb_path(path: &str) -> PathBuf {
    store::cache_dir()
        .join("thumbs")
        .join(format!("{:x}.rgba", md5::compute(path.as_bytes())))
}

/// `ffmpeg` is installed (`PATH` lookup; nothing is run, so this is fine at startup).
pub fn ffmpeg_available() -> bool {
    crate::util::in_path("ffmpeg")
}

/// Render a 320×180 RGBA frame (video at ~10 %, or embedded cover art for audio).
pub fn make_thumb(m: &Media) -> Option<PathBuf> {
    let out = thumb_path(&m.path);
    if out.exists() {
        return Some(out);
    }
    std::fs::create_dir_all(out.parent()?).ok()?;
    let seek = if m.kind == "video" && m.duration > 0.0 {
        m.duration * 0.1
    } else if m.kind == "video" {
        5.0
    } else {
        0.0
    };
    let vf = format!(
        "scale={THUMB_W}:{THUMB_H}:force_original_aspect_ratio=decrease,pad={THUMB_W}:{THUMB_H}:(ow-iw)/2:(oh-ih)/2"
    );
    let run = |ss: f64| {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args(["-v", "error", "-nostdin"]);
        if ss > 0.0 {
            // Input seeking; skipped for audio so the attached cover picture is used.
            cmd.args(["-ss", &format!("{ss:.2}")]);
        }
        cmd.args(["-i", &m.path, "-an", "-frames:v", "1", "-vf", &vf])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .output()
            .ok()
            .map(|o| o.stdout)
            .filter(|b| b.len() == (THUMB_W * THUMB_H * 4) as usize)
    };
    let Some(bytes) = run(seek).or_else(|| run(0.0)) else {
        // Remember that this file has no picture so it is not retried every start.
        let _ = std::fs::write(out.with_extension("none"), b"");
        return None;
    };
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, bytes).ok()?;
    std::fs::rename(&tmp, &out).ok()?;
    Some(out)
}

/// Cover art (audio) or a frame at ~10 % (video) as a 256 px PNG, for MPRIS widgets.
#[cfg(target_os = "linux")]
pub fn make_art(path: &str, out: &Path) {
    if let Some(d) = out.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = out.with_extension("tmp.png");
    let audio = kind_of(Path::new(path)) == Some("audio");
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin", "-y"]);
    if !audio {
        cmd.args(["-ss", "5"]);
    }
    let ok = cmd
        .args(["-i", path, "-an", "-frames:v", "1", "-vf", "scale=256:-2"])
        .arg(&tmp)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        let _ = std::fs::rename(&tmp, out);
    }
}

pub fn load_thumb(p: &Path) -> Option<slint::Image> {
    let b = std::fs::read(p).ok()?;
    if b.len() != (THUMB_W * THUMB_H * 4) as usize {
        return None;
    }
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&b, THUMB_W, THUMB_H);
    Some(slint::Image::from_rgba8(buf))
}

/// Background worker: receives media, produces thumbnails, reports the path back.
pub fn thumb_worker(done: impl Fn(String, PathBuf) + Send + 'static) -> mpsc::Sender<Media> {
    let (tx, rx) = mpsc::channel::<Media>();
    std::thread::Builder::new()
        .name("vivi-thumbs".into())
        .spawn(move || {
            while let Ok(m) = rx.recv() {
                if let Some(p) = make_thumb(&m) {
                    done(m.path, p);
                }
            }
        })
        .ok();
    tx
}

/// `yt-dlp` is installed (`PATH` lookup, which includes the YouTube plugin's managed
/// copy). Running `yt-dlp --version` here cost 0.2-0.4 s of Python startup on the UI thread.
pub fn ytdlp_available() -> bool {
    crate::util::in_path("yt-dlp")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural_order() {
        let mut v = vec!["Ep 10", "Ep 2", "ep 1"];
        v.sort_by_key(|s| natural(s));
        assert_eq!(v, ["ep 1", "Ep 2", "Ep 10"]);
    }
    #[test]
    fn scan_tree() {
        let dir = std::env::temp_dir().join(format!("vivi-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Show/S01")).unwrap();
        std::fs::write(dir.join("Show/S01/e01.mkv"), b"x").unwrap();
        std::fs::write(dir.join("song.mp3"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let mut c = open_db(&dir.join("t.db")).unwrap();
        let roots = vec![dir.to_string_lossy().into_owned()];
        assert_eq!(scan(&mut c, &roots).unwrap(), 2);
        assert_eq!(scan_changes(&mut c, &roots).unwrap(), (2, 0));
        std::fs::remove_file(dir.join("song.mp3")).unwrap();
        scan(&mut c, &roots).unwrap();
        let a = all(&c);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, "video");
        assert_eq!(a[0].title, "e01");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
