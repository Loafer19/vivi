//! Lyrics: synced (LRC) lyrics shown as a karaoke-style overlay that highlights the
//! current line. Sources, in order: a `.lrc` file next to the media file, the on-disk
//! cache (`$XDG_CACHE_HOME/vivi/lyrics`), then LRCLIB (<https://lrclib.net>) using the
//! artist / title / album / duration from the file tags (lofty) or mpv metadata.

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

#[derive(serde::Deserialize)]
struct LrclibItem {
    #[serde(rename = "syncedLyrics")]
    synced: Option<String>,
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

#[derive(Default)]
pub struct Lyrics {
    lrc: Option<Lrc>,
    index: Option<usize>,
    rx: Option<Receiver<(String, Option<String>)>>,
    path: Option<String>,
}

impl Lyrics {
    fn show(&mut self, cx: &Ctx, lrc: Option<Lrc>) {
        self.index = None;
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
            "online" => self.load(cx),
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
        if let Some(rx) = &self.rx {
            if let Ok((key, res)) = rx.try_recv() {
                self.rx = None;
                if Some(&key) == self.path.as_ref() {
                    self.show(cx, res.map(|s| Lrc::parse(&s)));
                }
            }
        }
        let Some(l) = &self.lrc else { return };
        let t: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(0.0);
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
