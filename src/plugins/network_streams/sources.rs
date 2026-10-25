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
                stationuuid: s(&e["stationuuid"]),
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
    radio_get(path, &params).map(|v| parse_stations(&v))
}

/// How long the discovered server list is kept (in memory).
const SERVERS_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The server list (base URLs, shuffled) and when it was made.
static RADIO_SERVERS: std::sync::Mutex<Option<(std::time::Instant, Vec<String>)>> =
    std::sync::Mutex::new(None);

/// Base URLs of the mirrors in a `/json/servers` answer, without duplicates (each mirror
/// is listed once per IP address). Only `*.api.radio-browser.info` names are kept.
pub fn parse_servers(v: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in v.as_array().into_iter().flatten() {
        let n = e["name"].as_str().unwrap_or("").trim().to_ascii_lowercase();
        let ok = n.strip_suffix(".api.radio-browser.info").is_some_and(|h| {
            !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
        let u = format!("https://{n}");
        if ok && !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

/// Shuffle in place (spreads the load over the mirrors as radio-browser asks); seeded
/// from the clock, no need for a real RNG.
pub(crate) fn shuffle<T>(v: &mut [T], seed: u64) {
    let mut x = seed | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

fn clock_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ u64::from(std::process::id()).rotate_left(32)
}

/// Mirrors to try, in order: the list radio-browser publishes (`/json/servers` on the
/// all-mirrors round robin, shuffled, kept for 15 minutes), else [`FALLBACK_SERVERS`].
pub fn radio_servers() -> Vec<String> {
    let mut c = RADIO_SERVERS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = c.as_ref() {
        if at.elapsed() < SERVERS_TTL && !v.is_empty() {
            return v.clone();
        }
    }
    let mut v = web::get_json_timeout(
        &format!("{}/json/servers", FALLBACK_SERVERS[0]),
        &[],
        "radio-browser.info",
        8,
    )
    .map(|j| parse_servers(&j))
    .unwrap_or_default();
    shuffle(&mut v, clock_seed());
    // The fallbacks stay at the end: a list of one mirror is common.
    for f in FALLBACK_SERVERS {
        if !v.iter().any(|s| s == f) {
            v.push(f.to_string());
        }
    }
    *c = Some((std::time::Instant::now(), v.clone()));
    v
}

/// Move `srv` to the end of the cached list after it failed.
fn demote_server(srv: &str) {
    let mut c = RADIO_SERVERS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, v)) = c.as_mut() {
        if let Some(i) = v.iter().position(|s| s == srv) {
            let s = v.remove(i);
            v.push(s);
        }
    }
}

/// One request's outcome: worth trying the next mirror (connection error, timeout,
/// overloaded or broken mirror), or final (a 4xx answer means the request itself).
#[derive(Debug, PartialEq)]
pub(crate) enum Attempt {
    Ok(serde_json::Value),
    Retry(String),
    Fail(String),
}

pub(crate) fn radio_try(base: &str, path: &str, q: &[(&str, &str)]) -> Attempt {
    let mut req = web::agent_as(15, &web::user_agent(), false)
        .get(&format!("{base}{path}"))
        .header("Accept", "application/json");
    for (k, v) in q {
        req = req.query(*k, *v);
    }
    let mut r = match req.call() {
        Ok(r) => r,
        Err(e) => return Attempt::Retry(format!("radio-browser.info: {e}")),
    };
    let status = r.status().as_u16();
    let body = r
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string();
    match (status, body) {
        (200..=299, Ok(b)) => match serde_json::from_str(&b) {
            Ok(v) => Attempt::Ok(v),
            Err(e) => Attempt::Retry(format!("radio-browser.info: {e}")),
        },
        (429 | 500..=599, _) => Attempt::Retry(format!(
            "radio-browser.info is busy ({status}), try again in a moment"
        )),
        (200..=299, Err(e)) => Attempt::Retry(format!("radio-browser.info: {e}")),
        _ => Attempt::Fail(format!("radio-browser.info error {status}")),
    }
}

/// GET `path` from the first mirror that answers; a mirror that fails moves to the back.
pub(crate) fn radio_get_from(
    servers: &[String],
    path: &str,
    q: &[(&str, &str)],
    mut failed: impl FnMut(&str),
) -> Result<serde_json::Value, String> {
    let mut err = "radio-browser.info: no server".to_string();
    for srv in servers {
        match radio_try(srv, path, q) {
            Attempt::Ok(v) => return Ok(v),
            Attempt::Fail(e) => return Err(e),
            Attempt::Retry(e) => {
                failed(srv);
                err = e;
            }
        }
    }
    Err(err)
}

pub(crate) fn radio_get(path: &str, q: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    radio_get_from(&radio_servers(), path, q, demote_server)
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

/// What the M3U source's query field holds.
#[derive(Debug, PartialEq)]
pub enum M3uQuery {
    Url,
    /// A playlist file that exists.
    File(PathBuf),
    /// Looks like a path (`/…`, `~/…`, `./…`, `file://…`, `C:\…`, `*.m3u`) but there is
    /// no such file: an error, not a channel name.
    Missing(PathBuf),
    /// Anything else: an iptv-org channel name.
    Name,
}

/// Local path of a query (`~/` expanded, `file://` URLs decoded).
fn local_path(q: &str) -> PathBuf {
    if let Some(r) = q.strip_prefix("file://") {
        return PathBuf::from(pct_decode(r));
    }
    match q.strip_prefix("~/") {
        Some(r) => crate::store::home().join(r),
        None => PathBuf::from(q),
    }
}

pub fn m3u_query(q: &str) -> M3uQuery {
    let q = q.trim();
    if q.starts_with("http://") || q.starts_with("https://") {
        return M3uQuery::Url;
    }
    let p = local_path(q);
    if p.is_file() {
        return M3uQuery::File(p);
    }
    let lower = q.to_ascii_lowercase();
    let b = q.as_bytes();
    let pathlike = q.starts_with(['/', '\\', '~', '.'])
        || lower.starts_with("file://")
        || (b.len() > 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'))
        || [".m3u", ".m3u8"].iter().any(|e| lower.ends_with(e));
    if pathlike {
        M3uQuery::Missing(p)
    } else {
        M3uQuery::Name
    }
}

/// Whether `q` names a playlist to load (a URL, a file, or a missing file whose error
/// should show) rather than a channel name.
pub fn is_playlist_source(q: &str) -> bool {
    m3u_query(q) != M3uQuery::Name
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

/// The click-counter path of a station id (`/json/url/<stationuuid>`), `None` for an id
/// that is not a UUID.
pub fn click_path(uuid: &str) -> Option<String> {
    let ok = uuid.len() == 36 && uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    ok.then(|| format!("/json/url/{}", uuid.to_ascii_lowercase()))
}

/// Tell the directory a station was played (its click counter, by station id as
/// radio-browser asks); best effort, in the background.
pub(crate) fn radio_click(uuid: &str) {
    let Some(path) = click_path(uuid) else {
        return;
    };
    std::thread::spawn(move || {
        let _ = radio_get(&path, &[]);
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
    radio_get("/json/tags", &p).map(|v| parse_tags(&v))
}

/// Stations of a genre picker key (`@topvote`, `@topclick` or a tag).
pub(crate) fn genre_stations(key: &str) -> Result<Vec<Stream>, String> {
    let path = match key {
        "@topvote" => "/json/stations/topvote",
        "@topclick" => "/json/stations/topclick",
        _ => return radio_search(3, key),
    };
    let p = [("limit", "100"), ("hidebroken", "true")];
    radio_get(path, &p).map(|v| parse_stations(&v))
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
    // radio-browser tags and SomaFM genres are comma lists; SomaFM adds its description
    // after " — " (also in favorites saved before), which is not a group.
    let sep = if matches!(s.source.as_str(), "SomaFM" | "radio-browser.info") {
        ','
    } else {
        ';'
    };
    s.group
        .split(';')
        .flat_map(move |g| g.split(" — ").next().unwrap_or("").split(sep))
        .map(str::trim)
        .filter(|g| !g.is_empty())
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
        let p = local_path(src);
        if !p.exists() {
            return Err(format!("Playlist file not found: {}", p.display()));
        }
        if !p.is_file() {
            return Err(format!("Not a playlist file: {}", p.display()));
        }
        std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?
    };
    let v = parse_m3u(&text);
    if v.is_empty() {
        return Err("No stream URLs found in that playlist".into());
    }
    Ok(v)
}

// ---------- Streams hub filters (#86) ----------

/// Countries offered by the hub's Radio and TV country filters (name, ISO code).
pub const HUB_COUNTRIES: [(&str, &str); 30] = [
    ("Ukraine", "UA"),
    ("United States", "US"),
    ("United Kingdom", "GB"),
    ("Germany", "DE"),
    ("France", "FR"),
    ("Poland", "PL"),
    ("Italy", "IT"),
    ("Spain", "ES"),
    ("Portugal", "PT"),
    ("Netherlands", "NL"),
    ("Belgium", "BE"),
    ("Switzerland", "CH"),
    ("Austria", "AT"),
    ("Czechia", "CZ"),
    ("Slovakia", "SK"),
    ("Romania", "RO"),
    ("Moldova", "MD"),
    ("Lithuania", "LT"),
    ("Latvia", "LV"),
    ("Estonia", "EE"),
    ("Finland", "FI"),
    ("Sweden", "SE"),
    ("Norway", "NO"),
    ("Denmark", "DK"),
    ("Greece", "GR"),
    ("Turkey", "TR"),
    ("Canada", "CA"),
    ("Brazil", "BR"),
    ("Japan", "JP"),
    ("Australia", "AU"),
];
/// Genre tags of the Radio genre filter.
pub const HUB_GENRES: [&str; 20] = [
    "pop",
    "rock",
    "jazz",
    "classical",
    "electronic",
    "dance",
    "hiphop",
    "ambient",
    "chillout",
    "lounge",
    "news",
    "talk",
    "metal",
    "oldies",
    "80s",
    "90s",
    "country",
    "blues",
    "reggae",
    "folk",
];
/// Languages of the Radio language filter (radio-browser names).
pub const HUB_LANGUAGES: [&str; 12] = [
    "english",
    "ukrainian",
    "german",
    "french",
    "spanish",
    "polish",
    "italian",
    "portuguese",
    "dutch",
    "czech",
    "japanese",
    "turkish",
];
/// Bitrate filter (label, minimum kbps).
pub const HUB_BITRATES: [(&str, u32); 5] = [
    ("Any", 0),
    ("64+ kbps", 64),
    ("128+ kbps", 128),
    ("192+ kbps", 192),
    ("320 kbps", 320),
];
/// Sort filter (label, radio-browser `order`). "Trending" and "Newest" are the
/// top-new choices.
pub const HUB_SORTS: [(&str, &str); 4] = [
    ("Top voted", "votes"),
    ("Most played", "clickcount"),
    ("Trending", "clicktrend"),
    ("Newest", "changetimestamp"),
];

/// The Radio tab's search: name words (may be empty) and its filters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RadioQuery {
    pub name: String,
    /// ISO code, "" = any.
    pub country: String,
    pub tag: String,
    pub language: String,
    /// Minimum kbps, 0 = any.
    pub bitrate: u32,
    /// radio-browser `order` ("" = votes).
    pub order: String,
}

/// `/json/stations/search` parameters of `q`.
pub fn radio_params(q: &RadioQuery) -> Vec<(&'static str, String)> {
    let mut p: Vec<(&'static str, String)> = Vec::new();
    let name = q.name.trim();
    if !name.is_empty() {
        p.push(("name", name.to_string()));
    }
    if !q.country.is_empty() {
        p.push(("countrycode", q.country.clone()));
    }
    if !q.tag.is_empty() {
        p.push(("tag", q.tag.clone()));
        p.push(("tagExact", "false".into()));
    }
    if !q.language.is_empty() {
        p.push(("language", q.language.clone()));
    }
    if q.bitrate > 0 {
        p.push(("bitrateMin", q.bitrate.to_string()));
    }
    let order = if q.order.is_empty() {
        "votes"
    } else {
        q.order.as_str()
    };
    p.push(("order", order.into()));
    // Newest first; the counters highest first.
    p.push(("reverse", "true".into()));
    p.push(("limit", "120".into()));
    p.push(("hidebroken", "true".into()));
    p
}

/// Stations for the Radio tab.
pub fn radio_filtered(q: &RadioQuery) -> Result<Vec<Stream>, String> {
    let p = radio_params(q);
    let p: Vec<(&str, &str)> = p.iter().map(|(k, v)| (*k, v.as_str())).collect();
    radio_get("/json/stations/search", &p).map(|v| parse_stations(&v))
}

#[cfg(test)]
mod hub_tests {
    use super::*;

    #[test]
    fn radio_filters_become_search_parameters() {
        let p = radio_params(&RadioQuery::default());
        assert!(p.contains(&("order", "votes".into())));
        assert!(!p.iter().any(|(k, _)| *k == "name" || *k == "countrycode"));
        let q = RadioQuery {
            name: " jazz ".into(),
            country: "UA".into(),
            tag: "jazz".into(),
            language: "ukrainian".into(),
            bitrate: 128,
            order: "changetimestamp".into(),
        };
        let p = radio_params(&q);
        for want in [
            ("name", "jazz"),
            ("countrycode", "UA"),
            ("tag", "jazz"),
            ("language", "ukrainian"),
            ("bitrateMin", "128"),
            ("order", "changetimestamp"),
            ("hidebroken", "true"),
        ] {
            assert!(p.contains(&(want.0, want.1.to_string())), "{want:?}");
        }
    }
}
