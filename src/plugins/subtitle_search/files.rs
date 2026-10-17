//! Per-file offsets and picks, and local video / subtitle folder pairing.

use super::*;

// ---------- per-file offsets and picks ----------

pub(crate) fn offsets_file() -> PathBuf {
    crate::store::data_dir().join("subtitle-offsets.json")
}
pub(crate) fn picks_file() -> PathBuf {
    crate::store::data_dir().join("subtitle-picks.json")
}

/// Round to 0.05 s and clamp to ±10 min.
pub fn clamp_offset(o: f64) -> f64 {
    ((o * 20.0).round() / 20.0).clamp(-600.0, 600.0)
}

/// "+0.1 s", "−1.25 s", "0.0 s".
pub fn offset_label(o: f64) -> String {
    if o.abs() < 0.025 {
        return "0.0 s".into();
    }
    let a = o.abs();
    let n = if ((a * 10.0).round() - a * 10.0).abs() < 1e-6 {
        format!("{a:.1}")
    } else {
        format!("{a:.2}")
    };
    format!("{}{n} s", if o > 0.0 { "+" } else { "−" })
}

/// The subtitle chosen for a file (remembered and loaded again).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Pick {
    pub file_id: i64,
    pub path: String,
    #[serde(default)]
    pub language: String,
}

/// Side-panel line for a remembered pick.
pub fn pick_label(p: &Pick, video: Option<&Path>) -> String {
    let next = video
        .and_then(Path::parent)
        .is_some_and(|d| Path::new(&p.path).parent() == Some(d));
    let lang = if p.language.is_empty() {
        String::new()
    } else {
        format!("{} · ", lang_name(&p.language))
    };
    format!(
        "{lang}{}",
        if next {
            "saved next to the video"
        } else {
            "in the cache"
        }
    )
}

pub fn load_picks(file: &Path) -> HashMap<String, Pick> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_pick(file: &Path, key: &str, pick: Option<Pick>) {
    let mut m = load_picks(file);
    match pick {
        Some(p) => m.insert(key.to_string(), p),
        None => m.remove(key),
    };
    if let Ok(s) = serde_json::to_string_pretty(&m) {
        let _ = crate::store::write_atomic(file, &s);
    }
}

pub(crate) const SUB_EXT: &[&str] = &["srt", "ass", "ssa", "vtt", "sub"];
pub(crate) const VIDEO_EXT: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "m4v", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "ogv",
];

pub(crate) fn has_ext(p: &Path, exts: &[&str]) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Files of `dir` with one of `exts`, sorted by name.
pub(crate) fn files_with(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.retain(|p| p.is_file() && has_ext(p, exts));
    v.sort_by_key(|p| file_name(p).to_lowercase());
    v
}

/// Whether subtitle `sub` belongs to `video`: same stem, or the stem followed by a
/// separator (`Movie.en.srt`, `Movie_English.srt`), case-insensitive.
pub fn sub_matches(video: &Path, sub: &Path) -> bool {
    let v = video
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let s = sub
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    !v.is_empty()
        && (s == v
            || s.strip_prefix(&v)
                .is_some_and(|r| r.starts_with(['.', '_', '-', ' ', '['])))
}

/// Pair each video of a folder with its best subtitle (exact stem first).
pub fn pair_folder(videos: &[PathBuf], subs: &[PathBuf]) -> Vec<(PathBuf, Option<PathBuf>)> {
    videos
        .iter()
        .map(|v| {
            let stem = v.file_stem().map(|s| s.to_string_lossy().to_lowercase());
            let exact = subs
                .iter()
                .find(|s| s.file_stem().map(|x| x.to_string_lossy().to_lowercase()) == stem);
            let sub = exact.or_else(|| subs.iter().find(|s| sub_matches(v, s)));
            (v.clone(), sub.cloned())
        })
        .collect()
}

/// "Videos / Shows / The Wand / S01": the last four components of a folder.
pub(crate) fn breadcrumb(dir: &Path) -> String {
    let parts: Vec<String> = dir
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let n = parts.len().saturating_sub(4);
    parts[n..].join(" / ")
}
