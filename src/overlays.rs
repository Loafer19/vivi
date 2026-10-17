//! Esc closes the top-most open overlay; only with nothing open does it leave the
//! miniplayer, then fullscreen.
//!
//! `AppWindow.top-overlay` (ui/app.slint) checks the overlays top-most first, in
//! reverse declaration order (later elements draw on top), so Esc always closes what
//! is visibly in front.

use crate::plugins::{self, lyrics, tagger};
use crate::AppWindow;
use slint::ComponentHandle;

/// Generated from `ui/core/types.slint`; `AppWindow.top-overlay` picks the top-most
/// open one with the same conditions as each overlay's `if` in `ui/app.slint`.
pub use crate::Overlay;

/// Close `o` the way its own close button does.
pub fn close(a: &AppWindow, o: Overlay) {
    match o {
        Overlay::None => {}
        Overlay::Menu => {
            a.global::<crate::Menus>().set_menu_kind("".into());
            a.global::<crate::Menus>().set_sub_open("".into());
        }
        Overlay::Shortcuts => a.global::<crate::Overlays>().set_shortcuts_open(false),
        Overlay::Info => a.global::<crate::Overlays>().set_info_open(false),
        Overlay::Onboarding => a.global::<crate::Settings>().invoke_onboarding_done(),
        Overlay::Customize => a.global::<crate::Customize>().invoke_done(false),
        Overlay::TagReview => {
            plugins::command(tagger::ID, "review-close", &[]);
            a.global::<crate::Overlays>().set_tr_open(false);
        }
        Overlay::LyricsSearch => {
            plugins::command(lyrics::ID, "search-close", &[]);
            a.global::<crate::Overlays>().set_ls_open(false);
        }
        Overlay::MusicDialog => {
            a.global::<crate::Overlays>().set_md_open(false);
            plugins::command(
                &a.global::<crate::MusicDialogState>().get_owner(),
                "close",
                &[],
            );
        }
        Overlay::Subtitles => a.global::<crate::Overlays>().set_subs_open(false),
        Overlay::Qr => a.global::<crate::Overlays>().set_qr_open(false),
        Overlay::Settings => a.global::<crate::Overlays>().set_settings_open(false),
        Overlay::NewPlaylist => a.global::<crate::Dialogs>().invoke_new_playlist_close(),
        Overlay::Network => {
            a.global::<crate::Overlays>().set_network_open(false);
            a.global::<crate::Dialogs>().set_net_error("".into());
        }
        Overlay::SidePanel => a.global::<crate::Overlays>().set_panel_open(false),
        Overlay::Library => a.global::<crate::Overlays>().set_library_open(false),
        Overlay::Karaoke => {
            plugins::command(lyrics::ID, "karaoke", &["off".into()]);
            a.global::<crate::Overlays>().set_karaoke_open(false);
        }
    }
}

/// The overlay Esc would close.
pub fn top(a: &AppWindow) -> Option<Overlay> {
    Some(a.get_top_overlay()).filter(|o| *o != Overlay::None)
}

/// Esc: close the top overlay, else leave the miniplayer, else leave fullscreen.
/// Returns false when there was nothing to do.
pub fn escape(a: &AppWindow) -> bool {
    if let Some(o) = top(a) {
        close(a, o);
        a.invoke_focus_main();
        return true;
    }
    if a.get_mini() {
        a.global::<crate::Playback>().invoke_toggle_mini();
        return true;
    }
    if a.window().is_fullscreen() {
        a.window().set_fullscreen(false);
        a.set_fullscreen(false);
        return true;
    }
    false
}
