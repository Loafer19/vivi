//! "Open Network Stream" (several links, optional playlist name) and "New Playlist"
//! (title, thumbnail, files by picker or drag & drop) from the Figma dialog frame.

use crate::player::Player;
use crate::{store, AppWindow};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

const SCHEMES: [&str; 14] = [
    "http", "https", "rtsp", "rtsps", "rtmp", "rtmps", "rtp", "udp", "srt", "mms", "ftp", "ytdl",
    "dvb", "file",
];

/// Split the URL box into links; Err names the first line that is not a stream URL.
pub fn parse_urls(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let l = l.trim();
        if l.is_empty() {
            continue;
        }
        let ok = l.split_once("://").is_some_and(|(s, rest)| {
            SCHEMES.contains(&s.to_ascii_lowercase().as_str())
                && !rest.is_empty()
                && !rest.contains(char::is_whitespace)
        });
        if !ok {
            return Err(format!(
                "Line {}: “{l}” is not a stream URL (http, https, rtsp, rtmp, udp, srt…).",
                i + 1
            ));
        }
        out.push(l.to_string());
    }
    if out.is_empty() {
        return Err("Enter at least one link.".into());
    }
    Ok(out)
}

/// Recent network streams for the dialog's history list.
/// The URL history it shares with the Streams hub's URL bar.
fn history() -> slint::ModelRc<slint::SharedString> {
    let v: Vec<slint::SharedString> = crate::streams::store::urls()
        .into_iter()
        .take(5)
        .map(Into::into)
        .collect();
    slint::ModelRc::new(slint::VecModel::from(v))
}

#[derive(Default)]
struct NewPlaylist {
    files: Vec<String>,
    cover: Option<PathBuf>,
}

thread_local! {
    static NP: RefCell<NewPlaylist> = RefCell::default();
}

fn push_np(a: &AppWindow) {
    NP.with(|n| {
        let n = n.borrow();
        let names: Vec<slint::SharedString> = n
            .files
            .iter()
            .map(|f| store::display_name(f).into())
            .collect();
        a.global::<crate::Dialogs>()
            .set_new_playlist_files(slint::ModelRc::new(slint::VecModel::from(names)));
        let weak = a.as_weak();
        match n.cover.as_deref().and_then(|c| {
            crate::images::get(c, crate::images::TILE, move |img| {
                if let Some(a) = weak.upgrade() {
                    a.global::<crate::Dialogs>().set_new_playlist_cover(img);
                    a.global::<crate::Dialogs>()
                        .set_new_playlist_has_cover(true);
                }
            })
        }) {
            Some(img) => {
                a.global::<crate::Dialogs>().set_new_playlist_cover(img);
                a.global::<crate::Dialogs>()
                    .set_new_playlist_has_cover(true);
            }
            None => a
                .global::<crate::Dialogs>()
                .set_new_playlist_has_cover(false),
        }
    });
}

/// A file dropped on the window: true when the New Playlist dialog took it.
pub fn take_drop(a: &AppWindow, path: &std::path::Path) -> bool {
    if !a.global::<crate::Overlays>().get_new_playlist_open() {
        return false;
    }
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    NP.with(|n| {
        n.borrow_mut()
            .files
            .push(abs.to_string_lossy().into_owned())
    });
    push_np(a);
    true
}

pub fn setup(app: &AppWindow, player: &Rc<Player>) {
    app.global::<crate::Dialogs>().set_net_history(history());
    {
        let w = app.as_weak();
        let p = player.clone();
        app.global::<crate::Dialogs>()
            .on_network_submit(move |text, name| {
                let Some(a) = w.upgrade() else { return };
                let urls = match parse_urls(&text) {
                    Ok(u) => u,
                    Err(e) => {
                        a.global::<crate::Dialogs>().set_net_error(e.into());
                        return;
                    }
                };
                for u in urls.iter().rev() {
                    crate::streams::store::push_url(u);
                }
                let name = name.trim().to_string();
                if !name.is_empty() {
                    match store::save_playlist(&name, &urls) {
                        Ok(path) => {
                            a.global::<crate::Panel>()
                                .set_playlists(crate::playlist_items());
                            a.global::<crate::Library>().invoke_refresh();
                            p.open(&path.to_string_lossy());
                        }
                        Err(e) => {
                            a.global::<crate::Dialogs>()
                                .set_net_error(format!("Could not save the playlist: {e}").into());
                            return;
                        }
                    }
                } else {
                    p.open(&urls[0]);
                    for u in &urls[1..] {
                        p.enqueue(u);
                    }
                }
                a.global::<crate::Dialogs>().set_net_error("".into());
                a.global::<crate::Dialogs>().set_net_urls("".into());
                a.global::<crate::Dialogs>().set_net_name("".into());
                a.global::<crate::Overlays>().set_network_open(false);
                a.global::<crate::Overlays>().set_library_open(false);
                let w = a.as_weak();
                // Recent is updated once the stream has loaded.
                slint::Timer::single_shot(std::time::Duration::from_secs(3), move || {
                    if let Some(a) = w.upgrade() {
                        a.global::<crate::Dialogs>().set_net_history(history());
                    }
                });
            });
    }
    {
        let w = app.as_weak();
        app.global::<crate::Dialogs>()
            .on_new_playlist_pick_cover(move || {
                let Some(img) = rfd::FileDialog::new()
                    .set_title("Playlist thumbnail")
                    .add_filter("Images", &["jpg", "jpeg", "png", "webp"])
                    .pick_file()
                else {
                    return;
                };
                NP.with(|n| n.borrow_mut().cover = Some(img));
                if let Some(a) = w.upgrade() {
                    push_np(&a);
                }
            });
    }
    {
        let w = app.as_weak();
        app.global::<crate::Dialogs>()
            .on_new_playlist_pick_files(move || {
                let Some(files) = rfd::FileDialog::new()
                    .set_title("Add files to the playlist")
                    .add_filter(
                        "Media",
                        &[
                            "mkv", "mp4", "webm", "avi", "mov", "m4v", "ts", "mp3", "flac", "ogg",
                            "opus", "m4a", "wav",
                        ],
                    )
                    .add_filter("All files", &["*"])
                    .pick_files()
                else {
                    return;
                };
                NP.with(|n| {
                    n.borrow_mut()
                        .files
                        .extend(files.iter().map(|f| f.to_string_lossy().into_owned()))
                });
                if let Some(a) = w.upgrade() {
                    push_np(&a);
                }
            });
    }
    {
        let w = app.as_weak();
        app.global::<crate::Dialogs>()
            .on_new_playlist_remove(move |i| {
                NP.with(|n| {
                    let mut n = n.borrow_mut();
                    if (0..n.files.len() as i32).contains(&i) {
                        n.files.remove(i as usize);
                    }
                });
                if let Some(a) = w.upgrade() {
                    push_np(&a);
                }
            });
    }
    {
        let w = app.as_weak();
        app.global::<crate::Dialogs>()
            .on_new_playlist_close(move || {
                NP.with(|n| *n.borrow_mut() = NewPlaylist::default());
                if let Some(a) = w.upgrade() {
                    a.global::<crate::Overlays>().set_new_playlist_open(false);
                    a.global::<crate::Dialogs>()
                        .set_new_playlist_name("".into());
                    push_np(&a);
                }
            });
    }
    {
        let w = app.as_weak();
        app.global::<crate::Dialogs>()
            .on_new_playlist_save(move |name| {
                let Some(a) = w.upgrade() else { return };
                let name = name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                if store::playlists().iter().any(|(n, _)| *n == name) {
                    a.global::<crate::Dialogs>().set_new_playlist_error(
                        format!("A playlist called “{name}” already exists.").into(),
                    );
                    return;
                }
                let (files, cover) = NP.with(|n| {
                    let n = n.borrow();
                    (n.files.clone(), n.cover.clone())
                });
                match store::save_playlist(&name, &files) {
                    Ok(path) => {
                        if let Some(c) = cover {
                            match store::set_playlist_cover(&path, &c) {
                                Ok(p) => crate::images::forget(&p),
                                Err(e) => eprintln!("vivi: playlist cover: {e}"),
                            }
                        }
                        a.global::<crate::Dialogs>()
                            .set_new_playlist_error("".into());
                        a.global::<crate::Panel>()
                            .set_playlists(crate::playlist_items());
                        a.global::<crate::Library>().invoke_refresh();
                        a.global::<crate::Dialogs>().invoke_new_playlist_close();
                    }
                    Err(e) => a
                        .global::<crate::Dialogs>()
                        .set_new_playlist_error(format!("Could not save: {e}").into()),
                }
            });
    }
}

/// QR code of `text` as an image (black modules on white, 1 px per module; the dialog
/// scales it up with pixelated rendering). `None` when the text is too long for a QR code.
pub fn qr_image(text: &str) -> Option<slint::Image> {
    let (w, dark) = qr_modules(text)?;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w as u32, w as u32);
    for (px, d) in buf.make_mut_slice().iter_mut().zip(dark) {
        let v = if d { 0 } else { 255 };
        *px = slint::Rgba8Pixel {
            r: v,
            g: v,
            b: v,
            a: 255,
        };
    }
    Some(slint::Image::from_rgba8(buf))
}

/// Modules of the QR code (width, row-major dark flags), error correction level M.
pub fn qr_modules(text: &str) -> Option<(usize, Vec<bool>)> {
    if text.is_empty() || text.len() > 1024 {
        return None;
    }
    let c =
        qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).ok()?;
    let dark = c
        .to_colors()
        .into_iter()
        .map(|m| m == qrcode::Color::Dark)
        .collect();
    Some((c.width(), dark))
}

/// `script-message vivi-qr <title> <text> [caption]` from a plugin: show `text` as a QR
/// code (e.g. Remote Control's pairing link) above everything else, Settings included.
pub fn show_qr(a: &AppWindow, title: &str, text: &str, caption: &str) {
    let Some(img) = qr_image(text.trim()) else {
        eprintln!("vivi: vivi-qr text is empty or too long");
        return;
    };
    let title = if title.trim().is_empty() {
        "QR code"
    } else {
        title.trim()
    };
    a.global::<crate::Dialogs>().set_qr_title(title.into());
    a.global::<crate::Dialogs>().set_qr_code(img);
    a.global::<crate::Dialogs>().set_qr_text(text.trim().into());
    a.global::<crate::Dialogs>()
        .set_qr_caption(caption.trim().into());
    a.global::<crate::Overlays>().set_qr_open(true);
}

/// Owner of the player's own confirmations (no plugin has this id): answers go to
/// [`core_answer`] instead of a plugin.
pub const CORE: &str = "vivi:core";

/// A core confirmation waiting for an answer: its key and what to do on "yes".
/// A core question waiting for its answer; the flag is the dialog's checkbox.
type Pending = (String, Box<dyn FnOnce(bool)>);

thread_local! {
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
}

/// Ask the user to confirm a core action (e.g. Settings → Privacy → Clear bookmarks);
/// `yes` runs when they do. Esc / × / Cancel drop it.
pub fn confirm_core(
    a: &AppWindow,
    key: &str,
    title: &str,
    heading: &str,
    text: &str,
    ok: &str,
    yes: impl FnOnce() + 'static,
) {
    pend(key, move |_| yes());
    confirm(a, CORE, key, title, heading, text, ok);
}

/// Like [`confirm_core`] with a checkbox under the text (`check`, e.g. "Keep its data");
/// while it is checked the dialog shows `checked_text` instead of `text`. `yes` gets
/// whether it was checked.
#[allow(clippy::too_many_arguments)]
pub fn confirm_core_check(
    a: &AppWindow,
    key: &str,
    title: &str,
    heading: &str,
    text: &str,
    ok: &str,
    (check, checked_text): (&str, &str),
    yes: impl FnOnce(bool) + 'static,
) {
    pend(key, yes);
    confirm(a, CORE, key, title, heading, text, ok);
    let d = a.global::<crate::Dialogs>();
    d.set_confirm_check(check.into());
    d.set_confirm_checked_text(checked_text.into());
}

fn pend(key: &str, f: impl FnOnce(bool) + 'static) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        p.retain(|(k, _)| k != key);
        p.push((key.to_string(), Box::new(f)));
    });
}

/// The answer to a [`confirm_core`] question: `[key, "yes" | "checked" | "no"]`.
pub fn core_answer(args: &[String]) {
    let (Some(key), Some(answer)) = (args.first(), args.get(1)) else {
        return;
    };
    let f = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let i = p.iter().position(|(k, _)| k == key)?;
        Some(p.remove(i).1)
    });
    if let Some(f) = f.filter(|_| answer == "yes" || answer == "checked") {
        f(answer == "checked");
    }
}

/// Ask the user to confirm something for plugin `owner`; the answer arrives as plugin
/// command `confirm <key> yes|no` (also "no" when the dialog is closed with Esc / ×).
pub fn confirm(
    a: &AppWindow,
    owner: &str,
    key: &str,
    title: &str,
    heading: &str,
    text: &str,
    ok: &str,
) {
    let d = a.global::<crate::Dialogs>();
    d.set_confirm_owner(owner.into());
    d.set_confirm_key(key.into());
    d.set_confirm_title(title.into());
    d.set_confirm_heading(heading.into());
    d.set_confirm_text(text.into());
    d.set_confirm_ok(ok.into());
    d.set_confirm_warning(true);
    d.set_confirm_check("".into());
    d.set_confirm_checked(false);
    d.set_confirm_checked_text("".into());
    a.global::<crate::Overlays>().set_confirm_open(true);
}

#[cfg(test)]
mod tests {
    #[test]
    fn qr_codes() {
        let (w, m) = super::qr_modules("http://192.168.1.5:8765/#pin=123456").unwrap();
        assert_eq!(m.len(), w * w);
        assert!(w >= 21 && (w - 17) % 4 == 0, "QR sizes are 17 + 4·version");
        // Finder pattern: the top-left module is dark, the one inside its ring light.
        assert!(m[0] && !m[w + 1]);
        assert!(super::qr_modules("").is_none());
        assert!(super::qr_modules(&"x".repeat(2000)).is_none());
    }

    use super::parse_urls;

    #[test]
    fn urls() {
        assert_eq!(
            parse_urls(" https://a.b/x.m3u8 \n\nrtsp://cam/1\n").unwrap(),
            vec!["https://a.b/x.m3u8", "rtsp://cam/1"]
        );
        assert!(parse_urls("").is_err());
        assert!(parse_urls("https://ok\nnot a url")
            .unwrap_err()
            .starts_with("Line 2"));
        assert!(parse_urls("javascript://x").is_err());
    }
}
