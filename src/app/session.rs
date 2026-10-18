//! Loading media: file-loaded handling, queue and resume, Open Folder, recent files and the stream error / UI-hidden helpers.

use crate::*;
use slint::ComponentHandle;
use std::rc::Rc;

/// Error toast of a network stream that failed to open (shown without media too),
/// with Retry (re-opens `url`) and Copy. It stays up long enough to use them.
pub(crate) fn show_stream_error(app: &AppWindow, msg: &str, url: &str) {
    let n = app.global::<crate::Notices>();
    n.set_stream_url(url.into());
    n.set_stream_copied(false);
    n.set_stream_error(msg.into());
    let w = app.as_weak();
    let m = msg.to_string();
    slint::Timer::single_shot(Duration::from_secs(12), move || {
        if let Some(a) = w.upgrade() {
            if a.global::<crate::Notices>().get_stream_error().as_str() == m {
                a.global::<crate::Notices>().set_stream_error("".into());
            }
        }
    });
}

/// Ctrl+\\ (and the menu's Hide UI): hide every control; a toast says how to get them back.
pub(crate) fn toggle_ui_hidden(a: &AppWindow) {
    let hide = !a.get_ui_hidden();
    a.set_ui_hidden(hide);
    if hide {
        let key = keymap::shown(keymap::Action::HideUi);
        plugins::toast(
            a,
            &format!("Interface hidden · press {key} to show it again"),
        );
    }
}

pub(crate) fn recent_items() -> slint::ModelRc<Item> {
    items(
        store::recent_entries()
            .into_iter()
            .take(8)
            .map(|e| (e.target.clone(), e.display_title(), false)),
    )
}

/// Thumbnail of a web video for Recent, fetched in the background: YouTube's own still
/// for YouTube ids, otherwise the `thumbnail` field from yt-dlp. Cached under
/// `web-thumbs/`; the entry then points at the local file and the views re-render.
fn fetch_web_thumb(a: &AppWindow, target: String) {
    let w = a.as_weak();
    std::thread::spawn(move || {
        let dir = store::cache_dir().join("web-thumbs");
        let base = dir.join(crate::plugins::network_streams::hash_name(&target));
        let cached = ["jpg", "png"]
            .iter()
            .map(|e| base.with_extension(e))
            .find(|p| p.is_file());
        let path = cached.or_else(|| {
            let url = match store::youtube_id(&target) {
                Some(id) => format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"),
                None => {
                    let out = std::process::Command::new("yt-dlp")
                        .args(["--no-playlist", "--skip-download", "--print", "thumbnail"])
                        .arg(&target)
                        .stderr(std::process::Stdio::null())
                        .output()
                        .ok()?;
                    let u = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    u.starts_with("http").then_some(u)?
                }
            };
            let b = crate::plugins::web::get_bytes(&url, 4 * 1024 * 1024)
                .ok()
                .flatten()?;
            // Slint picks the decoder from the extension; other formats are skipped.
            let ext = match b.as_slice() {
                [0x89, b'P', b'N', b'G', ..] => "png",
                [0xFF, 0xD8, ..] => "jpg",
                _ => return None,
            };
            std::fs::create_dir_all(&dir).ok()?;
            let p = base.with_extension(ext);
            std::fs::write(&p, b).ok()?;
            Some(p)
        });
        let Some(path) = path else { return };
        store::set_recent_image(&target, &path.to_string_lossy());
        let _ = w.upgrade_in_event_loop(|a| {
            a.global::<crate::Library>()
                .invoke_menu("".into(), "".into(), "refresh-recent".into());
        });
    });
}

/// Remember the file in Recent and offer "Start over" when mpv resumed mid-file.
pub(crate) fn on_file_loaded(a: &AppWindow, p: &Rc<Player>, log: bool) {
    if let Some(path) = p.get("path") {
        let abs = std::fs::canonicalize(&path)
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.clone());
        let mut e = store::take_recent_hint(&path)
            .map(|mut h| {
                h.target = abs.clone();
                h
            })
            .unwrap_or_else(|| store::RecentEntry::new(&abs));
        // URLs: mpv's media-title is the page / ICY title when there is one.
        if e.title.is_empty() && store::url_host(&abs).is_some() {
            let t = p.get("media-title").unwrap_or_default();
            if !t.trim().is_empty() && t != store::display_name(&abs) && t != abs {
                e.title = t;
            }
        }
        if matches!(e.kind, store::RecentKind::Web) && e.image.is_empty() {
            fetch_web_thumb(a, e.target.clone());
        }
        store::push_recent_entry(e);
        // Updates the Open Recent list and re-renders the home shelf / panel section.
        a.global::<crate::Library>()
            .invoke_menu("".into(), "".into(), "refresh-recent".into());
    }
    let p = p.clone();
    let w = a.as_weak();
    slint::Timer::single_shot(Duration::from_millis(300), move || {
        let Some(a) = w.upgrade() else { return };
        let t: f64 = p.mpv.get_property("time-pos").unwrap_or(0.0);
        if log {
            eprintln!("loaded, start at {t:.2}");
        }
        if t > 5.0 {
            let s = t as i64;
            let msg = format!("Resumed from {}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60);
            a.global::<crate::Notices>()
                .set_resume_offer(msg.as_str().into());
            a.global::<crate::Notices>().set_resume_text(msg.into());
            let w = a.as_weak();
            slint::Timer::single_shot(Duration::from_secs(8), move || {
                if let Some(a) = w.upgrade() {
                    a.global::<crate::Notices>().set_resume_text("".into());
                }
            });
        } else {
            a.global::<crate::Notices>().set_resume_text("".into());
        }
    });
}

pub(crate) fn queue_and_resume(app: &AppWindow, player: &Rc<Player>, log: bool) {
    app.global::<crate::Library>()
        .set_recent_files(recent_items());
    app.global::<crate::Panel>().set_playlists(playlist_items());

    let p = player.clone();
    app.global::<crate::Panel>()
        .on_queue_play(move |i| p.play_index(i));
    let p = player.clone();
    app.global::<crate::Panel>()
        .on_queue_move(move |i, d| p.move_index(i, d));
    let p = player.clone();
    app.global::<crate::Panel>()
        .on_queue_remove(move |i| p.remove_index(i));
    let p = player.clone();
    app.global::<crate::Panel>()
        .on_queue_clear(move || p.clear_queue());
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_queue_sort(move || p.sort_queue());
    let p = player.clone();
    app.global::<crate::Panel>().on_queue_add(move || {
        if let Some(files) = rfd::FileDialog::new()
            .set_title("Add to queue")
            .add_filter(
                "Media",
                &[
                    "mkv", "mp4", "webm", "avi", "mov", "m4v", "ts", "mp3", "flac", "ogg", "opus",
                    "m4a", "wav",
                ],
            )
            .add_filter("All files", &["*"])
            .pick_files()
        {
            for f in files {
                p.enqueue(&f.to_string_lossy());
            }
        }
    });
    let p = player.clone();
    let weak = app.as_weak();
    app.global::<crate::Panel>().on_playlist_save(move |name| {
        let files: Vec<String> = p
            .queue()
            .into_iter()
            .map(|(f, _)| {
                std::fs::canonicalize(&f)
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or(f)
            })
            .collect();
        if files.is_empty() {
            return;
        }
        match store::save_playlist(&name, &files) {
            Ok(path) if log => eprintln!("saved playlist {}", path.display()),
            Err(e) => eprintln!("vivi: save playlist: {e}"),
            _ => {}
        }
        if let Some(a) = weak.upgrade() {
            a.global::<crate::Panel>().set_playlists(playlist_items());
        }
    });
    let p = player.clone();
    app.global::<crate::Panel>().on_playlist_load(move |path| {
        let mut e = store::RecentEntry::new(&path);
        e.kind = store::RecentKind::Playlist;
        store::push_recent_entry(e);
        p.open(&path)
    });
    let weak = app.as_weak();
    app.global::<crate::Panel>()
        .on_playlist_delete(move |path| {
            store::delete_playlist(std::path::Path::new(path.as_str()));
            if let Some(a) = weak.upgrade() {
                a.global::<crate::Panel>().set_playlists(playlist_items());
            }
        });
    let p = player.clone();
    app.global::<crate::Library>()
        .on_open_recent(move |path| p.open(&path));
    let weak = app.as_weak();
    // The library controller clears and re-renders the views (home shelf, Recent view).
    app.global::<crate::Library>().on_clear_recent(move || {
        if let Some(a) = weak.upgrade() {
            a.global::<crate::Library>()
                .invoke_menu("".into(), "".into(), "clear-recent".into());
        }
    });
    let p = player.clone();
    let weak = app.as_weak();
    app.global::<crate::Notices>().on_start_over(move || {
        p.seek_abs(0.0);
        if let Some(a) = weak.upgrade() {
            a.global::<crate::Notices>().set_resume_text("".into());
        }
    });
    let weak = app.as_weak();
    app.global::<crate::Notices>().on_dismiss_resume(move || {
        if let Some(a) = weak.upgrade() {
            a.global::<crate::Notices>().set_resume_text("".into());
        }
    });
    // Save the resume position periodically so a crash or kill keeps it.
    let p = player.clone();
    let t = Box::leak(Box::new(slint::Timer::default()));
    t.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(5),
        move || {
            if p.get("pause").as_deref() == Some("no") {
                p.save_position();
            }
        },
    );
}

/// Media files under a folder (recursive), for "Open Folder".
pub(crate) fn walk_media(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_media(&p));
        } else if library::kind_of(&p).is_some() {
            out.push(p.to_string_lossy().into_owned());
        }
    }
    out
}
