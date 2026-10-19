//! Seek-bar hover preview (#10). The frames come from the Chapter Thumbnails package
//! (#75): it renders `seek-NN.jpg` per file into its cache (any count, see `scenes.rs`)
//! and sends `vivi-preview-frames`; the core only shows them. The first seek-bar hover
//! per file sends it `script-message vivi-preview-wanted` (its default "On hover" mode
//! starts rendering then). Without the package the bar shows the hover time only, and
//! the first hover over the seek bar of a local video offers the package once per
//! session (the hint above the bar, `[interface] hint_chapter_thumbnails` = false hides
//! it for good).

use crate::config::Config;
use crate::AppWindow;
use slint::{ComponentHandle, Image, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::time::Duration;

/// The package that renders seek-bar frames.
pub const PLUGIN: &str = "chapter-thumbnails";
/// How long the hint stays when nothing is clicked.
const HINT_SECS: u64 = 15;

thread_local! {
    /// Local video playing now (previews and the hint apply only to those).
    static CURRENT: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The hint was shown this session.
    static HINTED: Cell<bool> = const { Cell::new(false) };
    /// The package was told about the first hover of the current file.
    static WANTED: Cell<bool> = const { Cell::new(false) };
}

fn plugin_root() -> std::path::PathBuf {
    crate::store::data_dir().join("plugin-data").join(PLUGIN)
}

fn is_local_video(path: &str, duration: f64) -> bool {
    let p = Path::new(path);
    duration > 1.0 && p.is_file() && crate::library::kind_of(p) == Some("video")
}

/// Show the Chapter Thumbnails package's cached frames for `path`; false when it has none.
pub fn load_cached(app: &AppWindow, path: &str) -> bool {
    let Some(files) = crate::scenes::seek_frames(&plugin_root(), path) else {
        return false;
    };
    let imgs: Vec<Image> = files
        .iter()
        .filter_map(|f| Image::load_from_path(f).ok())
        .collect();
    app.global::<crate::Playback>()
        .set_preview_frames(ModelRc::new(VecModel::from(imgs)));
    true
}

/// A file was loaded: drop the old frames and show the package's cache of a local video.
pub fn start(app: &AppWindow, path: &str, duration: f64) {
    app.global::<crate::Playback>()
        .set_preview_frames(ModelRc::new(VecModel::<Image>::default()));
    let local = is_local_video(path, duration);
    CURRENT.with(|c| *c.borrow_mut() = local.then(|| path.to_string()));
    WANTED.with(|w| w.set(false));
    if local {
        load_cached(app, path);
    }
}

/// What the hint offers: `Some("Install")` / `Some("Enable")`, None when the package
/// already runs.
fn hint_action(installed: bool, enabled: bool) -> Option<&'static str> {
    match (installed, enabled) {
        (false, _) => Some("Install"),
        (true, false) => Some("Enable"),
        (true, true) => None,
    }
}

/// The pointer went over the seek bar of the current file. Returns true on the first
/// hover of a local video while the package is enabled: the caller sends it
/// `vivi-preview-wanted`. Otherwise offers Chapter Thumbnails (once per session) when
/// the package is not installed or not enabled and the user has not turned the hint off.
pub fn hovered(app: &AppWindow, c: &Config) -> bool {
    if CURRENT.with(|p| p.borrow().is_none()) {
        return false;
    }
    let m = crate::plugins::manifests()
        .into_iter()
        .find(|m| m.id == PLUGIN);
    let enabled = m.as_ref().is_some_and(|m| crate::plugins::is_enabled(c, m));
    if enabled {
        return !WANTED.with(|w| w.replace(true));
    }
    if HINTED.with(Cell::get) || !c.interface.hint_chapter_thumbnails {
        return false;
    }
    let Some(action) = hint_action(m.is_some(), enabled) else {
        return false;
    };
    HINTED.with(|h| h.set(true));
    let text = if action == "Install" {
        "Frame previews — install Chapter Thumbnails"
    } else {
        "Frame previews — enable Chapter Thumbnails"
    };
    let n = app.global::<crate::Notices>();
    n.set_hint_text(text.into());
    n.set_hint_action(action.into());
    let w = app.as_weak();
    slint::Timer::single_shot(Duration::from_secs(HINT_SECS), move || {
        if let Some(a) = w.upgrade() {
            if a.global::<crate::Notices>().get_hint_text().as_str() == text {
                hide_hint(&a);
            }
        }
    });
    false
}

pub fn hide_hint(app: &AppWindow) {
    app.global::<crate::Notices>().set_hint_text("".into());
}

/// Hint button "Install" / "Enable": the marketplace install flow (Extensions → Browse,
/// permission prompt), and the package is enabled once installed. Nothing installs
/// without the user confirming there.
pub fn hint_accept(app: &AppWindow) {
    let action = app.global::<crate::Notices>().get_hint_action();
    hide_hint(app);
    if action.as_str() == "Enable" {
        let msg = crate::plugins::setting_changed(&crate::plugins::toggle_id(PLUGIN), "true");
        crate::settings::refresh_ui();
        crate::plugins::packages::toast(msg.as_deref().unwrap_or("Enabled Chapter Thumbnails"));
    } else {
        crate::plugins::packages::install_and_enable(app, PLUGIN);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hint_offers_install_or_enable() {
        assert_eq!(super::hint_action(false, false), Some("Install"));
        assert_eq!(super::hint_action(true, false), Some("Enable"));
        assert_eq!(super::hint_action(true, true), None);
    }

    #[test]
    fn only_local_videos() {
        assert!(!super::is_local_video("https://example.com/v.mp4", 100.0));
        assert!(!super::is_local_video("/nonexistent/v.mp4", 100.0));
    }
}
