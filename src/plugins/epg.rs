//! Light XMLTV program guide for Network Streams: what is on now / next per `tvg-id`.
//!
//! A guide (the playlist's `url-tvg` / `x-tvg-url`, or the plugin's EPG setting) is
//! downloaded on demand (`.xml` or `.xml.gz`), and only the next 12 hours, at most three
//! programmes per channel and only their titles are kept. That compact copy is cached as
//! JSON in `$XDG_CACHE_HOME/vivi/epg/` for six hours; the raw XML is never stored.

use super::web;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prog {
    pub start: i64,
    pub stop: i64,
    pub title: String,
}

/// Programmes by lowercase channel id, sorted by start.
pub type Guide = HashMap<String, Vec<Prog>>;

const KEEP: usize = 3;
const HORIZON: i64 = 12 * 3600;
const TTL: i64 = 6 * 3600;
/// Download cap of a guide (compressed size) and of its decompressed text.
const MAX_DOWNLOAD: u64 = 96 * 1024 * 1024;
const MAX_XML: u64 = 512 * 1024 * 1024;

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// XMLTV time (`20261015183000 +0300`) as Unix seconds.
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let d = s.get(..14)?;
    if !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |a: usize, b: usize| d[a..b].parse::<i64>().ok();
    let mut t = days_from_civil(n(0, 4)?, n(4, 6)?, n(6, 8)?) * 86400
        + n(8, 10)? * 3600
        + n(10, 12)? * 60
        + n(12, 14)?;
    let z = s[14..].trim();
    if z.len() >= 5 && (z.starts_with('+') || z.starts_with('-')) {
        let hh: i64 = z[1..3].parse().ok()?;
        let mm: i64 = z[3..5].parse().ok()?;
        let off = hh * 3600 + mm * 60;
        t -= if z.starts_with('-') { -off } else { off };
    }
    Some(t)
}

fn attr(tag: &str, key: &str) -> Option<String> {
    for q in ['"', '\''] {
        let pat = format!(" {key}={q}");
        if let Some(i) = tag.find(&pat) {
            let rest = &tag[i + pat.len()..];
            return rest.find(q).map(|j| unescape(&rest[..j]));
        }
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Programmes running now or starting within the next 12 hours.
pub fn parse_xmltv(xml: &str, now: i64) -> Guide {
    let mut g = Guide::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<programme") {
        rest = &rest[i..];
        let Some(end) = rest.find("</programme>") else {
            break;
        };
        let block = &rest[..end];
        rest = &rest[end + 12..];
        let head = &block[..block.find('>').unwrap_or(block.len())];
        let (Some(ch), Some(start), Some(stop)) = (
            attr(head, "channel"),
            attr(head, "start").and_then(|t| parse_time(&t)),
            attr(head, "stop").and_then(|t| parse_time(&t)),
        ) else {
            continue;
        };
        if stop <= now || start > now + HORIZON {
            continue;
        }
        let title = block
            .find("<title")
            .and_then(|t| {
                let b = &block[t..];
                let (o, c) = (b.find('>')?, b.find("</title>")?);
                (o < c).then(|| unescape(b[o + 1..c].trim()))
            })
            .unwrap_or_default();
        if !title.is_empty() {
            g.entry(ch.trim().to_lowercase())
                .or_default()
                .push(Prog { start, stop, title });
        }
    }
    for v in g.values_mut() {
        v.sort_by_key(|p| p.start);
        v.dedup_by_key(|p| p.start);
        v.truncate(KEEP);
    }
    g
}

/// Programmes of a playlist `tvg-id` (`BBCOne.uk@HD` also matches `BBCOne.uk`).
pub fn lookup<'a>(g: &'a Guide, tvg_id: &str) -> Option<&'a Vec<Prog>> {
    let id = tvg_id.trim().to_lowercase();
    if id.is_empty() {
        return None;
    }
    g.get(&id)
        .or_else(|| id.split('@').next().and_then(|b| g.get(b)))
}

/// (on now, next) at `now`.
pub fn now_next(v: &[Prog], now: i64) -> (Option<&Prog>, Option<&Prog>) {
    let cur = v.iter().find(|p| p.start <= now && now < p.stop);
    let next = v.iter().find(|p| p.start >= now && Some(*p) != cur);
    (cur, next)
}

fn mins(s: i64) -> String {
    let m = (s.max(0) + 59) / 60;
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h {:02} min", m / 60, m % 60)
    }
}

/// "Now: News (ends in 25 min)" / "Next: Film (in 25 min)" lines.
pub fn describe(v: &[Prog], now: i64) -> Vec<String> {
    let (c, n) = now_next(v, now);
    let mut out = Vec::new();
    if let Some(c) = c {
        out.push(format!("Now: {} (ends in {})", c.title, mins(c.stop - now)));
    }
    if let Some(n) = n {
        out.push(format!("Next: {} (in {})", n.title, mins(n.start - now)));
    }
    out
}

#[derive(Serialize, Deserialize)]
struct Cached {
    fetched: i64,
    guide: Guide,
}

fn cache_file(url: &str) -> std::path::PathBuf {
    crate::store::cache_dir()
        .join("epg")
        .join(format!("{}.json", super::streams::hash_name(url)))
}

/// Load a guide (compact cache when fresh, else download; stale cache on errors).
pub fn load(url: &str) -> Result<Guide, String> {
    let p = cache_file(url);
    let old: Option<Cached> = std::fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let t = now();
    if let Some(c) = &old {
        if t - c.fetched < TTL {
            return Ok(c.guide.clone());
        }
    }
    let fetched = (|| {
        let b = web::get_bytes(url, MAX_DOWNLOAD)?.ok_or("EPG not found (404)")?;
        let mut xml = String::new();
        if b.starts_with(&[0x1f, 0x8b]) {
            flate2::read::GzDecoder::new(&b[..])
                .take(MAX_XML)
                .read_to_string(&mut xml)
                .map_err(|e| format!("EPG: {e}"))?;
        } else {
            xml = String::from_utf8_lossy(&b).into_owned();
        }
        Ok::<_, String>(parse_xmltv(&xml, t))
    })();
    match fetched {
        Ok(guide) => {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            if let Ok(j) = serde_json::to_string(&Cached {
                fetched: t,
                guide: guide.clone(),
            }) {
                let _ = std::fs::write(&p, j);
            }
            Ok(guide)
        }
        Err(e) => old.map(|c| c.guide).ok_or(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xmltv_now_next() {
        assert_eq!(parse_time("19700101000000 +0000"), Some(0));
        assert_eq!(parse_time("20261015183000 +0300"), Some(1_792_078_200));
        assert_eq!(parse_time("20261015153000"), Some(1_792_078_200));
        let now = parse_time("20261015153500 +0000").unwrap();
        let xml = r#"<tv><channel id="A.uk"><display-name>A</display-name></channel>
<programme start="20261015150000 +0000" stop="20261015160000 +0000" channel="A.uk"><title lang="en">News &amp; Weather</title></programme>
<programme start="20261015160000 +0000" stop="20261015170000 +0000" channel="A.uk"><title>Film</title></programme>
<programme start="20261015100000 +0000" stop="20261015110000 +0000" channel="A.uk"><title>Old</title></programme>
<programme start="20261016150000 +0000" stop="20261016160000 +0000" channel="A.uk"><title>Tomorrow</title></programme>
</tv>"#;
        let g = parse_xmltv(xml, now);
        let v = lookup(&g, "a.UK@HD").unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(
            describe(v, now),
            [
                "Now: News & Weather (ends in 25 min)",
                "Next: Film (in 25 min)"
            ]
        );
        assert!(lookup(&g, "").is_none());
    }
}
