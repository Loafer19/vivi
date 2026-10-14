//! Small HTTP helpers shared by the music plugins: a ureq agent with a descriptive
//! User-Agent (MusicBrainz requires one with contact information), the MusicBrainz
//! rate limit (one request per second, shared by every thread) and size-capped downloads.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `Vivi/<version> ( https://github.com/Loafer19/vivi )`, as MusicBrainz asks for.
pub fn user_agent() -> String {
    format!(
        "Vivi/{} ( https://github.com/Loafer19/vivi )",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(user_agent())
        .http_status_as_error(false)
        .build()
        .into()
}

static MB_LAST: Mutex<Option<Instant>> = Mutex::new(None);

/// Block until at least `gap` has passed since the previous MusicBrainz request.
pub fn mb_throttle(gap: Duration) {
    let mut last = MB_LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = *last {
        let since = t.elapsed();
        if since < gap {
            std::thread::sleep(gap - since);
        }
    }
    *last = Some(Instant::now());
}

/// GET a JSON document; non-2xx answers become errors naming `what`.
pub fn get_json(
    url: &str,
    query: &[(&str, &str)],
    what: &str,
) -> Result<serde_json::Value, String> {
    let mut req = agent().get(url).header("Accept", "application/json");
    for (k, v) in query {
        req = req.query(*k, *v);
    }
    let mut r = req.call().map_err(|e| format!("{what}: {e}"))?;
    let status = r.status().as_u16();
    let body = r
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("{what}: {e}"))?;
    match status {
        200..=299 => serde_json::from_str(&body).map_err(|e| format!("{what}: {e}")),
        503 | 429 => Err(format!("{what}: rate limited, try again in a moment")),
        _ => {
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v["error"]["message"]
                        .as_str()
                        .or(v["error"].as_str())
                        .or(v["headers"]["error_message"].as_str())
                        .map(String::from)
                })
                .unwrap_or_default();
            Err(format!("{what} error {status} {msg}").trim().to_string())
        }
    }
}

/// A MusicBrainz web service call (`/ws/2/...`, JSON), rate limited to 1 request/s.
pub fn musicbrainz(path: &str, query: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    mb_throttle(Duration::from_millis(1100));
    let mut q = query.to_vec();
    q.push(("fmt", "json"));
    get_json(
        &format!("https://musicbrainz.org/ws/2/{path}"),
        &q,
        "MusicBrainz",
    )
}

/// Download a URL into memory (redirects followed), at most `limit` bytes.
/// `Ok(None)` for 404.
pub fn get_bytes(url: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let mut r = agent().get(url).call().map_err(|e| e.to_string())?;
    if r.status().as_u16() == 404 {
        return Ok(None);
    }
    if !r.status().is_success() {
        return Err(format!("download failed ({})", r.status()));
    }
    r.body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map(Some)
        .map_err(|e| e.to_string())
}

/// URL-safe base64 without padding (the encoding of AcoustID fingerprints).
pub fn base64_url(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..c.len() + 1 {
            s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    s
}

/// A file name without path separators or characters Windows rejects.
pub fn safe_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() {
        "untitled".into()
    } else {
        t.chars().take(120).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64() {
        assert_eq!(base64_url(b""), "");
        assert_eq!(base64_url(b"f"), "Zg");
        assert_eq!(base64_url(b"fo"), "Zm8");
        assert_eq!(base64_url(b"foo"), "Zm9v");
        assert_eq!(base64_url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn names() {
        assert_eq!(safe_name("AC/DC: Live?"), "AC_DC_ Live_");
        assert_eq!(safe_name(" .. "), "untitled");
    }

    #[test]
    fn throttle_spacing() {
        let t = Instant::now();
        mb_throttle(Duration::from_millis(50));
        mb_throttle(Duration::from_millis(50));
        assert!(t.elapsed() >= Duration::from_millis(50));
    }
}
