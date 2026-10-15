//! Remote Control helper of the `remote-control` plugin.
//!
//! `vivi-remote --ipc <mpv socket> --port 8765 --pin 123456`
//!
//! Serves the phone page, refreshes the snapshot from mpv twice a second and applies the
//! page's commands. The URL is shown as a Vivi toast and in `user-data/remote/url`.
//! Exits when mpv closes the IPC connection.

use std::collections::HashMap;
use std::io;
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vivi_helpers::remote::{lan_ip, serve, Cmd, Shared, Snapshot, MAX_QUEUE};
use vivi_helpers::{args, Mpv};

fn snapshot(m: &mut Mpv) -> io::Result<Snapshot> {
    let n = (m.get_f64("playlist-count")? as usize).min(MAX_QUEUE);
    let mut queue = Vec::with_capacity(n);
    for i in 0..n {
        let mut t = m.get_str(&format!("playlist/{i}/title"))?;
        if t.is_empty() {
            let f = m.get_str(&format!("playlist/{i}/filename"))?;
            t = f.rsplit(['/', '\\']).next().unwrap_or(&f).to_string();
        }
        queue.push(t);
    }
    let mut title = m.get_str("metadata/by-key/title")?;
    if title.is_empty() {
        title = m.get_str("media-title")?;
    }
    Ok(Snapshot {
        title,
        artist: m.get_str("metadata/by-key/artist")?,
        path: m.get_str("path")?,
        paused: m.get_bool("pause")?,
        position: m.get_f64("time-pos")?,
        duration: m.get_f64("duration")?,
        volume: m.get_f64("volume")?,
        volume_max: m.get_f64("volume-max")?,
        queue,
        current: m.get_f64("playlist-pos")? as i64,
        has_art: false,
    })
}

/// Cover for a local file: the embedded picture (mpv decodes it as an album-art video
/// track, saved with `screenshot-to-file … video`) or cover/folder/front next to it.
fn art(m: &mut Mpv, path: &str) -> io::Result<Option<(Vec<u8>, &'static str)>> {
    if path.is_empty() || path.contains("://") {
        return Ok(None);
    }
    if m.get_bool("current-tracks/video/albumart")? {
        let tmp = std::env::temp_dir().join(format!("vivi-remote-art-{}.jpg", std::process::id()));
        let t = tmp.to_string_lossy().into_owned();
        if m.cmd(&["screenshot-to-file", &t, "video"]).is_ok() {
            if let Ok(b) = std::fs::read(&tmp) {
                let _ = std::fs::remove_file(&tmp);
                return Ok(Some((b, "image/jpeg")));
            }
        }
    }
    let Some(dir) = Path::new(path).parent() else {
        return Ok(None);
    };
    for n in ["cover", "folder", "front", "Cover", "Folder", "Front"] {
        for (ext, mime) in [
            ("jpg", "image/jpeg"),
            ("jpeg", "image/jpeg"),
            ("png", "image/png"),
        ] {
            if let Ok(b) = std::fs::read(dir.join(format!("{n}.{ext}"))) {
                return Ok(Some((b, mime)));
            }
        }
    }
    Ok(None)
}

fn apply(m: &mut Mpv, c: Cmd) -> io::Result<()> {
    let r = match c {
        Cmd::Pause => m.cmd(&["cycle", "pause"]),
        Cmd::Seek(t) => m.cmd(&["seek", &format!("{t:.2}"), "absolute"]),
        Cmd::SeekRel(t) => m.cmd(&["seek", &format!("{t:.2}"), "relative"]),
        Cmd::Volume(v) => m.cmd(&["set", "volume", &format!("{v:.0}")]),
        Cmd::Next => m.cmd(&["playlist-next"]),
        Cmd::Prev => m.cmd(&["playlist-prev"]),
        Cmd::PlayIndex(i) if i >= 0 => m.cmd(&["playlist-play-index", &i.to_string()]),
        Cmd::PlayIndex(_) => return Ok(()),
    };
    match r {
        Err(e) if e.kind() != io::ErrorKind::Other => Err(e),
        _ => Ok(()),
    }
}

fn run(m: &mut Mpv, sh: &Shared, rx: &Receiver<Cmd>) -> io::Result<()> {
    let mut art_for = None::<String>;
    loop {
        while let Ok(c) = rx.try_recv() {
            apply(m, c)?;
        }
        let mut s = snapshot(m)?;
        if art_for.as_deref() != Some(&s.path) {
            let a = art(m, &s.path)?;
            *sh.art.lock().unwrap_or_else(|e| e.into_inner()) = a;
            art_for = Some(s.path.clone());
        }
        s.has_art = sh.art.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        *sh.snap.lock().unwrap_or_else(|e| e.into_inner()) = s;
        // Commands are applied within 100 ms, the snapshot refreshes every 500 ms.
        for _ in 0..5 {
            if let Ok(c) = rx.recv_timeout(Duration::from_millis(100)) {
                apply(m, c)?;
            }
        }
    }
}

fn main() {
    let a = args();
    let ipc = a.get("ipc").cloned().unwrap_or_default();
    let port: u16 = a
        .get("port")
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(8765);
    let pin = a
        .get("pin")
        .map(|p| p.trim().to_string())
        .unwrap_or_default();
    let mut m = match Mpv::connect(&ipc) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("vivi-remote: mpv IPC {ipc}: {e}");
            std::process::exit(1);
        }
    };
    if pin.is_empty() {
        let _ = m.toast("Remote control: no PIN set");
        return;
    }
    let l = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            let _ = m.toast(&format!("Remote control: port {port}: {e}"));
            let _ = m.set_status("remote/url", &format!("Port {port} unavailable: {e}"));
            std::process::exit(1);
        }
    };
    let sh = Arc::new(Shared {
        snap: Mutex::new(Snapshot::default()),
        art: Mutex::new(None),
        pin: Mutex::new(pin.clone()),
        stop: AtomicBool::new(false),
        fails: Mutex::new(HashMap::new()),
    });
    let (tx, rx) = channel();
    let s2 = sh.clone();
    std::thread::spawn(move || serve(l, s2, tx));
    let host = lan_ip()
        .map(|i| i.to_string())
        .unwrap_or("<this computer>".into());
    let url = format!("http://{host}:{port}");
    eprintln!("vivi-remote: serving {url}");
    let _ = m.set_status("remote/url", &format!("{url} · PIN {pin}"));
    let _ = m.toast(&format!(
        "Remote control: open {url} on your phone, PIN {pin}"
    ));
    let _ = run(&mut m, &sh, &rx);
    sh.stop.store(true, Ordering::Relaxed);
}
