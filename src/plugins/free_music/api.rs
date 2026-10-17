//! Free Music sources: Internet Archive and Jamendo search clients, track tags and the
//! downloader (audio, cover, licence text).

use super::*;

pub(crate) const IA_ROWS: usize = 12;
pub(crate) const IA_TRACKS_PER_ITEM: usize = 8;
pub(crate) const IA_ALBUM_TRACKS: usize = 60;
/// Longest track kept (seconds); longer files are mixes, lectures or shows.
pub(crate) const IA_MAX_LEN: f64 = 20.0 * 60.0;
pub(crate) const IA_TIMEOUT: u64 = 12;

pub(crate) fn ia_clean(text: &str) -> String {
    let q: String = text
        .chars()
        .map(|c| {
            if "():\"\\[]{}^~*?!+-&|/".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    q.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Advanced-search query: music collections only, no spoken word, licence filter.
pub fn ia_query(q: &Query) -> String {
    let mut parts = Vec::new();
    let t = ia_clean(&q.text);
    if let Some((_, a)) = &q.artist {
        parts.push(format!("creator:(\"{}\")", ia_clean(a)));
    } else if !t.is_empty() {
        parts.push(format!(
            "(title:({t}) OR creator:({t}) OR subject:({t}) OR description:({t}))"
        ));
    }
    let g = q.genre();
    if !g.is_empty() {
        let alt = match g {
            "hiphop" => "hiphop OR \"hip hop\" OR rap",
            "lofi" => "lofi OR \"lo-fi\" OR chillhop",
            "soundtrack" => "soundtrack OR filmscore OR score",
            g => g,
        };
        // FMA items have no subjects; their genre is often only in the description.
        parts.push(format!("(subject:({alt}) OR description:({alt}))"));
    }
    let m = q.mood();
    if !m.is_empty() {
        parts.push(format!("(subject:({m}) OR description:({m}))"));
    }
    parts.push("mediatype:audio".into());
    parts.push(
        match q.lic {
            1 => "(licenseurl:*publicdomain* OR (licenseurl:*creativecommons* AND NOT licenseurl:*nc*))",
            2 => "(licenseurl:*publicdomain* OR (licenseurl:*creativecommons* AND NOT licenseurl:*nd*))",
            3 => "licenseurl:*licenses/by/*",
            4 => "licenseurl:*licenses/by-sa/*",
            5 => "licenseurl:*publicdomain*",
            _ => "(licenseurl:*creativecommons* OR licenseurl:*publicdomain*)",
        }
        .into(),
    );
    parts.push(if q.source == 3 {
        "collection:(freemusicarchive)".into()
    } else if t.is_empty() && q.artist.is_none() {
        // Browsing: curated netlabel / FMA collections only; the community upload
        // pool (opensource_audio) is dominated by game rips and mislabelled uploads.
        "collection:(netlabels OR freemusicarchive OR audio_music)".into()
    } else {
        "collection:(netlabels OR opensource_audio OR audio_music OR freemusicarchive OR 78rpm OR etree OR jamendo-albums)".into()
    });
    parts.push("NOT collection:(podcasts OR librivoxaudio OR oldtimeradio OR audio_bookspoetry OR radioprograms OR audio_religion OR audio_podcast OR audio_tech OR audio_news OR audio_foreign)".into());
    parts.push("NOT subject:(podcast OR lecture OR lectures OR sermon OR audiobook OR interview OR talk OR speech OR \"radio show\" OR episode OR philosophy OR quran OR koran OR bhajan OR kirtan OR \"sound effects\" OR sfx)".into());
    if q.chip == Some(2) {
        parts.push("collection:(netlabels)".into());
    }
    parts.join(" AND ")
}

/// Sort parameter of an IA search (empty = IA relevance).
pub fn ia_sort(q: &Query) -> &'static str {
    match (q.sort, q.chip) {
        (2, _) | (0, Some(1)) => "publicdate desc",
        (1, _) | (0, Some(0) | Some(2)) => "downloads desc",
        _ if q.text.trim().is_empty() => "downloads desc",
        _ => "",
    }
}

/// Item ids of a search page and whether more pages exist.
pub fn ia_ids(v: &serde_json::Value, page: usize) -> (Vec<(String, u64)>, bool) {
    let ids = v["response"]["docs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some((
                d["identifier"].as_str()?.to_string(),
                d["downloads"].as_u64().unwrap_or(0),
            ))
        })
        .collect();
    let found = v["response"]["numFound"].as_u64().unwrap_or(0) as usize;
    (ids, found > (page + 1) * IA_ROWS)
}

pub(crate) fn ia_search_ids(q: &Query) -> Result<(Vec<(String, u64)>, bool), String> {
    let query = ia_query(q);
    let rows = IA_ROWS.to_string();
    let page = (q.page + 1).to_string();
    let mut p = vec![
        ("q", query.as_str()),
        ("fl[]", "identifier"),
        ("fl[]", "downloads"),
        ("rows", rows.as_str()),
        ("page", page.as_str()),
        ("output", "json"),
    ];
    let sort = ia_sort(q);
    if !sort.is_empty() {
        p.push(("sort[]", sort));
    }
    let v = web::get_json_timeout("https://archive.org/advancedsearch.php", &p, IA, 20)?;
    Ok(ia_ids(&v, q.page))
}

pub(crate) fn ia_item(id: &str, max: usize, downloads: u64) -> Vec<Track> {
    web::get_json_timeout(
        &format!("https://archive.org/metadata/{}", urlencode(id)),
        &[],
        IA,
        IA_TIMEOUT,
    )
    .map(|m| {
        let mut v = ia_tracks(&m, max);
        for t in &mut v {
            t.popularity = downloads;
        }
        v
    })
    .unwrap_or_default()
}

/// Track length in seconds from an IA `length` field ("245.3" or "4:05").
pub(crate) fn ia_len(v: &serde_json::Value) -> Option<f64> {
    let l = s(v);
    if l.contains(':') {
        Some(
            l.split(':')
                .fold(0.0, |a, p| a * 60.0 + p.parse::<f64>().unwrap_or(0.0)),
        )
    } else {
        l.parse().ok()
    }
}

/// Drop upload noise from a title: "Song (ID: 396093)", "Song [www.site.com]".
pub fn clean_title(t: &str) -> String {
    let mut t = t.trim().to_string();
    for (open, close) in [('(', ')'), ('[', ']')] {
        if let (Some(a), true) = (t.rfind(open), t.ends_with(close)) {
            let inner = t[a + 1..t.len() - 1].to_lowercase();
            if a > 0
                && (inner.starts_with("id:") || inner.contains("www.") || inner.contains("http"))
            {
                t = t[..a].trim_end().to_string();
            }
        }
    }
    t
}

/// Title of a file name stem: drops a leading track number, separators and a trailing
/// "by <artist>" ("03_-_My_Song", "deep-and-dirty-by-kevin-macleod").
pub(crate) fn tidy_stem(stem: &str, artist: &str) -> String {
    let mut t = stem.replace('_', " ");
    if !t.contains(' ') {
        t = t.replace('-', " ");
    }
    let a = norm(artist);
    if !a.is_empty() {
        let low = t.to_lowercase();
        if let Some(i) = low.rfind(&format!(" by {a}")) {
            if i > 0 && low[i..].trim_end().len() == a.len() + 4 {
                t.truncate(i);
            }
        }
    }
    let t = t.as_str();
    let t = t.trim_start_matches(|c: char| c.is_ascii_digit());
    let t = t.trim_start_matches(|c: char| " -.".contains(c));
    if t.is_empty() {
        stem.to_string()
    } else {
        t.trim().to_string()
    }
}

/// MP3 tracks of an Internet Archive item (`/metadata/<id>` JSON), at most `max`.
pub fn ia_tracks(m: &serde_json::Value, max: usize) -> Vec<Track> {
    let md = &m["metadata"];
    let id = s(&md["identifier"]);
    let lic = s(&md["licenseurl"]);
    if id.is_empty() || lic.is_empty() {
        return Vec::new();
    }
    let files: Vec<&serde_json::Value> = m["files"].as_array().into_iter().flatten().collect();
    let fmt = |f: &serde_json::Value| s(&f["format"]);
    // Prefer VBR MP3 derivatives, else any MP3.
    let short = |f: &&&serde_json::Value| ia_len(&f["length"]).is_none_or(|l| l <= IA_MAX_LEN);
    let mut mp3: Vec<&&serde_json::Value> = files
        .iter()
        .filter(|f| fmt(f) == "VBR MP3")
        .filter(short)
        .collect();
    if mp3.is_empty() {
        mp3 = files
            .iter()
            .filter(|f| fmt(f).contains("MP3"))
            .filter(short)
            .collect();
    }
    mp3.sort_by_key(|f| {
        s(&f["track"])
            .split('/')
            .next()
            .and_then(|n| n.trim().parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    let fma = strings(&md["collection"])
        .iter()
        .any(|c| c == "freemusicarchive");
    let item_artist = s(&md["creator"]);
    let subjects: Vec<String> = strings(&md["subject"])
        .into_iter()
        .flat_map(|x| {
            x.split([';', ','])
                .map(|p| p.trim().to_lowercase())
                .collect::<Vec<_>>()
        })
        .filter(|x| !x.is_empty() && x.len() < 30)
        .take(8)
        .collect();
    let dl = |name: &str| {
        format!(
            "https://archive.org/download/{}/{}",
            urlencode(&id),
            name.split('/').map(urlencode).collect::<Vec<_>>().join("/")
        )
    };
    // Item artwork: a JPEG/PNG original (cover.jpg, folder.jpg, …), else the thumbnail.
    let thumb = format!("https://archive.org/services/img/{}", urlencode(&id));
    let art = files
        .iter()
        .filter(|f| {
            let f = fmt(f);
            f == "JPEG" || f == "PNG" || f == "Item Image"
        })
        .max_by_key(|f| {
            let n = s(&f["name"]).to_lowercase();
            (n.contains("cover") || n.contains("folder") || n.contains("front")) as u8
        })
        .map(|f| dl(&s(&f["name"])))
        .unwrap_or_else(|| thumb.clone());
    mp3.into_iter()
        .take(max)
        .map(|f| {
            let name = s(&f["name"]);
            let url = dl(&name);
            let stem = Path::new(&name)
                .file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default();
            let title = s(&f["title"]);
            let artist = s(&f["artist"]);
            let artist = if artist.is_empty() {
                item_artist.clone()
            } else {
                artist
            };
            let mut genres = strings(&f["genre"]);
            genres.retain(|g| !g.eq_ignore_ascii_case("other"));
            let title = if title.is_empty() {
                tidy_stem(&stem, &artist)
            } else {
                title
            };
            Track {
                id: format!("{id}/{name}"),
                title: clean_title(&title),
                artist_id: artist.clone(),
                artist,
                album: {
                    let a = s(&f["album"]);
                    if a.is_empty() {
                        s(&md["title"])
                    } else {
                        a
                    }
                },
                album_id: id.clone(),
                date: s(&md["date"]).chars().take(10).collect(),
                duration: ia_len(&f["length"]).unwrap_or(0.0).round() as u32,
                genres: genres.into_iter().map(|g| g.to_lowercase()).collect(),
                tags: subjects.clone(),
                license_url: lic.clone(),
                stream: url.clone(),
                download: url,
                cover: thumb.clone(),
                cover_full: art.clone(),
                page: format!("https://archive.org/details/{}", urlencode(&id)),
                popularity: 0,
                source: if fma { FMA } else { IA },
            }
        })
        .collect()
}

pub(crate) fn urlencode(s: &str) -> String {
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

/// How a Jamendo text search asks: full-text, track-name prefix, exact artist name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JMode {
    Search,
    Name,
    Artist,
    Browse,
}

/// Query parameters of a Jamendo `/tracks` request (without the client id).
pub fn jamendo_params(q: &Query, mode: JMode) -> Vec<(&'static str, String)> {
    let mut p: Vec<(&'static str, String)> = vec![
        ("format", "json".into()),
        ("limit", PAGE.to_string()),
        ("offset", (q.page * PAGE).to_string()),
        ("include", "musicinfo".into()),
        ("audioformat", "mp32".into()),
        ("audiodlformat", "mp32".into()),
        ("imagesize", "300".into()),
    ];
    let text = q.text.trim().to_string();
    if let Some((_, a)) = &q.album {
        // Jamendo lists album tracks in album order by default ("position" is no order).
        p.push(("album_id", a.clone()));
        return p;
    }
    if let Some((_, a)) = &q.artist {
        p.push(("artist_id", a.clone()));
    }
    match mode {
        JMode::Search => p.push(("search", text)),
        JMode::Name => p.push(("namesearch", text)),
        JMode::Artist => p.push(("artist_name", text)),
        JMode::Browse => {}
    }
    // Genre in `fuzzytags`, mood in `vartags` (both together: jazz AND relaxing).
    if !q.genre().is_empty() {
        p.push(("fuzzytags", q.genre().to_string()));
    }
    if !q.mood().is_empty() {
        p.push(("vartags", q.mood()));
    }
    if q.chip == Some(2) {
        p.push(("featured", "1".into()));
    }
    match q.lic {
        1 => p.push(("ccnc", "false".into())),
        2 => p.push(("ccnd", "false".into())),
        3 => {
            p.push(("ccnc", "false".into()));
            p.push(("ccnd", "false".into()));
            p.push(("ccsa", "false".into()));
        }
        4 => {
            p.push(("ccnc", "false".into()));
            p.push(("ccnd", "false".into()));
            p.push(("ccsa", "true".into()));
        }
        _ => {}
    }
    let (lo, hi) = q.len_range();
    if hi > 0 {
        p.push(("durationbetween", format!("{lo}_{hi}")));
    }
    let order = match (q.sort, q.chip) {
        (1, _) => "popularity_total",
        (2, _) | (0, Some(1)) => "releasedate_desc",
        (3, _) => "duration",
        (4, _) => "duration_desc",
        _ if mode == JMode::Browse || q.artist.is_some() => "popularity_month",
        _ => "",
    };
    if !order.is_empty() {
        p.push(("order", order.into()));
    }
    p
}

pub(crate) fn jamendo_get(
    key: &str,
    params: &[(&'static str, String)],
) -> Result<Vec<Track>, String> {
    let mut p: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    p.push(("client_id", key));
    // Jamendo sometimes answers an empty page for a query that has results; one retry.
    let mut last = Vec::new();
    for attempt in 0..3 {
        let v = web::get_json_timeout("https://api.jamendo.com/v3.0/tracks/", &p, JAMENDO, 15)
            .map_err(|e| scrub(e, key))?;
        if v["headers"]["status"] != "success" {
            return Err(format!(
                "Jamendo: {}",
                v["headers"]["error_message"]
                    .as_str()
                    .filter(|m| !m.is_empty())
                    .map(|m| scrub(m.to_string(), key))
                    .unwrap_or("request failed (check the client ID)".into())
            ));
        }
        last = jamendo_tracks(&v);
        if !last.is_empty() || attempt == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(250 * (attempt + 1)));
    }
    Ok(last)
}

/// One Jamendo page for `q`; text searches ask three ways in parallel and merge.
pub fn jamendo_search(key: &str, q: &Query) -> Result<(Vec<Track>, bool), String> {
    let modes: &[JMode] = if q.text.trim().is_empty() || q.album.is_some() {
        &[JMode::Browse]
    } else if q.page > 0 {
        &[JMode::Search, JMode::Name]
    } else {
        &[JMode::Search, JMode::Name, JMode::Artist]
    };
    let hs: Vec<_> = modes
        .iter()
        .map(|m| {
            let (key, p) = (key.to_string(), jamendo_params(q, *m));
            std::thread::spawn(move || jamendo_get(&key, &p))
        })
        .collect();
    let mut out: Vec<Track> = Vec::new();
    let mut seen = HashSet::new();
    let mut err = None;
    let mut more = false;
    // Interleave so that every way of asking contributes its best hits first.
    let lists: Vec<Vec<Track>> = hs
        .into_iter()
        .filter_map(
            |h| match h.join().unwrap_or_else(|_| Err("Jamendo: failed".into())) {
                Ok(v) => {
                    more |= v.len() >= PAGE;
                    Some(v)
                }
                Err(e) => {
                    err = Some(e);
                    None
                }
            },
        )
        .collect();
    for i in 0..PAGE {
        for l in &lists {
            if let Some(t) = l.get(i) {
                if seen.insert(t.id.clone()) {
                    out.push(t.clone());
                }
            }
        }
    }
    match (out.is_empty(), err) {
        (true, Some(e)) => Err(e),
        _ => Ok((out, more)),
    }
}

pub fn jamendo_tracks(v: &serde_json::Value) -> Vec<Track> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let mi = &r["musicinfo"]["tags"];
            let image = s(&r["image"]);
            Track {
                id: s(&r["id"]),
                title: unescape(&s(&r["name"])),
                artist: unescape(&s(&r["artist_name"])),
                artist_id: s(&r["artist_id"]),
                album: unescape(&s(&r["album_name"])),
                album_id: s(&r["album_id"]),
                date: s(&r["releasedate"]),
                duration: r["duration"].as_u64().unwrap_or(0) as u32,
                genres: strings(&mi["genres"]),
                tags: strings(&mi["vartags"]),
                license_url: s(&r["license_ccurl"]),
                stream: s(&r["audio"]),
                download: if r["audiodownload_allowed"].as_bool() == Some(false) {
                    String::new()
                } else {
                    s(&r["audiodownload"])
                },
                cover_full: if image.contains("width=") {
                    image.replace("width=300", "width=600")
                } else {
                    image.clone()
                },
                cover: image,
                page: s(&r["shareurl"]),
                popularity: 0,
                source: JAMENDO,
            }
        })
        .filter(|t| !t.stream.is_empty())
        .collect()
}

// ---------- similar ----------

/// Query for tracks like `t`: its genre and first mood tag, popular first, same source.
pub fn similar_query(t: &Track) -> Query {
    let genre = t.genres.iter().chain(&t.tags).find_map(|g| {
        CHIPS
            .iter()
            .position(|c| !c.1.is_empty() && c.1 == g.as_str())
    });
    let mood = t
        .tags
        .iter()
        .find_map(|g| MOODS.iter().skip(1).position(|m| m.eq_ignore_ascii_case(g)))
        .map(|i| i + 1)
        .unwrap_or(0);
    let text = if genre.is_none() {
        t.genres
            .iter()
            .chain(&t.tags)
            .next()
            .cloned()
            .unwrap_or_else(|| t.artist.clone())
    } else {
        String::new()
    };
    Query {
        text,
        source: SOURCES.iter().position(|x| *x == t.source).unwrap_or(0),
        chip: genre,
        mood,
        sort: 1,
        ..Default::default()
    }
}

// ---------- download ----------

/// Target file of a download: `dir/<Artist>/<Artist> - <Title>.mp3`.
pub fn target(t: &Track, dir: &Path) -> PathBuf {
    let artist = if t.artist.is_empty() {
        "Unknown artist"
    } else {
        &t.artist
    };
    dir.join(web::safe_name(artist)).join(format!(
        "{} - {}.mp3",
        web::safe_name(artist),
        web::safe_name(&t.title)
    ))
}

/// Tags written to a download: licence and attribution in copyright / licence / comment.
pub fn tags_for(t: &Track) -> Tags {
    let year: String = t.date.chars().take(4).collect();
    Tags {
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        album_artist: t.artist.clone(),
        date: t.date.clone(),
        genre: t
            .genres
            .iter()
            .take(3)
            .map(|g| {
                let mut c = g.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("; "),
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
    }
}

/// Text of the `<file>.licence.txt` next to a download.
pub fn licence_text(t: &Track) -> String {
    format!(
        "{}\n\nTitle: {}\nArtist: {}\nAlbum: {}\nSource: {}\nLicence: {} — {}\n{}\n",
        t.attribution(),
        t.title,
        t.artist,
        t.album,
        t.page,
        t.license(),
        t.license_url,
        lic_summary(&t.license_url)
    )
}

/// Download a track into [`target`] with tags, cover and a licence file; `progress`
/// gets per-mille progress. Returns the saved path.
pub fn download(t: &Track, dir: &Path, progress: &AtomicU32) -> Result<PathBuf, String> {
    use std::io::{Read, Write};
    if t.download.is_empty() {
        return Err("The artist does not allow downloads of this track (preview only)".into());
    }
    let out = target(t, dir);
    std::fs::create_dir_all(out.parent().unwrap_or(dir)).map_err(|e| e.to_string())?;
    let mut r = web::agent_with_timeout(600)
        .get(&t.download)
        .call()
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("download failed ({})", r.status()));
    }
    let total: u64 = r
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // lofty picks the format from the extension, so the partial file ends in .mp3.
    let tmp = out.with_extension("part.mp3");
    let res = (|| -> Result<(), String> {
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut rd = r.body_mut().with_config().limit(300 * 1024 * 1024).reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = 0u64;
        loop {
            let n = rd.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            got += n as u64;
            if let Some(pm) = (got * 1000).checked_div(total) {
                progress.store(pm.min(999) as u32, Ordering::Relaxed);
            }
        }
        if got == 0 {
            return Err("empty download".into());
        }
        let cover_url = if t.cover_full.is_empty() {
            &t.cover
        } else {
            &t.cover_full
        };
        let cover = (!cover_url.is_empty())
            .then(|| web::get_bytes(cover_url, 10 * 1024 * 1024).ok().flatten())
            .flatten()
            .or_else(|| {
                (cover_url != &t.cover && !t.cover.is_empty())
                    .then(|| web::get_bytes(&t.cover, 10 * 1024 * 1024).ok().flatten())
                    .flatten()
            });
        auto_tagger::write_tags(&tmp, &tags_for(t), cover.as_deref())?;
        std::fs::rename(&tmp, &out).map_err(|e| e.to_string())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let _ = std::fs::write(out.with_extension("licence.txt"), licence_text(t));
    progress.store(1000, Ordering::Relaxed);
    Ok(out)
}

// ---------- plugin ----------
