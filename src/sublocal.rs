//! Load from Computer (#84): the "From Computer" source of the Add Subtitles dialog,
//! in core since 0.15.0 so it works with the Subtitle Search plugin off (online search
//! stays in the plugin).
//!
//! *To This File*: the subtitle files of a folder (the playing file's by default), the one
//! mpv would pick preselected; the checked ones are added (the first selected).
//! *To This Folder*: the folder's videos paired with their subtitles (exact stem first,
//! then `Video.en.srt` / `Video_English.srt`); each pair is remembered for the session and
//! the subtitle is attached when that video loads.

use crate::player::Player;
use crate::{AppWindow, Item};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct State {
    dir: Option<PathBuf>,
    subs: Vec<PathBuf>,
    checked: Vec<bool>,
    pairs: Vec<(PathBuf, Option<PathBuf>)>,
    /// Subtitles assigned with "To This Folder" (canonical video path → subtitle).
    assigned: HashMap<PathBuf, PathBuf>,
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State::default());
}

fn local_path(p: &Player) -> Option<PathBuf> {
    let p = PathBuf::from(p.get("path")?);
    p.is_file().then_some(p)
}

fn status(a: &AppWindow, s: &str) {
    let g = a.global::<crate::Subtitles>();
    g.set_status(s.into());
    g.set_busy(false);
}

/// Wire the `Subtitles` callbacks of the dialog's From Computer half.
pub fn setup(app: &AppWindow, player: &std::rc::Rc<Player>) {
    let g = app.global::<crate::Subtitles>();
    let (w, p) = (app.as_weak(), player.clone());
    g.on_open_local(move || {
        if let Some(a) = w.upgrade() {
            open_dialog(&a, &p);
        }
    });
    let (w, p) = (app.as_weak(), player.clone());
    g.on_local_cmd(move |name, arg| {
        if let Some(a) = w.upgrade() {
            command(&a, &p, &name, &arg);
        }
    });
}

/// Open the dialog on "From Computer" (the subtitle menu, the side panel, `VIVI_SUBS`).
/// With Subtitle Search on, the plugin prepares its online half too.
pub fn open_dialog(a: &AppWindow, p: &Player) {
    a.global::<crate::Subtitles>().set_source(1);
    a.global::<crate::Overlays>().set_subs_open(true);
    if a.global::<crate::Subtitles>().get_plugin() {
        crate::plugins::command(crate::plugins::subtitle_search::ID, "open", &[]);
    } else {
        open(a, p);
    }
}

/// The dialog opened (or a new file started while it is open): scan the playing file's
/// folder.
pub fn open(a: &AppWindow, p: &Player) {
    let dir = local_path(p).and_then(|p| p.parent().map(Path::to_path_buf));
    S.with(|s| s.borrow_mut().dir = dir);
    scan(a, p);
}

/// The dialog's From Computer commands: `mode` (target changed), `browse`, `toggle <i>`,
/// `save-local`.
pub fn command(a: &AppWindow, p: &Player, name: &str, arg: &str) {
    match name {
        "mode" => scan(a, p),
        "browse" => {
            let mut d = rfd::FileDialog::new().set_title("Folder with subtitles");
            if let Some(dir) = S.with(|s| s.borrow().dir.clone()) {
                d = d.set_directory(dir);
            }
            if let Some(dir) = d.pick_folder() {
                S.with(|s| s.borrow_mut().dir = Some(dir));
                scan(a, p);
            }
        }
        "toggle" => {
            let Ok(i) = arg.parse::<usize>() else { return };
            if a.global::<crate::Subtitles>().get_target() == 0 {
                let items = S.with(|s| {
                    let mut s = s.borrow_mut();
                    if let Some(c) = s.checked.get_mut(i) {
                        *c = !*c;
                    }
                    items(&s)
                });
                a.global::<crate::Subtitles>()
                    .set_local(ModelRc::new(VecModel::from(items)));
            }
        }
        "save-local" => save(a, p),
        _ => {}
    }
}

/// Subtitle assigned to `video` with "To This Folder", if any.
pub fn assigned_for(video: &Path) -> Option<PathBuf> {
    let v = std::fs::canonicalize(video).ok()?;
    S.with(|s| s.borrow().assigned.get(&v).cloned())
}

/// A file started: attach its assigned subtitle (the Subtitle Search plugin does this
/// itself, after its remembered pick, when it is on).
pub fn file_loaded(a: &AppWindow, p: &Player) {
    if a.global::<crate::Subtitles>().get_plugin() {
        return;
    }
    if let Some(s) = local_path(p).as_deref().and_then(assigned_for) {
        let c = std::fs::canonicalize(&s).unwrap_or(s.clone());
        let loaded = loaded_subs(p);
        if !loaded.contains(&c) {
            p.cmd("sub-add", &[&s.to_string_lossy(), "select"]);
        }
    }
    if a.global::<crate::Overlays>().get_subs_open()
        && a.global::<crate::Subtitles>().get_source() == 1
    {
        open(a, p);
    }
}

/// External subtitle files mpv has loaded (canonical paths).
pub fn loaded_subs(p: &Player) -> Vec<PathBuf> {
    let n: usize = p
        .get("track-list/count")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    (0..n)
        .filter_map(|i| p.get(&format!("track-list/{i}/external-filename")))
        .filter(|f| !f.is_empty())
        .map(|f| {
            let p = PathBuf::from(f);
            std::fs::canonicalize(&p).unwrap_or(p)
        })
        .collect()
}

fn items(s: &State) -> Vec<Item> {
    s.subs
        .iter()
        .zip(&s.checked)
        .map(|(f, c)| Item {
            key: file_name(f).into(),
            label: "".into(),
            selected: *c,
        })
        .collect()
}

/// Rescan the folder and show it in the dialog.
fn scan(a: &AppWindow, p: &Player) {
    let g = a.global::<crate::Subtitles>();
    let folder = g.get_target() == 1;
    let Some(dir) = S.with(|s| s.borrow().dir.clone()) else {
        g.set_folder("Choose a folder…".into());
        g.set_local(ModelRc::default());
        status(
            a,
            "Open a local file or click the folder bar to choose a folder.",
        );
        return;
    };
    g.set_folder(breadcrumb(&dir).into());
    let video = local_path(p);
    let rows = S.with(|s| {
        let mut s = s.borrow_mut();
        s.subs = files_with(&dir, SUB_EXT);
        if folder {
            s.pairs = pair_folder(&files_with(&dir, VIDEO_EXT), &s.subs);
            s.pairs
                .iter()
                .map(|(v, f)| Item {
                    key: file_name(v).into(),
                    label: f.as_deref().map(file_name).unwrap_or_default().into(),
                    selected: f.is_some(),
                })
                .collect::<Vec<_>>()
        } else {
            let pick = video.as_deref().and_then(|v| preselect(v, &s.subs));
            s.checked = (0..s.subs.len()).map(|i| Some(i) == pick).collect();
            items(&s)
        }
    });
    let empty = rows.is_empty();
    g.set_local(ModelRc::new(VecModel::from(rows)));
    status(
        a,
        match (empty, folder) {
            (true, false) => "No subtitle files in this folder.",
            (true, true) => "No videos in this folder.",
            _ => "",
        },
    );
}

/// The subtitle mpv would pick for `video`: the exact name, else the first match.
pub fn preselect(video: &Path, subs: &[PathBuf]) -> Option<usize> {
    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_lowercase());
    subs.iter()
        .position(|s| stem(s) == stem(video))
        .or_else(|| subs.iter().position(|s| sub_matches(video, s)))
}

fn save(a: &AppWindow, p: &Player) {
    let current = local_path(p).and_then(|p| std::fs::canonicalize(p).ok());
    let msg = if a.global::<crate::Subtitles>().get_target() == 0 {
        let files: Vec<PathBuf> = S.with(|s| {
            let s = s.borrow();
            s.subs
                .iter()
                .zip(&s.checked)
                .filter(|(_, c)| **c)
                .map(|(f, _)| f.clone())
                .collect()
        });
        if files.is_empty() {
            status(a, "Select at least one subtitle file");
            return;
        }
        for (i, f) in files.iter().enumerate() {
            let flag = if i == 0 { "select" } else { "auto" };
            p.cmd("sub-add", &[&f.to_string_lossy(), flag]);
        }
        let n = files.len();
        format!("{n} subtitle file{} added", if n == 1 { "" } else { "s" })
    } else {
        let n = S.with(|s| {
            let mut s = s.borrow_mut();
            let pairs = s.pairs.clone();
            let mut n = 0;
            for (v, f) in pairs {
                let (Some(f), Ok(v)) = (f, std::fs::canonicalize(&v)) else {
                    continue;
                };
                if current.as_ref() == Some(&v) {
                    p.cmd("sub-add", &[&f.to_string_lossy(), "select"]);
                }
                s.assigned.insert(v, f);
                n += 1;
            }
            n
        });
        format!(
            "Subtitles assigned to {n} video{}",
            if n == 1 { "" } else { "s" }
        )
    };
    crate::plugins::toast(a, &msg);
    status(a, "");
    a.global::<crate::Overlays>().set_subs_open(false);
}

// ---------- folders and pairing (pure) ----------

pub const SUB_EXT: &[&str] = &["srt", "ass", "ssa", "vtt", "sub"];
pub const VIDEO_EXT: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "m4v", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "ogv",
];

pub fn has_ext(p: &Path, exts: &[&str]) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Files of `dir` with one of `exts`, sorted by name.
pub fn files_with(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
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
pub fn breadcrumb(dir: &Path) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_pairing() {
        let p = |s: &str| PathBuf::from(s);
        assert!(sub_matches(&p("/a/Ep01.mp4"), &p("/a/Ep01.srt")));
        assert!(sub_matches(&p("/a/Ep01.mp4"), &p("/a/Ep01_English.srt")));
        assert!(sub_matches(&p("/a/ep01.mp4"), &p("/a/EP01.en.srt")));
        assert!(!sub_matches(&p("/a/Ep01.mp4"), &p("/a/Ep010.srt")));
        let pairs = pair_folder(
            &[p("/a/Ep01.mp4"), p("/a/Ep02.mp4")],
            &[p("/a/Ep01_English.srt"), p("/a/Ep01.srt")],
        );
        assert_eq!(pairs[0].1, Some(p("/a/Ep01.srt")));
        assert_eq!(pairs[1].1, None);
        assert_eq!(
            breadcrumb(&p("/home/u/Videos/Shows/The Wand/S01")),
            "Videos / Shows / The Wand / S01"
        );
    }

    #[test]
    fn preselects_the_exact_name_first() {
        let p = PathBuf::from;
        let subs = [p("/a/Ep01.en.srt"), p("/a/Ep01.srt"), p("/a/Ep02.srt")];
        assert_eq!(preselect(&p("/a/Ep01.mkv"), &subs), Some(1));
        assert_eq!(preselect(&p("/a/Ep01.mkv"), &subs[..1]), Some(0));
        assert_eq!(preselect(&p("/a/Ep03.mkv"), &subs), None);
    }

    #[test]
    fn extensions_ignore_case() {
        assert!(has_ext(Path::new("/a/B.SRT"), SUB_EXT));
        assert!(has_ext(Path::new("/a/b.Mkv"), VIDEO_EXT));
        assert!(!has_ext(Path::new("/a/b.txt"), SUB_EXT));
    }
}
