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

/// AcoustID lookup of a fingerprint: the recording matches, best first.
///
/// The key and the fingerprint go in a POST body, never in the URL, so neither ends up in
/// an error message or a log line; the key is also masked in any error text.
pub fn acoustid_lookup(key: &str, dur: u32, fp: &str) -> Result<Vec<Match>, String> {
    let d = dur.to_string();
    let mask = |e: String| redact(&e, key);
    let mut r = web::agent_as(30, &web::user_agent(), false)
        .post("https://api.acoustid.org/v2/lookup")
        .header("Accept", "application/json")
        .send_form([
            ("client", key),
            ("meta", "recordings releasegroups"),
            ("duration", d.as_str()),
            ("fingerprint", fp),
        ])
        .map_err(|e| mask(format!("AcoustID: {e}")))?;
    let status = r.status().as_u16();
    let body = r
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| mask(format!("AcoustID: {e}")))?;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    if v["status"] != "ok" {
        let msg = v["error"]["message"].as_str().unwrap_or(match status {
            429 | 503 => "rate limited, try again in a moment",
            _ => "lookup failed",
        });
        return Err(mask(format!("AcoustID: {msg}")));
    }
    Ok(parse_acoustid_matches(&v))
}

/// `s` with every occurrence of the secret `key` replaced by `***`.
pub fn redact(s: &str, key: &str) -> String {
    if key.trim().is_empty() {
        s.to_string()
    } else {
        s.replace(key.trim(), "***")
    }
}

/// (recording id, score 0–100) of an AcoustID lookup answer, best first.
pub fn parse_acoustid(v: &serde_json::Value) -> Vec<(String, u32)> {
    parse_acoustid_matches(v)
        .into_iter()
        .map(|m| (m.id, m.score))
        .collect()
}

/// The recordings of an AcoustID lookup answer (with or without `meta=recordings`),
/// each once with its best score, best first. The album is the first release group of
/// type Album, else the first one.
pub fn parse_acoustid_matches(v: &serde_json::Value) -> Vec<Match> {
    let score = |r: &serde_json::Value| {
        (r["score"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0) * 100.0).round() as u32
    };
    let mut results: Vec<&serde_json::Value> =
        v["results"].as_array().into_iter().flatten().collect();
    // Best result first, so a recording listed twice keeps its best score.
    results.sort_by_key(|r| std::cmp::Reverse(score(r)));
    let mut out: Vec<Match> = Vec::new();
    for r in results {
        let s = score(r);
        for rec in r["recordings"].as_array().into_iter().flatten() {
            let Some(id) = rec["id"].as_str() else {
                continue;
            };
            if out.iter().any(|m| m.id == id) {
                continue;
            }
            let groups: Vec<&serde_json::Value> = rec["releasegroups"]
                .as_array()
                .into_iter()
                .flatten()
                .collect();
            let album = groups
                .iter()
                .find(|g| g["type"].as_str() == Some("Album"))
                .or(groups.first())
                .and_then(|g| g["title"].as_str())
                .unwrap_or("");
            out.push(Match {
                id: id.into(),
                score: s,
                title: rec["title"].as_str().unwrap_or("").into(),
                artist: credit(&rec["artists"]),
                album: album.into(),
                duration: rec["duration"].as_f64().unwrap_or(0.0),
            });
        }
    }
    out
}

/// Fingerprint `path` and look it up on AcoustID, or take the cached answer for this
/// file (same path, size and modification time). Fresh answers, including "no match",
/// are cached. Returns the matches and how they were obtained (for the notes).
pub fn acoustid_matches(path: &Path, key: &str) -> Result<(Vec<Match>, String), String> {
    if let Some(m) = cache::cached(path) {
        return Ok((m, "cached AcoustID answer".into()));
    }
    let (d, fp, how) = fingerprint_with(path)?;
    let m = acoustid_lookup(key.trim(), d, &fp)?;
    cache::remember(path, &m);
    Ok((m, format!("Fingerprint: {how}")))
}

/// The best match whose score is at least `min` (0–100) and that has a recording id;
/// among equal scores the first one with a title (AcoustID lists the best first).
pub fn best_confident(matches: &[Match], min: u32) -> Option<Match> {
    let ok: Vec<&Match> = matches
        .iter()
        .filter(|m| m.score >= min && !m.id.is_empty())
        .collect();
    let top = ok.iter().map(|m| m.score).max()?;
    ok.iter()
        .filter(|m| m.score == top)
        .find(|m| !m.title.is_empty())
        .or_else(|| ok.iter().find(|m| m.score == top))
        .map(|m| (*m).clone())
}

/// Full MusicBrainz recording lookup.
pub fn lookup_recording(id: &str) -> Result<serde_json::Value, String> {
    web::musicbrainz(
        &format!("recording/{id}"),
        &[("inc", "artist-credits+releases+release-groups+media+genres")],
    )
}

/// Identify one file: AcoustID (when `key` is set; answers cached per file) then
/// MusicBrainz search. Returns the current tags and the candidates, best first.
pub fn identify(path: &Path, key: &str) -> Result<(Tags, Vec<Candidate>, Vec<String>), String> {
    let (old, dur) = read_tags(path)?;
    let mut notes = Vec::new();
    let mut out = Vec::new();
    if !key.trim().is_empty() {
        match acoustid_matches(path, key) {
            Ok((ids, how)) => {
                notes.push(how);
                for m in ids.into_iter().filter(|m| m.score >= 50).take(3) {
                    let rec = lookup_recording(&m.id)?;
                    out.extend(candidates_from_recording(&rec, m.score, "AcoustID", 4));
                }
                if out.is_empty() {
                    notes.push("AcoustID: no match for the fingerprint".to_string());
                }
            }
            Err(e) => notes.push(redact(&e, key)),
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
