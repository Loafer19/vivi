//! Applying matches: backups / restore, tag and synced-lyrics writing, review fields, cover candidates and per-file results.

use super::*;

pub fn backup_dir() -> PathBuf {
    crate::store::data_dir().join("tag-backups")
}

pub(crate) fn backup_file(dir: &Path, path: &Path) -> PathBuf {
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

pub(crate) fn mime_of(data: &[u8]) -> MimeType {
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
    crate::plugins::lyrics::Lrc::parse(lrc)
        .lines
        .into_iter()
        .map(|(t, l)| ((t * 1000.0).round() as u32, l))
        .collect()
}

/// Replace the ID3v2 SYLT (synchronised lyrics, ms timestamps) frame of an MP3.
pub(crate) fn write_sylt(path: &Path, lrc: &str) -> Result<(), String> {
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
        let cur_src = crate::plugins::tagsources::source_of(&self.cand, name);
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
pub(crate) const CONFIDENT: u32 = 90;

// ---------- plugin ----------

#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub(crate) path: PathBuf,
    pub(crate) old: Tags,
    pub(crate) cand: Candidate,
    /// Proposed file name (AcoustID fingerprint match or the AcoustID package).
    pub(crate) rename: Option<String>,
    /// How it was identified ("Fingerprint: system fpcalc", …).
    pub(crate) note: String,
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
