//! The one now-playing card of streams (#86): the overlay card over the player for
//! audio-only stations and the Streams hub's bar, with ★ / lyrics / record / download.
//! It used to live in Network Streams (stations only); every stream gets it now.
//! Packages that describe a URL (host `meta`, e.g. YouTube Music) fill the text and
//! logo themselves; the core still fills what can be done with the stream.

use std::path::PathBuf;

use slint::ComponentHandle;

use crate::player::Player;
use crate::plugins::network_streams::{self as ns, display_name, looks_like_station};
use crate::AppWindow;

use super::store;

#[derive(Default)]
pub struct State {
    /// Stream the card shows.
    url: String,
    /// Whether `url` is a station we know (favorites / recent), checked once per URL.
    known: bool,
    logo_task: crate::util::Task<(String, Option<PathBuf>)>,
    /// URL whose recent entry still waits for mpv's title.
    unnamed: String,
    /// Plugin + command of "Download for offline" (looked up every few seconds).
    download: Option<(String, String)>,
    poll: u32,
}

thread_local! {
    static READY: std::cell::RefCell<Option<(String, slint::Image)>> = const { std::cell::RefCell::new(None) };
}

/// The plugin command that downloads the playing stream ("add-current"), if any.
fn download_command() -> Option<(String, String)> {
    use crate::plugins::{contrib, contributions};
    [
        contrib::CONTEXT_PLAYER,
        contrib::PALETTE_COMMAND,
        contrib::DOWNLOADS_ACTION,
    ]
    .iter()
    .flat_map(|p| contributions(p))
    .find(|c| c.command == "add-current")
    .map(|c| (c.id, c.command))
}

/// Called every hub tick (~250 ms).
pub fn tick(st: &mut State, a: &AppWindow, p: &Player) {
    let np = a.global::<crate::NowPlaying>();
    st.poll = st.poll.wrapping_add(1);
    if st.poll % 8 == 1 {
        st.download = download_command();
        np.set_has_lyrics(crate::plugins::active(crate::plugins::lyrics::ID));
    }
    let has_media = a.global::<crate::Playback>().get_has_media();
    let path = p.get("path").unwrap_or_default();
    let stream = has_media && path.contains("://") && !path.starts_with("file://");
    if !stream {
        if np.get_bar() {
            np.set_bar(false);
        }
        if !crate::plugins::host::has_meta(&path) && np.get_show() {
            np.set_show(false);
        }
        np.set_url("".into());
        st.url.clear();
        return;
    }
    let g = |k: &str| p.get(k).filter(|v| !v.trim().is_empty());
    let live = !g("duration")
        .and_then(|d| d.parse::<f64>().ok())
        .is_some_and(|d| d > 0.0);
    np.set_live(live);
    np.set_can_record(live);
    np.set_recording(g("stream-record").is_some());
    np.set_can_download(!live && st.download.is_some());
    let fresh = path != st.url;
    if fresh {
        st.url = path.clone();
        st.known = store::favorites()
            .iter()
            .chain(store::recent().iter())
            .any(|s| s.url == path && store::kind_of(s) == "radio");
        st.unnamed = path.clone();
        // Every stream played (URL bar, a card, a plugin, the command line) goes on top
        // of the hub's Recent, keeping what was known about it.
        let s = store::favorites()
            .into_iter()
            .chain(store::recent())
            .find(|s| s.url == path)
            .unwrap_or_else(|| store::Stream {
                url: path.clone(),
                kind: if crate::store::RecentKind::infer(&path) == crate::store::RecentKind::Web {
                    "web".into()
                } else {
                    String::new()
                },
                ..Default::default()
            });
        store::push_recent(&s);
        super::refresh_later();
        np.set_url(path.clone().into());
        np.set_fav(store::is_favorite(&path));
    }
    // Recent: name the entry once mpv knows the title.
    if st.unnamed == path {
        if let Some(t) = g("metadata/by-key/icy-name")
            .or_else(|| g("media-title"))
            .filter(|t| *t != path)
        {
            store::name_recent(&path, &t);
            st.unnamed.clear();
        }
    }
    if !np.get_bar() {
        np.set_bar(true);
    }
    if crate::plugins::host::has_meta(&path) {
        return; // a package described this URL: the host fills the text and logo
    }
    let station =
        a.global::<crate::Viz>().get_audio() && looks_like_station(st.known, |k| p.get(k));
    if fresh {
        let known = store::favorites()
            .into_iter()
            .chain(store::recent())
            .find(|s| s.url == path);
        np.set_has_logo(false);
        np.set_music(known.as_ref().is_some_and(|s| store::kind_of(s) == "music"));
        st.logo_task.cancel();
        let name = known
            .as_ref()
            .map(|s| display_name(&s.name))
            .filter(|n| !n.is_empty() && *n != path)
            .unwrap_or_default();
        np.set_station(name.into());
        if let Some(logo) = known.map(|s| s.logo).filter(|l| l.starts_with("http")) {
            let u = path.clone();
            st.logo_task = crate::util::Task::spawn(move || (u, ns::fetch_logo(&logo)));
        }
    }
    if np.get_station().is_empty() || np.get_station() == path.as_str() {
        let n = g("metadata/by-key/icy-name")
            .or_else(|| g("media-title"))
            .unwrap_or_else(|| display_name(path.rsplit('/').next().unwrap_or(&path)));
        np.set_station(n.into());
    }
    let track = g("metadata/by-key/icy-title").unwrap_or_default();
    let mut info = Vec::new();
    if let Some(c) = g("audio-codec-name") {
        info.push(c.to_uppercase());
    }
    if let Some(b) = g("audio-bitrate")
        .and_then(|b| b.parse::<f64>().ok())
        .filter(|b| *b > 0.0)
    {
        info.push(format!("{:.0} kbps", b / 1000.0));
    }
    if let Some(r) = g("audio-params/samplerate").and_then(|r| r.parse::<f64>().ok()) {
        info.push(format!("{:.1} kHz", r / 1000.0));
    }
    np.set_track(track.into());
    np.set_info(info.join(" · ").into());
    if let Some((u, path)) = st.logo_task.poll() {
        if u == st.url {
            if let Some(p) = path {
                let key = u.clone();
                if let Some(i) = crate::images::get(&p, crate::images::TILE, move |i| {
                    READY.with(|r| *r.borrow_mut() = Some((key, i)))
                }) {
                    np.set_logo(i);
                    np.set_has_logo(true);
                }
            }
        }
    }
    if let Some((u, i)) = READY.with(|r| r.borrow_mut().take()) {
        if u == st.url {
            np.set_logo(i);
            np.set_has_logo(true);
        }
    }
    if np.get_show() != station {
        np.set_show(station);
    }
}

/// A button of the card: "fav" | "lyrics" | "record" | "download" | "open".
pub fn action(st: &mut State, a: &AppWindow, what: &str) {
    let np = a.global::<crate::NowPlaying>();
    match what {
        "fav" => {
            let url = np.get_url().to_string();
            if url.is_empty() {
                return;
            }
            let s = store::recent()
                .into_iter()
                .find(|s| s.url == url)
                .unwrap_or_else(|| store::Stream {
                    name: np.get_station().to_string(),
                    url: url.clone(),
                    ..Default::default()
                });
            let on = store::toggle_favorite(&s);
            np.set_fav(on);
            super::refresh_later();
        }
        "lyrics" => {
            let id = crate::plugins::lyrics::ID.to_string();
            super::later(move || crate::plugins::command(&id, "karaoke", &[]));
        }
        "record" => a.global::<crate::Playback>().invoke_toggle_record(),
        "download" => {
            if let Some((id, cmd)) = st.download.clone() {
                super::later(move || {
                    crate::plugins::run(&id, &cmd, &[]);
                });
            }
        }
        "open" => super::open_page(a, "streams"),
        _ => {}
    }
}
