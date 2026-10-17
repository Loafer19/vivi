//! Match candidates: MusicBrainz search, AcoustID lookups, recording details, ranking.

use super::*;

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

pub(crate) fn rank_key(s: &str) -> String {
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

pub(crate) fn credit(v: &serde_json::Value) -> String {
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

pub(crate) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Escape a Lucene term for a MusicBrainz search query.
pub(crate) fn lucene(s: &str) -> String {
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
