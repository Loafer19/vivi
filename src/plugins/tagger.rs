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
        };
    }
    Ok((t, dur))
}

// ---------- fingerprinting ----------

/// Chromaprint fingerprint (compressed, base64url) and duration in seconds.
pub fn fingerprint(path: &Path) -> Result<(u32, String), String> {
    match fpcalc(path) {
        Some(r) => Ok(r),
        None => fingerprint_builtin(path),
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
            }
        })
        .collect();
    if out.is_empty() && !base.title.is_empty() {
        out.push(Candidate {
            tags: base,
            release_group: String::new(),
            score,
            source,
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
        match fingerprint(path).and_then(|(d, fp)| acoustid_lookup(key.trim(), d, &fp)) {
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
    Ok((old, out, notes))
}

/// Front cover from the Cover Art Archive (release, then release group), ≤ 500 px.
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
        .map_err(|e| e.to_string())
}

/// Back up, fetch the cover (if wanted) and write one candidate.
fn apply(path: &Path, c: &Candidate, with_cover: bool) -> Result<bool, String> {
    backup(&backup_dir(), path)?;
    let cover = if with_cover {
        cover_art(&c.tags.mb_release, &c.release_group)
    } else {
        None
    };
    write_tags(path, &c.tags, cover.as_deref())?;
    Ok(cover.is_some())
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
}

enum Msg {
    Status(String),
    Rows(Vec<Row>, String),
    Row(Box<Row>),
    Written(Result<String, String>),
    Done(String),
}

#[derive(Default)]
pub struct AutoTagger {
    rows: Vec<Row>,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    batch: bool,
}

impl AutoTagger {
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
        a.set_md_primary("Write tags".into());
        a.set_md_secondary("Write all".into());
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
        s += "Cover:  front cover from the Cover Art Archive, if there is one\n";
        s
    }

    fn start(&mut self, cx: &Ctx) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let target = PathBuf::from(cx.app.get_md_query().trim().to_string());
        let key = cx.get("acoustid-key");
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.rows.clear();
        self.show_rows(cx, None);
        cx.app.set_md_detail("".into());
        self.batch = target.is_dir();
        if self.batch {
            Self::ui(cx, "Scanning folder…", true);
            std::thread::spawn(move || batch(&target, &key, &cancel, &tx));
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
                        let rows = c
                            .into_iter()
                            .map(|cand| Row {
                                path: target.clone(),
                                old: old.clone(),
                                cand,
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

    fn write(&mut self, cx: &Ctx, which: Vec<usize>) {
        let rows: Vec<Row> = which
            .iter()
            .filter_map(|&i| self.rows.get(i).cloned())
            .collect();
        if rows.is_empty() {
            Self::ui(cx, "Select a match first", false);
            return;
        }
        let cover = cx.flag("cover");
        let (tx, rx) = channel();
        self.rx = Some(rx);
        Self::ui(cx, "Writing tags…", true);
        std::thread::spawn(move || {
            let (mut ok, mut covers, mut errs) = (0, 0, Vec::new());
            for (n, r) in rows.iter().enumerate() {
                let _ = tx.send(Msg::Status(format!("Writing {}/{}…", n + 1, rows.len())));
                match apply(&r.path, &r.cand, cover) {
                    Ok(c) => {
                        ok += 1;
                        covers += c as usize;
                    }
                    Err(e) => errs.push(format!(
                        "{}: {e}",
                        r.path.file_name().unwrap_or_default().to_string_lossy()
                    )),
                }
            }
            let m = if errs.is_empty() {
                Ok(format!(
                    "Tagged {ok} file{} ({covers} with cover). Originals backed up; Restore in Settings undoes it.",
                    if ok == 1 { "" } else { "s" }
                ))
            } else {
                Err(format!("Tagged {ok}; failed: {}", errs.join("; ")))
            };
            let _ = tx.send(Msg::Written(m));
        });
    }
}

fn batch(dir: &Path, key: &str, cancel: &AtomicBool, tx: &Sender<Msg>) {
    let files = audio_files(dir);
    let total = files.len();
    let (mut found, mut skipped) = (0, 0);
    for (i, p) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let _ = tx.send(Msg::Status(format!(
            "Identifying {}/{total}: {}",
            i + 1,
            p.file_name().unwrap_or_default().to_string_lossy()
        )));
        match identify(p, key) {
            Ok((old, c, _)) => match c.into_iter().next() {
                Some(cand) if cand.score >= CONFIDENT && !diff(&old, &cand.tags).is_empty() => {
                    found += 1;
                    let _ = tx.send(Msg::Row(Box::new(Row {
                        path: p.clone(),
                        old,
                        cand,
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
            "Identifies songs by their audio (AcoustID fingerprint) or tags, looks them up on MusicBrainz and writes title, artist, album, date, track, genre and Cover Art Archive covers after a preview. Originals are backed up.",
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
            "select" => {
                if let Some(i) = idx() {
                    cx.app.set_md_detail(self.detail(i).into());
                }
            }
            "primary" => self.write(cx, idx().into_iter().collect()),
            "secondary" => {
                // One row per file: the first (best) one.
                let mut seen = std::collections::HashSet::new();
                let all = (0..self.rows.len())
                    .filter(|&i| seen.insert(self.rows[i].path.clone()))
                    .collect();
                self.write(cx, all);
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
                Msg::Status(s) => cx.app.set_md_status(s.into()),
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
                Msg::Written(r) => {
                    let s = match r {
                        Ok(s) | Err(s) => s,
                    };
                    Self::ui(cx, &s, false);
                    cx.toast(&s);
                    self.rx = None;
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
