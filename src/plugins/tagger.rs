//! Auto-tagger (#69, includes AcoustID tagging #65): identifies audio files and fixes
//! their tags.
//!
//! 1. **Fingerprint** with Chromaprint: the `fpcalc` tool when it is installed, otherwise
//!    the bundled pure-Rust port (`rusty-chromaprint`, audio decoded with symphonia).
//! 2. **AcoustID** lookup of the fingerprint (needs a free application API key from
//!    <https://acoustid.org/new-application>, a plugin setting) gives MusicBrainz
//!    recording ids. Without a key, or when AcoustID knows nothing, the tagger falls
//!    back to a MusicBrainz search by the current title/artist (or the file name).
//! 3. **MusicBrainz** lookups give title, artist, album, album artist, date, track
//!    number and genre per release; **Cover Art Archive** gives the front cover.
//!    MusicBrainz is called at most once per second with a descriptive User-Agent.
//! 4. A **preview** dialog shows the candidates and a diff (current → new) per field.
//!    Writing goes through lofty; the original tags (and pictures) of a file are backed
//!    up once to `$XDG_DATA_HOME/vivi/tag-backups/` and can be restored.
//!
//! *Tag library folder* scans a folder (default: the first library folder), keeps the
//! best confident match per file and lets the user write them all at once.
//!
//! The AcoustID package (#76) fingerprints with `fpcalc` itself and hands its AcoustID
//! answer to this preview with `script-message vivi-tagger-acoustid <path> <json>`
//! (`{"acoustid": <lookup response>, "pattern": "{artist} - {title}"}`): the recordings
//! are looked up on MusicBrainz here, and each candidate also proposes a new file name
//! from the pattern, shown as a "File name" row in the review and renamed on apply
//! (Undo renames back).

use super::web;
use super::{Ctx, Manifest, Plugin, SettingSpec};
use crate::Item;
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
use slint::{ModelRc, VecModel};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub const ID: &str = "auto-tagger";
/// Seconds of audio fingerprinted (same as `fpcalc`'s default).
const FP_SECONDS: u32 = 120;

/// The tag fields the tagger reads and writes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub date: String,
    pub track: String,
    pub genre: String,
    pub mb_recording: String,
    pub mb_release: String,
    /// Extra free-form fields (free music: licence, attribution, source URL).
    pub copyright: String,
    pub license: String,
    pub comment: String,
    /// Lyrics, LRC text when synced (written per format by [`write_tags`], #80).
    pub lyrics: String,
}

impl Tags {
    fn fields(&self) -> [(&'static str, ItemKey, &str); 12] {
        [
            ("Title", ItemKey::TrackTitle, &self.title),
            ("Artist", ItemKey::TrackArtist, &self.artist),
            ("Album", ItemKey::AlbumTitle, &self.album),
            ("Album artist", ItemKey::AlbumArtist, &self.album_artist),
            ("Date", ItemKey::RecordingDate, &self.date),
            ("Track", ItemKey::TrackNumber, &self.track),
            ("Genre", ItemKey::Genre, &self.genre),
            (
                "MusicBrainz recording",
                ItemKey::MusicBrainzRecordingId,
                &self.mb_recording,
            ),
            (
                "MusicBrainz release",
                ItemKey::MusicBrainzReleaseId,
                &self.mb_release,
            ),
            ("Copyright", ItemKey::CopyrightMessage, &self.copyright),
            // Not every format has a licence field (ID3v2 has none): free music also
            // puts the licence URL into the copyright message and the comment.
            ("Licence", ItemKey::License, &self.license),
            ("Comment", ItemKey::Comment, &self.comment),
        ]
    }
}

/// Changed fields as (name, current, new); empty new values never overwrite.
pub fn diff(old: &Tags, new: &Tags) -> Vec<(&'static str, String, String)> {
    old.fields()
        .into_iter()
        .zip(new.fields())
        .filter(|((_, _, o), (_, _, n))| !n.is_empty() && o != n)
        .map(|((name, _, o), (_, _, n))| (name, o.to_string(), n.to_string()))
        .collect()
}

/// File name for `path` from a pattern such as `{artist} - {title}` (fields: artist, title,
/// album, albumartist, year, date, track, genre; `{track}` is zero-padded to two digits).
/// Characters that are not allowed in file names become `_`; the extension is kept.
/// None when the title is unknown, the result is empty or it equals the current name.
pub fn rename_to(pattern: &str, t: &Tags, path: &Path) -> Option<String> {
    if t.title.trim().is_empty() || pattern.trim().is_empty() {
        return None;
    }
    let track = t.track.split('/').next().unwrap_or("").trim().to_string();
    let track = match track.parse::<u32>() {
        Ok(n) => format!("{n:02}"),
        Err(_) => track,
    };
    let year: String = t.date.chars().take(4).collect();
    let mut out = pattern.to_string();
    for (k, v) in [
        ("{artist}", t.artist.as_str()),
        ("{title}", t.title.as_str()),
        ("{album}", t.album.as_str()),
        ("{albumartist}", t.album_artist.as_str()),
        ("{year}", year.as_str()),
        ("{date}", t.date.as_str()),
        ("{track}", track.as_str()),
        ("{genre}", t.genre.as_str()),
    ] {
        out = out.replace(k, v);
    }
    let name = sanitize_file_name(&out);
    if name.is_empty() {
        return None;
    }
    let file = match path.extension() {
        Some(e) => format!("{name}.{}", e.to_string_lossy()),
        None => name,
    };
    let current = path.file_name().map(|f| f.to_string_lossy().into_owned());
    (current.as_deref() != Some(file.as_str())).then_some(file)
}

/// Longest file name stem kept (bytes), well under the 255-byte limit of most file systems.
const MAX_STEM: usize = 180;

/// A file name stem safe on Linux, macOS and Windows: path separators, `: * ? " < > |`
/// and control characters become `_`, whitespace is collapsed, separators left by empty
/// fields ("Artist -  - Title") are tidied, leading/trailing dots, dashes and spaces are
/// dropped, Windows device names (CON, NUL, COM1…) get a `_` suffix and the result is
/// cut to [`MAX_STEM`] bytes on a character boundary.
pub fn sanitize_file_name(s: &str) -> String {
    let clean: String = s
        .chars()
        .map(|c| {
            if c.is_whitespace() {
                ' '
            } else if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                || c.is_control()
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut name = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    while name.contains("- -") {
        name = name.replace("- -", "-");
    }
    let trim = |n: &str| {
        n.trim_matches(|c: char| c == '-' || c == ' ' || c == '.')
            .to_string()
    };
    let mut name = trim(&name);
    if name.len() > MAX_STEM {
        let mut end = MAX_STEM;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name = trim(&name[..end]);
    }
    let base = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit());
    if reserved {
        name.insert(name.find('.').unwrap_or(name.len()), '_');
    }
    name
}

/// Rename `path` to `name` in the same folder; never replaces an existing file (also
/// one that differs only in case) and never leaves the folder.
pub fn rename_file(path: &Path, name: &str) -> Result<PathBuf, String> {
    if name.contains('/') || name.contains('\\') || name.starts_with('.') || name.is_empty() {
        return Err(format!("{name}: not a plain file name"));
    }
    let to = path.with_file_name(name);
    let clash = std::fs::read_dir(path.parent().unwrap_or(Path::new(".")))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n == name.to_lowercase() && e.path() != path
        });
    if to.exists() || clash {
        return Err(format!("{name} already exists"));
    }
    std::fs::rename(path, &to).map_err(|e| e.to_string())?;
    Ok(to)
}

/// Current tags and duration (seconds) of a file.
pub fn read_tags(path: &Path) -> Result<(Tags, f64), String> {
    let tf = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let dur = tf.properties().duration().as_secs_f64();
    let mut t = Tags::default();
    if let Some(tag) = tf.primary_tag().or_else(|| tf.first_tag()) {
        let g = |k: ItemKey| tag.get_string(k).unwrap_or_default().to_string();
        t = Tags {
            title: g(ItemKey::TrackTitle),
            artist: g(ItemKey::TrackArtist),
            album: g(ItemKey::AlbumTitle),
            album_artist: g(ItemKey::AlbumArtist),
            date: {
                let d = g(ItemKey::RecordingDate);
                if d.is_empty() {
                    g(ItemKey::Year)
                } else {
                    d
                }
            },
            track: g(ItemKey::TrackNumber),
            genre: g(ItemKey::Genre),
            mb_recording: g(ItemKey::MusicBrainzRecordingId),
            mb_release: g(ItemKey::MusicBrainzReleaseId),
            copyright: g(ItemKey::CopyrightMessage),
            license: g(ItemKey::License),
            comment: g(ItemKey::Comment),
            lyrics: {
                let l = g(ItemKey::Lyrics);
                if l.is_empty() {
                    g(ItemKey::UnsyncLyrics)
                } else {
                    l
                }
            },
        };
    }
    Ok((t, dur))
}

// ---------- fingerprinting ----------

/// Chromaprint fingerprint (compressed, base64url) and duration in seconds.
/// Also says which fingerprinter ran: the system `fpcalc` first (no extra download or
/// binary size), otherwise the built-in Chromaprint port.
pub fn fingerprint_with(path: &Path) -> Result<(u32, String, &'static str), String> {
    match fpcalc(path) {
        Some((d, f)) => Ok((d, f, "system fpcalc")),
        None => fingerprint_builtin(path).map(|(d, f)| (d, f, "built-in Chromaprint")),
    }
}

/// Run `fpcalc -json` if it is installed.
fn fpcalc(path: &Path) -> Option<(u32, String)> {
    let out = std::process::Command::new("fpcalc")
        .arg("-json")
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Some((
        v["duration"].as_f64()? as u32,
        v["fingerprint"].as_str()?.to_string(),
    ))
}

/// Pure-Rust Chromaprint (rusty-chromaprint) over the first [`FP_SECONDS`] seconds.
pub fn fingerprint_builtin(path: &Path) -> Result<(u32, String), String> {
    let config = rusty_chromaprint::Configuration::preset_test2();
    let (dur, raw) = fingerprint_raw(path, &config)?;
    let c = rusty_chromaprint::FingerprintCompressor::from(&config).compress(&raw);
    Ok((dur, web::base64_url(&c)))
}

/// Raw Chromaprint sub-fingerprints and the duration in seconds.
fn fingerprint_raw(
    path: &Path,
    config: &rusty_chromaprint::Configuration,
) -> Result<(u32, Vec<u32>), String> {
    use rusty_chromaprint::Fingerprinter;
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(e) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(e);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("cannot decode: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("no audio track")?;
    let tid = track.id;
    let rate = track
        .codec_params
        .sample_rate
        .ok_or("unknown sample rate")?;
    let frames = track.codec_params.n_frames;
    let mut dec = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("cannot decode: {e}"))?;
    let mut fp = Fingerprinter::new(config);
    let mut started = false;
    let mut fed: u64 = 0;
    let mut total: u64 = 0;
    let mut buf: Option<SampleBuffer<i16>> = None;
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != tid {
            continue;
        }
        let d = match dec.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let spec = *d.spec();
        let channels = spec.channels.count().max(1);
        if !started {
            fp.start(rate, channels as u32)
                .map_err(|e| format!("fingerprint: {e:?}"))?;
            started = true;
        }
        let need = d.capacity();
        if buf.as_ref().is_none_or(|b| b.capacity() < need * channels) {
            buf = Some(SampleBuffer::new(need as u64, spec));
        }
        let b = buf.as_mut().ok_or("buffer")?;
        b.copy_interleaved_ref(d);
        let n = b.samples().len() / channels;
        total += n as u64;
        let limit = FP_SECONDS as u64 * rate as u64;
        if fed < limit {
            let take = ((limit - fed) as usize).min(n);
            fp.consume(&b.samples()[..take * channels]);
            fed += take as u64;
        } else if frames.is_some() {
            break;
        }
    }
    if !started || fed == 0 {
        return Err("no audio decoded".into());
    }
    fp.finish();
    let dur = match frames {
        Some(f) => f / rate as u64,
        None => total / rate as u64,
    } as u32;
    Ok((dur, fp.fingerprint().to_vec()))
}

// ---------- AcoustID + MusicBrainz ----------

/// One way a file could be tagged.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub tags: Tags,
    pub release_group: String,
    /// 0–100: AcoustID score or MusicBrainz search score.
    pub score: u32,
    pub source: &'static str,
    /// Review fields filled by another source than `source`: (field label, source).
    pub sources: Vec<(&'static str, &'static str)>,
    /// Cover URLs from other sources: (label, url).
    pub extra_covers: Vec<(String, String)>,
    /// Values other sources propose for a field that is already set (pick in the review).
    pub alts: Vec<Alt>,
    /// Sources that confirmed this candidate (same album / year / track / genre).
    pub agree: Vec<&'static str>,
}

/// An alternative value for a review field and where it comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct Alt {
    pub field: &'static str,
    pub value: String,
    pub source: &'static str,
}

impl Candidate {
    /// Match confidence 0–100: the AcoustID / MusicBrainz score plus 4 per other source
    /// that agrees with it (at most +12).
    pub fn confidence(&self) -> u32 {
        (self.score + 4 * (self.agree.len() as u32).min(3)).min(100)
    }
}

/// Field of `t` by review label.
pub fn field_mut<'a>(t: &'a mut Tags, name: &str) -> Option<&'a mut String> {
    Some(match name {
        "Title" => &mut t.title,
        "Artist" => &mut t.artist,
        "Album" => &mut t.album,
        "Album artist" => &mut t.album_artist,
        "Year" => &mut t.date,
        "Track" => &mut t.track,
        "Genre" => &mut t.genre,
        "Lyrics" => &mut t.lyrics,
        _ => return None,
    })
}

fn rank_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Merge and rank candidates from all lookups (AcoustID, MusicBrainz search): duplicates
/// (same recording + release, or same title/artist/album) collapse into the best scored
/// one, candidates whose title and artist match the file's current tags get +5, then
/// they are sorted by confidence (ties: official dates first, as they arrived).
pub fn rank_candidates(v: Vec<Candidate>, current: &Tags) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for mut c in v {
        if !current.title.is_empty()
            && rank_key(&c.tags.title) == rank_key(&current.title)
            && (current.artist.is_empty() || rank_key(&c.tags.artist) == rank_key(&current.artist))
        {
            c.score = (c.score + 5).min(100);
        }
        let key = |c: &Candidate| {
            if !c.tags.mb_release.is_empty() {
                format!("{}|{}", c.tags.mb_recording, c.tags.mb_release)
            } else {
                format!(
                    "{}|{}|{}",
                    rank_key(&c.tags.title),
                    rank_key(&c.tags.artist),
                    rank_key(&c.tags.album)
                )
            }
        };
        match out.iter_mut().find(|o| key(o) == key(&c)) {
            Some(o) if c.score > o.score => *o = c,
            Some(_) => {}
            None => out.push(c),
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.confidence()));
    out
}

fn credit(v: &serde_json::Value) -> String {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            format!(
                "{}{}",
                c["name"].as_str().unwrap_or(""),
                c["joinphrase"].as_str().unwrap_or("")
            )
        })
        .collect()
}

/// Candidates from a MusicBrainz recording (search hit or lookup) and its releases.
pub fn candidates_from_recording(
    rec: &serde_json::Value,
    score: u32,
    source: &'static str,
    max_releases: usize,
) -> Vec<Candidate> {
    let base = Tags {
        title: rec["title"].as_str().unwrap_or("").into(),
        artist: credit(&rec["artist-credit"]),
        date: rec["first-release-date"].as_str().unwrap_or("").into(),
        genre: rec["genres"]
            .as_array()
            .and_then(|g| g.iter().max_by_key(|x| x["count"].as_i64().unwrap_or(0)))
            .and_then(|g| g["name"].as_str())
            .map(capitalize)
            .unwrap_or_default(),
        mb_recording: rec["id"].as_str().unwrap_or("").into(),
        ..Default::default()
    };
    let mut rels: Vec<&serde_json::Value> =
        rec["releases"].as_array().into_iter().flatten().collect();
    // Official albums first, then by date.
    rels.sort_by_key(|r| {
        (
            r["status"].as_str() != Some("Official"),
            r["release-group"]["primary-type"].as_str() != Some("Album"),
            r["date"].as_str().unwrap_or("9999").to_string(),
        )
    });
    let mut out: Vec<Candidate> = rels
        .into_iter()
        .take(max_releases)
        .map(|r| {
            let mut t = base.clone();
            t.album = r["title"].as_str().unwrap_or("").into();
            let aa = credit(&r["artist-credit"]);
            t.album_artist = if aa.is_empty() { t.artist.clone() } else { aa };
            if let Some(d) = r["date"].as_str().filter(|d| !d.is_empty()) {
                t.date = d.into();
            }
            t.mb_release = r["id"].as_str().unwrap_or("").into();
            let media = r["media"].as_array().and_then(|m| m.first());
            if let Some(m) = media {
                let tr = m["track"].as_array().or(m["tracks"].as_array());
                if let Some(n) = tr
                    .and_then(|t| t.first())
                    .and_then(|t| t["number"].as_str())
                {
                    t.track = n.into();
                }
            }
            Candidate {
                tags: t,
                release_group: r["release-group"]["id"].as_str().unwrap_or("").into(),
                score,
                source,
                sources: Vec::new(),
                extra_covers: Vec::new(),
                alts: Vec::new(),
                agree: Vec::new(),
            }
        })
        .collect();
    if out.is_empty() && !base.title.is_empty() {
        out.push(Candidate {
            tags: base,
            release_group: String::new(),
            score,
            source,
            sources: Vec::new(),
            extra_covers: Vec::new(),
            alts: Vec::new(),
            agree: Vec::new(),
        });
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Escape a Lucene term for a MusicBrainz search query.
fn lucene(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if "+-&|!(){}[]^\"~*?:\\/".contains(c) {
                format!("\\{c}")
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Title and artist to search for: the tags, else "Artist - Title" from the file name.
pub fn search_terms(t: &Tags, path: &Path) -> (String, String) {
    if !t.title.is_empty() {
        return (t.title.clone(), t.artist.clone());
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().replace('_', " "))
        .unwrap_or_default();
    let stem = stem.trim_start_matches(|c: char| c.is_ascii_digit() || " .-".contains(c));
    match stem.split_once(" - ") {
        Some((a, b)) => (b.trim().to_string(), a.trim().to_string()),
        None => (stem.trim().to_string(), String::new()),
    }
}

/// MusicBrainz recording search by title (+ artist); candidates sorted by score.
pub fn search_musicbrainz(title: &str, artist: &str, dur: f64) -> Result<Vec<Candidate>, String> {
    if title.trim().is_empty() {
        return Err("No title to search for: the file has no tags and an unhelpful name".into());
    }
    let exact = |t: &str| format!("\"{}\"", lucene(t));
    let words = |t: &str| format!("({})", lucene(t));
    let query = |f: &dyn Fn(&str) -> String| {
        let mut q = format!("recording:{}", f(title));
        if !artist.trim().is_empty() {
            q += &format!(" AND artist:{}", f(artist));
        }
        q
    };
    let mut v = web::musicbrainz("recording", &[("query", &query(&exact)), ("limit", "8")])?;
    // No exact phrase match (e.g. a slightly wrong title): search the words.
    if v["recordings"].as_array().is_none_or(|a| a.is_empty()) {
        v = web::musicbrainz("recording", &[("query", &query(&words)), ("limit", "8")])?;
    }
    let mut out = Vec::new();
    for r in v["recordings"].as_array().into_iter().flatten() {
        let mut score = r["score"].as_u64().unwrap_or(0) as u32;
        // Penalise recordings whose length differs a lot from the file.
        if let (Some(len), true) = (r["length"].as_f64(), dur > 0.0) {
            if (len / 1000.0 - dur).abs() > 15.0 {
                score = score.saturating_sub(20);
            }
        }
        out.extend(candidates_from_recording(r, score, "MusicBrainz search", 3));
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.score));
    out.truncate(12);
    Ok(out)
}

/// AcoustID lookup: (recording id, score 0–100), best first.
pub fn acoustid_lookup(key: &str, dur: u32, fp: &str) -> Result<Vec<(String, u32)>, String> {
    let d = dur.to_string();
    let v = web::get_json(
        "https://api.acoustid.org/v2/lookup",
        &[
            ("client", key),
            ("meta", "recordingids"),
            ("duration", &d),
            ("fingerprint", fp),
        ],
        "AcoustID",
    )?;
    if v["status"] != "ok" {
        return Err(format!(
            "AcoustID: {}",
            v["error"]["message"].as_str().unwrap_or("lookup failed")
        ));
    }
    Ok(parse_acoustid(&v))
}

pub fn parse_acoustid(v: &serde_json::Value) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for r in v["results"].as_array().into_iter().flatten() {
        let s = (r["score"].as_f64().unwrap_or(0.0) * 100.0).round() as u32;
        for rec in r["recordings"].as_array().into_iter().flatten() {
            if let Some(id) = rec["id"].as_str() {
                if !out.iter().any(|(i, _)| i == id) {
                    out.push((id.into(), s));
                }
            }
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.1));
    out
}

/// Full MusicBrainz recording lookup.
pub fn lookup_recording(id: &str) -> Result<serde_json::Value, String> {
    web::musicbrainz(
        &format!("recording/{id}"),
        &[("inc", "artist-credits+releases+release-groups+media+genres")],
    )
}

/// Identify one file: AcoustID (when `key` is set) then MusicBrainz search.
/// Returns the current tags and the candidates, best first.
pub fn identify(path: &Path, key: &str) -> Result<(Tags, Vec<Candidate>, Vec<String>), String> {
    let (old, dur) = read_tags(path)?;
    let mut notes = Vec::new();
    let mut out = Vec::new();
    if !key.trim().is_empty() {
        match fingerprint_with(path).and_then(|(d, fp, how)| {
            notes.push(format!("Fingerprint: {how}"));
            acoustid_lookup(key.trim(), d, &fp)
        }) {
            Ok(ids) => {
                for (id, score) in ids.into_iter().filter(|(_, s)| *s >= 50).take(3) {
                    let rec = lookup_recording(&id)?;
                    out.extend(candidates_from_recording(&rec, score, "AcoustID", 4));
                }
                if out.is_empty() {
                    notes.push("AcoustID: no match for the fingerprint".to_string());
                }
            }
            Err(e) => notes.push(e),
        }
    } else {
        notes.push("No AcoustID key: matched by tags/file name only".into());
    }
    if out.is_empty() {
        let (title, artist) = search_terms(&old, path);
        out = search_musicbrainz(&title, &artist, dur)?;
    }
    Ok((old.clone(), rank_candidates(out, &old), notes))
}

/// Front cover from the Cover Art Archive (release, then release group), ≤ 500 px.
#[cfg(test)]
pub fn cover_art(release: &str, group: &str) -> Option<Vec<u8>> {
    let urls = [
        (!release.is_empty())
            .then(|| format!("https://coverartarchive.org/release/{release}/front-500")),
        (!group.is_empty())
            .then(|| format!("https://coverartarchive.org/release-group/{group}/front-500")),
    ];
    urls.into_iter()
        .flatten()
        .find_map(|u| web::get_bytes(&u, 10 * 1024 * 1024).ok().flatten())
}

// ---------- writing, backup, restore ----------

pub fn backup_dir() -> PathBuf {
    crate::store::data_dir().join("tag-backups")
}

fn backup_file(dir: &Path, path: &Path) -> PathBuf {
    let key = format!("{:x}", md5::compute(path.to_string_lossy().as_bytes()));
    dir.join(format!("{key}.json"))
}

/// Save the file's current tag (all text items and pictures) unless a backup exists,
/// so the backup is always the state before Vivi first changed the file.
pub fn backup(dir: &Path, path: &Path) -> Result<PathBuf, String> {
    let out = backup_file(dir, path);
    if out.exists() {
        return Ok(out);
    }
    let tf = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let tt = tf.primary_tag_type();
    let mut items = Vec::new();
    let mut pics = Vec::new();
    if let Some(tag) = tf.tag(tt) {
        for it in tag.items() {
            if let (Some(k), Some(v)) = (it.key().map_key(tt), it.value().text()) {
                items.push(serde_json::json!([k, v]));
            }
        }
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        for (i, p) in tag.pictures().iter().enumerate() {
            let f = out.with_extension(format!("pic{i}"));
            std::fs::write(&f, p.data()).map_err(|e| e.to_string())?;
            pics.push(serde_json::json!({
                "type": p.pic_type().as_u8(),
                "mime": p.mime_type().map(|m| m.as_str().to_string()),
                "file": f.file_name().map(|n| n.to_string_lossy().into_owned()),
            }));
        }
    }
    let j = serde_json::json!({
        "path": path.to_string_lossy(),
        "had_tag": tf.tag(tt).is_some(),
        "items": items,
        "pictures": pics,
    });
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    crate::store::write_atomic(&out, &serde_json::to_string_pretty(&j).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Put back the tags saved by [`backup`] and delete the backup.
pub fn restore(dir: &Path, path: &Path) -> Result<(), String> {
    let f = backup_file(dir, path);
    let s = std::fs::read_to_string(&f).map_err(|_| "No tag backup for this file".to_string())?;
    let j: serde_json::Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
    let tf = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let tt = tf.primary_tag_type();
    let mut tag = Tag::new(tt);
    for it in j["items"].as_array().into_iter().flatten() {
        if let (Some(k), Some(v)) = (it[0].as_str(), it[1].as_str()) {
            if let Some(key) = ItemKey::from_key(tt, k) {
                tag.push(lofty::tag::TagItem::new(
                    key,
                    lofty::tag::ItemValue::Text(v.into()),
                ));
            }
        }
    }
    for p in j["pictures"].as_array().into_iter().flatten() {
        let file = dir.join(p["file"].as_str().unwrap_or(""));
        let data = std::fs::read(&file).map_err(|e| e.to_string())?;
        let mut b = Picture::unchecked(data)
            .pic_type(PictureType::from_u8(p["type"].as_u64().unwrap_or(3) as u8));
        if let Some(m) = p["mime"].as_str() {
            b = b.mime_type(MimeType::from_str(m));
        }
        tag.push_picture(b.build());
    }
    if j["had_tag"].as_bool() == Some(false) {
        tt.remove_from_path(path, WriteOptions::default())
            .map_err(|e| e.to_string())?;
    } else {
        tag.save_to_path(path, WriteOptions::default())
            .map_err(|e| e.to_string())?;
    }
    for p in j["pictures"].as_array().into_iter().flatten() {
        let _ = std::fs::remove_file(dir.join(p["file"].as_str().unwrap_or("")));
    }
    let _ = std::fs::remove_file(&f);
    Ok(())
}

fn mime_of(data: &[u8]) -> MimeType {
    if data.starts_with(b"\x89PNG") {
        MimeType::Png
    } else {
        MimeType::Jpeg
    }
}

/// Write the non-empty fields of `t` (and a front cover) into the file's primary tag.
pub fn write_tags(path: &Path, t: &Tags, cover: Option<&[u8]>) -> Result<(), String> {
    let mut tf = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let tt = tf.primary_tag_type();
    if tf.tag(tt).is_none() {
        tf.insert_tag(Tag::new(tt));
    }
    let tag = tf.tag_mut(tt).ok_or("cannot create a tag")?;
    for (_, key, v) in t.fields() {
        if !v.is_empty() {
            tag.insert_text(key, v.to_string());
        }
    }
    let id3 = tt == lofty::tag::TagType::Id3v2;
    if !t.lyrics.is_empty() {
        // ID3v2: USLT with the LRC text (SYLT is added below); Vorbis comments: LYRICS;
        // MP4: ©lyr.
        let key = if id3 {
            ItemKey::UnsyncLyrics
        } else {
            ItemKey::Lyrics
        };
        tag.insert_text(key, t.lyrics.clone());
    }
    if let Some(c) = cover {
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(
            Picture::unchecked(c.to_vec())
                .pic_type(PictureType::CoverFront)
                .mime_type(mime_of(c))
                .build(),
        );
    }
    tag.save_to_path(path, WriteOptions::default())
        .map_err(|e| e.to_string())?;
    if id3 && !t.lyrics.is_empty() {
        write_sylt(path, &t.lyrics)?;
    }
    Ok(())
}

/// (milliseconds, text) lines of LRC lyrics; empty when it has no time tags.
pub fn lrc_lines(lrc: &str) -> Vec<(u32, String)> {
    super::lyrics::Lrc::parse(lrc)
        .lines
        .into_iter()
        .map(|(t, l)| ((t * 1000.0).round() as u32, l))
        .collect()
}

/// Replace the ID3v2 SYLT (synchronised lyrics, ms timestamps) frame of an MP3.
fn write_sylt(path: &Path, lrc: &str) -> Result<(), String> {
    use lofty::config::ParseOptions;
    use lofty::id3::v2::{
        BinaryFrame, Frame, FrameId, SyncTextContentType, SynchronizedTextFrame, TimestampFormat,
    };
    use lofty::TextEncoding;
    let lines = lrc_lines(lrc);
    if lines.is_empty() {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut mf =
        lofty::mpeg::MpegFile::read_from(&mut f, ParseOptions::new()).map_err(|e| e.to_string())?;
    let Some(tag) = mf.id3v2_mut() else {
        return Ok(());
    };
    let sylt = SynchronizedTextFrame::new(
        TextEncoding::UTF8,
        *b"XXX",
        TimestampFormat::MS,
        SyncTextContentType::Lyrics,
        None,
        lines,
    );
    let data = sylt
        .as_bytes(WriteOptions::default())
        .map_err(|e| e.to_string())?;
    let id = FrameId::Valid("SYLT".into());
    let _ = tag.remove(&id).count();
    tag.insert(Frame::Binary(BinaryFrame::new(id, data)));
    use std::io::Seek;
    f.rewind().map_err(|e| e.to_string())?;
    mf.save_to(&mut f, WriteOptions::default())
        .map_err(|e| e.to_string())
}

/// Text of the SYLT frame of an MP3 as (ms, line) (Lyrics plugin, tests).
pub fn read_sylt(path: &Path) -> Vec<(u32, String)> {
    use lofty::config::ParseOptions;
    use lofty::id3::v2::{Frame, FrameFlags, FrameId, SynchronizedTextFrame};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(mf) = lofty::mpeg::MpegFile::read_from(&mut f, ParseOptions::new()) else {
        return Vec::new();
    };
    let Some(tag) = mf.id3v2() else {
        return Vec::new();
    };
    match tag.get(&FrameId::Valid("SYLT".into())) {
        Some(Frame::Binary(b)) => SynchronizedTextFrame::parse(&b.data, FrameFlags::default())
            .map(|s| s.content)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

// ---------- review: per-field diff, cover candidates, apply + undo ----------

/// The fields shown in the review, as (label, current, proposed).
pub fn review_fields(old: &Tags, new: &Tags) -> Vec<(&'static str, String, String)> {
    vec![
        ("Title", old.title.clone(), new.title.clone()),
        ("Artist", old.artist.clone(), new.artist.clone()),
        ("Album", old.album.clone(), new.album.clone()),
        (
            "Album artist",
            old.album_artist.clone(),
            new.album_artist.clone(),
        ),
        ("Year", old.date.clone(), new.date.clone()),
        ("Track", old.track.clone(), new.track.clone()),
        ("Genre", old.genre.clone(), new.genre.clone()),
        ("Lyrics", old.lyrics.clone(), new.lyrics.clone()),
    ]
}

/// Short form of lyrics for the review row: "Synced, 42 lines: first line".
pub fn lyrics_summary(l: &str) -> String {
    if l.trim().is_empty() {
        return String::new();
    }
    let synced = lrc_lines(l);
    let (kind, n, first) = if synced.is_empty() {
        let lines: Vec<&str> = l.lines().filter(|x| !x.trim().is_empty()).collect();
        ("Plain", lines.len(), lines.first().map(|x| x.to_string()))
    } else {
        let first = synced.iter().find(|x| !x.1.is_empty()).map(|x| x.1.clone());
        ("Synced", synced.len(), first)
    };
    format!("{kind}, {n} lines: {}", first.unwrap_or_default())
}

/// Whether a proposed value would change the field (empty proposals never overwrite).
pub fn field_changes(old: &str, new: &str) -> bool {
    !new.trim().is_empty() && old.trim() != new.trim()
}

/// Tags holding only the chosen fields of `new` (by review index); the MusicBrainz
/// ids come along whenever anything is written.
pub fn pick_fields(new: &Tags, on: &[bool]) -> Tags {
    let g = |i: usize, v: &str| {
        if on.get(i).copied().unwrap_or(false) {
            v.to_string()
        } else {
            String::new()
        }
    };
    let any = on.iter().any(|b| *b);
    Tags {
        title: g(0, &new.title),
        artist: g(1, &new.artist),
        album: g(2, &new.album),
        album_artist: g(3, &new.album_artist),
        date: g(4, &new.date),
        track: g(5, &new.track),
        genre: g(6, &new.genre),
        lyrics: g(7, &new.lyrics),
        mb_recording: if any {
            new.mb_recording.clone()
        } else {
            String::new()
        },
        mb_release: if any {
            new.mb_release.clone()
        } else {
            String::new()
        },
        ..Tags::default()
    }
}

/// Pixel size of a PNG or JPEG.
pub fn image_size(d: &[u8]) -> Option<(u32, u32)> {
    if d.len() > 24 && d.starts_with(b"\x89PNG") {
        let be = |o: usize| u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
        return Some((be(16), be(20)));
    }
    if !d.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2;
    while i + 9 < d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = d[i + 1];
        let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let h = u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32;
            let w = u16::from_be_bytes([d[i + 7], d[i + 8]]) as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

/// "500×500 · 48 KB" for an image.
pub fn image_info(d: &[u8]) -> String {
    let kb = (d.len() as f64 / 1024.0).round() as u64;
    let kind = if d.starts_with(b"\x89PNG") {
        "PNG"
    } else {
        "JPEG"
    };
    match image_size(d) {
        Some((w, h)) => format!("{w}×{h}\n{kb} KB {kind}"),
        None => format!("{kb} KB"),
    }
}

/// Front cover embedded in the file.
pub fn embedded_cover(path: &Path) -> Option<Vec<u8>> {
    let tf = lofty::read_from_path(path).ok()?;
    let tag = tf.primary_tag().or_else(|| tf.first_tag())?;
    tag.pictures()
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())
        .map(|p| p.data().to_vec())
}

/// A downloaded cover candidate.
#[derive(Debug, Clone)]
pub struct CoverOpt {
    pub label: String,
    pub data: Vec<u8>,
}

/// Image URLs of a release on the Cover Art Archive: front first, then the others
/// (back, booklet…), as (label, url of the 500 px version or the original).
pub fn caa_images(release: &str) -> Vec<(String, String)> {
    if release.is_empty() {
        return Vec::new();
    }
    let url = format!("https://coverartarchive.org/release/{release}");
    let Some(b) = web::get_bytes(&url, 2 * 1024 * 1024).ok().flatten() else {
        return Vec::new();
    };
    let Ok(j) = serde_json::from_slice::<serde_json::Value>(&b) else {
        return Vec::new();
    };
    let mut v: Vec<(bool, String, String)> = j["images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|im| {
            let u = im["thumbnails"]["500"]
                .as_str()
                .or_else(|| im["thumbnails"]["large"].as_str())
                .or_else(|| im["image"].as_str())?;
            let types: Vec<&str> = im["types"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_str())
                .collect();
            let front = im["front"].as_bool().unwrap_or(false);
            let label = if types.is_empty() {
                "Image".to_string()
            } else {
                types.join(", ")
            };
            Some((front, label, u.replace("http://", "https://")))
        })
        .collect();
    v.sort_by_key(|(f, _, _)| !*f);
    v.into_iter().map(|(_, l, u)| (l, u)).collect()
}

/// Up to `max` cover candidates: the release's CAA images, its release-group front,
/// then the fronts of the other matched releases.
pub fn cover_candidates(c: &Candidate, others: &[Candidate], max: usize) -> Vec<CoverOpt> {
    let mut urls: Vec<(String, String)> = caa_images(&c.tags.mb_release);
    // Deezer / iTunes / Discogs covers (often larger) right after the release's own.
    urls.extend(c.extra_covers.iter().cloned());
    if !c.release_group.is_empty() {
        urls.push((
            "Group front".into(),
            format!(
                "https://coverartarchive.org/release-group/{}/front-500",
                c.release_group
            ),
        ));
    }
    for o in others {
        if !o.tags.mb_release.is_empty() && o.tags.mb_release != c.tags.mb_release {
            urls.push((
                o.tags.album.clone(),
                format!(
                    "https://coverartarchive.org/release/{}/front-500",
                    o.tags.mb_release
                ),
            ));
        }
    }
    urls.truncate(max + 4);
    // Cover Art Archive redirects to archive.org, which can be slow: fetch in parallel.
    let got: Vec<Option<CoverOpt>> = std::thread::scope(|sc| {
        let hs: Vec<_> = urls
            .iter()
            .map(|(label, u)| {
                sc.spawn(move || {
                    let data = web::get_bytes(u, 10 * 1024 * 1024).ok().flatten()?;
                    image_size(&data)?;
                    Some(CoverOpt {
                        label: label.clone(),
                        data,
                    })
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    let mut out: Vec<CoverOpt> = Vec::new();
    for c in got.into_iter().flatten() {
        // The release-group front is often the same picture as the release front.
        if out.len() < max && !out.iter().any(|o| o.data == c.data) {
            out.push(c);
        }
    }
    out
}

/// One file in the review.
#[derive(Debug, Clone)]
pub struct Review {
    pub path: PathBuf,
    pub old: Tags,
    pub cand: Candidate,
    pub on: bool,
    pub fields: Vec<bool>,
    pub old_cover: Option<Vec<u8>>,
    pub covers: Vec<CoverOpt>,
    /// Chosen cover candidate; None keeps the current cover.
    pub cover: Option<usize>,
    /// Proposed new file name (AcoustID package's pattern) and whether to apply it.
    pub rename: Option<String>,
    pub rename_on: bool,
    /// How the file was identified (fingerprinter, sources, lyrics setting).
    pub note: String,
}

impl Review {
    pub fn new(
        path: PathBuf,
        old: Tags,
        cand: Candidate,
        covers: Vec<CoverOpt>,
        embed: bool,
    ) -> Self {
        let fields = review_fields(&old, &cand.tags)
            .iter()
            .map(|(_, o, n)| field_changes(o, n))
            .collect();
        let old_cover = embedded_cover(&path);
        // Embed by default when the file has no cover; replacing one is opt-in.
        let cover = (embed && old_cover.is_none() && !covers.is_empty()).then_some(0);
        Review {
            path,
            old,
            cand,
            on: true,
            fields,
            old_cover,
            covers,
            cover,
            rename: None,
            rename_on: false,
            note: String::new(),
        }
    }
    /// Propose a new file name; renaming is opt-in (unticked until the user ticks it).
    pub fn with_rename(mut self, name: Option<String>) -> Self {
        self.rename_on = false;
        self.rename = name;
        self
    }
    /// Alternatives other sources offer for review field `j`.
    pub fn alts_of(&self, j: usize) -> usize {
        let name = review_fields(&self.old, &self.cand.tags)
            .get(j)
            .map(|f| f.0)
            .unwrap_or("");
        self.cand.alts.iter().filter(|a| a.field == name).count()
    }
    /// Switch review field `j` to its next alternative value: the current value (and its
    /// source) goes to the back of the list, so repeated clicks cycle through all of them.
    /// The field is ticked when the new value changes the file.
    pub fn next_alt(&mut self, j: usize) -> bool {
        let Some(name) = review_fields(&self.old, &self.cand.tags)
            .get(j)
            .map(|f| f.0)
        else {
            return false;
        };
        let Some(k) = self.cand.alts.iter().position(|a| a.field == name) else {
            return false;
        };
        let alt = self.cand.alts.remove(k);
        let cur_src = super::tagsources::source_of(&self.cand, name);
        let Some(v) = field_mut(&mut self.cand.tags, name) else {
            return false;
        };
        let cur = std::mem::replace(v, alt.value.clone());
        if !cur.trim().is_empty() {
            self.cand.alts.push(Alt {
                field: name,
                value: cur,
                source: cur_src,
            });
        }
        self.cand.sources.push((name, alt.source));
        let (o, n) = {
            let f = review_fields(&self.old, &self.cand.tags);
            (f[j].1.clone(), f[j].2.clone())
        };
        if let Some(b) = self.fields.get_mut(j) {
            *b = field_changes(&o, &n);
        }
        true
    }
    /// Number of fields (cover included) this file would change.
    pub fn changes(&self) -> usize {
        self.fields.iter().filter(|b| **b).count()
            + usize::from(self.cover.is_some())
            + usize::from(self.rename_on && self.rename.is_some())
    }
}

/// Write the chosen fields and cover of one file. `session` gets a backup of the
/// tags right before this write (for Undo); the one-time original backup is kept too.
pub fn apply_review(r: &Review, originals: &Path, session: &Path) -> Result<(usize, bool), String> {
    let n = r.fields.iter().filter(|b| **b).count();
    let cover = r
        .cover
        .and_then(|i| r.covers.get(i))
        .map(|c| c.data.as_slice());
    if n == 0 && cover.is_none() {
        return Ok((0, false));
    }
    backup(originals, &r.path)?;
    backup(session, &r.path)?;
    write_tags(&r.path, &pick_fields(&r.cand.tags, &r.fields), cover)?;
    Ok((n, cover.is_some()))
}

/// Audio files under a folder, sorted.
pub fn audio_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| crate::library::kind_of(p) == Some("audio"))
        .collect();
    v.sort();
    v
}

/// A batch match is written only when it is this confident.
const CONFIDENT: u32 = 90;

// ---------- plugin ----------

#[derive(Debug, Clone)]
struct Row {
    path: PathBuf,
    old: Tags,
    cand: Candidate,
    /// Proposed file name (AcoustID fingerprint match or the AcoustID package).
    rename: Option<String>,
    /// How it was identified ("Fingerprint: system fpcalc", …).
    note: String,
}

/// Outcome of one file in the after-apply summary.
#[derive(Debug, Clone, PartialEq)]
pub struct FileResult {
    /// Path before the apply (backups are keyed by it).
    pub orig: PathBuf,
    /// Path now (differs when renamed).
    pub path: PathBuf,
    /// "tagged", "renamed", "skipped", "failed" or "undone".
    pub status: &'static str,
    pub detail: String,
    pub changes: Vec<String>,
    /// Written, so Undo can restore it.
    pub written: bool,
}

/// Summary headline counts: (tagged, renamed, skipped, failed).
pub fn result_counts(v: &[FileResult]) -> (usize, usize, usize, usize) {
    let n = |s: &str| v.iter().filter(|r| r.status == s).count();
    (
        n("tagged") + n("renamed"),
        n("renamed"),
        n("skipped"),
        n("failed"),
    )
}

enum Msg {
    Status(String),
    Rows(Vec<Row>, String),
    Row(Box<Row>),
    Done(String),
    Reviews(Vec<Review>),
    Applied(Vec<FileResult>),
}

#[derive(Default)]
pub struct AutoTagger {
    rows: Vec<Row>,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    batch: bool,
    reviews: Vec<Review>,
    /// Backup folder of the last apply and the files it changed (Undo).
    session: Option<(PathBuf, Vec<PathBuf>)>,
    /// Per-file results of the last apply (summary, per-file Undo).
    results: Vec<FileResult>,
}

fn cover_cache() -> PathBuf {
    crate::store::cache_dir().join("tagger-covers")
}

/// Show image bytes in Slint (through a small cache file; femtovg loads from paths).
fn slint_image(data: &[u8]) -> slint::Image {
    let dir = cover_cache();
    let _ = std::fs::create_dir_all(&dir);
    let ext = if data.starts_with(b"\x89PNG") {
        "png"
    } else {
        "jpg"
    };
    let f = dir.join(format!("{:x}.{ext}", md5::compute(data)));
    if !f.exists() {
        let _ = std::fs::write(&f, data);
    }
    slint::Image::load_from_path(&f).unwrap_or_default()
}

impl AutoTagger {
    /// A key from the plugin settings, else the environment, else (AcoustID) the build-time
    /// default (never logged).
    fn secret(cx: &Ctx, key: &str, env: &[&str]) -> String {
        let v = cx.get(key);
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
        env.iter()
            .find_map(|e| std::env::var(e).ok().filter(|v| !v.trim().is_empty()))
            .or_else(|| {
                // AcoustID application key baked in at build time (CI variable, like the
                // Jamendo client id); an application key, not a user secret.
                (key == "acoustid-key")
                    .then(|| option_env!("VIVI_ACOUSTID_KEY").map(String::from))
                    .flatten()
            })
            .unwrap_or_default()
    }

    /// The rename pattern, empty when "Suggest file names" is off.
    fn pattern(cx: &Ctx) -> String {
        if cx.get("rename") == "false" {
            return String::new();
        }
        let p = cx.get("rename-pattern");
        if p.trim().is_empty() {
            "{artist} - {title}".into()
        } else {
            p
        }
    }

    fn ui(cx: &Ctx, status: &str, busy: bool) {
        cx.app.set_md_status(status.into());
        cx.app.set_md_busy(busy);
    }

    fn open(&mut self, cx: &Ctx, target: String) {
        let a = cx.app;
        a.set_md_owner(ID.into());
        a.set_md_title("Auto-tagger".into());
        a.set_md_placeholder("File or folder to tag".into());
        a.set_md_search_label("Identify".into());
        a.set_md_primary("Review changes…".into());
        a.set_md_secondary("Review all files…".into());
        a.set_md_sources(ModelRc::new(VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
        a.set_md_query(target.into());
        a.set_md_detail("".into());
        a.set_md_open(true);
        self.rows.clear();
        self.show_rows(cx, None);
        self.start(cx);
    }

    fn show_rows(&self, cx: &Ctx, selected: Option<usize>) {
        let items: Vec<Item> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let t = &r.cand.tags;
                let key = if self.batch {
                    format!(
                        "{}  →  {} — {}",
                        r.path.file_name().unwrap_or_default().to_string_lossy(),
                        t.artist,
                        t.title
                    )
                } else {
                    format!("{} — {}", t.artist, t.title)
                };
                Item {
                    key: key.into(),
                    label: format!(
                        "{}{}  ·  {}  ·  {}%",
                        t.album,
                        if t.date.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", &t.date[..t.date.len().min(4)])
                        },
                        r.cand.source,
                        r.cand.score
                    )
                    .into(),
                    selected: Some(i) == selected,
                }
            })
            .collect();
        cx.app.set_md_results(ModelRc::new(VecModel::from(items)));
    }

    fn detail(&self, i: usize) -> String {
        let Some(r) = self.rows.get(i) else {
            return String::new();
        };
        let d = diff(&r.old, &r.cand.tags);
        let mut s = format!("{}\n\n", r.path.display());
        if d.is_empty() {
            s += "Tags already match.\n";
        }
        for (f, o, n) in d {
            let o = if o.is_empty() { "—".to_string() } else { o };
            s += &format!("{f}:  {o}  →  {n}\n");
        }
        if let Some(n) = &r.rename {
            s += &format!(
                "File name:  {}  →  {n}\n",
                r.path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        s += "Cover:  front cover from the Cover Art Archive, if there is one\n";
        s
    }

    fn render_review(&self, cx: &Ctx) {
        use crate::{TagCover, TagField, TagFile};
        let files: Vec<TagFile> = self
            .reviews
            .iter()
            .map(|r| {
                let mut fields: Vec<TagField> = review_fields(&r.old, &r.cand.tags)
                    .into_iter()
                    .zip(&r.fields)
                    .enumerate()
                    .map(|(j, ((name, o, n), on))| {
                        let changed = field_changes(&o, &n);
                        let (o, n) = if name == "Lyrics" {
                            (lyrics_summary(&o), lyrics_summary(&n))
                        } else {
                            (o, n)
                        };
                        TagField {
                            name: name.into(),
                            changed,
                            old: o.into(),
                            new: n.into(),
                            checked: *on,
                            source: super::tagsources::source_of(&r.cand, name).into(),
                            alts: r.alts_of(j) as i32,
                        }
                    })
                    .collect();
                // Rename proposal: the row after the tag fields.
                if let Some(n) = &r.rename {
                    fields.push(TagField {
                        name: "File name".into(),
                        changed: true,
                        old: r
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .as_ref()
                            .into(),
                        new: n.as_str().into(),
                        checked: r.rename_on,
                        source: "Your pattern".into(),
                        alts: 0,
                    });
                }
                let covers: Vec<TagCover> = r
                    .covers
                    .iter()
                    .map(|c| TagCover {
                        img: slint_image(&c.data),
                        label: c.label.as_str().into(),
                        info: image_info(&c.data).into(),
                    })
                    .collect();
                TagFile {
                    name: r
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .as_ref()
                        .into(),
                    source: format!(
                        "{} · {}%",
                        if r.cand.source.starts_with("AcoustID") {
                            "AcoustID + MusicBrainz"
                        } else {
                            "MusicBrainz search"
                        },
                        r.cand.confidence()
                    )
                    .into(),
                    confidence: r.cand.confidence() as i32,
                    note: r.note.as_str().into(),
                    checked: r.on,
                    fields: ModelRc::new(VecModel::from(fields)),
                    has_old_cover: r.old_cover.is_some(),
                    old_cover: r.old_cover.as_deref().map(slint_image).unwrap_or_default(),
                    old_cover_info: r
                        .old_cover
                        .as_deref()
                        .map(image_info)
                        .unwrap_or_default()
                        .into(),
                    covers: ModelRc::new(VecModel::from(covers)),
                    cover_pick: r.cover.map(|i| i as i32).unwrap_or(-1),
                }
            })
            .collect();
        cx.app.set_tr_files(ModelRc::new(VecModel::from(files)));
        let on: Vec<&Review> = self.reviews.iter().filter(|r| r.on).collect();
        let n: usize = on.iter().map(|r| r.changes()).sum();
        cx.app.set_tr_status(
            format!(
                "{} of {} files selected · {n} changes · ⇄ switches to another source's value · 🔍 enlarges a cover",
                on.len(),
                self.reviews.len()
            )
            .into(),
        );
    }

    /// Build the review for the given rows (one per file) in the background:
    /// reads the current cover and downloads the cover candidates.
    fn review(&mut self, cx: &Ctx, which: Vec<usize>) {
        let picks: Vec<(Row, Vec<Candidate>)> = which
            .iter()
            .filter_map(|&i| self.rows.get(i).cloned())
            .map(|r| {
                let others = self
                    .rows
                    .iter()
                    .filter(|o| o.path == r.path)
                    .map(|o| o.cand.clone())
                    .collect();
                (r, others)
            })
            .collect();
        if picks.is_empty() {
            Self::ui(cx, "Select a match first", false);
            return;
        }
        let embed = cx.flag("cover");
        let opts = super::tagsources::Opts {
            deezer: cx.flag("deezer"),
            itunes: cx.flag("itunes"),
            lastfm_key: Self::secret(cx, "lastfm-key", &["VIVI_LASTFM_KEY", "LASTFM_API_KEY"]),
            discogs_token: Self::secret(
                cx,
                "discogs-token",
                &["VIVI_DISCOGS_TOKEN", "DISCOGS_TOKEN"],
            ),
            lyrics: cx.flag("lyrics"),
        };
        let mut asked: Vec<&str> = vec!["MusicBrainz"];
        if opts.deezer {
            asked.push("Deezer");
        }
        if opts.itunes {
            asked.push("iTunes");
        }
        if !opts.lastfm_key.is_empty() {
            asked.push("Last.fm");
        }
        if !opts.discogs_token.is_empty() {
            asked.push("Discogs");
        }
        let asked = format!(
            "Sources: {}{}",
            asked.join(", "),
            if opts.lyrics {
                " · lyrics: LRCLIB"
            } else {
                " · lyrics embedding off"
            }
        );
        let (tx, rx) = channel();
        self.rx = Some(rx);
        Self::ui(cx, "Asking more sources and fetching cover art…", true);
        std::thread::spawn(move || {
            let total = picks.len();
            let v = picks
                .into_iter()
                .enumerate()
                .map(|(i, (mut r, others))| {
                    let _ = tx.send(Msg::Status(format!(
                        "Deezer / iTunes / Last.fm / Discogs / lyrics {}/{total}…",
                        i + 1
                    )));
                    let dur = read_tags(&r.path).map(|(_, d)| d).unwrap_or(0.0);
                    for n in super::tagsources::enrich(&mut r.cand, &opts, dur) {
                        eprintln!("vivi: tagger source: {n}");
                    }
                    let _ = tx.send(Msg::Status(format!("Fetching covers {}/{total}…", i + 1)));
                    let covers = cover_candidates(&r.cand, &others, 6);
                    let mut rv =
                        Review::new(r.path, r.old, r.cand, covers, embed).with_rename(r.rename);
                    rv.note = [r.note.as_str(), asked.as_str()]
                        .iter()
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    rv
                })
                .collect();
            let _ = tx.send(Msg::Reviews(v));
        });
    }

    fn apply_reviews(&mut self, cx: &Ctx) {
        if !self.reviews.iter().any(|r| r.on && r.changes() > 0) {
            cx.app.set_tr_status("Nothing selected to change".into());
            return;
        }
        let all = self.reviews.clone();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let session = backup_dir().join(format!("session-{stamp}"));
        let (tx, rx) = channel();
        self.rx = Some(rx);
        cx.app.set_tr_busy(true);
        cx.app.set_tr_status("Writing tags…".into());
        let dir = session.clone();
        self.session = Some((session, Vec::new()));
        std::thread::spawn(move || {
            let total = all.len();
            let results = all
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let _ = tx.send(Msg::Status(format!("Writing tags {}/{total}…", i + 1)));
                    apply_one(r, &backup_dir(), &dir)
                })
                .collect();
            let _ = tx.send(Msg::Applied(results));
        });
    }

    /// Show the summary of [`Self::results`].
    fn render_summary(&self, cx: &Ctx) {
        use crate::TagResult;
        let (tagged, renamed, skipped, failed) = result_counts(&self.results);
        let undone = self.results.iter().filter(|r| r.status == "undone").count();
        let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        let head = if tagged == 0 && undone > 0 {
            "Changes undone".to_string()
        } else if tagged == 0 {
            "Nothing was written".to_string()
        } else {
            format!("{} tagged", plural(tagged, "file"))
        };
        let mut sub = Vec::new();
        if renamed > 0 {
            sub.push(format!("{renamed} renamed"));
        }
        if skipped > 0 {
            sub.push(format!("{skipped} skipped"));
        }
        if failed > 0 {
            sub.push(format!("{failed} failed"));
        }
        if undone > 0 {
            sub.push(format!("{undone} undone"));
        }
        cx.app.set_tr_summary(head.as_str().into());
        cx.app.set_tr_summary_sub(sub.join(" · ").into());
        cx.app.set_tr_summary_ok(failed == 0 && tagged > 0);
        let items: Vec<TagResult> = self
            .results
            .iter()
            .map(|r| TagResult {
                name: r
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref()
                    .into(),
                status: r.status.into(),
                detail: r.detail.as_str().into(),
                changes: ModelRc::new(VecModel::from(
                    r.changes
                        .iter()
                        .map(|c| slint::SharedString::from(c.as_str()))
                        .collect::<Vec<_>>(),
                )),
                can_undo: r.written && r.status != "undone",
            })
            .collect();
        cx.app.set_tr_results(ModelRc::new(VecModel::from(items)));
        cx.app.set_tr_can_undo(
            self.results
                .iter()
                .any(|r| r.written && r.status != "undone"),
        );
    }

    /// Restore one file of the last apply (name first, then tags and cover).
    fn undo_one(&mut self, i: usize) -> Result<(), String> {
        let Some((dir, _)) = &self.session else {
            return Err("Nothing to undo".into());
        };
        let Some(r) = self.results.get_mut(i) else {
            return Err("No such file".into());
        };
        if !r.written || r.status == "undone" {
            return Ok(());
        }
        if r.path != r.orig {
            if r.orig.exists() {
                return Err(format!("{} exists again", r.orig.display()));
            }
            std::fs::rename(&r.path, &r.orig).map_err(|e| e.to_string())?;
            r.path = r.orig.clone();
        }
        restore(dir, &r.orig)?;
        r.status = "undone";
        r.detail = "Previous tags, cover and name restored".into();
        Ok(())
    }

    fn undo(&mut self, cx: &Ctx, which: Option<usize>) {
        let idx: Vec<usize> = match which {
            Some(i) => vec![i],
            None => (0..self.results.len()).collect(),
        };
        let errs: Vec<String> = idx
            .into_iter()
            .filter_map(|i| self.undo_one(i).err())
            .collect();
        let left = self
            .results
            .iter()
            .any(|r| r.written && r.status != "undone");
        if !left {
            if let Some((dir, _)) = self.session.take() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        let s = if errs.is_empty() {
            "Previous tags, covers and file names restored".to_string()
        } else {
            format!("Undo failed: {}", errs.join("; "))
        };
        self.render_summary(cx);
        cx.app.set_tr_status(s.as_str().into());
        cx.toast(&s);
    }

    /// Candidates from the AcoustID package: `args` = [path, json] with the AcoustID lookup
    /// response and the file name pattern. MusicBrainz is queried here (same as
    /// [`identify`]), then the usual candidates list and review follow.
    fn external(&mut self, cx: &Ctx, args: &[String]) {
        let (Some(path), Some(json)) = (args.first(), args.get(1)) else {
            return;
        };
        let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
        let pattern = v["pattern"].as_str().unwrap_or("").to_string();
        let ids = parse_acoustid(&v["acoustid"]);
        let target = PathBuf::from(path);
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let a = cx.app;
        a.set_settings_open(false);
        a.set_md_owner(ID.into());
        a.set_md_title("Auto-tagger".into());
        a.set_md_placeholder("File or folder to tag".into());
        a.set_md_search_label("Identify".into());
        a.set_md_primary("Review changes…".into());
        a.set_md_secondary("Review all files…".into());
        a.set_md_sources(ModelRc::new(VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
        a.set_md_query(path.as_str().into());
        a.set_md_detail("".into());
        a.set_md_open(true);
        self.batch = false;
        self.rows.clear();
        self.show_rows(cx, None);
        Self::ui(
            cx,
            "AcoustID match from the plugin: looking up MusicBrainz…",
            true,
        );
        let (tx, rx) = channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let m = match external_rows(&target, &ids, &pattern) {
                Ok(rows) if rows.is_empty() => {
                    Msg::Done("AcoustID: no match for the fingerprint".into())
                }
                Ok(rows) => Msg::Rows(rows, "Fingerprinted by the AcoustID plugin".into()),
                Err(e) => Msg::Done(e),
            };
            let _ = tx.send(m);
        });
    }

    fn start(&mut self, cx: &Ctx) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let target = PathBuf::from(cx.app.get_md_query().trim().to_string());
        let key = Self::secret(
            cx,
            "acoustid-key",
            &["VIVI_ACOUSTID_KEY", "ACOUSTID_API_KEY"],
        );
        let pattern = Self::pattern(cx);
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.rows.clear();
        self.show_rows(cx, None);
        cx.app.set_md_detail("".into());
        self.batch = target.is_dir();
        if self.batch {
            Self::ui(cx, "Scanning folder…", true);
            std::thread::spawn(move || batch(&target, &key, &pattern, &cancel, &tx));
        } else if target.is_file() {
            Self::ui(
                cx,
                if key.trim().is_empty() {
                    "Searching MusicBrainz…"
                } else {
                    "Fingerprinting and looking up AcoustID…"
                },
                true,
            );
            std::thread::spawn(move || {
                let m = match identify(&target, &key) {
                    Ok((old, c, notes)) => {
                        let note = notes.join(" · ");
                        let rows = c
                            .into_iter()
                            .map(|cand| Row {
                                path: target.clone(),
                                old: old.clone(),
                                rename: proposal(&cand, &pattern, &target),
                                cand,
                                note: note.clone(),
                            })
                            .collect();
                        Msg::Rows(rows, notes.join(". "))
                    }
                    Err(e) => Msg::Done(e),
                };
                let _ = tx.send(m);
            });
        } else {
            Self::ui(cx, "Choose an audio file or a folder", false);
            self.rx = None;
        }
    }
}

/// Apply one review and describe the outcome for the summary.
pub fn apply_one(r: &Review, originals: &Path, session: &Path) -> FileResult {
    let mut out = FileResult {
        orig: r.path.clone(),
        path: r.path.clone(),
        status: "skipped",
        detail: String::new(),
        changes: Vec::new(),
        written: false,
    };
    if !r.on {
        out.detail = "Unticked in the review".into();
        return out;
    }
    if r.changes() == 0 {
        out.detail = "Nothing to change: tags already match".into();
        return out;
    }
    let (n, c) = match apply_review(r, originals, session) {
        Ok(v) => v,
        Err(e) => {
            out.status = "failed";
            out.detail = format!("Tags not written: {e}");
            return out;
        }
    };
    out.written = n > 0 || c;
    for ((f, o, nv), on) in review_fields(&r.old, &r.cand.tags).iter().zip(&r.fields) {
        if *on {
            let (o, nv) = if *f == "Lyrics" {
                (lyrics_summary(o), lyrics_summary(nv))
            } else {
                (o.clone(), nv.clone())
            };
            let o = if o.is_empty() { "—".to_string() } else { o };
            out.changes.push(format!("{f}:  {o}  →  {nv}"));
        }
    }
    if c {
        out.changes.push(if r.old_cover.is_some() {
            "Cover:  replaced".into()
        } else {
            "Cover:  embedded".into()
        });
    }
    out.status = if out.written { "tagged" } else { "skipped" };
    out.detail = format!(
        "{n} field{}{}",
        if n == 1 { "" } else { "s" },
        if c { " + cover" } else { "" }
    );
    if let (true, Some(name)) = (r.rename_on, &r.rename) {
        match rename_file(&r.path, name) {
            Ok(to) => {
                out.changes.push(format!(
                    "File name:  {}  →  {name}",
                    r.path.file_name().unwrap_or_default().to_string_lossy()
                ));
                out.path = to;
                out.status = "renamed";
                out.written = true;
            }
            Err(e) => out.detail += &format!(" · not renamed: {e}"),
        }
    }
    out
}

/// Rows for AcoustID recording ids (score ≥ 50, best three) with rename proposals.
fn external_rows(path: &Path, ids: &[(String, u32)], pattern: &str) -> Result<Vec<Row>, String> {
    let (old, _) = read_tags(path)?;
    let mut rows = Vec::new();
    for (id, score) in ids.iter().filter(|(_, s)| *s >= 50).take(3) {
        let rec = lookup_recording(id)?;
        for cand in candidates_from_recording(&rec, *score, "AcoustID", 4) {
            rows.push(Row {
                path: path.to_path_buf(),
                old: old.clone(),
                rename: rename_to(pattern, &cand.tags, path),
                cand,
                note: "Fingerprint: AcoustID plugin (fpcalc)".into(),
            });
        }
    }
    Ok(rows)
}

/// File name proposal for a fingerprint (AcoustID) match; None for tag/name searches or
/// when renaming is off (empty pattern).
fn proposal(cand: &Candidate, pattern: &str, path: &Path) -> Option<String> {
    cand.source
        .starts_with("AcoustID")
        .then(|| rename_to(pattern, &cand.tags, path))
        .flatten()
}

fn batch(dir: &Path, key: &str, pattern: &str, cancel: &AtomicBool, tx: &Sender<Msg>) {
    let files = audio_files(dir);
    let total = files.len();
    let (mut found, mut skipped) = (0, 0);
    for (i, p) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let _ = tx.send(Msg::Status(format!(
            "{}% · identifying {}/{total}: {} · {found} matched, {skipped} skipped",
            i * 100 / total.max(1),
            i + 1,
            p.file_name().unwrap_or_default().to_string_lossy()
        )));
        match identify(p, key) {
            Ok((old, c, notes)) => match c.into_iter().next() {
                Some(cand) if cand.score >= CONFIDENT && !diff(&old, &cand.tags).is_empty() => {
                    found += 1;
                    let _ = tx.send(Msg::Row(Box::new(Row {
                        path: p.clone(),
                        old,
                        rename: proposal(&cand, pattern, p),
                        cand,
                        note: notes.join(" · "),
                    })));
                }
                _ => skipped += 1,
            },
            Err(_) => skipped += 1,
        }
    }
    let _ = tx.send(Msg::Done(format!(
        "{total} audio files: {found} confident matches to review, {skipped} unchanged or not found"
    )));
}

impl Plugin for AutoTagger {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Auto-tagger",
            "Identifies songs by their audio (AcoustID fingerprint) or tags, looks them up on MusicBrainz, Deezer, iTunes, Last.fm and Discogs and writes tags, hi-res covers and optional LRCLIB synced lyrics after a preview that shows each field's source. Originals are backed up.",
            "tag",
        )
        .setting(SettingSpec::secret(
            "acoustid-key",
            "AcoustID API key",
            "Optional. Register a free application at acoustid.org/new-application for audio fingerprint matching; without it songs are matched by tags or file name.",
        ))
        .setting(SettingSpec::toggle(
            "cover",
            "Embed cover art",
            "Download the front cover from the Cover Art Archive.",
            true,
        ))
        .setting(SettingSpec::toggle(
            "deezer",
            "Deezer",
            "Fill album, track, date and add a 1000 px cover from Deezer (no account needed).",
            true,
        ))
        .setting(SettingSpec::toggle(
            "itunes",
            "iTunes",
            "Fill album, track, date, genre and add a 1400 px cover from the iTunes Search API (no account needed).",
            true,
        ))
        .setting(SettingSpec::secret(
            "lastfm-key",
            "Last.fm API key",
            "Optional. Genre from Last.fm's top tags; create a free key at last.fm/api/account/create.",
        ))
        .setting(SettingSpec::secret(
            "discogs-token",
            "Discogs token",
            "Optional. Year, genre/style and cover from Discogs; personal access token from discogs.com/settings/developers.",
        ))
        .setting(SettingSpec::toggle(
            "lyrics",
            "Embed synced lyrics",
            "Adds LRCLIB lyrics to the review: MP3 gets SYLT + USLT, FLAC/Ogg/Opus a LYRICS comment, M4A ©lyr (LRC text).",
            false,
        ))
        .setting(SettingSpec::toggle(
            "rename",
            "Suggest file names",
            "When the audio fingerprint identifies a song, the review offers a new file name (unticked: renaming is opt-in). Never overwrites a file; characters not allowed in file names become _.",
            true,
        ))
        .setting(SettingSpec::text(
            "rename-pattern",
            "File name pattern",
            "Fields: {artist} {title} {album} {albumartist} {year} {date} {track} {genre}.",
            "{artist} - {title}",
            "{artist} - {title}",
        ))
        .setting(SettingSpec::text(
            "folder",
            "Library folder",
            "Folder for Tag library folder; empty = first library folder.",
            "~/Music",
            "",
        ))
        .setting(SettingSpec::action(
            "open",
            "Tag the playing file",
            "Opens the preview dialog for the current file.",
            "Tag file…",
        ))
        .setting(SettingSpec::action(
            "batch",
            "Tag library folder",
            "Finds confident matches for every audio file; you review before writing.",
            "Tag folder…",
        ))
        .setting(SettingSpec::action(
            "restore",
            "Restore original tags",
            "Puts back the tags of the playing file from before the first auto-tag.",
            "Restore",
        ))
    }
    fn disable(&mut self, cx: &Ctx) {
        self.cancel.store(true, Ordering::Relaxed);
        self.rx = None;
        if cx.app.get_md_owner() == ID {
            cx.app.set_md_open(false);
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let idx = || args.first().and_then(|a| a.parse::<usize>().ok());
        match name {
            "open" => {
                let p = cx
                    .local_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                cx.app.set_settings_open(false);
                self.open(cx, p);
            }
            "batch" => {
                let f = cx.get("folder");
                let f = if f.trim().is_empty() {
                    crate::config::Config::load()
                        .library
                        .folders
                        .first()
                        .cloned()
                        .unwrap_or_default()
                } else {
                    f
                };
                let f = match f.strip_prefix("~/") {
                    Some(r) => crate::store::home().join(r).to_string_lossy().into_owned(),
                    None => f,
                };
                cx.app.set_settings_open(false);
                self.open(cx, f);
            }
            "restore" => match cx.local_path() {
                Some(p) => match restore(&backup_dir(), &p) {
                    Ok(()) => cx.toast("Original tags restored"),
                    Err(e) => cx.toast(&e),
                },
                None => cx.toast("Play a local file first"),
            },
            "search" => self.start(cx),
            "acoustid" => self.external(cx, args),
            "select" => {
                if let Some(i) = idx() {
                    cx.app.set_md_detail(self.detail(i).into());
                }
            }
            "primary" => self.review(cx, idx().into_iter().collect()),
            "secondary" => {
                // One row per file: the first (best) one.
                let mut seen = std::collections::HashSet::new();
                let all = (0..self.rows.len())
                    .filter(|&i| seen.insert(self.rows[i].path.clone()))
                    .collect();
                self.review(cx, all);
            }
            "toggle-file" | "toggle-field" | "cover-pick" | "field-alt" => {
                let a = args.first().map(String::as_str).unwrap_or("");
                let mut it = a.split(':');
                let i = it.next().and_then(|x| x.parse::<usize>().ok());
                let j = it.next().and_then(|x| x.parse::<i64>().ok());
                let Some(r) = i.and_then(|i| self.reviews.get_mut(i)) else {
                    return;
                };
                match (name, j) {
                    ("toggle-file", _) => r.on = !r.on,
                    ("toggle-field", Some(j)) if j as usize >= r.fields.len() => {
                        r.rename_on = !r.rename_on && r.rename.is_some();
                    }
                    ("toggle-field", Some(j)) => {
                        let (o, n) = {
                            let f = review_fields(&r.old, &r.cand.tags);
                            let (_, o, n) = f[j as usize].clone();
                            (o, n)
                        };
                        if field_changes(&o, &n) {
                            if let Some(b) = r.fields.get_mut(j as usize) {
                                *b = !*b;
                            }
                        }
                    }
                    ("field-alt", Some(j)) if j >= 0 => {
                        r.next_alt(j as usize);
                    }
                    ("cover-pick", Some(j)) => {
                        r.cover = (j >= 0 && (j as usize) < r.covers.len()).then_some(j as usize)
                    }
                    _ => {}
                }
                self.render_review(cx);
            }
            "apply-selected" => self.apply_reviews(cx),
            "apply-all" => {
                for r in &mut self.reviews {
                    r.on = true;
                    let f = review_fields(&r.old, &r.cand.tags);
                    r.fields = f.iter().map(|(_, o, n)| field_changes(o, n)).collect();
                    if r.cover.is_none() && !r.covers.is_empty() {
                        r.cover = Some(0);
                    }
                    // Renaming stays opt-in: only files whose rename box is ticked.
                }
                self.render_review(cx);
                self.apply_reviews(cx);
            }
            "undo" => self.undo(cx, None),
            "undo-file" => self.undo(cx, idx()),
            "open-folder" => {
                if let Some(r) = idx().and_then(|i| self.results.get(i)) {
                    if let Some(d) = r.path.parent() {
                        let _ = std::process::Command::new("xdg-open").arg(d).spawn();
                    }
                }
            }
            "review-close" => {
                cx.app.set_tr_open(false);
                self.reviews.clear();
                cx.app.set_tr_files(ModelRc::default());
                // Thumbnails are only needed while the dialog is open.
                let _ = std::fs::remove_dir_all(cover_cache());
            }
            "close" => {
                self.cancel.store(true, Ordering::Relaxed);
                self.rx = None;
            }
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        let Some(rx) = &self.rx else { return };
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Status(s) => {
                    cx.app.set_md_status(s.as_str().into());
                    cx.app.set_tr_status(s.into());
                }
                Msg::Row(r) => {
                    self.rows.push(*r);
                    self.show_rows(cx, None);
                }
                Msg::Rows(r, note) => {
                    self.rows = r;
                    self.show_rows(cx, None);
                    let s = if self.rows.is_empty() {
                        "No match found on MusicBrainz".to_string()
                    } else {
                        format!(
                            "{} candidates. Select one to preview the changes.",
                            self.rows.len()
                        )
                    };
                    let s = if note.is_empty() {
                        s
                    } else {
                        format!("{s} {note}.")
                    };
                    Self::ui(cx, &s, false);
                    if !self.rows.is_empty() {
                        cx.app.set_md_detail(self.detail(0).into());
                    }
                    self.rx = None;
                    // Screenshots/tests: go straight to the review of the best match.
                    if std::env::var_os("VIVI_TAGREVIEW").is_some() && !self.rows.is_empty() {
                        self.review(cx, vec![0]);
                    }
                    return;
                }
                Msg::Done(s) => {
                    Self::ui(cx, &s, false);
                    if self.batch && !self.rows.is_empty() {
                        cx.app.set_md_detail(self.detail(0).into());
                    }
                    self.rx = None;
                    return;
                }
                Msg::Reviews(v) => {
                    self.reviews = v;
                    self.rx = None;
                    Self::ui(cx, "", false);
                    cx.app.set_tr_summary("".into());
                    cx.app.set_tr_busy(false);
                    cx.app.set_tr_can_undo(false);
                    self.render_review(cx);
                    cx.app.set_md_open(false);
                    cx.app.set_tr_open(true);
                    if std::env::var("VIVI_TAGREVIEW").as_deref() == Ok("apply") {
                        self.apply_reviews(cx);
                    }
                    return;
                }
                Msg::Applied(results) => {
                    self.rx = None;
                    if let Some(sess) = &mut self.session {
                        sess.1 = results
                            .iter()
                            .filter(|r| r.written)
                            .map(|r| r.orig.clone())
                            .collect();
                    }
                    self.results = results;
                    cx.app.set_tr_busy(false);
                    self.render_summary(cx);
                    cx.app.set_tr_status(
                        "Backups kept in ~/.local/share/vivi/tag-backups · Undo restores tags, covers and names".into(),
                    );
                    let (t, _, _, f) = result_counts(&self.results);
                    cx.toast(&format!(
                        "Auto-tagger: {t} tagged{}",
                        if f > 0 {
                            format!(", {f} failed")
                        } else {
                            String::new()
                        }
                    ));
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEDIA: &str = "/workspace/vivi-media/forecast.mp3";

    fn tmp_copy(name: &str) -> Option<PathBuf> {
        let src = Path::new(MEDIA);
        if !src.exists() {
            return None;
        }
        let d = std::env::temp_dir().join(format!("vivi-tagger-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("forecast.mp3");
        std::fs::copy(src, &p).unwrap();
        Some(p)
    }

    #[test]
    fn diff_skips_empty_and_equal() {
        let old = Tags {
            title: "a".into(),
            artist: "x".into(),
            ..Default::default()
        };
        let new = Tags {
            title: "a".into(),
            artist: "y".into(),
            album: "".into(),
            date: "2014".into(),
            ..Default::default()
        };
        let d = diff(&old, &new);
        assert_eq!(
            d,
            vec![
                ("Artist", "x".into(), "y".into()),
                ("Date", "".into(), "2014".into())
            ]
        );
    }

    fn cand(title: &str, artist: &str, album: &str, rel: &str, score: u32) -> Candidate {
        Candidate {
            tags: Tags {
                title: title.into(),
                artist: artist.into(),
                album: album.into(),
                mb_recording: "rec".into(),
                mb_release: rel.into(),
                ..Default::default()
            },
            release_group: String::new(),
            score,
            source: "MusicBrainz search",
            sources: Vec::new(),
            extra_covers: Vec::new(),
            alts: Vec::new(),
            agree: Vec::new(),
        }
    }

    #[test]
    fn ranking_merges_duplicates_and_prefers_confirmed_matches() {
        let current = Tags {
            title: "Local Forecast".into(),
            artist: "Kevin MacLeod".into(),
            ..Default::default()
        };
        let mut confirmed = cand("Other Song", "Band", "B", "r3", 88);
        confirmed.agree = vec!["Deezer", "iTunes", "Discogs", "Last.fm"];
        let v = vec![
            cand("Local Forecast", "Kevin MacLeod", "A", "r1", 80),
            cand("Local Forecast", "Kevin MacLeod", "A", "r1", 90), // duplicate, better
            cand("Something", "Else", "C", "r2", 92),
            confirmed,
        ];
        let r = rank_candidates(v, &current);
        assert_eq!(r.len(), 3, "same recording + release collapses");
        // 88 + 12 (three agreeing sources max) = 100 beats 90 + 5 (tags match) and 92.
        assert_eq!(r[0].tags.mb_release, "r3");
        assert_eq!(r[0].confidence(), 100);
        assert_eq!((r[1].tags.mb_release.as_str(), r[1].score), ("r1", 95));
        assert_eq!(r[2].tags.mb_release, "r2");
    }

    #[test]
    fn review_cycles_alternative_values() {
        let old = Tags {
            album: "Old".into(),
            ..Default::default()
        };
        let mut c = cand("T", "A", "MB Album", "r1", 90);
        c.alts.push(Alt {
            field: "Album",
            value: "Old".into(),
            source: "iTunes",
        });
        c.alts.push(Alt {
            field: "Album",
            value: "Deezer Album".into(),
            source: "Deezer",
        });
        let mut r = Review::new(PathBuf::from("/nonexistent.mp3"), old, c, Vec::new(), true);
        assert_eq!(r.alts_of(2), 2);
        assert!(r.fields[2]);
        assert!(r.next_alt(2));
        // "Old" equals the current value: nothing to write, the box unticks.
        assert_eq!(r.cand.tags.album, "Old");
        assert!(!r.fields[2]);
        assert_eq!(
            super::super::tagsources::source_of(&r.cand, "Album"),
            "iTunes"
        );
        assert!(r.next_alt(2));
        assert_eq!(r.cand.tags.album, "Deezer Album");
        assert!(r.fields[2]);
        assert!(r.next_alt(2));
        assert_eq!(r.cand.tags.album, "MB Album");
        assert_eq!(
            super::super::tagsources::source_of(&r.cand, "Album"),
            "MusicBrainz"
        );
        assert!(!r.next_alt(0), "no alternatives for the title");
    }

    #[test]
    fn rename_sanitizes_and_never_overwrites() {
        assert_eq!(
            sanitize_file_name("AC/DC: Back <in> Black?"),
            "AC_DC_ Back _in_ Black_"
        );
        assert_eq!(
            sanitize_file_name("  ..Artist -  - Title.. "),
            "Artist - Title"
        );
        assert_eq!(sanitize_file_name("con"), "con_");
        assert_eq!(sanitize_file_name("COM3.x"), "COM3_.x");
        assert_eq!(sanitize_file_name("Comet"), "Comet");
        assert_eq!(sanitize_file_name("a\tb\u{7}c"), "a b_c");
        let long = "ж".repeat(200);
        let s = sanitize_file_name(&long);
        assert!(s.len() <= MAX_STEM && s.chars().all(|c| c == 'ж'));
        let t = Tags {
            title: "Song/Live".into(),
            artist: "Band".into(),
            track: "3/12".into(),
            ..Default::default()
        };
        assert_eq!(
            rename_to("{track}. {artist} - {title}", &t, Path::new("/m/x.flac")).as_deref(),
            Some("03. Band - Song_Live.flac")
        );
        assert_eq!(
            rename_to("{title}", &t, Path::new("/m/Song_Live.mp3")),
            None
        );
        let d = std::env::temp_dir().join(format!("vivi-rename-safe-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("a.mp3");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(d.join("Taken.mp3"), b"b").unwrap();
        assert!(
            rename_file(&a, "taken.mp3").is_err(),
            "case-insensitive clash"
        );
        assert!(rename_file(&a, "../escape.mp3").is_err());
        assert!(rename_file(&a, ".hidden.mp3").is_err());
        let to = rename_file(&a, "New.mp3").unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"a");
        assert_eq!(std::fs::read(d.join("Taken.mp3")).unwrap(), b"b");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn summary_results_skip_fail_and_count() {
        let c = cand("T", "A", "Al", "r1", 90);
        let mut r = Review::new(
            PathBuf::from("/nonexistent/x.mp3"),
            Tags::default(),
            c,
            Vec::new(),
            true,
        );
        r.on = false;
        let tmp = std::env::temp_dir().join(format!("vivi-sum-{}", std::process::id()));
        assert_eq!(apply_one(&r, &tmp, &tmp).status, "skipped");
        r.on = true;
        let f = apply_one(&r, &tmp, &tmp);
        assert_eq!(f.status, "failed");
        assert!(f.detail.starts_with("Tags not written"), "{}", f.detail);
        let ok = FileResult {
            status: "renamed",
            written: true,
            ..f.clone()
        };
        assert_eq!(result_counts(&[f, ok]), (1, 1, 0, 1));
    }

    #[test]
    fn terms_from_name() {
        let t = Tags::default();
        assert_eq!(
            search_terms(&t, Path::new("/m/03 - Kevin MacLeod - Local Forecast.mp3")),
            ("Local Forecast".into(), "Kevin MacLeod".into())
        );
        assert_eq!(lucene("AC/DC (live)"), "AC\\/DC \\(live\\)");
    }

    #[test]
    fn parse_recording() {
        let rec = serde_json::json!({
            "id": "r1", "title": "Song", "first-release-date": "2010",
            "artist-credit": [{"name": "A", "joinphrase": " feat. "}, {"name": "B"}],
            "genres": [{"name": "jazz", "count": 3}, {"name": "pop", "count": 1}],
            "releases": [
                {"id": "bootleg", "title": "Live", "status": "Bootleg", "date": "2001"},
                {"id": "rel1", "title": "Album", "status": "Official", "date": "2011-02-03",
                 "release-group": {"id": "rg1", "primary-type": "Album"},
                 "media": [{"tracks": [{"number": "7"}]}]}
            ]
        });
        let c = candidates_from_recording(&rec, 95, "AcoustID", 4);
        assert_eq!(c.len(), 2);
        let t = &c[0].tags;
        assert_eq!(t.artist, "A feat. B");
        assert_eq!((t.album.as_str(), t.track.as_str()), ("Album", "7"));
        assert_eq!((t.date.as_str(), t.genre.as_str()), ("2011-02-03", "Jazz"));
        assert_eq!(c[0].release_group, "rg1");
        assert_eq!(c[1].tags.mb_release, "bootleg");
    }

    #[test]
    fn review_apply_and_undo_restores_tags_and_cover() {
        let Some(p) = tmp_copy("review") else { return };
        let png: Vec<u8> = {
            // 1×1 PNG
            let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01".to_vec();
            v.extend_from_slice(&[8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89]);
            v
        };
        assert_eq!(image_size(&png), Some((1, 1)));
        let (old, _) = read_tags(&p).unwrap();
        let mut new = old.clone();
        new.title = "Reviewed Title".into();
        new.album = "Reviewed Album".into();
        let cand = Candidate {
            tags: new,
            release_group: String::new(),
            score: 95,
            source: "AcoustID",
            sources: Vec::new(),
            extra_covers: Vec::new(),
            alts: Vec::new(),
            agree: Vec::new(),
        };
        let cover = CoverOpt {
            label: "Front".into(),
            data: png.clone(),
        };
        let mut r = Review::new(p.clone(), old.clone(), cand, vec![cover], true);
        // Only the title: the album change is unticked.
        r.fields[2] = false;
        r.cover = Some(0);
        let session = std::env::temp_dir().join(format!("vivi-review-{}", std::process::id()));
        let (n, c) = apply_review(&r, &session.join("orig"), &session).unwrap();
        assert_eq!((n, c), (1, true));
        let (now, _) = read_tags(&p).unwrap();
        assert_eq!(now.title, "Reviewed Title");
        assert_eq!(now.album, old.album);
        assert_eq!(embedded_cover(&p).as_deref(), Some(png.as_slice()));
        restore(&session, &p).unwrap();
        let (back, _) = read_tags(&p).unwrap();
        assert_eq!(back.title, old.title);
        assert_ne!(embedded_cover(&p).as_deref(), Some(png.as_slice()));
        let _ = std::fs::remove_dir_all(&session);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn embeds_synced_lyrics() {
        let Some(p) = tmp_copy("lyrics") else { return };
        let lrc = "[00:01.50]First line\n[00:04.00]Second line\n";
        let t = Tags {
            lyrics: lrc.into(),
            ..Default::default()
        };
        write_tags(&p, &t, None).unwrap();
        let (back, _) = read_tags(&p).unwrap();
        assert_eq!(back.lyrics.trim(), lrc.trim(), "USLT holds the LRC text");
        assert_eq!(
            read_sylt(&p),
            vec![
                (1500, "First line".to_string()),
                (4000, "Second line".to_string())
            ]
        );
        // Writing again replaces the SYLT frame instead of adding one.
        write_tags(&p, &t, None).unwrap();
        assert_eq!(read_sylt(&p).len(), 2);
        assert_eq!(lyrics_summary(lrc), "Synced, 2 lines: First line");
        // FLAC / Opus / M4A: one text field (LYRICS / ©lyr) with the LRC.
        for ext in ["flac", "opus", "m4a"] {
            let out = p.with_extension(ext);
            let ok = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-i"])
                .arg(&p)
                .args(["-t", "3", "-map", "0:a"])
                .arg(&out)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                continue;
            }
            write_tags(&out, &t, None).unwrap();
            assert_eq!(
                read_tags(&out).unwrap().0.lyrics.trim(),
                lrc.trim(),
                "{ext}"
            );
            let _ = std::fs::remove_file(&out);
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rename_pattern() {
        let t = Tags {
            title: "Song: Part 1/2".into(),
            artist: "AC/DC".into(),
            album: "Album".into(),
            date: "1980-07-25".into(),
            track: "3/10".into(),
            ..Default::default()
        };
        let p = Path::new("/m/01 track.mp3");
        assert_eq!(
            rename_to("{artist} - {title}", &t, p).as_deref(),
            Some("AC_DC - Song_ Part 1_2.mp3")
        );
        assert_eq!(
            rename_to("{track} {artist} - {album} ({year}) - {title}", &t, p).as_deref(),
            Some("03 AC_DC - Album (1980) - Song_ Part 1_2.mp3")
        );
        // Empty fields leave no dangling separators.
        let bare = Tags {
            title: "T".into(),
            ..Default::default()
        };
        assert_eq!(
            rename_to("{artist} - {title}", &bare, p).as_deref(),
            Some("T.mp3")
        );
        assert_eq!(rename_to("{artist} - {title}", &Tags::default(), p), None);
        assert_eq!(
            rename_to("{title}", &bare, Path::new("/m/T.mp3")),
            None,
            "same name"
        );
        let d = std::env::temp_dir().join(format!("vivi-rename-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.mp3"), b"x").unwrap();
        std::fs::write(d.join("b.mp3"), b"y").unwrap();
        assert!(
            rename_file(&d.join("a.mp3"), "b.mp3").is_err(),
            "never overwrites"
        );
        let to = rename_file(&d.join("a.mp3"), "c.mp3").unwrap();
        assert_eq!(std::fs::read(to).unwrap(), b"x");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parse_acoustid_results() {
        let v = serde_json::json!({"status": "ok", "results": [
            {"score": 0.5, "recordings": [{"id": "b"}]},
            {"score": 0.97, "recordings": [{"id": "a"}, {"id": "b"}]}
        ]});
        assert_eq!(
            parse_acoustid(&v),
            vec![("a".to_string(), 97), ("b".to_string(), 50)]
        );
    }

    #[test]
    fn write_backup_restore_roundtrip() {
        let Some(p) = tmp_copy("rt") else { return };
        let dir = p.parent().unwrap().join("backups");
        let (before, _) = read_tags(&p).unwrap();
        assert_eq!(before.artist, "Kevin MacLeod");
        backup(&dir, &p).unwrap();
        let new = Tags {
            title: "Local Forecast - Elevator".into(),
            album: "Rollin' at 5".into(),
            track: "12".into(),
            mb_recording: "b1d46379-5d1c-4a36-98d3-d7986dd33dc4".into(),
            ..Default::default()
        };
        let jpg = std::fs::read("/workspace/vivi-media/cover.jpg").ok();
        write_tags(&p, &new, jpg.as_deref()).unwrap();
        let (after, _) = read_tags(&p).unwrap();
        assert_eq!(after.title, new.title);
        assert_eq!(after.album, new.album);
        assert_eq!(after.track, "12");
        assert_eq!(after.mb_recording, new.mb_recording);
        assert_eq!(after.artist, "Kevin MacLeod");
        if jpg.is_some() {
            let tf = lofty::read_from_path(&p).unwrap();
            let tag = tf.primary_tag().unwrap();
            assert!(tag
                .pictures()
                .iter()
                .any(|x| x.pic_type() == PictureType::CoverFront));
        }
        // A second backup keeps the original.
        backup(&dir, &p).unwrap();
        restore(&dir, &p).unwrap();
        let (back, _) = read_tags(&p).unwrap();
        assert_eq!(back.title, before.title);
        assert_eq!(back.album, before.album);
        assert_eq!(back.mb_recording, "");
        assert!(restore(&dir, &p).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn builtin_fingerprint_matches_fpcalc() {
        let Some(p) = tmp_copy("fp") else { return };
        let (d, fp) = fingerprint_builtin(&p).unwrap();
        assert!((185..=192).contains(&d), "{d}");
        assert!(fp.len() > 1000 && fp.starts_with("AQAD"));
        // Compare raw sub-fingerprints with fpcalc's (if installed): decoders and
        // resamplers differ slightly, so allow some bit errors (AcoustID does too).
        let out = std::process::Command::new("fpcalc")
            .args(["-raw", "-json"])
            .arg(&p)
            .output();
        if let Some(out) = out.ok().filter(|o| o.status.success()) {
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            let theirs: Vec<u32> = v["fingerprint"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_u64().unwrap() as u32)
                .collect();
            let (_, ours) =
                fingerprint_raw(&p, &rusty_chromaprint::Configuration::preset_test2()).unwrap();
            let best = (0..8)
                .flat_map(|o| [(o, 0), (0, o)])
                .map(|(a, b)| {
                    let n = ours
                        .len()
                        .saturating_sub(a)
                        .min(theirs.len().saturating_sub(b));
                    let errs: u32 = (0..n)
                        .map(|i| (ours[i + a] ^ theirs[i + b]).count_ones())
                        .sum();
                    errs as f64 / (n as f64 * 32.0)
                })
                .fold(1.0, f64::min);
            eprintln!("bit error rate vs fpcalc: {best:.3}");
            assert!(best < 0.15, "{best}");
        }
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    /// Live MusicBrainz + Cover Art Archive (no key needed). `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_musicbrainz_and_cover() {
        let Some(p) = tmp_copy("live") else { return };
        let (old, c, _) = identify(&p, "").unwrap();
        assert_eq!(old.artist, "Kevin MacLeod");
        assert!(!c.is_empty());
        eprintln!("{:#?}", &c[..c.len().min(3)]);
        let best = c.iter().find(|c| !c.tags.mb_release.is_empty()).unwrap();
        let rec = lookup_recording(&best.tags.mb_recording).unwrap();
        assert!(!candidates_from_recording(&rec, 100, "x", 3).is_empty());
        let art = cover_art("a252b0a1-4465-45cf-9292-37ab4321b8af", "").unwrap();
        assert!(art.starts_with(&[0xff, 0xd8]));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
