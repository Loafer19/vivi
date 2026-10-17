//! The window `key` callback: keymap actions first, then unbound keys go to mpv.

use crate::*;
use slint::ComponentHandle;
use std::rc::Rc;

pub(crate) fn wire(
    app: &AppWindow,
    player: &Rc<Player>,
    toggle_fs: impl Fn() + 'static,
    open_file: impl Fn() + 'static,
) {
    let p = player.clone();
    let weak = app.as_weak();
    app.on_key(move |text, shift, ctrl, alt| {
        let s = text.as_str();
        if key_debug() {
            let cps: Vec<String> = s.chars().map(|c| format!("U+{:04X}", c as u32)).collect();
            eprintln!(
                "vivi: key {s:?} [{}] shift={shift} ctrl={ctrl} alt={alt}",
                cps.join(" ")
            );
        }
        use keymap::Action as A;
        let action = keymap::lookup(s, shift, ctrl);
        // Alt combinations belong to mpv scripts / plugin packages (V is the exception:
        // the visualizer cycles with Alt held too, as it always did).
        if alt && action != Some(A::Visualizer) {
            return forward_key(&p, s, ctrl, alt);
        }
        let Some(action) = action else {
            forward_key(&p, s, ctrl, alt);
            return false;
        };
        let skip =
            |a: &std::sync::atomic::AtomicU32| a.load(std::sync::atomic::Ordering::Relaxed) as f64;
        let step = if shift {
            skip(&settings::LONG_SKIP)
        } else {
            skip(&settings::SHORT_SKIP)
        };
        let vstep = skip(&settings::VOLUME_STEP);
        let Some(a) = weak.upgrade() else {
            return false;
        };
        match action {
            A::PlayPause => p.toggle_pause(),
            A::SeekForward => p.seek_rel(step),
            A::SeekBack => p.seek_rel(-step),
            A::VolumeUp => p.add_volume(vstep),
            A::VolumeDown => p.add_volume(-vstep),
            A::Fullscreen => toggle_fs(),
            A::Miniplayer => {
                if a.global::<crate::Playback>().get_has_media() {
                    a.global::<crate::Playback>().invoke_toggle_mini();
                }
            }
            A::Mute => p.toggle_mute(),
            A::Stop => a.global::<crate::Playback>().invoke_stop(),
            A::QueueStep => p.playlist_step(!shift),
            A::AbLoop => a.global::<crate::Panel>().invoke_run("ab-loop".into()),
            A::Karaoke => crate::plugins::command(plugins::lyrics::ID, "karaoke", &[]),
            A::FindLyrics => crate::plugins::command(plugins::lyrics::ID, "search-open", &[]),
            // [ ] lyrics timing offset (Shift or { }: 0.5 s).
            A::LyricsOffset => {
                let big = shift || s == "{" || s == "}";
                let d = if big { 0.5 } else { 0.1 };
                let d = if s == "[" || s == "{" { -d } else { d };
                crate::plugins::command(plugins::lyrics::ID, "offset", &[d.to_string()]);
            }
            // z / x subtitle offset −0.1 / +0.1 s (Shift: 1 s), saved per file.
            A::SubtitleOffset => {
                let big = shift || s == "Z" || s == "X";
                let d = if big { 1.0 } else { 0.1 };
                let d = if s.eq_ignore_ascii_case("z") { -d } else { d };
                crate::plugins::command(plugins::subtitles::ID, "offset", &[d.to_string()]);
            }
            A::Visualizer => {
                let n = a.global::<crate::Viz>().get_modes().row_count() as i32;
                if !(a.global::<crate::Playback>().get_has_media()
                    && a.global::<crate::Viz>().get_audio())
                    || n == 0
                {
                    forward_key(&p, s, ctrl, alt);
                    return false;
                }
                a.global::<crate::Viz>()
                    .invoke_select((a.global::<crate::Viz>().get_mode() + 1).rem_euclid(n));
            }
            A::Record => a.global::<crate::Playback>().invoke_toggle_record(),
            A::HideUi => toggle_ui_hidden(&a),
            A::Quit => {
                let _ = slint::quit_event_loop();
            }
            A::OpenFile => open_file(),
            A::Settings => {
                a.global::<crate::Overlays>()
                    .set_settings_open(!a.global::<crate::Overlays>().get_settings_open());
                a.global::<crate::Settings>().invoke_settings_refresh(
                    a.global::<crate::Settings>().get_settings_category(),
                    "".into(),
                    false,
                );
            }
            A::Bookmark => a.global::<crate::Panel>().invoke_add_bookmark(),
            A::Shortcuts => a
                .global::<crate::Overlays>()
                .set_shortcuts_open(!a.global::<crate::Overlays>().get_shortcuts_open()),
            A::Escape => {
                overlays::escape(&a);
            }
        }
        true
    });
}
