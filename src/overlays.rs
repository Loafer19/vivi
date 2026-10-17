//! Esc closes the top-most open overlay; only with nothing open does it leave the
//! miniplayer, then fullscreen.
//!
//! [`ORDER`] lists every overlay top-most first. It mirrors the declaration order in
//! `ui/app.slint` (later elements draw on top), so Esc always closes what is visibly
//! in front.

use crate::plugins::{self, lyrics, tagger};
use crate::AppWindow;
use slint::ComponentHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Menu,
    Shortcuts,
    Info,
    Onboarding,
    Customize,
    TagReview,
    LyricsSearch,
    MusicDialog,
    Subtitles,
    Qr,
    Settings,
    NewPlaylist,
    Network,
    SidePanel,
    Library,
    Karaoke,
}

/// Top-most first (reverse declaration order in `ui/app.slint`).
pub const ORDER: [Overlay; 16] = [
    Overlay::Menu,
    Overlay::Shortcuts,
    Overlay::Info,
    Overlay::Onboarding,
    Overlay::Customize,
    Overlay::TagReview,
    Overlay::LyricsSearch,
    Overlay::MusicDialog,
    Overlay::Subtitles,
    Overlay::Qr,
    Overlay::Settings,
    Overlay::NewPlaylist,
    Overlay::Network,
    Overlay::SidePanel,
    Overlay::Library,
    Overlay::Karaoke,
];

/// Whether `o` is on screen (same conditions as its `if` in `ui/app.slint`).
pub fn is_open(a: &AppWindow, o: Overlay) -> bool {
    let media = a.get_has_media();
    let mini = a.get_mini();
    match o {
        Overlay::Menu => !a.get_menu_kind().is_empty(),
        Overlay::Shortcuts => a.get_shortcuts_open(),
        Overlay::Info => a.get_info_open(),
        Overlay::Onboarding => a.get_onboarding_open(),
        Overlay::Customize => a.get_customize_open(),
        Overlay::TagReview => a.get_tr_open(),
        Overlay::LyricsSearch => a.get_ls_open(),
        Overlay::MusicDialog => a.get_md_open(),
        Overlay::Subtitles => a.get_subs_open(),
        Overlay::Qr => a.get_qr_open(),
        Overlay::Settings => a.get_settings_open(),
        Overlay::NewPlaylist => a.get_np_open(),
        Overlay::Network => a.get_network_open(),
        Overlay::SidePanel => a.get_panel_open() && media && !mini,
        // Without media the library is the home screen, not an overlay.
        Overlay::Library => a.get_library_open() && media,
        Overlay::Karaoke => a.get_karaoke_open() && media && !mini && !a.get_library_open(),
    }
}

/// Close `o` the way its own close button does.
pub fn close(a: &AppWindow, o: Overlay) {
    match o {
        Overlay::Menu => {
            a.set_menu_kind("".into());
            a.set_sub_open("".into());
        }
        Overlay::Shortcuts => a.set_shortcuts_open(false),
        Overlay::Info => a.set_info_open(false),
        Overlay::Onboarding => a.invoke_onboarding_done(),
        Overlay::Customize => a.invoke_customize_done(false),
        Overlay::TagReview => {
            plugins::command(tagger::ID, "review-close", &[]);
            a.set_tr_open(false);
        }
        Overlay::LyricsSearch => {
            plugins::command(lyrics::ID, "search-close", &[]);
            a.set_ls_open(false);
        }
        Overlay::MusicDialog => {
            a.set_md_open(false);
            plugins::command(&a.get_md_owner(), "close", &[]);
        }
        Overlay::Subtitles => a.set_subs_open(false),
        Overlay::Qr => a.set_qr_open(false),
        Overlay::Settings => a.set_settings_open(false),
        Overlay::NewPlaylist => a.invoke_np_close(),
        Overlay::Network => {
            a.set_network_open(false);
            a.set_net_error("".into());
        }
        Overlay::SidePanel => a.set_panel_open(false),
        Overlay::Library => a.set_library_open(false),
        Overlay::Karaoke => {
            plugins::command(lyrics::ID, "karaoke", &["off".into()]);
            a.set_karaoke_open(false);
        }
    }
}

/// The overlay Esc would close.
pub fn top(a: &AppWindow) -> Option<Overlay> {
    ORDER.into_iter().find(|o| is_open(a, *o))
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
        a.invoke_toggle_mini();
        return true;
    }
    if a.window().is_fullscreen() {
        a.window().set_fullscreen(false);
        a.set_fullscreen(false);
        return true;
    }
    false
}
