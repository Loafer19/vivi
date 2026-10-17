//! Tracks and LRC parsing (line and word timings), local `.lrc` lookup.

use super::*;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: f64,
}

/// Timed words of a line: (start seconds, text).
pub type Words = Vec<(f64, String)>;

/// Parsed LRC: lines sorted by time (seconds) and, for enhanced LRC
/// (`[00:12.00]<00:12.00>Never <00:12.40>gonna`), the timed words of each line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lrc {
    pub lines: Vec<(f64, String)>,
    /// Same length as `lines` (or empty); a word's text keeps its trailing space. An
    /// entry with empty text marks the end of the last word.
    pub words: Vec<Vec<(f64, String)>>,
}

impl Lrc {
    /// Parse LRC text. Supports several time tags per line, enhanced `<mm:ss.xx>` word
    /// tags and the `[offset:±ms]` tag.
    pub fn parse(src: &str) -> Self {
        let mut offset = 0.0;
        let mut rows: Vec<(f64, String, Words)> = Vec::new();
        for raw in src.lines() {
            let mut rest = raw.trim();
            let mut times = Vec::new();
            while let Some(r) = rest.strip_prefix('[') {
                let Some(end) = r.find(']') else { break };
                let tag = &r[..end];
                rest = &r[end + 1..];
                if let Some(o) = tag.strip_prefix("offset:") {
                    offset = o.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
                } else if let Some(t) = tag_time(tag) {
                    times.push(t);
                }
            }
            let Some(&t0) = times.first() else { continue };
            let (text, words) = parse_words(rest, t0);
            for t in times {
                let w = words
                    .iter()
                    .map(|(wt, s)| (wt - t0 + t, s.clone()))
                    .collect();
                rows.push((t, text.clone(), w));
            }
        }
        // A positive offset shows lyrics earlier.
        let sh = |t: f64| (t - offset).max(0.0);
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        let enhanced = rows.iter().any(|r| !r.2.is_empty());
        let mut l = Lrc::default();
        for (t, s, w) in rows {
            l.lines.push((sh(t), s));
            if enhanced {
                l.words
                    .push(w.into_iter().map(|(wt, s)| (sh(wt), s)).collect());
            }
        }
        l
    }
    /// Index of the line being sung at `t`.
    pub fn index_at(&self, t: f64) -> Option<usize> {
        let n = self.lines.partition_point(|l| l.0 <= t);
        n.checked_sub(1)
    }
    pub fn has_words(&self) -> bool {
        self.words.iter().any(|w| !w.is_empty())
    }
    /// Word being sung in line `i` at `t` and how far it is (0..1). Words before it are
    /// done, words after it are still to come. None when the line has no word timing.
    pub fn word_at(&self, i: usize, t: f64) -> Option<(usize, f64)> {
        let w = self.words.get(i).filter(|w| !w.is_empty())?;
        let next_line = self.lines.get(i + 1).map(|l| l.0);
        let k = w.partition_point(|x| x.0 <= t).saturating_sub(1);
        let start = w[k].0;
        let end = w
            .get(k + 1)
            .map(|x| x.0)
            .or(next_line)
            .unwrap_or(start + 1.0)
            .max(start + 0.01);
        let p = ((t - start) / (end - start)).clamp(0.0, 1.0);
        Some((k, p))
    }
    /// The line texts (no timing).
    pub fn texts(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.1.clone()).collect()
    }
}

/// Text of a line body and its timed words ("<00:01.00>Hel<00:01.20>lo <00:01.50>").
pub(crate) fn parse_words(rest: &str, line_t: f64) -> (String, Vec<(f64, String)>) {
    let mut words: Vec<(f64, String)> = Vec::new();
    let mut text = String::new();
    let mut cur: Option<f64> = None;
    let mut buf = String::new();
    let mut s = rest;
    let mut tagged = false;
    let flush = |cur: Option<f64>, buf: &mut String, words: &mut Vec<(f64, String)>| {
        if !buf.trim().is_empty() {
            words.push((cur.unwrap_or(line_t), std::mem::take(buf)));
        } else {
            buf.clear();
        }
    };
    loop {
        let tag = s.find('<').and_then(|i| {
            let j = s[i..].find('>')? + i;
            tag_time(&s[i + 1..j]).map(|t| (i, j, t))
        });
        match tag {
            Some((i, j, t)) => {
                buf.push_str(&s[..i]);
                text.push_str(&s[..i]);
                flush(cur, &mut buf, &mut words);
                cur = Some(t);
                tagged = true;
                s = &s[j + 1..];
            }
            None => {
                buf.push_str(s);
                text.push_str(s);
                flush(cur, &mut buf, &mut words);
                break;
            }
        }
    }
    if !tagged {
        return (rest.trim().to_string(), Vec::new());
    }
    // A trailing time tag without text marks where the last word ends.
    if let Some(t) = cur.filter(|t| words.last().is_some_and(|w| w.0 < *t)) {
        words.push((t, String::new()));
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (text, words)
}

/// "mm:ss.xx" → seconds.
pub(crate) fn tag_time(tag: &str) -> Option<f64> {
    let (m, s) = tag.split_once(':')?;
    let m: f64 = m.trim().parse().ok()?;
    let s: f64 = s.trim().replace(':', ".").parse().ok()?;
    (m >= 0.0 && s >= 0.0).then_some(m * 60.0 + s)
}

/// Whether text has LRC line time tags.
pub fn is_synced(text: &str) -> bool {
    !Lrc::parse(text).lines.is_empty()
}

/// `song.lrc` (or `song.LRC`) next to `song.flac`.
pub fn local_lrc(media: &Path) -> Option<PathBuf> {
    ["lrc", "LRC"]
        .iter()
        .map(|e| media.with_extension(e))
        .find(|p| p.is_file())
}
