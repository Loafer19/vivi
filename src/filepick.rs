//! File dialogs that keep the event loop running.
//!
//! `rfd::FileDialog` blocks the UI thread until the dialog closes: the window stops
//! answering the compositor (Hyprland's "Application Not Responding"), and every event
//! that piled up meanwhile lands at once when it returns. The "+" / Open file actions use
//! these async pickers instead; the result is handled from the event loop as a task.

use std::cell::Cell;
use std::future::Future;
use std::path::PathBuf;

thread_local! {
    /// One picker at a time: a second "+" while one is open does nothing.
    static OPEN: Cell<bool> = const { Cell::new(false) };
}

/// Media types the open / add pickers filter on.
pub const MEDIA: &[&str] = &[
    "mkv", "mp4", "webm", "avi", "mov", "m4v", "ts", "mp3", "flac", "ogg", "opus", "m4a", "wav",
];

/// Show `dialog` without blocking, then call `done` with the picked paths (empty when
/// cancelled). Ignored while another picker from here is open.
pub fn spawn(
    dialog: impl Future<Output = Vec<PathBuf>> + 'static,
    done: impl FnOnce(Vec<PathBuf>) + 'static,
) {
    if OPEN.with(|o| o.replace(true)) {
        return;
    }
    let task = slint::spawn_local(async move {
        let picked = dialog.await;
        OPEN.with(|o| o.set(false));
        done(picked);
    });
    if let Err(e) = task {
        OPEN.with(|o| o.set(false));
        eprintln!("vivi: file dialog: {e}");
    }
}

/// Media files (one, or several with `many`), with Media and All files filters.
pub fn media(title: &str, many: bool, done: impl FnOnce(Vec<PathBuf>) + 'static) {
    let d = rfd::AsyncFileDialog::new()
        .set_title(title)
        .add_filter("Media", MEDIA)
        .add_filter("All files", &["*"]);
    spawn(
        async move {
            let files: Vec<rfd::FileHandle> = if many {
                d.pick_files().await.unwrap_or_default()
            } else {
                d.pick_file().await.into_iter().collect()
            };
            files.into_iter().map(|f| f.path().to_path_buf()).collect()
        },
        done,
    );
}

/// One folder.
pub fn folder(title: &str, done: impl FnOnce(Option<PathBuf>) + 'static) {
    let d = rfd::AsyncFileDialog::new().set_title(title);
    spawn(
        async move {
            d.pick_folder()
                .await
                .map(|f| f.path().to_path_buf())
                .into_iter()
                .collect()
        },
        move |v| done(v.into_iter().next()),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn media_filter_has_the_common_types() {
        for e in ["mkv", "mp4", "mp3", "flac", "opus"] {
            assert!(super::MEDIA.contains(&e));
        }
    }
}
