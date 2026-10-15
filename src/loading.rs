//! Loading overlay for network streams: while mpv opens or buffers a URL (before the
//! first frame or audio, or paused for cache) the player shows a spinner with the
//! stream name instead of a black screen; a failed open shows an error toast.

use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Display names for URLs, registered by whoever opens them (Network Streams).
    static NAMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Remember a friendly name for `url` (shown by the spinner).
pub fn set_name(url: &str, name: &str) {
    NAMES.with(|n| {
        let mut n = n.borrow_mut();
        if n.len() > 200 {
            n.clear();
        }
        n.insert(url.to_string(), name.to_string());
    });
}

/// Name of a URL: the registered name, else `title` unless it is the URL, else the
/// last path segment (percent-decoded).
pub fn name_for(url: &str, title: &str) -> String {
    if let Some(n) = NAMES.with(|n| n.borrow().get(url).cloned()) {
        return n;
    }
    if !title.trim().is_empty() && title != url {
        return title.trim().to_string();
    }
    let tail = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .find(|p| !p.is_empty())
        .unwrap_or(url);
    crate::plugins::streams::display_name(tail)
}

pub fn is_network(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [
        "http://", "https://", "rtmp://", "rtsp://", "rtp://", "udp://", "srt://", "mms://",
        "ytdl://",
    ]
    .iter()
    .any(|s| p.starts_with(s))
}

#[derive(Debug, Default)]
pub struct Loading {
    url: Option<String>,
    started: bool,
    buffering: bool,
}

impl Loading {
    /// A file starts loading; tracked only for network streams.
    pub fn start(&mut self, path: &str) {
        *self = Self::default();
        if is_network(path) {
            self.url = Some(path.to_string());
        }
    }
    /// First frame / audio out (mpv `playback-restart`).
    pub fn restart(&mut self) {
        self.started = true;
    }
    pub fn cache(&mut self, paused_for_cache: bool) {
        self.buffering = paused_for_cache;
    }
    /// File ended; returns the URL when it was a network stream that failed.
    pub fn end(&mut self, error: bool) -> Option<String> {
        let u = self.url.take();
        *self = Self::default();
        u.filter(|_| error)
    }
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }
    pub fn visible(&self) -> bool {
        self.url.is_some() && (!self.started || self.buffering)
    }
    pub fn buffering(&self) -> bool {
        self.started && self.buffering
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states() {
        let mut l = Loading::default();
        l.start("/home/a.mkv");
        assert!(!l.visible());
        l.start("https://x/live.m3u8");
        assert!(l.visible());
        l.restart();
        assert!(!l.visible());
        l.cache(true);
        assert!(l.visible() && l.buffering());
        l.cache(false);
        assert!(!l.visible());
        assert_eq!(l.end(true).as_deref(), Some("https://x/live.m3u8"));
        assert!(!l.visible());
        l.start("http://y");
        assert_eq!(l.end(false), None);
    }

    #[test]
    fn names() {
        assert_eq!(
            name_for("http://h/a/My%20Radio.mp3?x=1", ""),
            "My Radio.mp3"
        );
        assert_eq!(name_for("http://h/s", "Title"), "Title");
        set_name("http://h/s", "Jazz FM");
        assert_eq!(name_for("http://h/s", "Title"), "Jazz FM");
    }
}
