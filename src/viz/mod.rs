//! Music visualizer, registered as the built-in `visualizer` plugin ([`VizPlugin`]). Self-contained: `setup` wires it to the window and player
//! and everything else lives here.
//!
//! * [`tap`] decodes the playing file a second time (symphonia, worker thread) and keeps
//!   a few seconds of mono PCM around mpv's `time-pos`.
//! * [`analysis`] turns a window of samples into features (FFT bands, loudness,
//!   onsets/beats, chroma/dominant pitch).
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
    synth: Synth,
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
            .set_viz_audio(cx.app.get_has_media() && audio_only(cx.player));
    }
    fn disable(&mut self, cx: &crate::plugins::Ctx) {
        ENABLED.with(|e| e.set(false));
        cx.app.set_viz_audio(false);
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
    app.set_viz_modes(ModelRc::new(VecModel::from(names)));
    app.set_viz_mode(mode as i32);

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
        synth: Synth::default(),
    }));

    let (w, v) = (app.as_weak(), viz.clone());
    app.on_viz_select(move |i| {
        let Some(a) = w.upgrade() else { return };
        let n = a.get_viz_modes().row_count();
        let i = (i.max(0) as usize).min(n.saturating_sub(1));
        v.borrow_mut().mode = i;
        a.set_viz_mode(i as i32);
        a.set_viz_note("".into());
        if let Some(name) = a.get_viz_modes().row_data(i) {
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

/// A gently changing mix of tones and noise, standing in for PCM that cannot be read
/// (network streams), so the visualizer animates instead of staying flat.
#[derive(Default)]
struct Synth {
    seed: u32,
    phase: f64,
    amp: [f32; 6],
    target: [f32; 6],
    left: f32,
}

impl Synth {
    const FREQ: [f32; 6] = [60.0, 140.0, 330.0, 800.0, 2000.0, 5200.0];
    fn rnd(&mut self) -> f32 {
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }
    fn fill(&mut self, out: &mut [f32], rate: u32, dt: f32) {
        self.left -= dt;
        if self.left <= 0.0 {
            // New "beat": pick fresh band levels every 120–360 ms.
            self.left = 0.12 + self.rnd() * 0.24;
            for i in 0..6 {
                self.target[i] = self.rnd() * (1.0 - i as f32 * 0.12);
            }
        }
        for i in 0..6 {
            self.amp[i] += (self.target[i] - self.amp[i]) * (dt * 8.0).min(1.0);
        }
        let step = 1.0 / rate as f64;
        for o in out.iter_mut() {
            self.phase += step;
            let t = self.phase as f32 * std::f32::consts::TAU;
            let mut v = 0.0;
            for i in 0..6 {
                v += self.amp[i] * (t * Self::FREQ[i]).sin();
            }
            *o = v * 0.12 + (self.rnd() - 0.5) * 0.08;
        }
        if self.phase > 3600.0 {
            self.phase = 0.0;
        }
    }
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
    fn tick(&mut self, a: &AppWindow, p: &Player) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.2);
        self.last = now;
        self.tick = self.tick.wrapping_add(1);

        // Media changes: poll cheaply (every ~250 ms while not drawing).
        if !enabled() {
            if self.tap_open {
                self.tap.open(None);
                self.tap_open = false;
                self.an = None;
            }
            self.path = None;
            return;
        }
        let drawing = self.audio_only && self.mode > 0 && a.get_has_media();
        if drawing || self.tick.is_multiple_of(8) {
            let path = a.get_has_media().then(|| p.get("path")).flatten();
            let tc: i64 = p.mpv.get_property("track-list/count").unwrap_or(0);
            if path != self.path || tc != self.track_count {
                if path != self.path {
                    self.tap_open = false;
                    self.an = None;
                }
                self.path = path;
                self.track_count = tc;
                self.audio_only = self.path.is_some() && audio_only(p);
                a.set_viz_audio(self.audio_only);
            }
        }
        let active = self.audio_only && self.mode > 0 && a.get_has_media();
        if !active {
            if self.tap_open {
                self.tap.open(None);
                self.tap_open = false;
                self.an = None;
            }
            return;
        }
        if !self.tap_open {
            self.tap.open(self.path.clone());
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
        let stream = self.path.as_deref().is_some_and(|p| p.contains("://"));
        let got = if paused {
            false
        } else if stream && !self.tap.ok() {
            // Network streams cannot be decoded a second time: animate from a synthetic
            // signal so the visual still moves (it does not follow the music).
            if self.an.as_ref().map(|a| a.rate()) != Some(48_000) {
                self.an = Some(Analyzer::new(48_000));
            }
            self.samples.resize(Analyzer::window_len(48_000), 0.0);
            self.synth.fill(&mut self.samples, 48_000, dt);
            true
        } else if !self.tap.ok() {
            false
        } else {
            let rate = match &self.an {
                Some(an) => an.rate(),
                None => 48_000,
            };
            self.samples.resize(Analyzer::window_len(rate), 0.0);
            match self.tap.read(t - delay, &mut self.samples) {
                Some(r) if r == rate => true,
                Some(r) => {
                    self.an = Some(Analyzer::new(r));
                    false
                }
                None => false,
            }
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
        if a.get_viz_note().as_str() != note {
            a.set_viz_note(note.into());
        }
        let mut buf = SharedPixelBuffer::<slint::Rgba8Pixel>::new(cw as u32, ch as u32);
        self.canvas.write_rgba(buf.make_mut_slice());
        a.set_viz_frame(slint::Image::from_rgba8(buf));
    }
}
