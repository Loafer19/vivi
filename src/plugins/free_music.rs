//! Free Music (#70): search Creative Commons / public-domain music on the Internet
//! Archive (advancedsearch + metadata APIs, no key) and Jamendo (API v3, needs a free
//! `client_id` from <https://devportal.jamendo.com>), preview it as a stream in the
//! player, and download it into a library folder with tags, cover, licence and
//! attribution (in the tags and in a `<file>.licence.txt` next to it).

use super::tagger::{self, Tags};
use super::web;
use super::{Ctx, Manifest, Plugin, SettingSpec};
use crate::Item;
use slint::{ModelRc, SharedString, VecModel};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

pub const ID: &str = "free-music";
pub const SOURCES: [&str; 2] = ["Internet Archive", "Jamendo"];
const NO_KEY: &str = "Jamendo needs a client ID. Create a free app at devportal.jamendo.com and enter it in Settings → Extensions → Free Music, or search the Internet Archive (no key).";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub date: String,
    pub license_url: String,
    pub stream: String,
    /// Empty when the source does not allow downloads.
    pub download: String,
    pub cover: String,
    pub page: String,
    pub source: &'static str,
}

impl Track {
    pub fn license(&self) -> String {
        license_name(&self.license_url)
    }
    /// "“Title” by Artist (page) is licensed under CC BY 4.0 (url)."
    pub fn attribution(&self) -> String {
        format!(
            "“{}” by {} ({}) via {}, licensed under {} ({}).",
            self.title,
            if self.artist.is_empty() {
                "unknown artist"
            } else {
                &self.artist
            },
            self.page,
            self.source,
            self.license(),
            self.license_url
        )
    }
}

/// Short licence name from a Creative Commons URL.
pub fn license_name(url: &str) -> String {
    let u = url.to_lowercase();
    let parts: Vec<&str> = u
        .split('/')
        .filter(|s| !s.is_empty() && !s.contains(':') && !s.contains("creativecommons"))
        .collect();
    match parts.as_slice() {
        ["licenses", kind, ver, ..] => format!("CC {} {ver}", kind.to_uppercase()),
        ["licenses", kind] => format!("CC {}", kind.to_uppercase()),
        ["publicdomain", "zero", ver, ..] => format!("CC0 {ver}"),
        ["publicdomain", "mark", ..] => "Public Domain Mark".into(),
        ["publicdomain", ..] => "Public domain".into(),
        _ if url.is_empty() => "Unknown licence".into(),
        _ => url.to_string(),
    }
}

// ---------- Internet Archive ----------

const IA_MAX_ITEMS: usize = 8;
const IA_TRACKS_PER_ITEM: usize = 6;

pub fn ia_search(query: &str) -> Result<Vec<Track>, String> {
    let q = format!(
        "({}) AND mediatype:audio AND (licenseurl:*creativecommons* OR licenseurl:*publicdomain*) AND (collection:(netlabels OR audio_music OR jamendo-albums OR freemusicarchive) OR subject:music)",
        query.trim()
    );
    let rows = IA_MAX_ITEMS.to_string();
    let v = web::get_json(
        "https://archive.org/advancedsearch.php",
        &[
            ("q", &q),
            ("fl[]", "identifier"),
            ("rows", &rows),
            ("output", "json"),
        ],
        "Internet Archive",
    )?;
    let ids: Vec<String> = v["response"]["docs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["identifier"].as_str().map(String::from))
        .collect();
    // Item metadata in parallel (one request per item).
    let handles: Vec<_> = ids
        .into_iter()
        .map(|id| {
            std::thread::spawn(move || {
                web::get_json(
                    &format!("https://archive.org/metadata/{id}"),
                    &[],
                    "Internet Archive",
                )
                .map(|m| ia_tracks(&m))
                .unwrap_or_default()
            })
        })
        .collect();
    Ok(handles
        .into_iter()
        .flat_map(|h| h.join().unwrap_or_default())
        .collect())
}

fn s(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.first().map(s).unwrap_or_default(),
        _ => String::new(),
    }
}

/// MP3 tracks of an Internet Archive item (`/metadata/<id>` JSON).
pub fn ia_tracks(m: &serde_json::Value) -> Vec<Track> {
    let md = &m["metadata"];
    let id = s(&md["identifier"]);
    let lic = s(&md["licenseurl"]);
    if id.is_empty() || lic.is_empty() {
        return Vec::new();
    }
    let files: Vec<&serde_json::Value> = m["files"].as_array().into_iter().flatten().collect();
    let fmt = |f: &serde_json::Value| s(&f["format"]);
    // Prefer VBR MP3 derivatives, else any MP3.
    let mut mp3: Vec<&&serde_json::Value> = files.iter().filter(|f| fmt(f) == "VBR MP3").collect();
    if mp3.is_empty() {
        mp3 = files.iter().filter(|f| fmt(f).contains("MP3")).collect();
    }
    let item_artist = s(&md["creator"]);
    mp3.into_iter()
        .take(IA_TRACKS_PER_ITEM)
        .map(|f| {
            let name = s(&f["name"]);
            let url = format!(
                "https://archive.org/download/{id}/{}",
                name.split('/').map(urlencode).collect::<Vec<_>>().join("/")
            );
            let stem = Path::new(&name)
                .file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default();
            let title = s(&f["title"]);
            let artist = s(&f["artist"]);
            Track {
                title: if title.is_empty() { stem } else { title },
                artist: if artist.is_empty() {
                    item_artist.clone()
                } else {
                    artist
                },
                album: {
                    let a = s(&f["album"]);
                    if a.is_empty() {
                        s(&md["title"])
                    } else {
                        a
                    }
                },
                date: s(&md["date"]).chars().take(10).collect(),
                license_url: lic.clone(),
                stream: url.clone(),
                download: url,
                cover: format!("https://archive.org/services/img/{id}"),
                page: format!("https://archive.org/details/{id}"),
                source: "Internet Archive",
            }
        })
        .collect()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

// ---------- Jamendo ----------

pub fn jamendo_search(client_id: &str, query: &str) -> Result<Vec<Track>, String> {
    let v = web::get_json(
        "https://api.jamendo.com/v3.0/tracks/",
        &[
            ("client_id", client_id),
            ("format", "json"),
            ("limit", "25"),
            ("search", query.trim()),
            ("audioformat", "mp32"),
            ("audiodlformat", "mp32"),
            ("imagesize", "500"),
        ],
        "Jamendo",
    )?;
    if v["headers"]["status"] != "success" {
        return Err(format!(
            "Jamendo: {}",
            v["headers"]["error_message"]
                .as_str()
                .filter(|m| !m.is_empty())
                .unwrap_or("request failed (check the client ID)")
        ));
    }
    Ok(jamendo_tracks(&v))
}

pub fn jamendo_tracks(v: &serde_json::Value) -> Vec<Track> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| Track {
            title: s(&r["name"]),
            artist: s(&r["artist_name"]),
            album: s(&r["album_name"]),
            date: s(&r["releasedate"]),
            license_url: s(&r["license_ccurl"]),
            stream: s(&r["audio"]),
            download: if r["audiodownload_allowed"].as_bool() == Some(false) {
                String::new()
            } else {
                s(&r["audiodownload"])
            },
            cover: s(&r["image"]),
            page: s(&r["shareurl"]),
            source: "Jamendo",
        })
        .filter(|t| !t.stream.is_empty())
        .collect()
}

// ---------- download ----------

/// Download a track into `dir/<Artist>/<Artist> - <Title>.mp3` with tags, cover and a
/// licence file. Returns the saved path.
pub fn download(t: &Track, dir: &Path) -> Result<PathBuf, String> {
    if t.download.is_empty() {
        return Err("The artist does not allow downloads of this track (preview only)".into());
    }
    let artist = if t.artist.is_empty() {
        "Unknown artist"
    } else {
        &t.artist
    };
    let folder = dir.join(web::safe_name(artist));
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let out = folder.join(format!(
        "{} - {}.mp3",
        web::safe_name(artist),
        web::safe_name(&t.title)
    ));
    let data = web::get_bytes(&t.download, 200 * 1024 * 1024)?.ok_or("file not found (404)")?;
    // lofty picks the format from the extension, so the partial file ends in .mp3.
    let tmp = out.with_extension("part.mp3");
    std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
    let cover = if t.cover.is_empty() {
        None
    } else {
        web::get_bytes(&t.cover, 10 * 1024 * 1024).ok().flatten()
    };
    let year: String = t.date.chars().take(4).collect();
    let tags = Tags {
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        date: t.date.clone(),
        copyright: format!(
            "{year} {}. Licensed under {}: {}",
            t.artist,
            t.license(),
            t.license_url
        )
        .trim()
        .to_string(),
        license: t.license_url.clone(),
        comment: t.attribution(),
        ..Default::default()
    };
    tagger::write_tags(&tmp, &tags, cover.as_deref())?;
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    let lic = format!(
        "{}\n\nTitle: {}\nArtist: {}\nAlbum: {}\nSource: {}\nLicence: {} — {}\n",
        t.attribution(),
        t.title,
        t.artist,
        t.album,
        t.page,
        t.license(),
        t.license_url
    );
    let _ = std::fs::write(out.with_extension("licence.txt"), lic);
    Ok(out)
}

// ---------- plugin ----------

enum Msg {
    Results(Result<Vec<Track>, String>),
    Saved(Result<PathBuf, String>),
}

#[derive(Default)]
pub struct FreeMusic {
    tracks: Vec<Track>,
    rx: Option<Receiver<Msg>>,
}

impl FreeMusic {
    fn ui(cx: &Ctx, status: &str, busy: bool) {
        cx.app.set_md_status(status.into());
        cx.app.set_md_busy(busy);
    }
    fn folder(cx: &Ctx) -> PathBuf {
        let f = cx.get("folder");
        let f = f.trim();
        if f.is_empty() {
            crate::store::home().join("Music").join("Free Music")
        } else if let Some(r) = f.strip_prefix("~/") {
            crate::store::home().join(r)
        } else {
            PathBuf::from(f)
        }
    }
    fn show(&self, cx: &Ctx, sel: Option<usize>) {
        let items: Vec<Item> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| Item {
                key: format!("{} — {}", t.artist, t.title).into(),
                label: format!(
                    "{}  ·  {}{}",
                    t.license(),
                    t.source,
                    if t.download.is_empty() {
                        "  ·  preview only"
                    } else {
                        ""
                    }
                )
                .into(),
                selected: Some(i) == sel,
            })
            .collect();
        cx.app.set_md_results(ModelRc::new(VecModel::from(items)));
    }
    fn detail(t: &Track) -> String {
        format!(
            "{}\n{}{}\n\nLicence: {}\n{}\n\nAttribution (saved in the tags and a .licence.txt):\n{}\n\nSource: {}",
            t.title,
            t.artist,
            if t.album.is_empty() { String::new() } else { format!(" · {}", t.album) },
            t.license(),
            t.license_url,
            t.attribution(),
            t.page
        )
    }
}

impl Plugin for FreeMusic {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Free Music",
            "Search Creative Commons and public-domain music on the Internet Archive and Jamendo, preview it, and download it to your library with tags, cover and licence attribution.",
            "free-music",
        )
        .setting(SettingSpec::secret(
            "jamendo-client-id",
            "Jamendo client ID",
            "Only for Jamendo search. Create a free app at devportal.jamendo.com. The Internet Archive needs no key.",
        ))
        .setting(SettingSpec::text(
            "folder",
            "Download folder",
            "Where downloads go (one folder per artist). Add it to the library to see them there.",
            "~/Music/Free Music",
            "",
        ))
        .setting(SettingSpec::action(
            "open",
            "Find free music",
            "Opens the search dialog.",
            "Browse…",
        ))
    }
    fn disable(&mut self, cx: &Ctx) {
        self.rx = None;
        if cx.app.get_md_owner() == ID {
            cx.app.set_md_open(false);
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let idx = || args.first().and_then(|a| a.parse::<usize>().ok());
        match name {
            "open" => {
                let a = cx.app;
                cx.app.set_settings_open(false);
                a.set_md_owner(ID.into());
                a.set_md_title("Free Music".into());
                a.set_md_placeholder("Artist, title, genre…".into());
                a.set_md_search_label("Search".into());
                a.set_md_primary("Download".into());
                a.set_md_secondary("Preview".into());
                let src: Vec<SharedString> = SOURCES.iter().map(|s| (*s).into()).collect();
                a.set_md_sources(ModelRc::new(VecModel::from(src)));
                if !SOURCES.contains(&a.get_md_source().as_str()) {
                    a.set_md_source(SOURCES[0].into());
                }
                a.set_md_detail("".into());
                a.set_md_open(true);
                self.show(cx, None);
                Self::ui(
                    cx,
                    "Everything here is free to share under its licence: keep the attribution when you share it.",
                    false,
                );
            }
            "search" => {
                let q = cx.app.get_md_query().to_string();
                if q.trim().is_empty() {
                    Self::ui(cx, "Type something to search for", false);
                    return;
                }
                let jamendo = cx.app.get_md_source() == SOURCES[1];
                let key = cx.get("jamendo-client-id").trim().to_string();
                if jamendo && key.is_empty() {
                    Self::ui(cx, NO_KEY, false);
                    return;
                }
                self.tracks.clear();
                self.show(cx, None);
                cx.app.set_md_detail("".into());
                Self::ui(cx, &format!("Searching {}…", cx.app.get_md_source()), true);
                let (tx, rx) = channel();
                self.rx = Some(rx);
                std::thread::spawn(move || {
                    let r = if jamendo {
                        jamendo_search(&key, &q)
                    } else {
                        ia_search(&q)
                    };
                    let _ = tx.send(Msg::Results(r));
                });
            }
            "select" => {
                if let Some(t) = idx().and_then(|i| self.tracks.get(i)) {
                    cx.app.set_md_detail(Self::detail(t).into());
                }
            }
            "secondary" => {
                if let Some(t) = idx().and_then(|i| self.tracks.get(i)) {
                    cx.player.cmd("loadfile", &[&t.stream]);
                    cx.toast(&format!("Previewing {} — {}", t.artist, t.title));
                } else {
                    Self::ui(cx, "Select a track first", false);
                }
            }
            "primary" => {
                let Some(t) = idx().and_then(|i| self.tracks.get(i)).cloned() else {
                    Self::ui(cx, "Select a track first", false);
                    return;
                };
                let dir = Self::folder(cx);
                Self::ui(cx, &format!("Downloading {}…", t.title), true);
                let (tx, rx) = channel();
                self.rx = Some(rx);
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::Saved(download(&t, &dir)));
                });
            }
            "close" => self.rx = None,
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        let Some(rx) = &self.rx else { return };
        let Ok(m) = rx.try_recv() else { return };
        self.rx = None;
        match m {
            Msg::Results(Ok(t)) => {
                let s = if t.is_empty() {
                    "Nothing found. Try other words.".to_string()
                } else {
                    format!("{} tracks. Select one to see its licence.", t.len())
                };
                self.tracks = t;
                self.show(cx, None);
                Self::ui(cx, &s, false);
            }
            Msg::Saved(Ok(p)) => {
                let s = format!("Saved to {}", p.display());
                Self::ui(cx, &s, false);
                cx.toast("Downloaded with licence and attribution");
            }
            Msg::Results(Err(e)) | Msg::Saved(Err(e)) => Self::ui(cx, &e, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licences() {
        assert_eq!(
            license_name("http://creativecommons.org/licenses/by-nc-sa/4.0/"),
            "CC BY-NC-SA 4.0"
        );
        assert_eq!(
            license_name("https://creativecommons.org/licenses/by/3.0/us/"),
            "CC BY 3.0"
        );
        assert_eq!(
            license_name("http://creativecommons.org/publicdomain/zero/1.0/"),
            "CC0 1.0"
        );
        assert_eq!(
            license_name("https://creativecommons.org/publicdomain/mark/1.0/"),
            "Public Domain Mark"
        );
    }

    #[test]
    fn parse_ia_item() {
        let m = serde_json::json!({
            "metadata": {"identifier": "my item", "creator": ["Band"], "title": "Live",
                "date": "2012-12-30T00:00:00Z",
                "licenseurl": "http://creativecommons.org/licenses/by/4.0/"},
            "files": [
                {"name": "a.flac", "format": "Flac"},
                {"name": "dir/b c.mp3", "format": "VBR MP3", "title": "Song B"},
                {"name": "c_64kb.mp3", "format": "64Kbps MP3"}
            ]
        });
        let t = ia_tracks(&m);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].title, "Song B");
        assert_eq!(t[0].artist, "Band");
        assert_eq!(t[0].album, "Live");
        assert_eq!(t[0].date, "2012-12-30");
        assert_eq!(
            t[0].download,
            "https://archive.org/download/my item/dir/b%20c.mp3"
        );
        assert!(t[0].attribution().contains("CC BY 4.0"));
    }

    #[test]
    fn parse_jamendo() {
        let v = serde_json::json!({"headers": {"status": "success"}, "results": [
            {"name": "T", "artist_name": "A", "album_name": "Al", "releasedate": "2020-01-01",
             "license_ccurl": "http://creativecommons.org/licenses/by-sa/3.0/",
             "audio": "https://s/1", "audiodownload": "https://d/1", "audiodownload_allowed": false,
             "image": "https://i/1", "shareurl": "https://jamendo.com/track/1"}
        ]});
        let t = jamendo_tracks(&v);
        assert_eq!(t[0].download, "");
        assert_eq!(t[0].license(), "CC BY-SA 3.0");
        let dir = std::env::temp_dir().join("vivi-fm-nodl");
        assert!(download(&t[0], &dir).is_err());
    }

    /// Live Internet Archive search + download with tags (no key). `--ignored`.
    #[test]
    #[ignore]
    fn live_internet_archive() {
        let t = ia_search("kevin macleod").unwrap();
        assert!(!t.is_empty());
        eprintln!("{} tracks, first: {:?}", t.len(), t[0]);
        let dir = std::env::temp_dir().join(format!("vivi-fm-{}", std::process::id()));
        let p = download(&t[0], &dir).unwrap();
        let (tags, dur) = tagger::read_tags(&p).unwrap();
        assert_eq!(tags.title, t[0].title);
        assert!(
            tags.copyright.contains(&t[0].license_url),
            "{}",
            tags.copyright
        );
        assert!(tags.comment.contains("licensed under"));
        assert!(dur > 1.0);
        assert!(p.with_extension("licence.txt").exists());
        eprintln!("saved {} ({dur:.0}s) {:?}", p.display(), tags);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
