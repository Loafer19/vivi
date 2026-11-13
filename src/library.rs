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
    pub kind: MediaKind,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub track: i64,
    pub duration: f64,
    pub size: i64,
    pub mtime: i64,
}

/// A library file is a video or a song (by extension, see [`kind_of`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MediaKind {
    #[default]
    Video,
    Audio,
}

impl MediaKind {
    /// The name in the library database, the control socket and the UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }
    /// From [`MediaKind::as_str`]; anything else is `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            _ => None,
        }
    }
}

impl rusqlite::types::ToSql for MediaKind {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl rusqlite::types::FromSql for MediaKind {
    /// Rows always hold "video" or "audio"; anything else reads as a video (as the
    /// library treats files it does not know).
    fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        Ok(Self::parse(v.as_str()?).unwrap_or_default())
    }
}

pub fn kind_of(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if VIDEO_EXT.contains(&ext.as_str()) {
        Some(MediaKind::Video)
    } else if AUDIO_EXT.contains(&ext.as_str()) {
        Some(MediaKind::Audio)
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
    // WAL + synchronous=NORMAL: commits no longer fsync every time (still crash-safe
    // in WAL mode; at worst the last transaction is rolled back after a power loss).
    // The indexes are created on existing databases too (IF NOT EXISTS).
    c.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA busy_timeout=2000;
         CREATE TABLE IF NOT EXISTS media(
            path TEXT PRIMARY KEY, kind TEXT, title TEXT, artist TEXT, album TEXT,
            genre TEXT, track INTEGER, duration REAL, size INTEGER, mtime INTEGER);
         CREATE INDEX IF NOT EXISTS media_kind ON media(kind);
         CREATE INDEX IF NOT EXISTS media_artist_album ON media(artist, album);
         CREATE TABLE IF NOT EXISTS plays(
            path TEXT NOT NULL, started_at INTEGER NOT NULL, seconds REAL NOT NULL,
            completed INTEGER NOT NULL DEFAULT 0);
         CREATE INDEX IF NOT EXISTS plays_path ON plays(path, started_at);
         CREATE INDEX IF NOT EXISTS plays_started ON plays(started_at);",
    )?;
    migrate(&c)?;
    Ok(c)
}

/// Schema version of the derived columns below (`PRAGMA user_version`).
const SCHEMA: i64 = 2;

/// Version 2 (#100): the library UI filters, sorts and pages in SQL, so each row also
/// keeps its folder (`dir`), natural-order sort keys (`nt` title, `nar` artist, `nal`
/// album, `np` path; "Ep 2" < "Ep 10") and a lowercase search text (`find`). Older
/// databases get the columns and are backfilled once; no user-visible change.
fn migrate(c: &Connection) -> rusqlite::Result<()> {
    let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v >= SCHEMA {
        return Ok(());
    }
    let have: Vec<String> = c
        .prepare("PRAGMA table_info(media)")?
        .query_map([], |r| r.get(1))?
        .filter_map(Result::ok)
        .collect();
    for col in ["dir", "nt", "nar", "nal", "np", "find"] {
        if !have.iter().any(|h| h == col) {
            c.execute_batch(&format!("ALTER TABLE media ADD COLUMN {col} TEXT"))?;
        }
    }
    // Backfill in one transaction (a few seconds for 100k rows, once).
    let rows: Vec<Media> = all(c);
    c.execute_batch("BEGIN")?;
    {
        let mut st = c.prepare(
            "UPDATE media SET dir=?2, nt=?3, nar=?4, nal=?5, np=?6, find=?7 WHERE path=?1",
        )?;
        for m in &rows {
            let d = Derived::of(m);
            st.execute(params![m.path, d.dir, d.nt, d.nar, d.nal, d.np, d.find])?;
        }
    }
    c.execute_batch(&format!(
        "CREATE INDEX IF NOT EXISTS media_dir ON media(dir);
         CREATE INDEX IF NOT EXISTS media_kind_nt ON media(kind, nt);
         CREATE INDEX IF NOT EXISTS media_kind_mtime ON media(kind, mtime);
         CREATE INDEX IF NOT EXISTS media_kind_nar ON media(kind, nar, nt);
         CREATE INDEX IF NOT EXISTS media_kind_nal ON media(kind, nal, track);
         CREATE INDEX IF NOT EXISTS media_kind_genre ON media(kind, genre);
         PRAGMA user_version={SCHEMA};
         COMMIT;"
    ))?;
    Ok(())
}

/// The derived (query) columns of a row.
struct Derived {
    dir: String,
    nt: String,
    nar: String,
    nal: String,
    np: String,
    find: String,
}

impl Derived {
    fn of(m: &Media) -> Self {
        Self {
            dir: m
                .path
                .rsplit_once('/')
                .map_or(String::new(), |(d, _)| d.to_string()),
            nt: natural(&m.title),
            nar: natural(&m.artist),
            nal: natural(&m.album),
            np: natural(&m.path),
            find: format!(
                "{}\n{}\n{}\n{}\n{}",
                m.title, m.artist, m.album, m.genre, m.path
            )
            .to_lowercase(),
        }
    }
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
    scan_changes(conn, roots, false).map(|(n, _)| n)
}

/// A dot-named file or folder (hidden on Unix).
fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|n| n.starts_with('.'))
}

/// [`scan`] that also returns how many entries were added, updated or removed. Unchanged
/// files (same mtime as in the index) are only stat'ed, not re-read. Files and folders
/// whose names start with a dot are skipped unless `hidden` (Settings → Library → *Include hidden
/// files*).
pub fn scan_changes(
    conn: &mut Connection,
    roots: &[String],
    hidden: bool,
) -> rusqlite::Result<(usize, usize)> {
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
            // Dot-named files and folders (with everything in them) only with
            // *Include hidden files*; the root itself is always scanned.
            .filter_entry(|e| hidden || e.depth() == 0 || !is_hidden(e.file_name()))
            .filter_map(Result::ok)
        {
            if !e.file_type().is_file() {
                continue;
            }
            let p = e.path();
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
                kind,
                size: md.len() as i64,
                mtime,
                ..Default::default()
            };
            read_tags(p, &mut m);
            changed += 1;
            insert(&tx, &m)?;
        }
    }
    // Index paths use forward slashes, so the roots must too: a `C:\Videos` root kept
    // nothing and every scan dropped and re-read the whole library on Windows.
    let in_roots = |p: &str| {
        roots.iter().any(|r| {
            let r = r.replace('\\', "/");
            p.starts_with(&format!("{}/", r.trim_end_matches('/')))
        })
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

/// Write one row (and its derived query columns).
fn insert(c: &Connection, m: &Media) -> rusqlite::Result<()> {
    let d = Derived::of(m);
    c.execute(
        "INSERT OR REPLACE INTO media
            (path,kind,title,artist,album,genre,track,duration,size,mtime,dir,nt,nar,nal,np,find)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![
            m.path, m.kind, m.title, m.artist, m.album, m.genre, m.track, m.duration, m.size,
            m.mtime, d.dir, d.nt, d.nar, d.nal, d.np, d.find
        ],
    )?;
    Ok(())
}

/// Every row. Only for one-shot tools, the schema migration and tests: the UI pages
/// through [`query`] instead (#100).
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

// ---------- queries (#100) ----------

/// Columns read into a [`Media`], in [`row`] order.
pub(crate) const COLS: &str = "path,kind,title,artist,album,genre,track,duration,size,mtime";

pub(crate) fn row(r: &rusqlite::Row) -> rusqlite::Result<Media> {
    Ok(Media {
        path: r.get(0)?,
        kind: r.get::<_, Option<MediaKind>>(1)?.unwrap_or_default(),
        title: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        artist: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        album: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        genre: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        track: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
        duration: r.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
        size: r.get::<_, Option<i64>>(8)?.unwrap_or(0),
        mtime: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
    })
}

/// The group name of rows without the tag.
pub const UNKNOWN: &str = "Unknown";

/// A tag column a music view groups or filters by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Field {
    Album,
    Artist,
    Genre,
}

impl Field {
    pub(crate) fn col(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Artist => "artist",
            Self::Genre => "genre",
        }
    }
}

/// What a library page shows: a filter, an order and a page. Built by the UI and
/// turned into one `SELECT … WHERE … ORDER BY … LIMIT … OFFSET …`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub kind: Option<MediaKind>,
    /// Words that must all occur in title, artist, album, genre or path (any case).
    pub search: String,
    /// Only files directly in this folder.
    pub dir: Option<String>,
    /// Only files under this folder (any depth).
    pub under: Option<String>,
    /// Only rows whose tag equals the value ("" = no tag, shown as "Unknown").
    pub field: Option<(Field, String)>,
    /// A sort key of [`sort_with`] ("title", "newest", "artist-desc", …).
    pub sort: String,
    pub offset: usize,
    /// 0 = no limit.
    pub limit: usize,
}

/// `LIKE` pattern matching `s` literally anywhere.
fn like_any(s: &str) -> String {
    let mut out = String::from("%");
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// `LIKE` pattern for everything under folder `d`.
fn like_under(d: &str) -> String {
    let mut p = like_any(d.trim_end_matches('/'));
    p.remove(0);
    p.pop();
    p + "/%"
}

impl Query {
    /// The `WHERE` clause (with a leading space, or empty) and its parameters.
    fn filter(&self) -> (String, Vec<String>) {
        let mut w: Vec<String> = Vec::new();
        let mut p: Vec<String> = Vec::new();
        if let Some(k) = self.kind {
            p.push(k.as_str().into());
            w.push(format!("kind=?{}", p.len()));
        }
        for word in self.search.to_lowercase().split_whitespace() {
            p.push(like_any(word));
            w.push(format!("find LIKE ?{} ESCAPE '\\'", p.len()));
        }
        if let Some(d) = &self.dir {
            p.push(d.trim_end_matches('/').into());
            w.push(format!("dir=?{}", p.len()));
        }
        if let Some(d) = &self.under {
            p.push(like_under(d));
            w.push(format!("path LIKE ?{} ESCAPE '\\'", p.len()));
        }
        if let Some((f, v)) = &self.field {
            // Untagged rows are grouped as "Unknown" (see [`groups`]).
            p.push(v.clone());
            if v == UNKNOWN {
                w.push(format!("IFNULL({},'') IN ('', ?{})", f.col(), p.len()));
            } else {
                w.push(format!("{}=?{}", f.col(), p.len()));
            }
        }
        let clause = if w.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", w.join(" AND "))
        };
        (clause, p)
    }

    /// `ORDER BY` for the sort key; `None` for the watched / unwatched orders, which
    /// depend on resume positions outside the database (sorted in Rust, see [`query`]).
    fn order(&self) -> Option<&'static str> {
        let (key, desc) = match self.sort.strip_suffix("-desc") {
            Some(k) => (k, true),
            None => (self.sort.as_str(), false),
        };
        Some(match (key, desc) {
            ("watched" | "unwatched", _) => return None,
            ("newest" | "date", false) | ("oldest", true) => "mtime DESC, nt",
            ("oldest", false) | ("newest" | "date", true) => "mtime, nt",
            ("size", false) => "size DESC, nt",
            ("size", true) => "size, nt",
            ("artist", false) => "nar, nt",
            ("artist", true) => "nar DESC, nt DESC",
            ("album", false) => "nal, track",
            ("album", true) => "nal DESC, track DESC",
            ("track", false) => "track, nt",
            ("track", true) => "track DESC, nt DESC",
            ("uri", false) => "np",
            ("uri", true) => "np DESC",
            (_, false) => "nt, np",
            (_, true) => "nt DESC, np DESC",
        })
    }
}

/// One page of `q`, already filtered and sorted. `watched` decides the watched /
/// unwatched orders (resume positions live in mpv's watch-later files, not here).
pub fn query(conn: &Connection, q: &Query, watched: &dyn Fn(&Media) -> bool) -> Vec<Media> {
    let (clause, p) = q.filter();
    let Some(order) = q.order() else {
        let mut all = q.clone();
        all.sort = "title".into();
        all.offset = 0;
        all.limit = 0;
        let mut v = query(conn, &all, watched);
        sort_with(&mut v, &q.sort, watched);
        let end = if q.limit == 0 {
            v.len()
        } else {
            (q.offset + q.limit).min(v.len())
        };
        return v.drain(q.offset.min(end)..end).collect();
    };
    let limit = if q.limit == 0 { -1 } else { q.limit as i64 };
    let sql = format!(
        "SELECT {COLS} FROM media{clause} ORDER BY {order} LIMIT {limit} OFFSET {}",
        q.offset
    );
    select(conn, &sql, &p)
}

pub(crate) fn select(conn: &Connection, sql: &str, p: &[String]) -> Vec<Media> {
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    st.query_map(rusqlite::params_from_iter(p), row)
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Rows matching `q` (its sort and page are ignored).
pub fn count(conn: &Connection, q: &Query) -> usize {
    let (clause, p) = q.filter();
    conn.query_row(
        &format!("SELECT COUNT(*) FROM media{clause}"),
        rusqlite::params_from_iter(p),
        |r| r.get::<_, i64>(0),
    )
    .map_or(0, |n| n as usize)
}

/// Tag groups of the rows matching `q`: (value, songs, first path by title), by
/// natural order of the value. An empty tag is its own group ("").
pub fn groups(conn: &Connection, q: &Query, f: Field) -> Vec<(String, usize, String)> {
    let (clause, p) = q.filter();
    let col = f.col();
    let sql = format!(
        "SELECT IFNULL({col},''), COUNT(*), MIN(nt || char(1) || path) FROM media{clause}
         GROUP BY IFNULL({col},'')"
    );
    let Ok(mut st) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let mut v: Vec<(String, usize, String)> = st
        .query_map(rusqlite::params_from_iter(p), |r| {
            let first: String = r.get::<_, Option<String>>(2)?.unwrap_or_default();
            Ok((
                r.get(0)?,
                r.get::<_, i64>(1)? as usize,
                first
                    .split_once('\u{1}')
                    .map_or(first.clone(), |(_, p)| p.into()),
            ))
        })
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default();
    // Untagged rows join an "Unknown" group (and a literal "Unknown" tag).
    if let Some(i) = v.iter().position(|g| g.0.is_empty()) {
        let (_, n, first) = v.remove(i);
        match v.iter_mut().find(|g| g.0 == UNKNOWN) {
            Some(g) => g.1 += n,
            None => v.push((UNKNOWN.into(), n, first)),
        }
    }
    v.sort_by_cached_key(|g| natural(&g.0));
    v
}

/// Files per folder for the rows matching `q` (folders that hold files directly).
pub fn dir_counts(conn: &Connection, q: &Query) -> BTreeMap<String, usize> {
    let (clause, p) = q.filter();
    let sql = format!("SELECT IFNULL(dir,''), COUNT(*) FROM media{clause} GROUP BY dir");
    let Ok(mut st) = conn.prepare(&sql) else {
        return BTreeMap::new();
    };
    st.query_map(rusqlite::params_from_iter(p), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
    })
    .map(|it| it.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// The sub-folders of `dir` that hold matching files at any depth, with their file
/// counts, by name.
pub fn subdirs(conn: &Connection, q: &Query, dir: &str) -> Vec<(String, usize)> {
    let dir = dir.trim_end_matches('/');
    let mut under = q.clone();
    under.dir = None;
    under.under = Some(dir.into());
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for (d, n) in dir_counts(conn, &under) {
        if let Some(rest) = d.strip_prefix(dir).and_then(|r| r.strip_prefix('/')) {
            let first = rest.split('/').next().unwrap_or(rest);
            *out.entry(first.to_string()).or_default() += n;
        }
    }
    out.into_iter().collect()
}

/// Library rows of `paths` (missing ones are skipped), keyed by path.
pub fn by_paths(conn: &Connection, paths: &[String]) -> std::collections::HashMap<String, Media> {
    let mut out = std::collections::HashMap::new();
    for chunk in paths.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let sql = format!("SELECT {COLS} FROM media WHERE path IN ({marks})");
        for m in select(conn, &sql, chunk) {
            out.insert(m.path.clone(), m);
        }
    }
    out
}

/// Files, total size and total length of the rows matching `q`.
pub fn totals(conn: &Connection, q: &Query) -> (usize, i64, f64) {
    let (clause, p) = q.filter();
    conn.query_row(
        &format!(
            "SELECT COUNT(*), IFNULL(SUM(size),0), IFNULL(SUM(duration),0) FROM media{clause}"
        ),
        rusqlite::params_from_iter(p),
        |r| Ok((r.get::<_, i64>(0)? as usize, r.get(1)?, r.get(2)?)),
    )
    .unwrap_or((0, 0, 0.0))
}

/// Artists of the audio files with their song counts (non-empty artists only).
pub fn artist_counts(conn: &Connection) -> Vec<(String, usize)> {
    let Ok(mut st) = conn.prepare(
        "SELECT artist, COUNT(*) FROM media WHERE kind='audio' AND IFNULL(artist,'')<>''
         GROUP BY artist",
    ) else {
        return Vec::new();
    };
    st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// The library row of `path`, if it is in the library.
pub fn by_path(conn: &Connection, path: &str) -> Option<Media> {
    conn.query_row(
        "SELECT path, kind, title, artist, album, genre, track, duration, size, mtime
         FROM media WHERE path = ?1",
        [path],
        |r| {
            Ok(Media {
                path: r.get(0)?,
                kind: r.get::<_, Option<MediaKind>>(1)?.unwrap_or_default(),
                title: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                artist: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                album: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                genre: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                track: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                duration: r.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
                size: r.get::<_, Option<i64>>(8)?.unwrap_or(0),
                mtime: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
            })
        },
    )
    .ok()
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

/// [`crate::util::human_size`] for the library's signed sizes.
pub fn human_size(n: i64) -> String {
    crate::util::human_size(n.max(0) as u64)
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
    let seek = if m.kind == MediaKind::Video && m.duration > 0.0 {
        m.duration * 0.1
    } else if m.kind == MediaKind::Video {
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

/// Cover art (audio) or a frame at 5 s (video) as a 256 px PNG, for the shared
/// now-playing track (`crate::nowplaying`: MPRIS widgets, plugins).
pub fn make_art(path: &str, out: &Path) {
    if let Some(d) = out.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = out.with_extension("tmp.png");
    let audio = kind_of(Path::new(path)) == Some(MediaKind::Audio);
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin", "-y"]);
    if !audio {
        cmd.args(["-ss", "5"]);
    }
    let ok = cmd
        .args(["-i", path, "-an", "-frames:v", "1", "-vf", "scale=256:-2"])
        .arg(&tmp)
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        let _ = std::fs::rename(&tmp, out);
    } else {
        // No picture (audio without embedded cover art): remember it, so ffmpeg is not
        // run again every time the file plays.
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::write(no_art_marker(out), b"");
    }
}

/// Marker [`make_art`] leaves next to `out` when the file has no picture.
fn no_art_marker(out: &Path) -> PathBuf {
    out.with_extension("none")
}

/// [`make_art`] found no picture in `path` since it last changed (a file that gets a
/// cover later, e.g. from the Auto-tagger, is tried again).
pub fn known_without_art(path: &str, out: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    matches!(
        (modified(&no_art_marker(out)), modified(Path::new(path))),
        (Some(marker), Some(file)) if marker >= file
    )
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
    fn art_marker_for_files_without_pictures() {
        if !ffmpeg_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("vivi-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("not-audio.mp3");
        std::fs::write(&f, b"no picture here").unwrap();
        let src = f.to_string_lossy().into_owned();
        let out = dir.join("art.png");
        make_art(&src, &out);
        assert!(!out.exists());
        assert!(known_without_art(&src, &out));
        // A file changed after the attempt (new cover art) is tried again.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(!known_without_art(&src, &out));
        let _ = std::fs::remove_dir_all(&dir);
    }
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
        assert_eq!(scan_changes(&mut c, &roots, false).unwrap(), (2, 0));
        // Hidden files only with *Include hidden files*; off again, they leave the index.
        std::fs::write(dir.join(".clip.mkv"), b"x").unwrap();
        assert_eq!(scan(&mut c, &roots).unwrap(), 2);
        assert_eq!(scan_changes(&mut c, &roots, true).unwrap(), (3, 1));
        assert_eq!(scan(&mut c, &roots).unwrap(), 2);
        // The same goes for everything inside dot-named folders.
        std::fs::create_dir_all(dir.join(".stash")).unwrap();
        std::fs::write(dir.join(".stash/e02.mkv"), b"x").unwrap();
        assert_eq!(scan_changes(&mut c, &roots, false).unwrap(), (2, 0));
        assert_eq!(scan_changes(&mut c, &roots, true).unwrap(), (4, 2));
        assert_eq!(scan_changes(&mut c, &roots, false).unwrap(), (2, 2));
        std::fs::remove_dir_all(dir.join(".stash")).unwrap();
        std::fs::remove_file(dir.join(".clip.mkv")).unwrap();
        std::fs::remove_file(dir.join("song.mp3")).unwrap();
        scan(&mut c, &roots).unwrap();
        let a = all(&c);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, MediaKind::Video);
        assert_eq!(a[0].title, "e01");
        assert_eq!(by_path(&c, &a[0].path), Some(a[0].clone()));
        assert_eq!(by_path(&c, "/nope.mp3"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn old_databases_get_the_indexes() {
        let dir = std::env::temp_dir().join(format!("vivi-db-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("old.db");
        // A database written by an older version: the table only.
        Connection::open(&db)
            .unwrap()
            .execute_batch(
                "CREATE TABLE media(
                    path TEXT PRIMARY KEY, kind TEXT, title TEXT, artist TEXT, album TEXT,
                    genre TEXT, track INTEGER, duration REAL, size INTEGER, mtime INTEGER);
                 INSERT INTO media VALUES('/m/a.mp3','audio','a','x','y','',1,1.0,1,1);",
            )
            .unwrap();
        let c = open_db(&db).unwrap();
        let q = "SELECT name FROM sqlite_master \
                 WHERE type='index' AND sql IS NOT NULL ORDER BY name";
        let idx: Vec<String> = c
            .prepare(q)
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            idx,
            [
                "media_artist_album",
                "media_dir",
                "media_kind",
                "media_kind_genre",
                "media_kind_mtime",
                "media_kind_nal",
                "media_kind_nar",
                "media_kind_nt",
                "plays_path",
                "plays_started",
            ]
        );
        // The old row was backfilled with its folder and search text.
        let (dir, find): (String, String) = c
            .query_row("SELECT dir, find FROM media", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(dir, "/m");
        assert!(find.contains('x') && find.contains("/m/a.mp3"));
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA);
        let sync: i64 = c.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
        assert_eq!(sync, 1, "NORMAL");
        assert_eq!(all(&c).len(), 1);
        drop(c);
        // Opening again is a no-op.
        assert_eq!(all(&open_db(&db).unwrap()).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An in-memory index with `rows` (path, kind, title, artist, album, genre, mtime).
    fn index(rows: &[(&str, &str, &str, &str, &str, &str, i64)]) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE media(
                path TEXT PRIMARY KEY, kind TEXT, title TEXT, artist TEXT, album TEXT,
                genre TEXT, track INTEGER, duration REAL, size INTEGER, mtime INTEGER);",
        )
        .unwrap();
        migrate(&c).unwrap();
        for (i, r) in rows.iter().enumerate() {
            let m = Media {
                path: r.0.into(),
                kind: MediaKind::parse(r.1).unwrap(),
                title: r.2.into(),
                artist: r.3.into(),
                album: r.4.into(),
                genre: r.5.into(),
                track: i as i64 + 1,
                duration: 60.0,
                size: 100 * (i as i64 + 1),
                mtime: r.6,
            };
            insert(&c, &m).unwrap();
        }
        c
    }

    fn paths(v: &[Media]) -> Vec<&str> {
        v.iter().map(|m| m.path.as_str()).collect()
    }

    #[test]
    fn queries_filter_sort_and_page_in_sql() {
        let c = index(&[
            ("/v/Show/Ep 10.mkv", "video", "Ep 10", "", "", "", 3),
            ("/v/Show/Ep 2.mkv", "video", "Ep 2", "", "", "", 2),
            ("/v/Show/S2/Ep 1.mkv", "video", "Ep 1", "", "", "", 1),
            ("/v/film.mkv", "video", "Film", "", "", "", 4),
            ("/a/x.mp3", "audio", "Song X", "Band", "LP", "Rock", 5),
            ("/a/y.mp3", "audio", "Song Y", "", "", "", 6),
        ]);
        let no = |_: &Media| false;
        let videos = Query {
            kind: Some(MediaKind::Video),
            sort: "title".into(),
            ..Default::default()
        };
        // Natural order: "Ep 2" before "Ep 10".
        let v = query(&c, &videos, &no);
        assert_eq!(
            paths(&v),
            [
                "/v/Show/S2/Ep 1.mkv",
                "/v/Show/Ep 2.mkv",
                "/v/Show/Ep 10.mkv",
                "/v/film.mkv"
            ]
        );
        // A page.
        let page = Query {
            offset: 1,
            limit: 2,
            ..videos.clone()
        };
        assert_eq!(
            paths(&query(&c, &page, &no)),
            ["/v/Show/Ep 2.mkv", "/v/Show/Ep 10.mkv"]
        );
        assert_eq!(count(&c, &videos), 4);
        // One folder, its sub-folders, a subtree.
        let show = Query {
            dir: Some("/v/Show".into()),
            ..videos.clone()
        };
        assert_eq!(count(&c, &show), 2);
        assert_eq!(subdirs(&c, &videos, "/v/Show"), [("S2".to_string(), 1)]);
        assert_eq!(subdirs(&c, &videos, "/v"), [("Show".to_string(), 3)]);
        let under = Query {
            under: Some("/v/Show".into()),
            ..videos.clone()
        };
        assert_eq!(count(&c, &under), 3);
        // Search: every word, any case, LIKE wildcards taken literally.
        let s = |q: &str| Query {
            search: q.into(),
            ..Default::default()
        };
        assert_eq!(count(&c, &s("ep SHOW")), 3);
        assert_eq!(count(&c, &s("rock band")), 1);
        assert_eq!(count(&c, &s("%")), 0);
        // Newest first, and the watched order (decided outside the database).
        let newest = Query {
            sort: "newest".into(),
            limit: 2,
            ..Default::default()
        };
        assert_eq!(paths(&query(&c, &newest, &no)), ["/a/y.mp3", "/a/x.mp3"]);
        let watched = Query {
            sort: "watched".into(),
            ..videos.clone()
        };
        let film = |m: &Media| m.title == "Film";
        assert_eq!(query(&c, &watched, &film)[0].title, "Film");
        // Tag groups, untagged as "Unknown", and filtering by a group.
        let songs = Query {
            kind: Some(MediaKind::Audio),
            ..Default::default()
        };
        let g = groups(&c, &songs, Field::Artist);
        assert_eq!(
            g,
            [
                ("Band".to_string(), 1, "/a/x.mp3".to_string()),
                (UNKNOWN.to_string(), 1, "/a/y.mp3".to_string())
            ]
        );
        let unknown = Query {
            field: Some((Field::Artist, UNKNOWN.into())),
            ..songs.clone()
        };
        assert_eq!(paths(&query(&c, &unknown, &no)), ["/a/y.mp3"]);
        assert_eq!(totals(&c, &under), (3, 600, 180.0));
        assert_eq!(by_paths(&c, &["/a/x.mp3".into(), "/nope".into()]).len(), 1);
        assert_eq!(artist_counts(&c), [("Band".to_string(), 1)]);
    }

    /// `cargo test --release library_query_perf -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn library_query_perf() {
        const N: usize = 100_000;
        let c = index(&[]);
        c.execute_batch("BEGIN").unwrap();
        for i in 0..N {
            let m = Media {
                path: format!("/m/{}/{}/file {i}.mkv", i % 50, i % 7),
                kind: if i % 3 == 0 {
                    MediaKind::Audio
                } else {
                    MediaKind::Video
                },
                title: format!("File {i}"),
                artist: format!("Artist {}", i % 400),
                mtime: i as i64,
                ..Default::default()
            };
            insert(&c, &m).unwrap();
        }
        c.execute_batch("COMMIT").unwrap();
        let no = |_: &Media| false;
        let t = std::time::Instant::now();
        let q = Query {
            kind: Some(MediaKind::Video),
            sort: "title".into(),
            limit: 400,
            ..Default::default()
        };
        let page = query(&c, &q, &no);
        let first = t.elapsed();
        let t = std::time::Instant::now();
        let dirs = subdirs(&c, &q, "/m/7");
        let sub = t.elapsed();
        let t = std::time::Instant::now();
        let n = count(
            &c,
            &Query {
                search: "file 99".into(),
                ..q.clone()
            },
        );
        let search = t.elapsed();
        println!(
            "{N} rows: first page ({}) {first:?}, sub-folders ({}) {sub:?}, search ({n}) {search:?}",
            page.len(),
            dirs.len()
        );
    }
}
