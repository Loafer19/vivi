//! Transport callbacks on `Playback`: play / pause, seek, volume, mute, prev / next,
//! fullscreen, open file, stop, record, A-B loop, miniplayer and open URL.

use crate::*;
use slint::ComponentHandle;
use std::rc::Rc;

/// Returns the fullscreen toggle and the Open File action, which keys also use.
pub(crate) fn wire(
    app: &AppWindow,
    player: &Rc<Player>,
) -> (impl Fn() + Clone + 'static, impl Fn() + Clone + 'static) {
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_toggle_play(move || p.toggle_pause());
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_seek(move |t| p.seek_abs(t as f64));
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_volume_changed(move |v| p.set_volume(v as f64));
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_toggle_mute(move || p.toggle_mute());
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_prev(move || p.playlist_step(false));
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_next(move || p.playlist_step(true));

    let weak = app.as_weak();
    let toggle_fs = move || {
        if let Some(a) = weak.upgrade() {
            let w = a.window();
            let fs = !w.is_fullscreen();
            w.set_fullscreen(fs);
            a.set_fullscreen(fs);
        }
    };
    let tfs = toggle_fs.clone();
    app.global::<crate::Playback>().on_toggle_fullscreen(tfs);

    let open_file = {
        let p = player.clone();
        let weak = app.as_weak();
        move || {
            // rfd uses the XDG desktop portal (GTK/KDE/Hyprland portals) on Linux.
            let file = rfd::FileDialog::new()
                .set_title("Open media")
                .add_filter(
                    "Media",
                    &[
                        "mkv", "mp4", "webm", "avi", "mov", "m4v", "ts", "mp3", "flac", "ogg",
                        "opus", "m4a", "wav",
                    ],
                )
                .add_filter("All files", &["*"])
                .pick_file();
            if let Some(f) = file {
                p.open(&f.to_string_lossy());
            }
            if let Some(a) = weak.upgrade() {
                a.window().request_redraw();
            }
        }
    };
    let of = open_file.clone();
    app.global::<crate::Playback>().on_open_file(of);

    {
        let p = player.clone();
        app.global::<crate::Playback>().on_stop(move || {
            p.save_position();
            p.run("stop");
        });
        let p = player.clone();
        let weak = app.as_weak();
        app.global::<crate::Playback>().on_toggle_record(move || {
            // mpv stream-record: dump the stream to a file next to the user's videos.
            let rec = p.get("stream-record").unwrap_or_default();
            let key = keymap::shown(keymap::Action::Record);
            let msg = if !rec.is_empty() {
                p.set("stream-record", "");
                format!("Recording stopped · saved to {rec}")
            } else {
                let dir = store::home().join("Videos");
                let dir = if dir.is_dir() { dir } else { store::home() };
                let mut n = 1;
                let out = loop {
                    let f = dir.join(format!("vivi-recording-{n}.mkv"));
                    if !f.exists() {
                        break f;
                    }
                    n += 1;
                };
                p.set("stream-record", &out.to_string_lossy());
                format!("Recording to {} · press {key} to stop", out.display())
            };
            if let Some(a) = weak.upgrade() {
                plugins::toast(&a, &msg);
            }
        });
        let p = player.clone();
        app.global::<crate::Library>().on_open_folder(move || {
            let Some(d) = rfd::FileDialog::new()
                .set_title("Open folder")
                .pick_folder()
            else {
                return;
            };
            let mut files: Vec<String> = walk_media(&d);
            files.sort();
            for (i, f) in files.iter().enumerate() {
                if i == 0 {
                    p.open(f);
                } else {
                    p.enqueue(f);
                }
            }
        });
    }
    (toggle_fs, open_file)
}
