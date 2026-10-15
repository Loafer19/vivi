//! Lyrics: synced (LRC) lyrics shown as a karaoke-style overlay that highlights the
//! current line. Sources, in order: a `.lrc` file next to the media file, the on-disk
//! cache (`$XDG_CACHE_HOME/vivi/lyrics`), then LRCLIB (<https://lrclib.net>) using the
//! artist / title / album / duration from the file tags (lofty) or mpv metadata.
//!
//! Network streams (radio): whenever the stream title (`icy-title`, else `media-title`,
//! usually "Artist - Title") changes, it is parsed and looked up on LRCLIB; ads, jingles
//! and station IDs are skipped and results are cached (memory + disk). Plain lyrics are
//! shown as a scrollable block, since the song's start time is unknown. Optionally
//! ("Radio lyrics: Approximate sync") synced lyrics are timed from the moment the title
//! changed, shifted by a manual offset.

use super::{Ctx, Manifest, Plugin, SettingSpec};
use slint::{ModelRc, SharedString, VecModel};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: f64,
}

/// Parsed LRC: lines sorted by time (seconds).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lrc {
    pub lines: Vec<(f64, String)>,
}

impl Lrc {
    /// Parse LRC text. Supports several time tags per line and the `[offset:±ms]` tag.
    pub fn parse(src: &str) -> Self {
        let mut offset = 0.0;
        let mut lines = Vec::new();
        for raw in src.lines() {
            let mut rest = raw.trim();
            let mut times = Vec::new();
            while let Some(r) = rest.strip_prefix('[') {
                let Some(end) = r.find(']') else { break };
                let tag = &r[..end];
                rest = &r[end + 1..];
                if let Some(o) = tag.strip_prefix("offset:") {
                    offset = o.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
                } else if let Some(t) = tag_time(tag) {
                    times.push(t);
                }
            }
            for t in times {
                lines.push((t, rest.trim().to_string()));
            }
        }
        // A positive offset shows lyrics earlier.
        for l in &mut lines {
            l.0 = (l.0 - offset).max(0.0);
        }
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        Lrc { lines }
    }
    /// Index of the line being sung at `t`.
    pub fn index_at(&self, t: f64) -> Option<usize> {
        let n = self.lines.partition_point(|l| l.0 <= t);
        n.checked_sub(1)
    }
}

/// "mm:ss.xx" → seconds.
fn tag_time(tag: &str) -> Option<f64> {
    let (m, s) = tag.split_once(':')?;
    let m: f64 = m.trim().parse().ok()?;
    let s: f64 = s.trim().replace(':', ".").parse().ok()?;
    Some(m * 60.0 + s)
}

/// `song.lrc` (or `song.LRC`) next to `song.flac`.
pub fn local_lrc(media: &Path) -> Option<PathBuf> {
    ["lrc", "LRC"]
        .iter()
        .map(|e| media.with_extension(e))
        .find(|p| p.is_file())
}

fn cache_file(t: &Track) -> PathBuf {
    let key = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        t.artist.to_lowercase(),
        t.title.to_lowercase(),
        t.album.to_lowercase(),
        t.duration.round()
    );
    crate::store::cache_dir()
        .join("lyrics")
        .join(format!("{:x}.lrc", md5::compute(key)))
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default, PartialEq)]
pub struct LrclibItem {
    #[serde(rename = "artistName", default)]
    artist: Option<String>,
    #[serde(rename = "trackName", default)]
    title: Option<String>,
    #[serde(rename = "syncedLyrics", default)]
    synced: Option<String>,
    #[serde(rename = "plainLyrics", default)]
    plain: Option<String>,
    #[serde(default)]
    instrumental: Option<bool>,
}

impl LrclibItem {
    fn has_lyrics(&self) -> bool {
        let ne = |s: &Option<String>| s.as_ref().is_some_and(|s| !s.trim().is_empty());
        ne(&self.synced) || ne(&self.plain)
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(12)))
        .user_agent(format!(
            "{} (https://github.com/Loafer19/vivi)",
            super::USER_AGENT
        ))
        .build()
        .into()
}

/// Synced lyrics from LRCLIB: `/api/get` (exact match), then `/api/search`.
pub fn fetch_lrclib(t: &Track) -> Result<Option<String>, String> {
    let a = agent();
    let mut req = a
        .get("https://lrclib.net/api/get")
        .query("artist_name", &t.artist)
        .query("track_name", &t.title);
    if !t.album.is_empty() {
        req = req.query("album_name", &t.album);
    }
    if t.duration > 0.0 {
        req = req.query("duration", format!("{}", t.duration.round()));
    }
    match req.call() {
        Ok(mut r) => {
            let it: LrclibItem = r.body_mut().read_json().map_err(|e| e.to_string())?;
            if let Some(s) = it.synced.filter(|s| !s.trim().is_empty()) {
                return Ok(Some(s));
            }
        }
        Err(ureq::Error::StatusCode(404)) => {}
        Err(e) => return Err(e.to_string()),
    }
    let mut r = a
        .get("https://lrclib.net/api/search")
        .query("artist_name", &t.artist)
        .query("track_name", &t.title)
        .call()
        .map_err(|e| e.to_string())?;
    let items: Vec<LrclibItem> = r.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(items
        .into_iter()
        .filter_map(|i| i.synced)
        .find(|s| !s.trim().is_empty()))
}

/// Artist/title/album/duration from tags (lofty), falling back to mpv metadata.
fn track_info(cx: &Ctx) -> Option<Track> {
    let mut t = Track::default();
    if let Some(p) = cx.local_path() {
        use lofty::prelude::*;
        if let Ok(tf) = lofty::read_from_path(&p) {
            t.duration = tf.properties().duration().as_secs_f64();
            if let Some(tag) = tf.primary_tag().or_else(|| tf.first_tag()) {
                let s = |v: Option<std::borrow::Cow<str>>| {
                    v.map(|x| x.trim().to_string()).unwrap_or_default()
                };
                t.artist = s(tag.artist());
                t.title = s(tag.title());
                t.album = s(tag.album());
            }
        }
    }
    let meta = |k: &str| {
        cx.player
            .get(&format!("metadata/by-key/{k}"))
            .or_else(|| {
                cx.player
                    .get(&format!("metadata/by-key/{}", k.to_uppercase()))
            })
            .unwrap_or_default()
    };
    if t.artist.is_empty() {
        t.artist = meta("artist");
    }
    if t.title.is_empty() {
        t.title = meta("title");
    }
    if t.album.is_empty() {
        t.album = meta("album");
    }
    if t.duration <= 0.0 {
        t.duration = cx.player.mpv.get_property("duration").unwrap_or(0.0);
    }
    (!t.artist.is_empty() && !t.title.is_empty()).then_some(t)
}

// ---------- Radio (network streams) ----------

/// Words that mark a stream title as an ad, jingle, station ID or talk segment.
const NOT_SONG: &[&str] = &[
    "advert",
    "commercial",
    "sponsor",
    "jingle",
    "station id",
    "promo",
    "spot block",
    "you're listening",
    "you are listening",
    "listening to",
    "now playing",
    "news",
    "weather",
    "traffic",
    "live stream",
    "on air",
    "unknown",
    "www.",
    "http",
    ".com",
    ".fm",
    "call us",
    "text us",
    "podcast",
];

/// "Artist - Title" from a stream title, or None for ads, station IDs and the like.
/// `station` (icy-name) is used to reject titles that are just the station's name.
pub fn parse_stream_title(raw: &str, station: &str) -> Option<(String, String)> {
    let s = raw.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    let low = s.to_lowercase();
    if s.len() < 3 || NOT_SONG.iter().any(|w| low.contains(w)) {
        return None;
    }
    let st = station.trim().to_lowercase();
    if !st.is_empty() && (low == st || low.starts_with(&format!("{st} -"))) {
        return None;
    }
    let (a, t) = [" - ", " – ", " — ", " ~ ", " | "]
        .iter()
        .find_map(|sep| s.split_once(sep))?;
    let artist = a.trim().to_string();
    let title = clean_title(t);
    let bad = |x: &str| x.is_empty() || x.chars().all(|c| !c.is_alphanumeric());
    if bad(&artist) || bad(&title) {
        return None;
    }
    Some((artist, title))
}

/// Drop decorations that LRCLIB does not know: "(Radio Edit)", "[Live]", "feat. X".
fn clean_title(t: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in t.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let low = out.to_lowercase();
    let cut = [" feat. ", " feat ", " ft. ", " ft "]
        .iter()
        .filter_map(|w| low.find(w))
        .min()
        .unwrap_or(out.len());
    out[..cut].split_whitespace().collect::<Vec<_>>().join(" ")
}

fn radio_cache_file(artist: &str, title: &str) -> PathBuf {
    let key = format!(
        "radio\u{1f}{}\u{1f}{}",
        artist.to_lowercase(),
        title.to_lowercase()
    );
    crate::store::cache_dir()
        .join("lyrics")
        .join(format!("{:x}.json", md5::compute(key)))
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Best LRCLIB entry for a radio title (no album or duration known): `/api/get`, then
/// `/api/search` keeping only results whose artist and title match (either way round,
/// some stations send "Title - Artist").
pub fn fetch_radio(artist: &str, title: &str) -> Result<Option<LrclibItem>, String> {
    let a = agent();
    match a
        .get("https://lrclib.net/api/get")
        .query("artist_name", artist)
        .query("track_name", title)
        .call()
    {
        Ok(mut r) => {
            let it: LrclibItem = r.body_mut().read_json().map_err(|e| e.to_string())?;
            if it.has_lyrics() {
                return Ok(Some(it));
            }
        }
        Err(ureq::Error::StatusCode(404)) => {}
        Err(e) => return Err(e.to_string()),
    }
    let mut r = a
        .get("https://lrclib.net/api/search")
        .query("q", format!("{artist} {title}"))
        .call()
        .map_err(|e| e.to_string())?;
    let items: Vec<LrclibItem> = r.body_mut().read_json().map_err(|e| e.to_string())?;
    let (na, nt) = (norm(artist), norm(title));
    let close =
        |x: &str, y: &str| !x.is_empty() && !y.is_empty() && (x.contains(y) || y.contains(x));
    let matches = |i: &LrclibItem| {
        let (ia, it) = (
            norm(i.artist.as_deref().unwrap_or("")),
            norm(i.title.as_deref().unwrap_or("")),
        );
        (close(&ia, &na) && close(&it, &nt)) || (close(&ia, &nt) && close(&it, &na))
    };
    let mut good: Vec<LrclibItem> = items
        .into_iter()
        .filter(|i| matches(i) && i.has_lyrics())
        .collect();
    // Prefer entries with synced lyrics.
    good.sort_by_key(|i| i.synced.as_ref().is_none_or(|s| s.trim().is_empty()));
    Ok(good.into_iter().next())
}

/// Plain text lines of an entry (from `plainLyrics`, else the synced text without tags).
fn plain_lines(it: &LrclibItem) -> Vec<String> {
    if let Some(p) = it.plain.as_ref().filter(|p| !p.trim().is_empty()) {
        return p.lines().map(|l| l.trim().to_string()).collect();
    }
    it.synced
        .as_deref()
        .map(|s| Lrc::parse(s).lines.into_iter().map(|(_, l)| l).collect())
        .unwrap_or_default()
}

/// Seconds from the "Radio sync offset" setting (positive = lyrics later).
fn parse_offset(s: &str) -> f64 {
    s.trim()
        .trim_end_matches('s')
        .trim()
        .replace(',', ".")
        .parse()
        .unwrap_or(0.0)
}

#[derive(Default)]
struct Radio {
    /// Raw stream title last seen.
    title: String,
    /// `time-pos` when it changed (approximate song start).
    since: f64,
    /// Lookup key ("artist\u{1f}title") whose result is displayed / awaited.
    key: String,
    mem: std::collections::HashMap<String, Option<LrclibItem>>,
    rx: Option<Receiver<(String, Option<LrclibItem>)>>,
    last_poll: Option<std::time::Instant>,
}

#[derive(Default)]
pub struct Lyrics {
    lrc: Option<Lrc>,
    index: Option<usize>,
    rx: Option<Receiver<(String, Option<String>)>>,
    path: Option<String>,
    /// Some while a network stream plays.
    radio: Option<Radio>,
    /// Synced lines are timed from the title change (radio, approximate sync).
    radio_synced: bool,
}

impl Lyrics {
    fn show(&mut self, cx: &Ctx, lrc: Option<Lrc>) {
        self.index = None;
        self.radio_synced = false;
        cx.app.set_lyrics_plain(false);
        let lines: Vec<SharedString> = lrc
            .as_ref()
            .map(|l| {
                l.lines
                    .iter()
                    .map(|(_, s)| SharedString::from(s.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        cx.app.set_lyrics_lines(ModelRc::new(VecModel::from(lines)));
        cx.app.set_lyrics_index(-1);
        self.lrc = lrc.filter(|l| !l.lines.is_empty());
    }
    fn apply_size(&self, cx: &Ctx) {
        cx.app.set_lyrics_size(match cx.get("size").as_str() {
            "Small" => 16.0,
            "Large" => 30.0,
            _ => 22.0,
        });
    }
    fn load(&mut self, cx: &Ctx) {
        self.rx = None;
        self.show(cx, None);
        self.path = cx.player.get("path");
        self.radio = None;
        let stream = self
            .path
            .as_deref()
            .is_some_and(|p| p.contains("://") && !p.starts_with("file://"));
        if stream {
            // Stream titles are polled in `tick`.
            self.radio = Some(Radio::default());
            return;
        }
        if let Some(p) = cx.local_path() {
            if let Some(l) = local_lrc(&p).and_then(|f| std::fs::read_to_string(f).ok()) {
                self.show(cx, Some(Lrc::parse(&l)));
                return;
            }
        }
        // Online lookup only for music (no real video track).
        if !crate::viz::audio_only(cx.player) {
            return;
        }
        let Some(t) = track_info(cx) else { return };
        let cache = cache_file(&t);
        if let Ok(s) = std::fs::read_to_string(&cache) {
            self.show(cx, Some(Lrc::parse(&s)));
            return;
        }
        if !cx.flag("online") || cache.with_extension("none").exists() {
            return;
        }
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let key = self.path.clone().unwrap_or_default();
        std::thread::spawn(move || {
            let res = fetch_lrclib(&t);
            if let Some(d) = cache.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            match &res {
                Ok(Some(s)) => {
                    let _ = std::fs::write(&cache, s);
                }
                Ok(None) => {
                    let _ = std::fs::write(cache.with_extension("none"), b"");
                }
                Err(e) => eprintln!("vivi: lyrics: {e}"),
            }
            let _ = tx.send((key, res.ok().flatten()));
        });
    }
}

impl Lyrics {
    /// Poll the stream title (twice a second) and look lyrics up when it changes.
    fn radio_tick(&mut self, cx: &Ctx) {
        let Some(r) = self.radio.as_mut() else { return };
        if r.last_poll
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        r.last_poll = Some(std::time::Instant::now());
        let raw = cx
            .player
            .get("metadata/by-key/icy-title")
            .filter(|s| !s.trim().is_empty())
            .or_else(|| cx.player.get("media-title"))
            .unwrap_or_default();
        if raw == r.title {
            return;
        }
        r.title = raw.clone();
        r.since = cx.player.mpv.get_property("time-pos").unwrap_or(0.0);
        r.rx = None;
        let station = cx
            .player
            .get("metadata/by-key/icy-name")
            .unwrap_or_default();
        let parsed = parse_stream_title(&raw, &station);
        let key = parsed
            .as_ref()
            .map(|(a, t)| format!("{a}\u{1f}{t}"))
            .unwrap_or_default();
        r.key = key.clone();
        let Some((artist, title)) = parsed else {
            self.show(cx, None); // ad / station ID / talk: hide
            return;
        };
        if let Some(hit) = r.mem.get(&key).cloned() {
            self.show_radio(cx, hit.as_ref());
            return;
        }
        let cache = radio_cache_file(&artist, &title);
        if let Ok(s) = std::fs::read_to_string(&cache) {
            if let Ok(it) = serde_json::from_str::<LrclibItem>(&s) {
                r.mem.insert(key, Some(it.clone()));
                self.show_radio(cx, Some(&it));
                return;
            }
        }
        let offline = !cx.flag("online") || cache.with_extension("none").exists();
        if offline {
            r.mem.insert(key.clone(), None);
        }
        let (tx, rx) = channel();
        if !offline {
            r.rx = Some(rx);
        }
        self.show(cx, None);
        if offline {
            return;
        }
        std::thread::spawn(move || {
            let res = fetch_radio(&artist, &title);
            if let Some(d) = cache.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            match &res {
                Ok(Some(it)) => {
                    if let Ok(j) = serde_json::to_string(it) {
                        let _ = std::fs::write(&cache, j);
                    }
                }
                Ok(None) => {
                    let _ = std::fs::write(cache.with_extension("none"), b"");
                }
                Err(e) => eprintln!("vivi: radio lyrics: {e}"),
            }
            let _ = tx.send((key, res.ok().flatten()));
        });
    }

    fn radio_result(&mut self, cx: &Ctx) {
        let Some(r) = self.radio.as_mut() else { return };
        let Some(rx) = &r.rx else { return };
        let Ok((key, it)) = rx.try_recv() else { return };
        r.rx = None;
        r.mem.insert(key.clone(), it.clone());
        if key == r.key {
            self.show_radio(cx, it.as_ref());
        }
    }

    fn show_radio(&mut self, cx: &Ctx, it: Option<&LrclibItem>) {
        let Some(it) = it.filter(|i| i.has_lyrics()) else {
            self.show(cx, None);
            return;
        };
        let synced = it
            .synced
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(Lrc::parse)
            .filter(|l| !l.lines.is_empty());
        match synced {
            Some(l) if cx.get("radio-mode") == "Approximate sync" => {
                self.show(cx, Some(l));
                self.radio_synced = true;
            }
            _ => {
                let lines: Vec<(f64, String)> =
                    plain_lines(it).into_iter().map(|l| (0.0, l)).collect();
                self.show(cx, Some(Lrc { lines }));
                cx.app.set_lyrics_plain(true);
            }
        }
    }
}

impl Plugin for Lyrics {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            "lyrics",
            "Lyrics",
            "Synced lyrics as a karaoke overlay, from a .lrc file next to the song or from LRCLIB.",
            "lyrics",
        )
        .setting(SettingSpec::toggle(
            "online",
            "Search lyrics online (LRCLIB)",
            "Sends artist, title, album and duration to lrclib.net. Results are cached.",
            true,
        ))
        .setting(SettingSpec::choice(
            "radio-mode",
            "Radio lyrics",
            "Network streams: lyrics are looked up when the stream title changes. The song's start is unknown, so synced lyrics can only be timed approximately from the title change.",
            &["Plain lyrics", "Approximate sync", "Off"],
        ))
        .setting(SettingSpec::text(
            "radio-offset",
            "Radio sync offset (seconds)",
            "Approximate sync only: positive shows lines later, negative earlier.",
            "0",
            "0",
        ))
        .setting(SettingSpec::choice(
            "size",
            "Text size",
            "",
            &["Medium", "Small", "Large"],
        ))
        .setting(SettingSpec::action(
            "clear-cache",
            "Downloaded lyrics",
            "Delete cached lyrics, including songs remembered as not found.",
            "Clear cache",
        ))
    }
    fn enable(&mut self, cx: &Ctx) {
        self.apply_size(cx);
        if cx.app.get_has_media() {
            self.load(cx);
        }
    }
    fn disable(&mut self, cx: &Ctx) {
        self.rx = None;
        self.show(cx, None);
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.load(cx);
    }
    fn setting_changed(&mut self, cx: &Ctx, key: &str) {
        match key {
            "size" => self.apply_size(cx),
            "online" | "radio-mode" => self.load(cx),
            _ => {}
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, _args: &[String]) {
        if name == "clear-cache" {
            let _ = std::fs::remove_dir_all(crate::store::cache_dir().join("lyrics"));
            cx.toast("Lyrics cache cleared");
            self.load(cx);
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        if self.radio.is_some() && cx.get("radio-mode") != "Off" {
            self.radio_result(cx);
            self.radio_tick(cx);
        }
        if let Some(rx) = &self.rx {
            if let Ok((key, res)) = rx.try_recv() {
                self.rx = None;
                if Some(&key) == self.path.as_ref() {
                    self.show(cx, res.map(|s| Lrc::parse(&s)));
                }
            }
        }
        let Some(l) = &self.lrc else { return };
        if self.radio.is_some() && !self.radio_synced {
            return; // plain lyrics: nothing to highlight
        }
        let mut t: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(0.0);
        if let (Some(r), true) = (&self.radio, self.radio_synced) {
            t -= r.since + parse_offset(&cx.get("radio-offset"));
        }
        let i = l.index_at(t + 0.15);
        if i != self.index {
            self.index = i;
            cx.app.set_lyrics_index(i.map(|i| i as i32).unwrap_or(-1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lrc() {
        let l = Lrc::parse(
            "[ar:Someone]\n[ti:Song]\n[00:12.00]First\n[00:05.50][01:00.00]Chorus\nno tag\n[00:20.5]  Third  ",
        );
        let t: Vec<f64> = l.lines.iter().map(|x| x.0).collect();
        assert_eq!(t, [5.5, 12.0, 20.5, 60.0]);
        assert_eq!(l.lines[2].1, "Third");
        assert_eq!(l.index_at(0.0), None);
        assert_eq!(l.index_at(12.0), Some(1));
        assert_eq!(l.index_at(30.0), Some(2));
        assert_eq!(l.index_at(1e9), Some(3));
    }

    #[test]
    fn offset_tag() {
        let l = Lrc::parse("[offset:+500]\n[00:10.00]A");
        assert_eq!(l.lines[0].0, 9.5);
    }

    #[test]
    fn local_file() {
        let d = std::env::temp_dir().join(format!("vivi-lrc-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let song = d.join("song.flac");
        assert_eq!(local_lrc(&song), None);
        std::fs::write(d.join("song.lrc"), "[00:01.00]x").unwrap();
        assert_eq!(local_lrc(&song), Some(d.join("song.lrc")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stream_titles() {
        let p = |s: &str| parse_stream_title(s, "Groove Salad");
        assert_eq!(
            p("Daft Punk - Get Lucky (Radio Edit) [feat. Pharrell]"),
            Some(("Daft Punk".into(), "Get Lucky".into()))
        );
        assert_eq!(
            p("Massive Attack – Teardrop ft. Liz Fraser"),
            Some(("Massive Attack".into(), "Teardrop".into()))
        );
        assert_eq!(p("Groove Salad"), None);
        assert_eq!(p("Groove Salad - Commercial Free"), None);
        assert_eq!(p("Advertisement - Buy now"), None);
        assert_eq!(p("You're listening to Radio X"), None);
        assert_eq!(p("www.radio.fm - best hits"), None);
        assert_eq!(p("Just a title"), None);
        assert_eq!(p(" - "), None);
        assert_eq!(parse_offset("-2.5"), -2.5);
        assert_eq!(parse_offset("3s"), 3.0);
        assert_eq!(parse_offset("x"), 0.0);
    }

    #[test]
    fn plain_from_synced() {
        let it = LrclibItem {
            synced: Some("[00:01.00]A\n[00:02.00]B".into()),
            ..Default::default()
        };
        assert_eq!(plain_lines(&it), ["A", "B"]);
        assert!(it.has_lyrics());
        assert!(!LrclibItem::default().has_lyrics());
    }

    /// Network test; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn lrclib_radio_live() {
        let it = fetch_radio("Daft Punk", "Get Lucky").unwrap().unwrap();
        assert!(!plain_lines(&it).is_empty());
        // Swapped "Title - Artist" still found through search.
        assert!(fetch_radio("Get Lucky", "Daft Punk").unwrap().is_some());
    }

    /// Network test; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn lrclib_live() {
        let t = Track {
            artist: "Borislav Slavov".into(),
            title: "I Want to Live".into(),
            album: "Baldur's Gate 3 (Original Game Soundtrack)".into(),
            duration: 233.0,
        };
        assert!(fetch_lrclib(&t).unwrap().is_some());
    }
}
