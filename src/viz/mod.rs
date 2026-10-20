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
//! * [`art`] picks the colour of the playing song's cover for *Colors → Album art*.
//!
//! It only runs while audio-only media is playing and a mode is selected.

mod analysis;
mod art;
mod canvas;
mod modes;
mod tap;

use crate::player::Player;
use crate::plugins::{contrib, SettingSpec};
use crate::{AppWindow, Theme};
use analysis::Analyzer;
use canvas::{Canvas, Palette, Rgb};
use slint::{ComponentHandle, Model, ModelRc, SharedPixelBuffer, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const FRAME: Duration = Duration::from_millis(33); // ~30 fps
/// Options of the *Colors* setting.
const COLORS: [&str; 2] = ["Theme accent", "Album art"];
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
    /// Palette of the last frame drawn (a change redraws a paused frame).
    pal: Option<Palette>,
    art: ArtColor,
}

thread_local! {
    /// Toggled by the plugin registry (Settings → Extensions).
    static ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    /// Setting *Colors* is *Album art*.
    static ART_COLORS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The visualizer as a plugin: enabling/disabling shows or hides it; modes are picked
/// in the player (dropdown, `V` / `Shift+V`, the command palette or IPC).
pub struct VizPlugin;

impl crate::plugins::Plugin for VizPlugin {
    fn manifest(&self) -> crate::plugins::Manifest {
        crate::plugins::Manifest::builtin(
            "visualizer",
            "Music Visualizer",
            "Spectrum, scope, waveform, radial, particles and pitch visuals for music and radio (anything without video), drawn from what mpv plays, so the equalizer shows. Pick a mode with the dropdown or V (Shift+V goes back); colors follow the theme accent or the song's album art.",
            "visualizer",
        )
        .contribute(contrib::PALETTE_COMMAND, "Next visualizer mode", "next")
        .contribute(contrib::PALETTE_COMMAND, "Previous visualizer mode", "prev")
        .setting(SettingSpec::choice(
            "colors",
            "Colors",
            "Album art: the visuals take the main color of the playing song's cover, made readable on the theme. Covers without a clear color (black and white, grey) keep the theme accent.",
            &COLORS,
        ))
    }
    fn enable(&mut self, cx: &crate::plugins::Ctx) {
        ENABLED.with(|e| e.set(true));
        ART_COLORS.with(|e| e.set(cx.get("colors") == COLORS[1]));
        cx.app.global::<crate::Viz>().set_enabled(true);
        cx.app
            .global::<crate::Viz>()
            .set_audio(cx.app.global::<crate::Playback>().get_has_media() && audio_only(cx.player));
    }
    fn disable(&mut self, cx: &crate::plugins::Ctx) {
        ENABLED.with(|e| e.set(false));
        cx.app.global::<crate::Viz>().set_enabled(false);
        cx.app.global::<crate::Viz>().set_audio(false);
    }
    fn setting_changed(&mut self, cx: &crate::plugins::Ctx, key: &str) {
        if key == "colors" {
            ART_COLORS.with(|e| e.set(cx.get("colors") == COLORS[1]));
        }
    }
    fn command(&mut self, cx: &crate::plugins::Ctx, name: &str, _args: &[String]) {
        let d = match name {
            "next" => 1,
            "prev" => -1,
            _ => return,
        };
        if !step(cx.app, d) {
            cx.toast("The visualizer shows while music or radio (no video) plays");
        }
    }
}

fn enabled() -> bool {
    ENABLED.with(|e| e.get())
}

/// Next (`d` = 1) or previous (-1) mode, "Disabled" included, wrapping around. False
/// (nothing changed) while the visualizer is off or nothing audio-only is playing.
pub fn step(a: &AppWindow, d: i32) -> bool {
    let v = a.global::<crate::Viz>();
    let n = v.get_modes().row_count() as i32;
    if !(a.global::<crate::Playback>().get_has_media() && v.get_audio()) || n == 0 {
        return false;
    }
    v.invoke_select((v.get_mode() + d).rem_euclid(n));
    true
}

/// Select a mode by name (case-insensitive) or index; the saved choice also applies
/// when nothing is playing. Err while the plugin is off or for an unknown mode.
pub fn select(a: &AppWindow, mode: &str) -> Result<(), String> {
    if !enabled() {
        return Err("the Music Visualizer is turned off (Settings → Extensions)".into());
    }
    let v = a.global::<crate::Viz>();
    let names: Vec<String> = v.get_modes().iter().map(|m| m.to_string()).collect();
    let i = names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(mode.trim()))
        .or_else(|| {
            mode.trim()
                .parse::<usize>()
                .ok()
                .filter(|&i| i < names.len())
        })
        .ok_or_else(|| format!("unknown mode {mode:?}; one of: {}", names.join(", ")))?;
    v.invoke_select(i as i32);
    Ok(())
}

/// Current mode name, all names, and whether the visualizer is on screen now.
pub fn state(a: &AppWindow) -> (String, Vec<String>, bool) {
    let v = a.global::<crate::Viz>();
    let names: Vec<String> = v.get_modes().iter().map(|m| m.to_string()).collect();
    let mode = names
        .get(v.get_mode() as usize)
        .cloned()
        .unwrap_or_default();
    let showing = enabled()
        && v.get_audio()
        && v.get_mode() > 0
        && a.global::<crate::Playback>().get_has_media();
    (mode, names, showing)
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
        pal: None,
        art: ArtColor::default(),
    }));

    let (w, v) = (app.as_weak(), viz.clone());
    app.global::<crate::Viz>().on_select(move |i| {
        let Some(a) = w.upgrade() else { return };
        let n = a.global::<crate::Viz>().get_modes().row_count();
        let i = (i.max(0) as usize).min(n.saturating_sub(1));
        {
            let mut v = v.borrow_mut();
            if v.mode != i {
                // Start the new mode on a clean canvas (no trails of the old one), and
                // draw it even while paused (the paused frame is otherwise kept).
                v.canvas = Canvas::new(1, 1);
                v.paused_for = 0.0;
            }
            v.mode = i;
        }
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

/// The playing song's cover colour for *Colors → Album art*: looked up on a worker
/// thread whenever the shared now-playing cover changes, then eased in.
#[derive(Default)]
struct ArtColor {
    /// Now-playing change counter seen last.
    np_id: i64,
    cover: String,
    rx: Option<Receiver<Option<Rgb>>>,
    /// Dominant colour of `cover` (None: none / not known yet).
    color: Option<Rgb>,
    /// Accent shown now, moving towards the target.
    shown: Option<Rgb>,
}

impl ArtColor {
    /// The accent to draw with: the cover colour when wanted and known, else `theme`.
    fn accent(&mut self, want: bool, path: Option<&str>, theme: Rgb, dark: bool, dt: f32) -> Rgb {
        if want {
            let id = crate::nowplaying::current_id();
            if id != self.np_id {
                self.np_id = id;
                let t = crate::nowplaying::current();
                let cover = if path == Some(t.path.as_str()) {
                    t.cover
                } else {
                    String::new()
                };
                if cover != self.cover {
                    self.cover = cover.clone();
                    self.color = None;
                    self.rx = None;
                    if !cover.is_empty() {
                        let (tx, rx) = channel();
                        self.rx = Some(rx);
                        let _ = std::thread::Builder::new()
                            .name("vivi-viz-art".into())
                            .spawn(move || {
                                let c = crate::plugins::host::fetch_cover(&cover)
                                    .and_then(|p| art::dominant(&p));
                                let _ = tx.send(c);
                            });
                    }
                }
            }
            if let Some(c) = self.rx.as_ref().and_then(|r| r.try_recv().ok()) {
                self.color = c;
                self.rx = None;
            }
        } else {
            self.np_id = -1; // look again when switched back on
        }
        let target = match (want, self.color) {
            (true, Some(c)) => art::usable(c, dark),
            _ => theme,
        };
        // Ease towards the target in ~0.4 s (song changes, switching the setting).
        let now = self
            .shown
            .unwrap_or(target)
            .lerp(target, (dt * 8.0).min(1.0));
        let close = (now.0 - target.0).abs() + (now.1 - target.1).abs() + (now.2 - target.2).abs();
        let now = if close < 0.01 { target } else { now };
        self.shown = Some(now);
        now
    }
}

/// Rebuilds a mono window from the tap's band levels: one sine per band with that band's
/// RMS, phases running continuously between frames. Bands are ~2.3 semitones apart, so a
/// tone between band centres lights up a peak band and its two neighbours: that group is
/// rendered as one sine at the tone's estimated frequency carrying the group's power, so
/// the chroma / Pitch mode name the right note instead of the nearest band centres.
#[derive(Default)]
struct Resynth {
    phase: Vec<f64>,
}

impl Resynth {
    fn fill(&mut self, tones: &[(f32, f32)], out: &mut [f32], rate: u32) {
        self.phase.resize(tones.len(), 0.0);
        out.iter_mut().for_each(|o| *o = 0.0);
        for (i, &(hz, lvl)) in tones.iter().enumerate() {
            let w = std::f64::consts::TAU * hz as f64 / rate as f64;
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

fn is_peak(b: &[f32; tap::BANDS], i: usize) -> bool {
    b[i] > 1e-4 && (i == 0 || b[i] > b[i - 1]) && (i + 1 >= tap::BANDS || b[i] >= b[i + 1])
}

/// The band whose tone band `i` belongs to: itself when it is a local peak, else an
/// adjacent peak (the louder one) whose skirt it is, else itself.
fn owner(b: &[f32; tap::BANDS], i: usize) -> usize {
    if is_peak(b, i) {
        return i;
    }
    let l = (i > 0 && is_peak(b, i - 1)).then(|| i - 1);
    let r = (i + 1 < tap::BANDS && is_peak(b, i + 1)).then_some(i + 1);
    match (l, r) {
        (Some(l), Some(r)) => {
            if b[l] >= b[r] {
                l
            } else {
                r
            }
        }
        (l, r) => l.or(r).unwrap_or(i),
    }
}

/// (frequency, RMS) to resynthesize per band: a local peak takes its neighbours' power
/// and moves to [`peak_freq`]; the neighbours it took are silent; other bands stay.
fn tones(b: &[f32; tap::BANDS]) -> [(f32, f32); tap::BANDS] {
    let mut out: [(f32, f32); tap::BANDS] = std::array::from_fn(|i| (tap::band_freq(i), 0.0));
    for (i, v) in b.iter().enumerate() {
        out[owner(b, i)].1 += v * v;
    }
    for (i, t) in out.iter_mut().enumerate() {
        t.1 = t.1.sqrt();
        if is_peak(b, i) {
            t.0 = peak_freq(b, i);
        }
    }
    out
}

/// Frequency to resynthesize band `i` at: its centre, or for a local peak the tone's
/// frequency estimated from the two neighbours. A band-pass's inverse power response
/// 1 / |H|² = 1 + Q²(x - 1/x)² is close to a parabola in log frequency, so the vertex
/// of the parabola through the three bands' 1 / power is where the tone sits.
fn peak_freq(bands: &[f32; tap::BANDS], i: usize) -> f32 {
    if i == 0 || i + 1 >= tap::BANDS {
        return tap::band_freq(i);
    }
    let (a, b, c) = (bands[i - 1], bands[i], bands[i + 1]);
    if b <= 1e-4 || b < a || b < c {
        return tap::band_freq(i);
    }
    let inv = |v: f32| 1.0 / (v as f64 * v as f64 + 1e-18);
    let (ya, yb, yc) = (inv(a), inv(b), inv(c));
    let curve = ya - 2.0 * yb + yc;
    if curve <= 0.0 {
        return tap::band_freq(i);
    }
    let d = (0.5 * (ya - yc) / curve).clamp(-0.5, 0.5) as f32;
    tap::band_freq_at(i as f32 + d)
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

        let th = a.global::<Theme>();
        let col = |c: slint::Color| {
            Rgb(
                c.red() as f32 / 255.0,
                c.green() as f32 / 255.0,
                c.blue() as f32 / 255.0,
            )
        };
        let dark = th.get_dark();
        let want_art = ART_COLORS.with(|e| e.get());
        let accent = self.art.accent(
            want_art,
            self.path.as_deref(),
            col(th.get_accent()),
            dark,
            dt,
        );
        let pal = Palette {
            accent,
            bg: col(th.get_bg_b1()),
            dark,
        };
        let pal_changed = self.pal != Some(pal);
        if self.pal.is_some_and(|o| o.bg != pal.bg) {
            self.canvas = Canvas::new(1, 1); // theme switch: no trails in the old background
        }
        self.pal = Some(pal);

        let paused = p.mpv.get_property::<bool>("pause").unwrap_or(true);
        self.paused_for = if !paused {
            0.0
        } else if pal_changed {
            self.paused_for.min(1.0) // redraw a few frames in the new colours
        } else {
            self.paused_for + dt
        };
        if self.paused_for > 1.5 && self.canvas.w > 1 {
            return; // settled; keep the last frame and save CPU
        }

        // Analysis
        let t: f64 = p.mpv.get_property("time-pos").unwrap_or(0.0);
        let delay: f64 = p.mpv.get_property("audio-delay").unwrap_or(0.0);
        let rate = 48_000;
        let frame = if paused {
            None
        } else if let Some(fr) = self.tap.read(t - delay) {
            if self.an.as_ref().map(|a| a.rate()) != Some(rate) {
                self.an = Some(Analyzer::new(rate));
            }
            self.samples.resize(Analyzer::window_len(rate), 0.0);
            let t = tones(&fr.bands);
            self.synth.fill(&t, &mut self.samples, rate);
            Some((fr, t))
        } else {
            None
        };
        let an = self.an.get_or_insert_with(|| Analyzer::new(48_000));
        if let Some((fr, t)) = &frame {
            let m = analysis::Measured {
                levels: &fr.bands,
                f_lo: tap::band_freq(0),
                f_hi: tap::band_freq(tap::BANDS - 1),
                tones: t,
            };
            an.process(&self.samples, Some(&m), dt);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Dominant pitch class the analyser finds in `fr` after resynthesis.
    pub(super) fn pitch_of(fr: &tap::Frame) -> usize {
        let rate = 48_000;
        let mut an = Analyzer::new(rate);
        let mut synth = Resynth::default();
        let mut s = vec![0.0; Analyzer::window_len(rate)];
        let t = tones(&fr.bands);
        let m = analysis::Measured {
            levels: &fr.bands,
            f_lo: tap::band_freq(0),
            f_hi: tap::band_freq(tap::BANDS - 1),
            tones: &t,
        };
        for _ in 0..8 {
            synth.fill(&t, &mut s, rate);
            an.process(&s, Some(&m), 0.033);
        }
        an.f.pitch
    }

    /// Band levels of a pure tone as the tap measures them (biquad band-pass, Q = 4).
    fn tone(hz: f32) -> tap::Frame {
        let mut bands = [0.0; tap::BANDS];
        for (i, b) in bands.iter_mut().enumerate() {
            let x = hz / tap::band_freq(i);
            let h = 1.0 / (1.0 + 16.0 * (x - 1.0 / x).powi(2)).sqrt();
            *b = 0.35 * h;
        }
        tap::Frame {
            t: 0.0,
            full: 0.35,
            bands,
        }
    }

    #[test]
    fn tones_between_bands_keep_their_note() {
        // A3, C4, E4, G4, A4, C5, F#5: none sits on a band centre.
        for (hz, pc) in [
            (220.0, 9),
            (261.63, 0),
            (329.63, 4),
            (392.0, 7),
            (440.0, 9),
            (523.25, 0),
            (739.99, 6),
        ] {
            assert_eq!(pitch_of(&tone(hz)), pc, "{hz} Hz");
        }
    }

    #[test]
    fn tones_keep_the_power() {
        let fr = tone(1000.0);
        let t = tones(&fr.bands);
        let p = |v: &mut dyn Iterator<Item = f32>| v.map(|x| x * x).sum::<f32>();
        let (a, b) = (
            p(&mut fr.bands.iter().copied()),
            p(&mut t.iter().map(|t| t.1)),
        );
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        // One loud tone at ~1 kHz, its neighbours folded into it.
        let loud: Vec<_> = t.iter().filter(|t| t.1 > 0.2).collect();
        assert_eq!(loud.len(), 1);
        assert!((loud[0].0 / 1000.0).log2().abs() < 0.03, "{loud:?}");
    }

    #[test]
    fn peak_estimate_is_close() {
        for hz in [100.0, 333.0, 1234.0, 5000.0] {
            let fr = tone(hz);
            let i = (0..tap::BANDS)
                .max_by(|&a, &b| fr.bands[a].total_cmp(&fr.bands[b]))
                .unwrap();
            let est = peak_freq(&fr.bands, i);
            // Within a third of a semitone.
            assert!((12.0 * (est / hz).log2()).abs() < 0.33, "{hz} → {est}");
        }
    }
}
