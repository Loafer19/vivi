//! MPRIS2 (`org.mpris.MediaPlayer2`) on the session bus, so media keys, playerctl,
//! Waybar and desktop widgets can control Vivi. Linux only.
//!
//! D-Bus method calls arrive on zbus' thread and are forwarded as [`Cmd`]s over a
//! channel that the UI thread drains; the UI pushes state back with [`Mpris::update`].

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use zbus::blocking::Connection;
use zbus::interface;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const PATH: &str = "/org/mpris/MediaPlayer2";
const TRACK: &str = "/io/github/loafer19/vivi/track";

#[derive(Debug, Clone)]
pub enum Cmd {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    /// Relative seek in seconds.
    Seek(f64),
    /// Absolute position in seconds.
    SetPosition(f64),
    Volume(f64),
    Open(String),
    Raise,
    Quit,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub playing: bool,
    pub has_media: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    /// Track number (0 = unknown).
    pub track: i64,
    pub url: String,
    pub art_url: String,
    pub length: f64,
    pub position: f64,
    pub volume: f64,
    pub rate: f64,
    pub can_next: bool,
    pub can_prev: bool,
}

type Shared = Arc<Mutex<State>>;

struct Root {
    tx: Sender<Cmd>,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        let _ = self.tx.send(Cmd::Raise);
    }
    fn quit(&self) {
        let _ = self.tx.send(Cmd::Quit);
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Vivi"
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "vivi"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        ["file", "http", "https", "rtsp", "rtmp", "udp", "ytdl"]
            .map(String::from)
            .to_vec()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        // Mirrors the MimeType= list in packaging/vivi.desktop (minus URL schemes).
        [
            "video/mp4",
            "video/x-matroska",
            "video/webm",
            "video/x-msvideo",
            "video/quicktime",
            "video/mpeg",
            "video/ogg",
            "video/x-flv",
            "video/3gpp",
            "video/mp2t",
            "audio/mpeg",
            "audio/flac",
            "audio/ogg",
            "audio/x-vorbis+ogg",
            "audio/opus",
            "audio/mp4",
            "audio/aac",
            "audio/x-wav",
            "audio/x-matroska",
            "audio/x-mpegurl",
            "application/x-mpegurl",
            "application/vnd.apple.mpegurl",
            "audio/x-m4a",
            "audio/m4a",
            "audio/mp3",
            "audio/x-mp3",
            "audio/wav",
            "audio/vnd.wave",
            "audio/x-flac",
            "audio/x-aiff",
            "audio/aiff",
            "audio/x-ms-wma",
            "audio/x-ape",
            "audio/x-wavpack",
            "audio/webm",
            "audio/x-aac",
            "audio/vorbis",
            "video/x-m4v",
            "video/x-ms-wmv",
            "video/x-ms-asf",
            "video/3gpp2",
            "video/avi",
            "video/vnd.avi",
            "video/x-ogm+ogg",
            "video/x-theora+ogg",
        ]
        .map(String::from)
        .to_vec()
    }
}

struct Player {
    tx: Sender<Cmd>,
    st: Shared,
}

impl Player {
    fn s(&self) -> State {
        self.st.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

fn us(secs: f64) -> i64 {
    (secs * 1e6) as i64
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play_pause(&self) {
        let _ = self.tx.send(Cmd::PlayPause);
    }
    fn play(&self) {
        let _ = self.tx.send(Cmd::Play);
    }
    fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }
    fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
    fn next(&self) {
        let _ = self.tx.send(Cmd::Next);
    }
    fn previous(&self) {
        let _ = self.tx.send(Cmd::Previous);
    }
    fn seek(&self, offset: i64) {
        let _ = self.tx.send(Cmd::Seek(offset as f64 / 1e6));
    }
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        if track_id.as_str() == TRACK {
            let _ = self.tx.send(Cmd::SetPosition(position as f64 / 1e6));
        }
    }
    fn open_uri(&self, uri: &str) {
        let _ = self.tx.send(Cmd::Open(uri.to_string()));
    }
    /// Emitted after a jump in position (seek), per the MPRIS spec.
    #[zbus(signal)]
    async fn seeked(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        position: i64,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        let s = self.s();
        if !s.has_media {
            "Stopped"
        } else if s.playing {
            "Playing"
        } else {
            "Paused"
        }
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        let r = self.s().rate;
        if r > 0.0 {
            r
        } else {
            1.0
        }
    }
    #[zbus(property)]
    fn set_rate(&self, _r: f64) {}
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        0.25
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        4.0
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let s = self.s();
        let mut m = HashMap::new();
        let mut put = |k: &str, v: Value<'_>| {
            if let Ok(v) = OwnedValue::try_from(v) {
                m.insert(k.to_string(), v);
            }
        };
        put(
            "mpris:trackid",
            Value::from(ObjectPath::from_static_str_unchecked(TRACK)),
        );
        if s.has_media {
            put("mpris:length", Value::from(us(s.length)));
            put("xesam:title", Value::from(s.title.clone()));
            if !s.artist.is_empty() {
                put("xesam:artist", Value::from(vec![s.artist.clone()]));
            }
            if !s.album.is_empty() {
                put("xesam:album", Value::from(s.album.clone()));
            }
            if !s.album_artist.is_empty() {
                put(
                    "xesam:albumArtist",
                    Value::from(vec![s.album_artist.clone()]),
                );
            }
            if s.track > 0 {
                put("xesam:trackNumber", Value::from(s.track as i32));
            }
            if !s.url.is_empty() {
                put("xesam:url", Value::from(s.url.clone()));
            }
            if !s.art_url.is_empty() {
                put("mpris:artUrl", Value::from(s.art_url.clone()));
            }
        }
        m
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.s().volume / 100.0
    }
    #[zbus(property)]
    fn set_volume(&self, v: f64) {
        let _ = self.tx.send(Cmd::Volume((v * 100.0).clamp(0.0, 150.0)));
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        us(self.s().position)
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.s().can_next
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.s().can_prev
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.s().has_media
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.s().has_media
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.s().has_media && self.s().length > 0.0
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

pub struct Mpris {
    conn: Connection,
    st: Shared,
    last: State,
}

impl Mpris {
    /// Register on the session bus. Returns `None` when there is no bus (e.g. CI, X11 without D-Bus).
    pub fn start() -> Option<(Self, Receiver<Cmd>)> {
        let (tx, rx) = channel();
        let st: Shared = Arc::default();
        let name = format!("org.mpris.MediaPlayer2.vivi.instance{}", std::process::id());
        let conn = zbus::blocking::connection::Builder::session()
            .ok()?
            .name(name)
            .ok()?
            .serve_at(PATH, Root { tx: tx.clone() })
            .ok()?
            .serve_at(PATH, Player { tx, st: st.clone() })
            .ok()?
            .build()
            .ok()?;
        Some((
            Self {
                conn,
                st,
                last: State::default(),
            },
            rx,
        ))
    }

    /// Publish new state; emits PropertiesChanged for what changed (not for position).
    pub fn update(&mut self, new: State) {
        if let Ok(mut s) = self.st.lock() {
            *s = new.clone();
        }
        let old = std::mem::replace(&mut self.last, new.clone());
        let Ok(iface) = self.conn.object_server().interface::<_, Player>(PATH) else {
            return;
        };
        let em = iface.signal_emitter();
        let p = iface.get();
        let r = zbus::block_on(async {
            if old.playing != new.playing || old.has_media != new.has_media {
                p.playback_status_changed(em).await?;
                p.can_play_changed(em).await?;
                p.can_pause_changed(em).await?;
                p.can_seek_changed(em).await?;
            }
            if (
                old.title,
                &old.artist,
                &old.album,
                &old.art_url,
                old.length.round() as i64,
            ) != (
                new.title.clone(),
                &new.artist,
                &new.album,
                &new.art_url,
                new.length.round() as i64,
            ) {
                p.metadata_changed(em).await?;
            }
            if (old.volume - new.volume).abs() > 0.5 {
                p.volume_changed(em).await?;
            }
            if old.rate != new.rate {
                p.rate_changed(em).await?;
            }
            if old.can_next != new.can_next || old.can_prev != new.can_prev {
                p.can_go_next_changed(em).await?;
                p.can_go_previous_changed(em).await?;
            }
            zbus::Result::Ok(())
        });
        if let Err(e) = r {
            eprintln!("vivi: mpris: {e}");
        }
    }

    pub fn seeked(&self, position: f64) {
        if let Ok(iface) = self.conn.object_server().interface::<_, Player>(PATH) {
            let _ = zbus::block_on(Player::seeked(iface.signal_emitter(), us(position)));
        }
    }
}
