//! Saved / recent streams and self-refreshing custom playlists ("My playlists").

use super::*;

// ---------- saved / recent ----------

pub(crate) fn dir() -> PathBuf {
    crate::store::data_dir().join("streams")
}

/// Put `s` first in `list` (dedup by URL), keep at most `max`.
#[cfg(test)]
pub fn push_front(list: &mut Vec<Stream>, s: &Stream, max: usize) {
    list.retain(|o| o.url != s.url);
    list.insert(0, s.clone());
    list.truncate(max);
}

/// Stream info of what is playing now, from mpv properties.
pub fn now_info(get: impl Fn(&str) -> Option<String>) -> String {
    let mut p = Vec::new();
    let title = get("metadata/by-key/icy-title")
        .or_else(|| get("media-title"))
        .unwrap_or_default();
    if !title.is_empty() {
        p.push(format!("Now playing: {title}"));
    }
    if let Some(n) = get("metadata/by-key/icy-name").filter(|s| !s.is_empty()) {
        p.push(format!("Station: {n}"));
    }
    let codec = get("audio-codec-name").unwrap_or_default();
    let br = get("audio-bitrate")
        .and_then(|b| b.parse::<f64>().ok())
        .filter(|b| *b > 0.0)
        .map(|b| format!(" {:.0} kbps", b / 1000.0))
        .unwrap_or_default();
    if !codec.is_empty() {
        p.push(format!("Audio: {codec}{br}"));
    }
    if let Some(v) = get("video-codec").filter(|s| !s.is_empty()) {
        p.push(format!("Video: {v}"));
    }
    if let Some(c) = get("demuxer-cache-duration").and_then(|c| c.parse::<f64>().ok()) {
        p.push(format!("Buffer: {c:.1} s"));
    }
    if p.is_empty() {
        "Nothing is playing".into()
    } else {
        p.join("\n")
    }
}

// ---------- My playlists ----------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub added: u64,
    /// Last successful refresh (Unix seconds, 0 = never).
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub count: usize,
    /// Last refresh error ("" when it worked).
    #[serde(default)]
    pub error: String,
}

pub(crate) fn now_secs() -> u64 {
    crate::plugins::epg::now().max(0) as u64
}

pub(crate) fn playlists() -> Vec<Playlist> {
    std::fs::read_to_string(dir().join("playlists.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub(crate) fn save_playlists(v: &[Playlist]) {
    let d = dir();
    let _ = std::fs::create_dir_all(&d);
    if let Ok(t) = serde_json::to_string_pretty(v) {
        let tmp = d.join(".playlists.json.tmp");
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, d.join("playlists.json"));
        }
    }
}

pub(crate) fn update_playlist(url: &str, f: impl FnOnce(&mut Playlist)) {
    let mut v = playlists();
    if let Some(p) = v.iter_mut().find(|p| p.url == url) {
        f(p);
        save_playlists(&v);
    }
}

/// "3 h ago" style age of a Unix time.
pub fn ago(t: u64, now: u64) -> String {
    if t == 0 {
        return "never".into();
    }
    let s = now.saturating_sub(t);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

/// Name for a playlist URL / path: its file name without the extension.
pub fn playlist_name(src: &str) -> String {
    let base = src
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(src)
        .split('?')
        .next()
        .unwrap_or("");
    let base = display_name(base);
    let n = base
        .strip_suffix(".m3u8")
        .or_else(|| base.strip_suffix(".m3u"))
        .unwrap_or(&base);
    if n.is_empty() {
        src.to_string()
    } else {
        n.to_string()
    }
}

pub(crate) fn playlist_cache(url: &str) -> PathBuf {
    hashed_file(&crate::store::cache_dir().join("iptv"), url, |h| {
        format!("custom-{h}.m3u")
    })
}

/// Channels of a custom playlist. URLs are cached and refreshed after `ttl` (or when
/// `force`); a failed refresh keeps the last good copy and records the error.
pub fn fetch_playlist(src: &str, ttl: u64, force: bool) -> Result<Vec<Stream>, String> {
    if !(src.starts_with("http://") || src.starts_with("https://")) {
        let r = load_m3u(src);
        let now = now_secs();
        update_playlist(src, |p| match &r {
            Ok(v) => {
                p.updated = now;
                p.count = v.len();
                p.error.clear();
            }
            Err(e) => p.error = e.clone(),
        });
        return r;
    }
    let c = playlist_cache(src);
    let age = std::fs::metadata(&c)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|e| e.as_secs());
    if !force && age.is_some_and(|a| a < ttl) {
        if let Ok(t) = std::fs::read_to_string(&c) {
            return Ok(parse_m3u(&t));
        }
    }
    let got = web::get_bytes(src, 32 * 1024 * 1024).and_then(|b| {
        let t = String::from_utf8_lossy(&b.ok_or("Playlist not found (404)")?).into_owned();
        let v = parse_m3u(&t);
        if v.is_empty() {
            return Err("No stream URLs in the playlist".into());
        }
        Ok((t, v))
    });
    let now = now_secs();
    match got {
        Ok((t, v)) => {
            let _ = std::fs::create_dir_all(c.parent().unwrap());
            let _ = std::fs::write(&c, t);
            update_playlist(src, |p| {
                p.updated = now;
                p.count = v.len();
                p.error.clear();
            });
            Ok(v)
        }
        Err(e) => {
            update_playlist(src, |p| p.error = e.clone());
            match std::fs::read_to_string(&c) {
                Ok(t) => Ok(parse_m3u(&t)),
                Err(_) => Err(e),
            }
        }
    }
}

/// Picker rows of "My playlists".
pub(crate) fn playlist_picks(v: &[Playlist]) -> Vec<Stream> {
    let now = now_secs();
    v.iter()
        .map(|p| {
            let mut l = format!("{} channels · updated {}", p.count, ago(p.updated, now));
            if !p.error.is_empty() {
                l += &format!(" · ⚠ {}", p.error.chars().take(80).collect::<String>());
            }
            let mut s = pick(&p.name, &l, &format!("{PL}{}", p.url));
            s.homepage = p.url.clone();
            s
        })
        .collect()
}

/// Refresh playlists older than `ttl`, then list them.
pub(crate) fn my_playlists(ttl: u64) -> Result<Vec<Stream>, String> {
    let now = now_secs();
    for p in playlists() {
        if now.saturating_sub(p.updated) >= ttl {
            let _ = fetch_playlist(&p.url, ttl, true);
        }
    }
    Ok(playlist_picks(&playlists()))
}

/// Radio or TV, for `search_streams` `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Radio,
    Tv,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "radio" => Some(Kind::Radio),
            "tv" => Some(Kind::Tv),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Radio => "radio",
            Kind::Tv => "tv",
        }
    }
}

/// Whether a stream is a radio station or a TV channel: radio-browser and SomaFM are
/// radio, iptv-org is TV; for your own playlists and favorites the entry decides (an
/// audio codec, an audio file or playlist extension, or "radio" in its group / name).
pub fn stream_kind(s: &Stream) -> Kind {
    match s.source.as_str() {
        "radio-browser.info" | "SomaFM" => return Kind::Radio,
        "iptv-org" => return Kind::Tv,
        _ => {}
    }
    if !s.tvg_id.is_empty() {
        return Kind::Tv;
    }
    let path = s
        .url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let audio_ext = [
        ".mp3", ".aac", ".ogg", ".oga", ".opus", ".flac", ".m4a", ".pls", ".wma", ".wav",
    ];
    let words = format!("{} {}", s.group, s.name).to_ascii_lowercase();
    if !s.codec.is_empty() || audio_ext.iter().any(|e| path.ends_with(e)) || words.contains("radio")
    {
        Kind::Radio
    } else {
        Kind::Tv
    }
}

/// Merge search results: favorites first, then each source in order; duplicates (same
/// URL) dropped, at most 500. Fails only when nothing was found and a source failed.
pub fn merge_results(
    parts: Vec<Result<Vec<Stream>, String>>,
    kind: Option<Kind>,
) -> Result<Vec<Stream>, String> {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for p in parts {
        match p {
            Ok(v) => out.extend(v),
            Err(e) => errs.push(e),
        }
    }
    if let Some(k) = kind {
        out.retain(|s| stream_kind(s) == k);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.url.clone()));
    out.truncate(500);
    if out.is_empty() && !errs.is_empty() {
        return Err(errs.join("; "));
    }
    Ok(out)
}

/// Favorites, iptv-org TV, your playlists (cached copies), SomaFM and radio-browser
/// stations matching `q`, without duplicates.
pub fn search_all(q: &str) -> Result<Vec<Stream>, String> {
    search_kind(q, None)
}

/// [`search_all`] limited to radio or TV (the other kind's directories are not asked).
pub fn search_kind(q: &str, kind: Option<Kind>) -> Result<Vec<Stream>, String> {
    let tv = kind != Some(Kind::Radio);
    let radio = kind != Some(Kind::Tv);
    let mut parts = vec![Ok(filter(&crate::streams::store::favorites(), q))];
    if tv {
        parts.push(iptv_search(q));
    }
    for p in playlists() {
        if let Ok(t) = std::fs::read_to_string(playlist_cache(&p.url)) {
            parts.push(Ok(filter(&parse_m3u(&t), q)));
        }
    }
    if radio {
        parts.push(somafm().map(|v| filter(&v, q)));
        parts.push(radio_any(q));
    }
    merge_results(parts, kind)
}
