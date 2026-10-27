//! Transitions between music tracks (#88): gapless playback and a soft crossfade.
//!
//! * Gapless: while a music file plays and `audio.gapless` is on, mpv keeps the audio
//!   output open into the next file (`gapless-audio=yes`) and opens it early
//!   (`prefetch-playlist=yes`). Video and streams keep mpv's defaults (`weak`, `no`).
//! * Crossfade (`audio.crossfade_secs`, 0 = off): mpv plays one playlist entry at a
//!   time, so two tracks cannot overlap without a second player. Instead the last half
//!   of the crossfade fades the ending track out and the first half fades the next one
//!   in: a short dip, not an overlap. The ramp is a labelled lavfi `volume` filter
//!   ([`LABEL`]) whose gain is an expression of the frame time (`eval=frame`), so it is
//!   sample-accurate and survives seeks (mpv rebuilds the filter graph on each seek and
//!   file) without a timer touching the audio.
//!
//!   It only runs from music to music (local files, the next queue entry an audio
//!   file), never for video or streams, not while one file repeats or an A-B loop is
//!   set, and not between two tracks of the same album while gapless is on (gapless
//!   albums play through untouched).
//!
//!   mpv builds the next file's filter graph the moment it starts, before Vivi hears of
//!   it, so the ending track's expression already fades in from its own time 0 once the
//!   track is past its fade-in ([`guard`]): the next file starts silent, and Vivi then
//!   swaps in that file's own expression.

use crate::config::Config;
use crate::player::Player;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// mpv filter label of the crossfade ramp.
pub const LABEL: &str = "@vivi-xfade";

/// `gapless-audio` and `prefetch-playlist` while the current file plays.
pub fn gapless_props(on: bool, music: bool) -> (&'static str, &'static str) {
    if on && music {
        ("yes", "yes")
    } else {
        ("weak", "no")
    }
}

/// One fade (out or in), in media seconds: half the crossfade at the playback speed.
/// None when off or when the file is too short for both fades.
pub fn half(secs: i64, speed: f64, duration: f64) -> Option<f64> {
    if secs <= 0 || !duration.is_finite() {
        return None;
    }
    let speed = if speed.is_finite() {
        speed.clamp(0.25, 4.0)
    } else {
        1.0
    };
    let h = secs as f64 / 2.0 * speed;
    (duration >= 2.0 * h + 1.0).then_some(h)
}

/// Whether the next entry continues the current album: same album tag when both are
/// known (ignoring case), else the same folder.
pub fn same_album(cur_path: &str, cur_album: &str, next_path: &str, next_album: &str) -> bool {
    let (a, b) = (cur_album.trim(), next_album.trim());
    if !a.is_empty() && !b.is_empty() {
        return a.to_lowercase() == b.to_lowercase();
    }
    let dir = |p: &str| Path::new(p).parent().map(Path::to_path_buf);
    dir(cur_path).is_some_and(|d| !d.as_os_str().is_empty() && Some(d) == dir(next_path))
}

/// URLs (not `file://`) are streams.
pub fn is_url(path: &str) -> bool {
    path.contains("://") && !path.starts_with("file://")
}

/// The next queue entry, as far as a transition cares.
#[derive(Debug, Clone, PartialEq)]
pub struct Next {
    /// A local audio file.
    pub music: bool,
    pub same_album: bool,
}

/// The current file, as far as a transition cares.
#[derive(Debug, Clone, PartialEq)]
pub struct Now {
    /// Audio only (cover art allowed), a local file.
    pub music: bool,
    pub duration: f64,
    pub speed: f64,
    /// `loop-file` or an A-B loop: the file does not end.
    pub looping: bool,
    pub next: Option<Next>,
}

/// The fade-out at the end of this file as (duration, fade length), if any.
pub fn fade_out(secs: i64, gapless: bool, now: &Now) -> Option<(f64, f64)> {
    let next = now.next.as_ref()?;
    if !now.music || now.looping || !next.music || (gapless && next.same_album) {
        return None;
    }
    half(secs, now.speed, now.duration).map(|h| (now.duration, h))
}

/// Once the track is past its own fade-in, the ending track's ramp also fades in from
/// time 0, which is where the next file starts before Vivi hears of it.
pub fn guard(out: Option<(f64, f64)>, pos: f64) -> Option<f64> {
    out.filter(|(_, h)| pos > h + 0.25).map(|(_, h)| h)
}

/// The mpv `af add` argument for a fade-in of `fade_in` seconds from time 0 and a
/// fade-out over the last `h` seconds before `d`; None when neither applies.
pub fn filter(fade_in: Option<f64>, fade_out: Option<(f64, f64)>) -> Option<String> {
    let fin = fade_in.map(|h| format!("clip(t/{h:.3},0,1)"));
    let fout = fade_out.map(|(d, h)| format!("clip(({d:.3}-t)/{h:.3},0,1)"));
    let e = match (fin, fout) {
        (Some(a), Some(b)) => format!("min({a},{b})"),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => return None,
    };
    Some(format!("{LABEL}:lavfi=[volume='{e}':eval=frame]"))
}

/// Whether the ending file got as far as its fade-out (so the next one fades in).
pub fn reached_fade(out: Option<(f64, f64)>, last_pos: f64) -> Option<f64> {
    out.filter(|(d, h)| last_pos >= d - h - 0.6).map(|(_, h)| h)
}

// ---------------------------------------------------------------------------------

#[derive(Default)]
struct State {
    /// The filter as last applied (`af add` argument), None when removed.
    applied: Option<String>,
    /// This file fades in (fade length), set when the previous file reached its fade.
    fade_in: Option<f64>,
    /// This file's planned fade-out.
    out: Option<(f64, f64)>,
    /// Last known position in this file.
    last_pos: f64,
    /// (next path, its album from the library) for the current next entry.
    next_album: Option<(String, String)>,
    /// The last gapless properties set.
    gapless: Option<(&'static str, &'static str)>,
    /// Between start-file and file-loaded: tracks are not known yet, so the filter is
    /// left as it is.
    loading: bool,
}

type Ctx = (Rc<Player>, Rc<RefCell<Config>>, bool);

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
    static TICK: slint::Timer = slint::Timer::default();
}

fn ctx() -> Option<Ctx> {
    CTX.with(|c| c.borrow().clone())
}

pub fn setup(player: &Rc<Player>, cfg: Rc<RefCell<Config>>, log: bool) {
    CTX.with(|c| *c.borrow_mut() = Some((player.clone(), cfg, log)));
    TICK.with(|t| {
        t.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(250),
            tick,
        )
    });
}

fn num(p: &Player, name: &str) -> Option<f64> {
    p.mpv
        .get_property::<f64>(name)
        .ok()
        .filter(|v| v.is_finite())
}

/// The library's album of `path` (empty when unknown), cached per next entry.
fn library_album(st: &mut State, path: &str) -> String {
    if let Some((p, a)) = &st.next_album {
        if p == path {
            return a.clone();
        }
    }
    let a = crate::library::open_db(&crate::library::db_path())
        .ok()
        .and_then(|c| crate::library::by_path(&c, path))
        .map(|m| m.album)
        .unwrap_or_default();
    st.next_album = Some((path.to_owned(), a.clone()));
    a
}

fn now(p: &Player, st: &mut State, path: &str) -> Now {
    let music = !is_url(path) && crate::viz::audio_only(p);
    let pos: i64 = p.mpv.get_property("playlist-pos").unwrap_or(-1);
    let count: i64 = p.mpv.get_property("playlist-count").unwrap_or(0);
    let wrap = crate::loopsync::looping(p.get("loop-playlist").as_deref());
    let idx = if pos >= 0 && pos + 1 < count {
        Some(pos + 1)
    } else if wrap && count > 1 && pos >= 0 {
        Some(0)
    } else {
        None
    };
    let next = idx
        .and_then(|i| p.get(&format!("playlist/{i}/filename")))
        .map(|n| {
            let local = !is_url(&n);
            let music = local && crate::library::kind_of(Path::new(&n)) == Some("audio");
            let album = if music {
                library_album(st, &n)
            } else {
                String::new()
            };
            let cur_album = p.get("metadata/by-key/album").unwrap_or_default();
            Next {
                music,
                same_album: music && same_album(path, &cur_album, &n, &album),
            }
        });
    Now {
        music,
        duration: num(p, "duration").unwrap_or(f64::NAN),
        speed: num(p, "speed").unwrap_or(1.0),
        looping: crate::loopsync::looping(p.get("loop-file").as_deref())
            || num(p, "ab-loop-a").is_some()
            || num(p, "ab-loop-b").is_some(),
        next,
    }
}

fn set_filter(p: &Player, st: &mut State, want: Option<String>, log: bool) {
    if st.applied == want {
        return;
    }
    match &want {
        Some(f) => p.cmd("af", &["add", f]),
        None => p.cmd("af", &["remove", LABEL]),
    }
    if log {
        eprintln!(
            "vivi: crossfade filter {}",
            want.as_deref().unwrap_or("off")
        );
    }
    st.applied = want;
}

/// Gapless properties for the current file (on file load and setting changes).
fn apply_gapless(p: &Player, c: &Config, st: &mut State, log: bool) {
    let path = p.get("path").unwrap_or_default();
    let music = !path.is_empty() && !is_url(&path) && crate::viz::audio_only(p);
    let g = gapless_props(c.audio.gapless, music);
    if st.gapless != Some(g) {
        p.set("gapless-audio", g.0);
        p.set("prefetch-playlist", g.1);
        if log {
            eprintln!("vivi: gapless-audio={} prefetch-playlist={}", g.0, g.1);
        }
        st.gapless = Some(g);
    }
}

fn tick() {
    let Some((p, cfg, log)) = ctx() else { return };
    let c = cfg.borrow();
    ST.with(|st| {
        let mut st = st.borrow_mut();
        let path = p.get("path").filter(|s| !s.is_empty());
        let secs = c.audio.crossfade_secs;
        if path.is_none() {
            st.loading = false;
        }
        if st.loading {
            return;
        }
        let Some(path) = path.filter(|_| secs > 0) else {
            st.out = None;
            st.fade_in = None;
            set_filter(&p, &mut st, None, log);
            return;
        };
        let pos = num(&p, "time-pos");
        if let Some(t) = pos {
            st.last_pos = t;
        }
        let n = now(&p, &mut st, &path);
        let out = fade_out(secs, c.audio.gapless, &n);
        st.out = out;
        let want = if n.music {
            let fin = st.fade_in.or(guard(out, pos.unwrap_or(0.0)));
            filter(fin, out)
        } else {
            st.fade_in = None;
            None
        };
        set_filter(&p, &mut st, want, log);
    });
}

/// A new file starts: it fades in when the previous one reached its fade-out.
pub fn start_file() {
    ST.with(|st| {
        let mut st = st.borrow_mut();
        st.fade_in = reached_fade(st.out.take(), st.last_pos);
        st.last_pos = 0.0;
        st.loading = true;
    });
}

/// The file is loaded: gapless properties and the crossfade filter for it.
pub fn file_loaded() {
    let Some((p, cfg, log)) = ctx() else { return };
    ST.with(|st| {
        let mut st = st.borrow_mut();
        st.loading = false;
        apply_gapless(&p, &cfg.borrow(), &mut st, log);
    });
    tick();
}

/// Settings changed (the next tick picks up the crossfade length).
pub fn apply(p: &Player, c: &Config) {
    let log = ctx().is_some_and(|c| c.2);
    ST.with(|st| apply_gapless(p, c, &mut st.borrow_mut(), log));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn music(d: f64) -> Now {
        Now {
            music: true,
            duration: d,
            speed: 1.0,
            looping: false,
            next: Some(Next {
                music: true,
                same_album: false,
            }),
        }
    }

    #[test]
    fn gapless_only_for_music() {
        assert_eq!(gapless_props(true, true), ("yes", "yes"));
        assert_eq!(gapless_props(true, false), ("weak", "no"));
        assert_eq!(gapless_props(false, true), ("weak", "no"));
    }

    #[test]
    fn fade_length_follows_speed_and_needs_room() {
        assert_eq!(half(0, 1.0, 200.0), None);
        assert_eq!(half(6, 1.0, 200.0), Some(3.0));
        assert_eq!(half(6, 2.0, 200.0), Some(6.0));
        assert_eq!(half(12, 1.0, 12.5), None, "too short for both fades");
        assert_eq!(half(6, 1.0, f64::NAN), None);
    }

    #[test]
    fn crossfade_only_between_music_tracks() {
        assert_eq!(fade_out(6, true, &music(180.0)), Some((180.0, 3.0)));
        assert_eq!(fade_out(0, true, &music(180.0)), None, "off");
        let mut video = music(180.0);
        video.music = false;
        assert_eq!(fade_out(6, true, &video), None, "never for video");
        let mut to_video = music(180.0);
        to_video.next = Some(Next {
            music: false,
            same_album: false,
        });
        assert_eq!(
            fade_out(6, true, &to_video),
            None,
            "next is a video or stream"
        );
        let mut last = music(180.0);
        last.next = None;
        assert_eq!(fade_out(6, true, &last), None, "end of the queue");
        let mut looping = music(180.0);
        looping.looping = true;
        assert_eq!(fade_out(6, true, &looping), None, "repeat one / A-B loop");
    }

    #[test]
    fn gapless_albums_play_through() {
        let mut n = music(180.0);
        n.next = Some(Next {
            music: true,
            same_album: true,
        });
        assert_eq!(fade_out(6, true, &n), None, "same album, gapless on");
        assert_eq!(fade_out(6, false, &n), Some((180.0, 3.0)), "gapless off");
    }

    #[test]
    fn same_album_by_tag_or_folder() {
        assert!(same_album(
            "/m/A/01.flac",
            "Album A",
            "/m/B/02.flac",
            "album a"
        ));
        assert!(!same_album(
            "/m/A/01.flac",
            "Album A",
            "/m/A/02.flac",
            "Album B"
        ));
        assert!(same_album("/m/A/01.flac", "", "/m/A/02.flac", "Album A"));
        assert!(!same_album("/m/A/01.flac", "", "/m/B/02.flac", ""));
        assert!(
            !same_album("01.flac", "", "02.flac", ""),
            "no folder to compare"
        );
    }

    #[test]
    fn ramp_expression() {
        assert_eq!(filter(None, None), None);
        assert_eq!(
            filter(None, Some((180.0, 3.0))).unwrap(),
            "@vivi-xfade:lavfi=[volume='clip((180.000-t)/3.000,0,1)':eval=frame]"
        );
        assert_eq!(
            filter(Some(3.0), Some((180.0, 3.0))).unwrap(),
            "@vivi-xfade:lavfi=[volume='min(clip(t/3.000,0,1),clip((180.000-t)/3.000,0,1))':eval=frame]"
        );
        // The ending track starts covering the next file's start once past its own fade-in.
        assert_eq!(guard(Some((180.0, 3.0)), 1.0), None);
        assert_eq!(guard(Some((180.0, 3.0)), 10.0), Some(3.0));
        assert_eq!(guard(None, 10.0), None);
    }

    #[test]
    fn next_fades_in_only_after_a_fade_out() {
        assert_eq!(reached_fade(Some((180.0, 3.0)), 178.0), Some(3.0));
        assert_eq!(
            reached_fade(Some((180.0, 3.0)), 60.0),
            None,
            "skipped early"
        );
        assert_eq!(reached_fade(None, 179.9), None);
    }
}
