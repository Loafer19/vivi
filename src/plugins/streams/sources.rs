//! Stream sources: radio-browser.info, SomaFM, iptv-org (cached) and M3U / M3U8 parsing.

use super::*;

// ---------- radio-browser.info ----------

/// Stations from a radio-browser JSON array.
pub fn parse_stations(v: &serde_json::Value) -> Vec<Stream> {
    let s = |x: &serde_json::Value| x.as_str().unwrap_or("").trim().to_string();
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let url = Some(s(&e["url_resolved"]))
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| s(&e["url"]));
            if url.is_empty() {
                return None;
            }
            Some(Stream {
                name: s(&e["name"]),
                url,
                logo: s(&e["favicon"]),
                group: s(&e["tags"]).replace(',', ", "),
                country: s(&e["country"]),
                codec: s(&e["codec"]),
                bitrate: e["bitrate"].as_u64().unwrap_or(0) as u32,
                homepage: s(&e["homepage"]),
                source: "radio-browser.info".into(),
                ..Default::default()
            })
        })
        .collect()
}

/// Query the directory. `mode` is a [`SOURCES`] index 0..=3.
pub fn radio_search(mode: usize, q: &str) -> Result<Vec<Stream>, String> {
    let q = q.trim();
    let (path, mut params): (&str, Vec<(&str, &str)>) = match mode {
        1 => ("/json/stations/topvote", vec![]),
        2 if q.len() == 2 => ("/json/stations/search", vec![("countrycode", q)]),
        2 => ("/json/stations/search", vec![("country", q)]),
        3 => ("/json/stations/search", vec![("tag", q)]),
        _ => ("/json/stations/search", vec![("name", q)]),
    };
    params.extend([
        ("limit", "100"),
        ("hidebroken", "true"),
        ("order", "votes"),
        ("reverse", "true"),
    ]);
    let mut err = String::new();
    for srv in SERVERS {
        match web::get_json(&format!("{srv}{path}"), &params, "radio-browser.info") {
            Ok(v) => return Ok(parse_stations(&v)),
            Err(e) => err = e,
        }
    }
    Err(err)
}

/// Stations whose name or tag matches `q` (name matches first, no duplicates).
pub fn radio_any(q: &str) -> Result<Vec<Stream>, String> {
    let mut v = radio_search(0, q)?;
    if let Ok(t) = radio_search(3, &q.to_lowercase()) {
        for s in t {
            if !v.iter().any(|o| o.url == s.url) {
                v.push(s);
            }
        }
    }
    Ok(v)
}

/// Whether `q` names a playlist we can load (URL or existing file).
pub fn is_playlist_source(q: &str) -> bool {
    let q = q.trim();
    if q.starts_with("http://") || q.starts_with("https://") {
        return true;
    }
    let p = match q.strip_prefix("~/") {
        Some(r) => crate::store::home().join(r),
        None => PathBuf::from(q),
    };
    p.is_file()
}

/// Placeholder of the query field per source.
pub fn placeholder(mode: usize) -> &'static str {
    match mode {
        0 => "Station name, e.g. jazz fm",
        1 => "Press Go for the most popular stations",
        2 => "Country or code, e.g. Ukraine or UA",
        3 => "Genre tag, e.g. folk, rock, news",
        M3U => "Playlist path or URL (.m3u / .m3u8), Choose file…, or a TV channel name",
        TV_COUNTRY => "Filter countries, then channels by name (Go = back to countries)",
        TV_CATEGORY => "Filter categories, then channels by name (Go = back to categories)",
        GENRES => "Filter genres, then stations (Go = back to genres)",
        SOMAFM => "Filter SomaFM channels by name or genre",
        SAVED => "Press Go to show favorites",
        RECENT => "Press Go to show recently played streams",
        MY_PL => "Filter your playlists, then channels (Go = back). Add one from the M3U source.",
        _ => "Channel or station name, e.g. news, jazz, bbc",
    }
}

/// Tell the directory a station was played (its click counter); best effort.
pub(crate) fn radio_click(url: &str) {
    let url = url.to_string();
    std::thread::spawn(move || {
        let _ = web::get_json(
            &format!("{}/json/url", SERVERS[0]),
            &[("url", &url)],
            "radio-browser.info",
        );
    });
}

/// Picker of genres: Popular, Trending, then the biggest radio-browser tags.
pub fn parse_tags(v: &serde_json::Value) -> Vec<Stream> {
    let mut out = vec![
        pick("Popular", "Most voted stations", "@topvote"),
        pick("Trending", "Most played right now", "@topclick"),
    ];
    for t in v.as_array().into_iter().flatten() {
        let name = t["name"].as_str().unwrap_or("").trim();
        let n = t["stationcount"].as_u64().unwrap_or(0);
        if name.len() > 1 && n > 0 {
            out.push(pick(name, &format!("{n} stations"), name));
        }
    }
    out
}

pub(crate) fn pick(name: &str, label: &str, key: &str) -> Stream {
    Stream {
        name: name.into(),
        url: key.into(),
        group: label.into(),
        source: PICK.into(),
        ..Default::default()
    }
}

pub(crate) fn genre_list() -> Result<Vec<Stream>, String> {
    let p = [
        ("order", "stationcount"),
        ("reverse", "true"),
        ("limit", "120"),
        ("hidebroken", "true"),
    ];
    let mut err = String::new();
    for srv in SERVERS {
        match web::get_json(&format!("{srv}/json/tags"), &p, "radio-browser.info") {
            Ok(v) => return Ok(parse_tags(&v)),
            Err(e) => err = e,
        }
    }
    Err(err)
}

/// Stations of a genre picker key (`@topvote`, `@topclick` or a tag).
pub(crate) fn genre_stations(key: &str) -> Result<Vec<Stream>, String> {
    let path = match key {
        "@topvote" => "/json/stations/topvote",
        "@topclick" => "/json/stations/topclick",
        _ => return radio_search(3, key),
    };
    let mut err = String::new();
    for srv in SERVERS {
        let p = [("limit", "100"), ("hidebroken", "true")];
        match web::get_json(&format!("{srv}{path}"), &p, "radio-browser.info") {
            Ok(v) => return Ok(parse_stations(&v)),
            Err(e) => err = e,
        }
    }
    Err(err)
}

// ---------- SomaFM ----------

/// SomaFM `channels.json`: one stream per channel, best MP3/AAC playlist.
pub fn parse_somafm(v: &serde_json::Value) -> Vec<Stream> {
    let s = |x: &serde_json::Value| x.as_str().unwrap_or("").trim().to_string();
    let rank = |q: &str| match q {
        "highest" => 3,
        "high" => 2,
        "low" => 1,
        _ => 0,
    };
    v["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let pl = c["playlists"].as_array()?;
            let best = pl
                .iter()
                .filter(|p| p["url"].as_str().is_some())
                .max_by_key(|p| (s(&p["format"]) == "mp3") as u8 * 10 + rank(&s(&p["quality"])))?;
            Some(Stream {
                name: s(&c["title"]),
                url: s(&best["url"]),
                logo: Some(s(&c["largeimage"]))
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| s(&c["image"])),
                group: format!(
                    "{} — {}",
                    s(&c["genre"]).replace('|', ", "),
                    s(&c["description"])
                ),
                codec: s(&best["format"]).to_uppercase(),
                homepage: format!("https://somafm.com/{}/", s(&c["id"])),
                source: "SomaFM".into(),
                ..Default::default()
            })
        })
        .collect()
}

pub(crate) fn somafm() -> Result<Vec<Stream>, String> {
    let v = cached_json(
        "somafm.json",
        "https://api.somafm.com/channels.json",
        "SomaFM",
    )?;
    Ok(parse_somafm(&v))
}

// ---------- iptv-org ----------

pub(crate) const TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Fetch `url` into the cache as `name` (reused for a day; stale copy on errors).
pub(crate) fn cached(name: &str, url: &str) -> Result<String, String> {
    let p = crate::store::cache_dir().join("iptv").join(name);
    let fresh = std::fs::metadata(&p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e < TTL);
    if fresh {
        if let Ok(t) = std::fs::read_to_string(&p) {
            return Ok(t);
        }
    }
    match web::get_bytes(url, 64 * 1024 * 1024) {
        Ok(Some(b)) => {
            let t = String::from_utf8_lossy(&b).into_owned();
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(&p, &t);
            Ok(t)
        }
        r => std::fs::read_to_string(&p).map_err(|_| match r {
            Err(e) => e,
            _ => format!("Not found: {url}"),
        }),
    }
}

pub(crate) fn cached_json(name: &str, url: &str, what: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(&cached(name, url)?).map_err(|e| format!("{what}: {e}"))
}

/// Countries (`code`, `name`, `flag`) or categories (`id`, `name`) of the iptv-org API.
pub fn parse_iptv_index(v: &serde_json::Value, countries: bool) -> Vec<Stream> {
    let mut out: Vec<Stream> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let name = e["name"].as_str()?.trim();
            let id = e[if countries { "code" } else { "id" }]
                .as_str()?
                .to_lowercase();
            let (dir, label) = if countries {
                (
                    "countries",
                    format!("{} {}", e["flag"].as_str().unwrap_or(""), id.to_uppercase()),
                )
            } else {
                ("categories", String::new())
            };
            Some(pick(name, label.trim(), &format!("{IPTV}/{dir}/{id}.m3u")))
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub(crate) fn iptv_index(countries: bool) -> Result<Vec<Stream>, String> {
    let f = if countries {
        "countries.json"
    } else {
        "categories.json"
    };
    let v = cached_json(f, &format!("{IPTV_API}/{f}"), "iptv-org")?;
    let v = parse_iptv_index(&v, countries);
    if v.is_empty() {
        return Err("iptv-org returned no entries".into());
    }
    Ok(v)
}

/// Channels of an iptv-org playlist URL (cached).
pub(crate) fn iptv_playlist(url: &str) -> Result<Vec<Stream>, String> {
    let name = url.rsplit('/').take(2).collect::<Vec<_>>().join("-");
    let mut v = parse_m3u(&cached(&name, url)?);
    for s in &mut v {
        s.source = "iptv-org".into();
    }
    Ok(v)
}

/// Channels of the whole iptv-org index whose name matches `q`.
pub(crate) fn iptv_search(q: &str) -> Result<Vec<Stream>, String> {
    let all = iptv_playlist(&format!("{IPTV}/index.m3u"))?;
    Ok(filter(&all, q))
}

/// Entries whose name (or group) contains every word of `q`, name matches first.
pub fn filter(all: &[Stream], q: &str) -> Vec<Stream> {
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return all.to_vec();
    }
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for s in all {
        let n = s.name.to_lowercase();
        if words.iter().all(|w| n.contains(w)) {
            a.push(s.clone());
        } else {
            let g = format!("{n} {}", s.group.to_lowercase());
            if words.iter().all(|w| g.contains(w)) {
                b.push(s.clone());
            }
        }
    }
    a.extend(b);
    a
}

// ---------- M3U ----------

pub(crate) fn attr(line: &str, key: &str) -> String {
    let pat = format!("{key}=\"");
    line.find(&pat)
        .and_then(|i| {
            let rest = &line[i + pat.len()..];
            rest.find('"').map(|j| rest[..j].to_string())
        })
        .unwrap_or_default()
}

/// Parse an M3U / extended M3U / IPTV playlist. Relative entries are skipped.
pub fn parse_m3u(text: &str) -> Vec<Stream> {
    let mut out = Vec::new();
    let mut pending: Option<Stream> = None;
    let mut guide = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#EXTM3U") {
            guide = epg_url(line);
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            // Name = text after the last comma outside quotes.
            let mut inq = false;
            let mut cut = None;
            for (i, c) in info.char_indices() {
                match c {
                    '"' => inq = !inq,
                    ',' if !inq => cut = Some(i),
                    _ => {}
                }
            }
            let name = cut.map(|i| info[i + 1..].trim()).unwrap_or("").to_string();
            let name = if name.is_empty() {
                attr(info, "tvg-name")
            } else {
                name
            };
            pending = Some(Stream {
                name: display_name(&name),
                logo: attr(info, "tvg-logo"),
                group: attr(info, "group-title"),
                country: attr(info, "tvg-country"),
                tvg_id: attr(info, "tvg-id"),
                epg: guide.clone(),
                source: "M3U".into(),
                ..Default::default()
            });
        } else if line.starts_with('#') {
            continue;
        } else if line.contains("://") {
            let mut s = pending.take().unwrap_or_else(|| Stream {
                source: "M3U".into(),
                ..Default::default()
            });
            s.url = line.to_string();
            if s.name.is_empty() {
                s.name = display_name(line.rsplit('/').find(|p| !p.is_empty()).unwrap_or(line));
            }
            out.push(s);
        } else {
            pending = None;
        }
    }
    out
}

/// First guide URL of an `#EXTM3U` header (`url-tvg` or `x-tvg-url`, comma lists allowed).
pub fn epg_url(header: &str) -> String {
    let v = Some(attr(header, "url-tvg"))
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| attr(header, "x-tvg-url"));
    v.split(',')
        .map(str::trim)
        .find(|u| u.starts_with("http"))
        .unwrap_or("")
        .to_string()
}

/// Group names of a `group-title` (iptv-org joins several with `;`).
pub(crate) fn groups_of(s: &Stream) -> impl Iterator<Item = &str> {
    s.group.split(';').map(str::trim).filter(|g| !g.is_empty())
}

/// The most common groups of a channel list, biggest first (at most `max`).
pub fn top_groups(all: &[Stream], max: usize) -> Vec<String> {
    let mut n: std::collections::HashMap<&str, usize> = Default::default();
    for s in all {
        for g in groups_of(s) {
            *n.entry(g).or_default() += 1;
        }
    }
    let mut v: Vec<(&str, usize)> = n.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.into_iter()
        .take(max)
        .map(|(g, _)| g.to_string())
        .collect()
}

/// Entries in group `g` (all when empty).
pub fn by_group(all: &[Stream], g: &str) -> Vec<Stream> {
    if g.is_empty() {
        return all.to_vec();
    }
    all.iter()
        .filter(|s| groups_of(s).any(|x| x.eq_ignore_ascii_case(g)))
        .cloned()
        .collect()
}

/// Favorites first (stable), keeping the order otherwise.
pub fn pin_favorites(v: &mut [Stream], favs: &std::collections::HashSet<String>) {
    v.sort_by_key(|s| !favs.contains(&s.url));
}

/// Load a playlist from a URL or a local path (`~/` expanded).
pub fn load_m3u(src: &str) -> Result<Vec<Stream>, String> {
    let src = src.trim();
    let text = if src.starts_with("http://") || src.starts_with("https://") {
        let b = web::get_bytes(src, 32 * 1024 * 1024)?.ok_or("Playlist not found (404)")?;
        String::from_utf8_lossy(&b).into_owned()
    } else {
        let p = match src.strip_prefix("~/") {
            Some(r) => crate::store::home().join(r),
            None => PathBuf::from(src),
        };
        std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?
    };
    let v = parse_m3u(&text);
    if v.is_empty() {
        return Err("No stream URLs found in that playlist".into());
    }
    Ok(v)
}
