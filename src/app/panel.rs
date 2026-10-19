//! Side panel: list models (tracks, chapters, playlists), chapter thumbnails and the panel callbacks.

use crate::*;
use slint::{ComponentHandle, Model};
use std::rc::Rc;

pub(crate) fn items(v: impl IntoIterator<Item = (String, String, bool)>) -> slint::ModelRc<Item> {
    let v: Vec<Item> = v
        .into_iter()
        .map(|(key, label, selected)| Item {
            key: key.into(),
            label: label.into(),
            selected,
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(v))
}

/// Chapter thumbnails from the Chapters & Previews package cache (empty images where missing).
pub(crate) fn chapter_thumbs(a: &AppWindow, p: &Player) {
    let n = p.chapters().len();
    let path = p.get("path").unwrap_or_default();
    let files = if path.contains("://") || path.is_empty() {
        vec![None; n]
    } else {
        scenes::thumbs(&scenes::cache_roots(), &path, n)
    };
    let has: Vec<bool> = files.iter().map(Option::is_some).collect();
    // Decoded on the image worker; each thumbnail is filled in when it is ready.
    let weak = a.as_weak();
    let imgs: Vec<slint::Image> = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let weak = weak.clone();
            f.as_deref()
                .and_then(|f| {
                    crate::images::get(f, crate::images::ROW * 2, move |img| {
                        let Some(a) = weak.upgrade() else { return };
                        let m = a.global::<crate::Panel>().get_chapter_thumbs();
                        if i < m.row_count() {
                            m.set_row_data(i, img);
                        }
                    })
                })
                .unwrap_or_default()
        })
        .collect();
    a.global::<crate::Panel>()
        .set_chapter_thumbs(slint::ModelRc::new(slint::VecModel::from(imgs)));
    a.global::<crate::Panel>()
        .set_chapter_has_thumb(slint::ModelRc::new(slint::VecModel::from(has)));
}

/// Re-read tracks, devices and chapters from mpv into the side panel models.
pub(crate) fn refresh_lists(a: &AppWindow, p: &Player, log: bool) {
    let tr = |kind| {
        items(
            p.tracks(kind)
                .into_iter()
                .map(|(id, l, s)| (id.to_string(), l, s)),
        )
    };
    a.global::<crate::Panel>().set_sub_tracks(tr("sub"));
    a.global::<crate::Panel>().set_audio_tracks(tr("audio"));
    let cur_dev = p.get("audio-device").unwrap_or_default();
    a.global::<crate::Panel>().set_audio_devices(items(
        p.audio_devices()
            .into_iter()
            .map(|(n, d)| (n.clone(), d, n == cur_dev)),
    ));
    let cur_ch: i64 = p.mpv.get_property("chapter").unwrap_or(-1);
    let chs = p.chapters();
    a.global::<crate::Panel>()
        .set_chapter_times(slint::ModelRc::new(slint::VecModel::from(
            chs.iter()
                .map(|(_, s)| slint::SharedString::from(hms(*s)))
                .collect::<Vec<_>>(),
        )));
    a.global::<crate::Playback>()
        .set_chapter_starts(slint::ModelRc::new(slint::VecModel::from(
            chs.iter().map(|(_, s)| *s as f32).collect::<Vec<_>>(),
        )));
    a.global::<crate::Panel>().set_chapters(items(
        chs.into_iter()
            .enumerate()
            .map(|(i, (t, _))| (i.to_string(), t, i as i64 == cur_ch)),
    ));
    chapter_thumbs(a, p);
    a.global::<crate::Panel>()
        .set_hwdec(p.get("hwdec").unwrap_or_default().into());
    let q = p.queue();
    a.global::<crate::Panel>().set_queue(items(
        q.iter()
            .enumerate()
            .map(|(i, (f, cur))| (i.to_string(), store::display_name(f), *cur)),
    ));
    mini::queue_meta(a, p, &q);
    if log {
        eprintln!(
            "lists aid={:?} sid={:?} audio={:?} sub={:?} queue={:?}",
            p.get("aid"),
            p.get("sid"),
            p.tracks("audio"),
            p.tracks("sub"),
            p.queue()
        );
    }
}

pub(crate) fn side_panel(app: &AppWindow, player: &Rc<Player>, log: bool) {
    let p = player.clone();
    let weak = app.as_weak();
    app.global::<crate::Panel>()
        .on_set_prop(move |name, value| {
            p.set(&name, &value);
            if name == "sub-delay" {
                crate::plugins::command(
                    plugins::subtitle_search::ID,
                    "offset-changed",
                    &[value.to_string()],
                );
            }
            if log {
                eprintln!("set {name}={value} -> {:?}", p.get(&name));
            }
            if let Some(a) = weak.upgrade() {
                if matches!(
                    name.as_str(),
                    "hwdec" | "chapter" | "audio-device" | "aid" | "sid"
                ) {
                    refresh_lists(&a, &p, log);
                }
            }
        });
    let p = player.clone();
    let weak = app.as_weak();
    {
        let (p, w) = (p.clone(), weak.clone());
        app.global::<crate::Panel>().on_chapters_refresh(move || {
            if let Some(a) = w.upgrade() {
                chapter_thumbs(&a, &p);
            }
        });
    }
    app.global::<crate::Panel>().on_run(move |cmd| {
        p.run(&cmd);
        if let Some(a) = weak.upgrade() {
            abloop::sync(&a, &p);
            if log {
                eprintln!("ab-loop {}", a.global::<crate::Playback>().get_ab_state());
            }
        }
    });
    {
        let (p, w) = (player.clone(), app.as_weak());
        app.global::<crate::Playback>().on_set_ab(move |x, y| {
            let v = |t: f32| {
                if t < 0.0 {
                    "no".to_string()
                } else {
                    format!("{t:.3}")
                }
            };
            p.set("ab-loop-a", &v(x));
            p.set("ab-loop-b", &v(y));
            if let Some(a) = w.upgrade() {
                abloop::sync(&a, &p);
            }
        });
    }
    let p = player.clone();
    app.global::<crate::Panel>().on_load_sub(move || {
        if let Some(f) = rfd::FileDialog::new()
            .set_title("Add subtitles")
            .add_filter("Subtitles", &["srt", "ass", "ssa", "vtt", "sub"])
            .pick_file()
        {
            let path = f.to_string_lossy().replace('"', "\\\"");
            p.run(&format!("sub-add \"{path}\" select"));
        }
    });
    // Sleep timer: pause after N minutes, or at end of file (-1).
    let sleep_timer = Rc::new(slint::Timer::default());
    let p = player.clone();
    let weak = app.as_weak();
    app.global::<crate::Panel>().on_sleep(move |min| {
        let Some(a) = weak.upgrade() else { return };
        sleep_timer.stop();
        p.set("keep-open", "yes");
        match min {
            0 => a.global::<crate::Panel>().set_sleep_state("".into()),
            -1 => {
                a.global::<crate::Panel>()
                    .set_sleep_state("Pausing at end of file".into());
                p.set("keep-open", "always");
            }
            m => {
                a.global::<crate::Panel>()
                    .set_sleep_state(format!("Pausing in {m} min").into());
                let p = p.clone();
                let w = weak.clone();
                sleep_timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_secs(m as u64 * 60),
                    move || {
                        p.set("pause", "yes");
                        if let Some(a) = w.upgrade() {
                            a.global::<crate::Panel>().set_sleep_state("".into());
                        }
                    },
                );
            }
        }
    });
    // In-memory bookmarks (persisted later with settings).
    let marks: Rc<slint::VecModel<Item>> = Rc::new(slint::VecModel::default());
    app.global::<crate::Panel>()
        .set_bookmarks(marks.clone().into());
    {
        let marks = marks.clone();
        app.global::<crate::Panel>().on_remove_bookmark(move || {
            let n = marks.row_count();
            if n > 0 {
                marks.remove(n - 1);
            }
        });
    }
    let p = player.clone();
    app.global::<crate::Panel>().on_add_bookmark(move || {
        let t: f64 = p.mpv.get_property("time-pos").unwrap_or(0.0);
        marks.push(Item {
            key: format!("{t:.3}").into(),
            label: format!("Bookmark {}", marks.row_count() + 1).into(),
            selected: false,
        });
        if log {
            eprintln!("bookmark at {t:.2}");
        }
    });
    // Debug/screenshot helpers.
    if let Some(c) = std::env::var("VIVI_SETTINGS")
        .ok()
        .and_then(|c| c.parse().ok())
    {
        app.global::<crate::Settings>().set_category(c);
        app.global::<crate::Overlays>().set_settings_open(true);
        app.global::<crate::Settings>()
            .invoke_refresh(c, "".into(), false);
    }
    if let Ok(t) = std::env::var("VIVI_PANEL") {
        app.global::<crate::Panel>()
            .set_tab(panel_tab(t.parse().unwrap_or(0)));
        app.global::<crate::Overlays>().set_panel_open(true);
        if let Some(y) = std::env::var("VIVI_PANEL_SCROLL")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            let w = app.as_weak();
            slint::Timer::single_shot(Duration::from_millis(2500), move || {
                if let Some(a) = w.upgrade() {
                    a.global::<crate::Panel>().set_scroll(-y);
                }
            });
        }
    }
    if screenshot_mode() {
        if let Some(v) = std::env::var("VIVI_VOLUME")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
        {
            let p = player.clone();
            slint::Timer::single_shot(Duration::from_millis(2000), move || p.set_volume(v));
        }
        // `VIVI_PLUGIN_CMDS="free-music open|free-music chip Jazz"`: plugin commands,
        // one every 4 s (screenshots of plugin dialogs).
        if let Ok(cmds) = std::env::var("VIVI_PLUGIN_CMDS") {
            app.global::<crate::Overlays>().set_onboarding_open(false);
            for (i, c) in cmds.split('|').enumerate() {
                let parts: Vec<String> = c.split_whitespace().map(String::from).collect();
                if parts.len() < 2 {
                    continue;
                }
                let at = Duration::from_millis(2500 + 4000 * i as u64);
                slint::Timer::single_shot(at, move || {
                    plugins::command(&parts[0], &parts[1], &parts[2..]);
                });
            }
        }
        if let Ok(m) = std::env::var("VIVI_MINI") {
            let weak = app.as_weak();
            slint::Timer::single_shot(Duration::from_millis(2500), move || {
                if let Some(a) = weak.upgrade() {
                    a.global::<crate::Playback>().invoke_toggle_mini();
                    if m == "queue" {
                        a.global::<crate::Playback>().invoke_toggle_mini_queue();
                    }
                }
            });
        }
    }
    // Scripted self-test, debug builds only.
    if cfg!(debug_assertions) && std::env::var_os("VIVI_SELFTEST").is_some() {
        let weak = app.as_weak();
        let t = Box::leak(Box::new(slint::Timer::default()));
        let step = Cell::new(0);
        t.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(700),
            move || {
                let Some(a) = weak.upgrade() else { return };
                let n = step.get();
                step.set(n + 1);
                let ops: &[(&str, &str)] = &[
                    ("aid", "2"),
                    ("sid", "2"),
                    ("sub-delay", "0.5"),
                    ("audio-delay", "-0.25"),
                    ("af", "lavfi=[bass=g=8]"),
                    ("video-aspect-override", "4:3"),
                    ("video-rotate", "90"),
                    ("video-rotate", "0"),
                    ("brightness", "20"),
                    ("speed", "1.5"),
                    ("sid", "1"),
                    ("aid", "1"),
                ];
                if let Some((k, v)) = ops.get(n) {
                    a.global::<crate::Panel>()
                        .invoke_set_prop((*k).into(), (*v).into());
                } else if n == ops.len() {
                    a.global::<crate::Panel>().invoke_add_bookmark();
                    a.global::<crate::Panel>()
                        .invoke_playlist_save("Selftest".into());
                    a.global::<crate::Panel>().invoke_save_preset();
                    a.global::<crate::Panel>().invoke_run("ab-loop".into());
                    a.global::<crate::Panel>().invoke_run("ab-loop".into());
                }
            },
        );
    }
}

pub(crate) use crate::util::hms;

#[cfg(test)]
mod hms_tests {
    #[test]
    fn formats_hours() {
        assert_eq!(super::hms(0.0), "00:00:00");
        assert_eq!(super::hms(3725.9), "01:02:05");
    }
}
