//! Shared, normalized "now playing" track for MPRIS, the now-playing card and plugins.
//!
//! Vivi merges what it knows about the playing entry, in this order per field:
//! metadata a package sent with `vivi-media-meta` (music services), the file's tags
//! (`metadata/by-key/*`), the stream title of Shoutcast / Icecast radio (`icy-title`,
//! "Artist - Title"), the library database, and mpv's `media-title` as the last resort.
//! The result is published as one mpv node map, `user-data/vivi/now-playing`, readable
//! from Lua (`mp.get_property_native`), JavaScript and mpv's JSON IPC:
//!
//! | key | value |
//! |---|---|
//! | `title`, `artist`, `album`, `album_artist`, `genre` | strings ("" when unknown) |
//! | `track` | track number (0 = unknown) |
//! | `duration` | seconds (0 = unknown / live) |
//! | `cover` | http(s) URL or `file://` PNG ("" = none) |
//! | `kind` | `music`, `video`, `podcast`, `stream` ("" while idle) |
//! | `source` | where title / artist came from: `plugin:<id>`, `tags`, `stream`, `library`, `file` |
//! | `path` | mpv `path` the values describe |
//! | `mbid` | MusicBrainz recording id (lower case) or "" |
//! | `service` | music service or station name ("" for local files) |
//! | `id` | change counter, +1 on every update |
//!
//! It is written whole (never half-updated) once the file is loaded and again only when
//! a value changes (a new ICY title, a cover that finished rendering, metadata a plugin
//! sent later). `user-data/vivi/now-playing-api` is the integer [`API_VERSION`] so
//! packages can tell a Vivi that publishes it from an older one.

use crate::player::Player;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

pub const KEY: &str = "user-data/vivi/now-playing";
pub const CAPABILITY: &str = "user-data/vivi/now-playing-api";
pub const API_VERSION: i64 = 1;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub track: i64,
    pub duration: f64,
    pub cover: String,
    pub kind: String,
    pub source: String,
    pub path: String,
    pub mbid: String,
    pub service: String,
}

/// Tag values read from mpv (trimmed, "" when missing).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub track: String,
    pub mbid: String,
    pub icy_title: String,
    pub icy_name: String,
}

/// Everything [`merge`] combines (gathered by [`gather`]; plain data for tests).
#[derive(Debug, Clone, Default)]
pub struct Sources {
    pub path: String,
    pub media_title: String,
    pub duration: f64,
    /// A real video track (not cover art / a still image).
    pub video: bool,
    pub tags: Tags,
    /// (package id, metadata) from `vivi-media-meta` for this URL.
    pub plugin: Option<(String, crate::plugins::host::Meta)>,
    pub library: Option<crate::library::Media>,
    /// `file://` URL of the rendered cover / frame, when ready.
    pub art: String,
}

fn first(v: &[&str]) -> String {
    v.iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Leading number of a "3/12" track tag (0 = none).
pub fn track_no(s: &str) -> i64 {
    let d: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    d.parse().unwrap_or(0)
}

/// A MusicBrainz id in lower case, or "" when `s` is not one (the first of "a; b").
pub fn mbid(s: &str) -> String {
    let s = s
        .split([';', '/', ','])
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let ok = s.len() == 36
        && s.char_indices().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        });
    if ok {
        s
    } else {
        String::new()
    }
}

/// Artist / title separators of ICY stream titles: hyphen, en and em dash.
const ICY_DASHES: [&str; 3] = [" - ", " – ", " — "];
/// Also taken by [`split_icy_loose`]: rarer separators, fine where a wrong split
/// only costs a lookup (lyrics) but not for what the player shows.
const ICY_LOOSE: [&str; 2] = [" ~ ", " | "];

/// "Artist - Title" of an ICY stream title: (artist, title); no dash = title only.
/// The first separator kind found wins, split at its first occurrence.
pub fn split_icy(s: &str) -> (String, String) {
    split_on(s, &ICY_DASHES)
}

/// [`split_icy`] that also splits at " ~ " and " | " when no dash is found.
pub fn split_icy_loose(s: &str) -> (String, String) {
    let (a, t) = split_icy(s);
    if a.is_empty() {
        split_on(s, &ICY_LOOSE)
    } else {
        (a, t)
    }
}

fn split_on(s: &str, seps: &[&str]) -> (String, String) {
    match seps.iter().find_map(|sep| s.split_once(sep)) {
        Some((a, t)) if !a.trim().is_empty() && !t.trim().is_empty() => {
            (a.trim().to_string(), t.trim().to_string())
        }
        _ => (String::new(), s.trim().to_string()),
    }
}

fn is_url(path: &str) -> bool {
    path.contains("://") && !path.starts_with("file://")
}

const KINDS: [&str; 4] = ["music", "video", "podcast", "stream"];

/// Merge the sources into one track (see the module docs for the order).
pub fn merge(s: &Sources) -> Track {
    if s.path.is_empty() {
        return Track::default();
    }
    let t = &s.tags;
    let pm = s.plugin.as_ref().map(|(_, m)| m);
    let pm_s = |f: fn(&crate::plugins::host::Meta) -> &str| pm.map(f).unwrap_or_default();
    let lib = s.library.as_ref();
    let lib_s = |f: fn(&crate::library::Media) -> &str| lib.map(f).unwrap_or_default();
    let (icy_artist, icy_title) = split_icy(&t.icy_title);
    let tagged = !t.title.is_empty() || !t.artist.is_empty();
    let icy = !t.icy_title.is_empty() && !tagged;
    let title = first(&[
        pm_s(|m| m.title.as_str()),
        &t.title,
        if icy { icy_title.as_str() } else { "" },
        lib_s(|m| m.title.as_str()),
        &s.media_title,
    ]);
    let artist = first(&[
        pm_s(|m| m.artist.as_str()),
        &t.artist,
        if icy { icy_artist.as_str() } else { "" },
        lib_s(|m| m.artist.as_str()),
    ]);
    let source = if let Some((id, m)) = &s.plugin {
        if !m.title.trim().is_empty() {
            format!("plugin:{id}")
        } else if tagged {
            "tags".into()
        } else {
            format!("plugin:{id}")
        }
    } else if tagged {
        "tags".into()
    } else if icy {
        "stream".into()
    } else if lib.is_some_and(|m| !m.title.is_empty() || !m.artist.is_empty()) {
        "library".into()
    } else {
        "file".into()
    };
    let genre = first(&[&t.genre, lib_s(|m| m.genre.as_str())]);
    let track = match track_no(&t.track) {
        0 => lib.map(|m| m.track).unwrap_or(0),
        n => n,
    };
    let duration = if s.duration > 0.0 {
        s.duration
    } else {
        pm.map(|m| m.duration)
            .filter(|d| *d > 0.0)
            .or(lib.map(|m| m.duration))
            .unwrap_or(0.0)
    };
    let ext = Path::new(&s.path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let g = genre.to_lowercase();
    let asked = pm_s(|m| m.kind.as_str()).trim().to_lowercase();
    let kind = if KINDS.contains(&asked.as_str()) {
        asked
    } else if s.video {
        "video".into()
    } else if g.contains("podcast") || g.contains("audiobook") || ext == "m4b" {
        "podcast".into()
    } else if is_url(&s.path) && pm.is_none() && (!t.icy_title.is_empty() || duration <= 0.0) {
        "stream".into()
    } else {
        "music".into()
    };
    let service = first(&[
        pm_s(|m| m.service.as_str()),
        pm_s(|m| m.info.as_str()),
        if is_url(&s.path) {
            t.icy_name.as_str()
        } else {
            ""
        },
    ]);
    Track {
        title,
        artist,
        album: first(&[
            pm_s(|m| m.album.as_str()),
            &t.album,
            lib_s(|m| m.album.as_str()),
        ]),
        album_artist: first(&[pm_s(|m| m.album_artist.as_str()), &t.album_artist]),
        genre,
        track,
        duration,
        cover: first(&[pm_s(|m| m.cover.as_str()), &s.art]),
        kind,
        source,
        path: s.path.clone(),
        mbid: [pm_s(|m| m.mbid.as_str()), t.mbid.as_str()]
            .iter()
            .map(|v| mbid(v))
            .find(|v| !v.is_empty())
            .unwrap_or_default(),
        service,
    }
}

#[derive(Default)]
struct State {
    cur: Track,
    id: i64,
    /// Path mpv finished loading (`file-loaded`); nothing is published for a new path
    /// before its tags are there.
    loaded: String,
    /// Path the art / library lookups were started for.
    looked_up: String,
    library: Option<crate::library::Media>,
    player: Option<Rc<Player>>,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

/// The track last published (default while idle).
pub fn current() -> Track {
    ST.with(|s| s.borrow().cur.clone())
}

/// Change counter of [`current`].
pub fn current_id() -> i64 {
    ST.with(|s| s.borrow().id)
}

/// Rendered cover / frame of a local file (`<cache>/art/<md5>.png`, made by ffmpeg).
fn art_file(path: &str) -> std::path::PathBuf {
    crate::store::cache_dir()
        .join("art")
        .join(format!("{:x}.png", md5::compute(path.as_bytes())))
}

fn gather(p: &Player, path: &str, library: Option<crate::library::Media>) -> Sources {
    let tag = |keys: &[&str]| {
        keys.iter()
            .filter_map(|k| p.get(&format!("metadata/by-key/{k}")))
            .map(|v| v.trim().to_string())
            .find(|v| !v.is_empty())
            .unwrap_or_default()
    };
    let yes = |k: &str| p.get(k).as_deref() == Some("yes");
    let video = p.get("current-tracks/video/id").is_some()
        && !yes("current-tracks/video/albumart")
        && !yes("current-tracks/video/image");
    let art = art_file(path);
    Sources {
        path: path.to_string(),
        media_title: p.get("media-title").unwrap_or_default(),
        duration: p.mpv.get_property::<f64>("duration").unwrap_or(0.0),
        video,
        tags: Tags {
            title: tag(&["title"]),
            artist: tag(&["artist"]),
            album: tag(&["album"]),
            album_artist: tag(&["album_artist", "albumartist", "album artist"]),
            genre: tag(&["genre"]),
            track: tag(&["track", "tracknumber"]),
            mbid: tag(&["musicbrainz_trackid", "MusicBrainz Track Id"]),
            icy_title: tag(&["icy-title"]),
            icy_name: tag(&["icy-name"]),
        },
        plugin: crate::plugins::host::meta_for(path),
        library,
        art: if art.is_file() {
            format!("file://{}", art.display())
        } else {
            String::new()
        },
    }
}

/// Library row of a local file, if the library knows it.
fn library_row(path: &str) -> Option<crate::library::Media> {
    let db = crate::library::db_path();
    if is_url(path) || !db.is_file() {
        return None;
    }
    let c = crate::library::open_db(&db).ok()?;
    crate::library::by_path(&c, path)
}

/// Re-read the playing entry and publish it when anything changed.
pub fn refresh() {
    let Some(p) = ST.with(|s| s.borrow().player.clone()) else {
        return;
    };
    let path = p.get("path").unwrap_or_default();
    let loaded = ST.with(|s| s.borrow().loaded.clone());
    if !path.is_empty() && path != loaded {
        return; // still opening: keep the old values until the tags are read
    }
    if !path.is_empty() && ST.with(|s| s.borrow().looked_up != path) {
        let lib = library_row(&path);
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.looked_up = path.clone();
            s.library = lib;
        });
        let out = art_file(&path);
        if Path::new(&path).is_file() && !out.exists() && crate::library::ffmpeg_available() {
            let pc = path.clone();
            std::thread::spawn(move || crate::library::make_art(&pc, &out));
        }
    }
    let lib = ST.with(|s| s.borrow().library.clone());
    let t = if path.is_empty() {
        Track::default()
    } else {
        merge(&gather(&p, &path, lib))
    };
    let changed = ST.with(|s| {
        let mut s = s.borrow_mut();
        if s.cur == t && s.id > 0 {
            return None;
        }
        s.cur = t.clone();
        s.id += 1;
        Some(s.id)
    });
    if let Some(id) = changed {
        publish(&p, &t, id);
    }
}

/// mpv loaded a file: publish its track right away.
pub fn file_loaded() {
    let path = ST.with(|s| {
        s.borrow()
            .player
            .as_ref()
            .and_then(|p| p.get("path"))
            .unwrap_or_default()
    });
    ST.with(|s| s.borrow_mut().loaded = path);
    refresh();
}

/// Set the capability flag, publish the idle state and poll every 500 ms (stream titles,
/// covers rendered in the background, metadata sent later).
pub fn setup(player: &Rc<Player>) {
    if let Err(e) = player.mpv.set_property(CAPABILITY, API_VERSION) {
        eprintln!("vivi: set {CAPABILITY}: {e:?}");
    }
    ST.with(|s| s.borrow_mut().player = Some(player.clone()));
    refresh();
    let timer = Box::leak(Box::new(slint::Timer::default()));
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        refresh,
    );
}

/// One string / int / double value of the node map.
enum Val<'a> {
    S(&'a str),
    I(i64),
    F(f64),
}

fn fields(t: &Track, id: i64) -> Vec<(&'static str, Val<'_>)> {
    use Val::*;
    vec![
        ("title", S(&t.title)),
        ("artist", S(&t.artist)),
        ("album", S(&t.album)),
        ("album_artist", S(&t.album_artist)),
        ("genre", S(&t.genre)),
        ("track", I(t.track)),
        ("duration", F(t.duration)),
        ("cover", S(&t.cover)),
        ("kind", S(&t.kind)),
        ("source", S(&t.source)),
        ("path", S(&t.path)),
        ("mbid", S(&t.mbid)),
        ("service", S(&t.service)),
        ("id", I(id)),
    ]
}

/// Set [`KEY`] to the track as one node map (atomic for readers).
fn publish(p: &Player, t: &Track, id: i64) {
    use libmpv2_sys as sys;
    use std::ffi::CString;
    let cstr = |s: &str| CString::new(s.replace('\0', "")).unwrap_or_default();
    let f = fields(t, id);
    let keys: Vec<CString> = f.iter().map(|(k, _)| cstr(k)).collect();
    let strs: Vec<Option<CString>> = f
        .iter()
        .map(|(_, v)| match v {
            Val::S(s) => Some(cstr(s)),
            _ => None,
        })
        .collect();
    let mut values: Vec<sys::mpv_node> = f
        .iter()
        .zip(&strs)
        .map(|((_, v), s)| {
            // SAFETY: mpv_node is plain C data; every union member is valid when zeroed.
            let mut n: sys::mpv_node = unsafe { std::mem::zeroed() };
            match v {
                Val::S(_) => {
                    n.format = sys::mpv_format_MPV_FORMAT_STRING;
                    n.u.string = s
                        .as_ref()
                        .map_or(std::ptr::null_mut(), |c| c.as_ptr() as *mut _);
                }
                Val::I(i) => {
                    n.format = sys::mpv_format_MPV_FORMAT_INT64;
                    n.u.int64 = *i;
                }
                Val::F(x) => {
                    n.format = sys::mpv_format_MPV_FORMAT_DOUBLE;
                    n.u.double_ = *x;
                }
            }
            n
        })
        .collect();
    let mut key_ptrs: Vec<*mut std::os::raw::c_char> =
        keys.iter().map(|k| k.as_ptr() as *mut _).collect();
    let mut list = sys::mpv_node_list {
        num: values.len() as _,
        values: values.as_mut_ptr(),
        keys: key_ptrs.as_mut_ptr(),
    };
    // SAFETY: see above.
    let mut root: sys::mpv_node = unsafe { std::mem::zeroed() };
    root.format = sys::mpv_format_MPV_FORMAT_NODE_MAP;
    root.u.list = &mut list;
    let name = cstr(KEY);
    // SAFETY: valid handle owned by `p.mpv`; mpv copies the node during the call and every
    // pointer in it (keys, strings, values, list) outlives the call.
    let r = unsafe {
        sys::mpv_set_property(
            p.mpv.ctx.as_ptr(),
            name.as_ptr(),
            sys::mpv_format_MPV_FORMAT_NODE,
            &mut root as *mut sys::mpv_node as *mut std::os::raw::c_void,
        )
    };
    if r < 0 {
        eprintln!("vivi: set {KEY}: mpv error {r}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::host::Meta;

    fn tags(title: &str, artist: &str) -> Tags {
        Tags {
            title: title.into(),
            artist: artist.into(),
            ..Default::default()
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(track_no("3/12"), 3);
        assert_eq!(track_no(" 07 "), 7);
        assert_eq!(track_no("x"), 0);
        let id = "0C2D4A3E-6C1B-4E8B-9A0B-1234567890AB";
        assert_eq!(mbid(id), id.to_ascii_lowercase());
        assert_eq!(mbid(&format!("{id}; other")), id.to_ascii_lowercase());
        assert_eq!(mbid("not-an-id"), "");
        assert_eq!(
            split_icy("Band - Song - Live"),
            ("Band".into(), "Song - Live".into())
        );
        assert_eq!(
            split_icy("Station jingle"),
            ("".into(), "Station jingle".into())
        );
        assert_eq!(
            split_icy("Massive Attack – Teardrop"),
            ("Massive Attack".into(), "Teardrop".into())
        );
        assert_eq!(split_icy("Band — Song"), ("Band".into(), "Song".into()));
        assert_eq!(split_icy(" - Song"), ("".into(), "- Song".into()));
        assert_eq!(
            split_icy("Station | Morning Show"),
            ("".into(), "Station | Morning Show".into()),
            "only dashes for what the player shows"
        );
        assert_eq!(
            split_icy_loose("Band | Song"),
            ("Band".into(), "Song".into())
        );
        assert_eq!(
            split_icy_loose("Band ~ Song"),
            ("Band".into(), "Song".into())
        );
        assert_eq!(
            split_icy_loose("A | B - C"),
            ("A | B".into(), "C".into()),
            "a dash wins"
        );
    }

    #[test]
    fn idle_is_empty() {
        assert_eq!(merge(&Sources::default()), Track::default());
    }

    #[test]
    fn tags_then_library_then_file_name() {
        let mut s = Sources {
            path: "/m/a.flac".into(),
            media_title: "a.flac".into(),
            duration: 200.0,
            tags: Tags {
                album_artist: "Various".into(),
                track: "4/10".into(),
                genre: "Rock".into(),
                mbid: "0c2d4a3e-6c1b-4e8b-9a0b-1234567890ab".into(),
                ..tags("Song", "Band")
            },
            art: "file:///c/art.png".into(),
            ..Default::default()
        };
        let t = merge(&s);
        assert_eq!((t.title.as_str(), t.artist.as_str()), ("Song", "Band"));
        assert_eq!(
            (t.track, t.kind.as_str(), t.source.as_str()),
            (4, "music", "tags")
        );
        assert_eq!(t.album_artist, "Various");
        assert_eq!(t.cover, "file:///c/art.png");
        assert_eq!(t.mbid.len(), 36);
        assert_eq!(t.service, "");
        // No tags: the library row, then mpv's media title.
        s.tags = Tags::default();
        s.library = Some(crate::library::Media {
            title: "Lib song".into(),
            artist: "Lib band".into(),
            track: 2,
            ..Default::default()
        });
        let t = merge(&s);
        assert_eq!(
            (t.title.as_str(), t.source.as_str(), t.track),
            ("Lib song", "library", 2)
        );
        s.library = None;
        let t = merge(&s);
        assert_eq!(
            (t.title.as_str(), t.artist.as_str(), t.source.as_str()),
            ("a.flac", "", "file")
        );
    }

    #[test]
    fn plugin_metadata_wins() {
        let s = Sources {
            path: "https://music.example/watch?v=1".into(),
            media_title: "watch?v=1".into(),
            duration: 180.0,
            video: true,
            tags: tags("Some upload title", ""),
            plugin: Some((
                "ytmusic".into(),
                Meta {
                    title: "Song".into(),
                    artist: "Band".into(),
                    album: "LP".into(),
                    cover: "https://img.example/c.jpg".into(),
                    info: "YouTube Music".into(),
                    kind: "music".into(),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        let t = merge(&s);
        assert_eq!(
            (t.title.as_str(), t.artist.as_str(), t.album.as_str()),
            ("Song", "Band", "LP")
        );
        assert_eq!(t.source, "plugin:ytmusic");
        assert_eq!(t.service, "YouTube Music");
        assert_eq!(t.cover, "https://img.example/c.jpg");
        // The plugin said music: its video track does not make it a video.
        assert_eq!(t.kind, "music");
        let mut s2 = s.clone();
        if let Some((_, m)) = s2.plugin.as_mut() {
            m.kind.clear();
        }
        assert_eq!(merge(&s2).kind, "video");
    }

    #[test]
    fn radio_stream_titles() {
        let s = Sources {
            path: "http://radio.example:8000/live".into(),
            media_title: "Band - Song".into(),
            tags: Tags {
                icy_title: "Band - Song".into(),
                icy_name: "Example FM".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let t = merge(&s);
        assert_eq!((t.title.as_str(), t.artist.as_str()), ("Song", "Band"));
        assert_eq!((t.kind.as_str(), t.source.as_str()), ("stream", "stream"));
        assert_eq!(t.service, "Example FM");
    }

    #[test]
    fn kinds() {
        let mut s = Sources {
            path: "/v/movie.mkv".into(),
            media_title: "movie.mkv".into(),
            duration: 5000.0,
            video: true,
            ..Default::default()
        };
        assert_eq!(merge(&s).kind, "video");
        s.video = false;
        s.path = "/b/book.m4b".into();
        assert_eq!(merge(&s).kind, "podcast");
        s.path = "/p/ep.mp3".into();
        s.tags.genre = "Podcast".into();
        assert_eq!(merge(&s).kind, "podcast");
    }

    #[test]
    fn node_fields_cover_every_key() {
        let t = Track::default();
        let f = fields(&t, 3);
        let keys: Vec<&str> = f.iter().map(|(k, _)| *k).collect();
        for k in [
            "title",
            "artist",
            "album",
            "album_artist",
            "track",
            "duration",
            "cover",
            "kind",
            "source",
            "path",
            "mbid",
            "service",
            "id",
        ] {
            assert!(keys.contains(&k), "{k}");
        }
    }
}
