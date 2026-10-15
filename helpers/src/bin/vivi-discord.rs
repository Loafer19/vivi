//! Discord Rich Presence helper of the `discord` plugin.
//!
//! `vivi-discord --ipc <mpv socket> --app-id <id> [--hide-names yes] [--image <key>]`
//!
//! Polls mpv every second; sends the activity when the title, pause state or position
//! (seek) changes. Retries Discord every 15 s. Status goes to `user-data/discord/status`.
//! Exits when mpv closes the IPC connection.

use std::io;
use std::time::{Duration, Instant};
use vivi_helpers::discord::{now_ms, Client, Presence};
use vivi_helpers::{args, Mpv};

fn current(m: &mut Mpv, hide: bool, image: &str) -> io::Result<Option<Presence>> {
    if m.get_str("path")?.is_empty() {
        return Ok(None);
    }
    let vid = m.get("current-tracks/video/id")?;
    let art =
        m.get_bool("current-tracks/video/albumart")? || m.get_bool("current-tracks/video/image")?;
    let audio = vid.is_none() || art;
    let mut title = m.get_str("metadata/by-key/title")?;
    if title.is_empty() {
        title = m.get_str("media-title")?;
    }
    let mut artist = m.get_str("metadata/by-key/artist")?;
    if artist.is_empty() {
        artist = m.get_str("metadata/by-key/icy-name")?;
    }
    Ok(Some(Presence::new(
        &title,
        &artist,
        audio,
        m.get_bool("pause")?,
        m.get_f64("time-pos")?,
        m.get_f64("duration")?,
        now_ms(),
        hide,
        image,
    )))
}

fn main() {
    let a = args();
    let ipc = a.get("ipc").cloned().unwrap_or_default();
    let id = a
        .get("app-id")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let hide = a.get("hide-names").is_some_and(|v| v == "yes");
    let image = a.get("image").cloned().unwrap_or_default();
    let mut m = match Mpv::connect(&ipc) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("vivi-discord: mpv IPC {ipc}: {e}");
            std::process::exit(1);
        }
    };
    let status = |m: &mut Mpv, s: &str| {
        eprintln!("vivi-discord: {s}");
        m.set_status("discord/status", s)
    };
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        let _ = m.toast(
            "Enter your Discord application ID in Settings → Extensions → Discord Rich Presence",
        );
        let _ = status(&mut m, "No Discord application ID set");
        return;
    }
    let mut client: Option<Client> = None;
    let mut sent: Option<Option<Presence>> = None;
    let mut next_try = Instant::now();
    // Ends when mpv quits (IPC error).
    while let Ok(cur) = current(&mut m, hide, &image) {
        let same = match (&sent, &cur) {
            (Some(Some(a)), Some(b)) => a.same(b),
            (Some(None), None) => true,
            _ => false,
        };
        if !same {
            if client.is_none() && Instant::now() >= next_try {
                match Client::connect(&id) {
                    Ok(c) => {
                        client = Some(c);
                        if status(&mut m, "Connected to Discord").is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        next_try = Instant::now() + Duration::from_secs(15);
                        if status(&mut m, &format!("Discord not reachable: {e}")).is_err() {
                            break;
                        }
                    }
                }
            }
            if let Some(c) = client.as_mut() {
                match c.set(cur.as_ref()) {
                    Ok(()) => sent = Some(cur),
                    Err(e) => {
                        client = None;
                        next_try = Instant::now() + Duration::from_secs(5);
                        if status(&mut m, &format!("Discord connection lost: {e}")).is_err() {
                            break;
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    if let Some(mut c) = client {
        let _ = c.set(None);
    }
}
