//! Per-file picks and local video / subtitle folder pairing.

use super::*;

// ---------- per-file picks ----------

pub(crate) fn picks_file() -> PathBuf {
    crate::store::data_dir().join("subtitle-picks.json")
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

// ---------- local folders ----------

// "From Computer" (Load from Computer…) is core since 0.15.0 (#84): src/sublocal.rs.
pub(crate) use crate::sublocal::{breadcrumb, file_name, has_ext, SUB_EXT};
pub use crate::sublocal::{pair_folder, sub_matches};
