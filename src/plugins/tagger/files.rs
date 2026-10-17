//! Tags read from / written to the file name, safe renames and audio fingerprints (fpcalc or built-in).

use super::*;

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
    pub(crate) fn fields(&self) -> [(&'static str, ItemKey, &str); 12] {
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
pub(crate) const MAX_STEM: usize = 180;

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
pub(crate) fn fpcalc(path: &Path) -> Option<(u32, String)> {
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
pub(crate) fn fingerprint_raw(
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
