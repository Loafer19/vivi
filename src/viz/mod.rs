//! Music visualizer, registered as the built-in `visualizer` plugin ([`VizPlugin`]). Self-contained: `setup` wires it to the window and player
//! and everything else lives here.
//!
//! * [`tap`] measures mpv's actual output (after EQ and the other audio filters) with an
//!   `af` lavfi filter: per-band levels with timestamps, for files and streams alike.
//! * [`Resynth`] turns those band levels back into a short signal (one sine per band),
//!   so [`analysis`] can keep deriving features (FFT bands, loudness, onsets/beats,
//!   chroma/dominant pitch) and the modes stay unchanged.
//! * [`modes`] holds the visual modes behind the [`modes::Visual`] trait; they draw
//!   into a small software [`canvas`] that Slint scales to the window.
//!
//! It only runs while audio-only media is playing and a mode is selected.

mod analysis;
mod canvas;
mod modes;
mod tap;

use crate::player::Player;
use crate::{AppWindow, Theme};
use analysis::Analyzer;
use canvas::{Canvas, Palette, Rgb};
use slint::{ComponentHandle, Model, ModelRc, SharedPixelBuffer, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const FRAME: Duration = Duration::from_millis(33); // ~30 fps
const NOTES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];

struct Viz {
    tap: tap::Tap,
    an: Option<Analyzer>,
    modes: Vec<Box<dyn modes::Visual>>,
    /// 0 = Disabled, i = modes[i - 1].
    mode: usize,
    canvas: Canvas,
    samples: Vec<f32>,
    path: Option<String>,
    track_count: i64,
    audio_only: bool,
    tap_open: bool,
    last: Instant,
    paused_for: f32,
    tick: u32,
    synth: Resynth,
}

thread_local! {
    /// Toggled by the plugin registry (Settings → Extensions).
    static ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// The visualizer as a plugin: enabling/disabling shows or hides it; modes are picked
/// in the player (dropdown or `V`).
pub struct VizPlugin;

impl crate::plugins::Plugin for VizPlugin {
    fn manifest(&self) -> crate::plugins::Manifest {
        crate::plugins::Manifest::builtin(
            "visualizer",
            "Music Visualizer",
            "Spectrum, scope, waveform and more for audio-only files. Pick a mode with the dropdown or V.",
            "visualizer",
        )
    }
    fn enable(&mut self, cx: &crate::plugins::Ctx) {
        ENABLED.with(|e| e.set(true));
        cx.app
            .global::<crate::Viz>()
            .set_audio(cx.app.global::<crate::Playback>().get_has_media() && audio_only(cx.player));
    }
    fn disable(&mut self, cx: &crate::plugins::Ctx) {
        ENABLED.with(|e| e.set(false));
        cx.app.global::<crate::Viz>().set_audio(false);
    }
}

fn enabled() -> bool {
    ENABLED.with(|e| e.get())
}

fn mode_file() -> std::path::PathBuf {
    crate::store::state_dir().join("visualizer")
}

pub fn setup(app: &AppWindow, player: &Rc<Player>) {
    let modes = modes::all();
    let mut names: Vec<SharedString> = vec!["Disabled".into()];
    names.extend(modes.iter().map(|m| SharedString::from(m.name())));
    let saved = std::fs::read_to_string(mode_file()).unwrap_or_default();
    let saved = saved.trim();
    let mode = names.iter().position(|n| n.as_str() == saved).unwrap_or(3); // Spectrum
    app.global::<crate::Viz>()
        .set_modes(ModelRc::new(VecModel::from(names)));
    app.global::<crate::Viz>().set_mode(mode as i32);

    let viz = Rc::new(RefCell::new(Viz {
        tap: tap::Tap::new(),
        an: None,
        modes,
        mode,
        canvas: Canvas::new(1, 1),
        samples: Vec::new(),
        path: None,
        track_count: -1,
        audio_only: false,
        tap_open: false,
        last: Instant::now(),
        paused_for: 0.0,
        tick: 0,
        synth: Resynth::default(),
    }));

    let (w, v) = (app.as_weak(), viz.clone());
    app.global::<crate::Viz>().on_select(move |i| {
        let Some(a) = w.upgrade() else { return };
        let n = a.global::<crate::Viz>().get_modes().row_count();
        let i = (i.max(0) as usize).min(n.saturating_sub(1));
        v.borrow_mut().mode = i;
        a.global::<crate::Viz>().set_mode(i as i32);
        a.global::<crate::Viz>().set_note("".into());
        if let Some(name) = a.global::<crate::Viz>().get_modes().row_data(i) {
            let _ = std::fs::create_dir_all(crate::store::state_dir());
            let _ = std::fs::write(mode_file(), name.as_str());
        }
    });

    let (w, p) = (app.as_weak(), player.clone());
    let timer = Box::leak(Box::new(slint::Timer::default()));
    timer.start(slint::TimerMode::Repeated, FRAME, move || {
        if let Some(a) = w.upgrade() {
            viz.borrow_mut().tick(&a, &p);
        }
    });
}

/// Rebuilds a mono window from the tap's band levels: one sine per band centre with
/// that band's RMS, phases running continuously between frames.
#[derive(Default)]
struct Resynth {
    phase: Vec<f64>,
}

impl Resynth {
    fn fill(&mut self, fr: &tap::Frame, out: &mut [f32], rate: u32) {
        self.phase.resize(tap::BANDS, 0.0);
        out.iter_mut().for_each(|o| *o = 0.0);
        for (i, &lvl) in fr.bands.iter().enumerate() {
            let w = std::f64::consts::TAU * tap::band_freq(i) as f64 / rate as f64;
            let ph = &mut self.phase[i];
            if lvl > 1e-5 {
                let amp = lvl * std::f32::consts::SQRT_2;
                for (k, o) in out.iter_mut().enumerate() {
                    *o += amp * (*ph + w * k as f64).sin() as f32;
                }
            }
            // Advance by one display frame's worth of samples (keeps the scope moving).
            *ph = (*ph + w * (rate as f64 / 30.0)) % std::f64::consts::TAU;
        }
    }
}

/// Puts the tap filter at the end of mpv's `af` chain (after EQ etc.); re-adds it when
/// another filter was appended behind it.
fn ensure_tap(p: &Player, filter: &str) {
    let chain = p.get("af").unwrap_or_default();
    if tap_is_last(&chain) {
        return;
    }
    if chain.contains(&format!("{}:", tap::LABEL)) {
        p.cmd("af", &["remove", tap::LABEL]);
    }
    p.cmd("af", &["add", &format!("{}:{filter}", tap::LABEL)]);
}

/// `af` property string → our filter is present and nothing follows it.
fn tap_is_last(chain: &str) -> bool {
    let Some(pos) = chain.find(&format!("{}:", tap::LABEL)) else {
        return false;
    };
    !chain[pos + 1..].contains('@') && chain.trim_end().ends_with("[vo]anull")
}

/// No video track (cover art does not count as video).
pub(crate) fn audio_only(p: &Player) -> bool {
    let n: i64 = p.mpv.get_property("track-list/count").unwrap_or(0);
    let mut audio = false;
    for i in 0..n {
        match p.get(&format!("track-list/{i}/type")).as_deref() {
            Some("video") => {
                if p.get(&format!("track-list/{i}/albumart")).as_deref() != Some("yes") {
                    return false;
                }
            }
            Some("audio") => audio = true,
            _ => {}
        }
    }
    audio
}

impl Viz {
    fn remove_tap(&mut self, p: &Player) {
        if self.tap_open {
            p.cmd("af", &["remove", tap::LABEL]);
            self.tap.clear();
            self.tap_open = false;
            self.an = None;
        }
    }

    fn tick(&mut self, a: &AppWindow, p: &Player) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.2);
        self.last = now;
        self.tick = self.tick.wrapping_add(1);

        // Media changes: poll cheaply (every ~250 ms while not drawing).
        if !enabled() {
            self.remove_tap(p);
            self.path = None;
            return;
        }
        let drawing =
            self.audio_only && self.mode > 0 && a.global::<crate::Playback>().get_has_media();
        if drawing || self.tick.is_multiple_of(8) {
            let path = a
                .global::<crate::Playback>()
                .get_has_media()
                .then(|| p.get("path"))
                .flatten();
            let tc: i64 = p.mpv.get_property("track-list/count").unwrap_or(0);
            if path != self.path || tc != self.track_count {
                if path != self.path {
                    self.tap.clear();
                    self.an = None;
                }
                self.path = path;
                self.track_count = tc;
                self.audio_only = self.path.is_some() && audio_only(p);
                a.global::<crate::Viz>().set_audio(self.audio_only);
            }
        }
        let active =
            self.audio_only && self.mode > 0 && a.global::<crate::Playback>().get_has_media();
        if !active {
            self.remove_tap(p);
            return;
        }
        // Install the tap, and every ~2 s make sure it is still last in the chain.
        if !self.tap_open || self.tick.is_multiple_of(60) {
            if let Some(f) = self.tap.filter() {
                ensure_tap(p, &f);
            }
            self.tap_open = true;
        }

        let paused = p.mpv.get_property::<bool>("pause").unwrap_or(true);
        self.paused_for = if paused { self.paused_for + dt } else { 0.0 };
        if self.paused_for > 1.5 && self.canvas.w > 1 {
            return; // settled; keep the last frame and save CPU
        }

        // Analysis
        let t: f64 = p.mpv.get_property("time-pos").unwrap_or(0.0);
        let delay: f64 = p.mpv.get_property("audio-delay").unwrap_or(0.0);
        let rate = 48_000;
        let got = if paused {
            false
        } else if let Some(fr) = self.tap.read(t - delay) {
            if self.an.as_ref().map(|a| a.rate()) != Some(rate) {
                self.an = Some(Analyzer::new(rate));
            }
            self.samples.resize(Analyzer::window_len(rate), 0.0);
            self.synth.fill(&fr, &mut self.samples, rate);
            true
        } else {
            false
        };
        let an = self.an.get_or_insert_with(|| Analyzer::new(48_000));
        if got {
            an.process(&self.samples, dt);
        } else {
            an.idle(dt);
        }

        // Canvas sized to the window, downscaled so the longer side stays ≤ ~800 px.
        let size = a.window().size();
        let s = (size.width.max(size.height) as f32 / 800.0).ceil().max(1.0) as u32;
        let (cw, ch) = (
            (size.width / s).max(16) as usize,
            (size.height / s).max(16) as usize,
        );
        let th = a.global::<Theme>();
        let col = |c: slint::Color| {
            Rgb(
                c.red() as f32 / 255.0,
                c.green() as f32 / 255.0,
                c.blue() as f32 / 255.0,
            )
        };
        let pal = Palette {
            accent: col(th.get_accent()),
            bg: col(th.get_bg_b1()),
            dark: th.get_dark(),
        };
        if self.canvas.w != cw || self.canvas.h != ch {
            self.canvas = Canvas::new(cw, ch);
            self.canvas.clear(pal.bg);
        }
        let Some(m) = self.modes.get_mut(self.mode - 1) else {
            return;
        };
        m.draw(&mut self.canvas, &an.f, &pal);
        let note = if m.name() == "Pitch" && an.f.pitch_conf > 0.15 {
            NOTES[an.f.pitch]
        } else {
            ""
        };
        if a.global::<crate::Viz>().get_note().as_str() != note {
            a.global::<crate::Viz>().set_note(note.into());
        }
        let mut buf = SharedPixelBuffer::<slint::Rgba8Pixel>::new(cw as u32, ch as u32);
        self.canvas.write_rgba(buf.make_mut_slice());
        a.global::<crate::Viz>()
            .set_frame(slint::Image::from_rgba8(buf));
    }
}
