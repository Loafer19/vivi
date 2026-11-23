//! Music visualizer, registered as the built-in `visualizer` plugin ([`VizPlugin`]). Self-contained: `setup` wires it to the window and player
//! and everything else lives here.
//!
//! * [`tap`] measures mpv's actual output (after EQ and the other audio filters) with an
//!   `af` lavfi filter: per-band levels with timestamps, for files and streams alike.
//! * [`Resynth`] turns those band levels back into a short signal (one sine per band),
//!   so [`analysis`] can keep deriving features (FFT bands, loudness, onsets/beats,
//!   chroma/dominant pitch) and the modes stay unchanged.
//! * [`modes`] holds the visual modes behind the [`modes::Visual`] trait; they draw
//!   into a software [`canvas`] sized to the window's physical pixels (capped, see
//!   [`canvas_size`]) that Slint scales to the window. Radial (Rays and Cover styles)
//!   gets the album cover drawn sharp on top by Slint (`VizView`).
//! * [`art`] decodes the playing song's cover: its palette for *Colors → Album art*
//!   and the picture for the cover slot.
//!
//! It only runs while audio-only media is playing and a mode is selected; once a
//! paused frame has settled the frame task stops until [`wake`] or [`arm`].

mod analysis;
mod art;
mod canvas;
mod modes;
mod tap;

use crate::player::Player;
use crate::plugins::{contrib, SettingSpec};
use crate::{AppWindow, Theme};
use analysis::Analyzer;
use canvas::{Canvas, ColorMode, Palette, Rgb, STOPS};
use slint::{ComponentHandle, Model, ModelRc, SharedPixelBuffer, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const FRAME: Duration = Duration::from_millis(33); // ~30 fps
/// Options of the *Colors* setting.
const COLORS: [&str; 3] = ["Theme accent", "Album art", "Frequency"];
/// Options of *Sensitivity*, *Bar density* and *Waterfall speed* (the middle one is the default).
const SENSITIVITY: [&str; 3] = ["Low", "Normal", "High"];
const DENSITY: [&str; 3] = ["Fewer", "Normal", "More"];
const SPEED: [&str; 3] = ["Slow", "Normal", "Fast"];

/// The plugin settings the frame task reads.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    color: ColorMode,
    /// Linear input gain (Sensitivity: Low −6 dB, High +6 dB).
    gain: f32,
    bars: usize,
    scroll: f32,
    led: bool,
}

impl Settings {
    const DEFAULT: Self = Self {
        color: ColorMode::Accent,
        gain: 1.0,
        bars: 64,
        scroll: 2.0,
        led: false,
    };
    fn read(cx: &crate::plugins::Ctx) -> Self {
        Self::from_values(
            &cx.get("colors"),
            &cx.get("sensitivity"),
            &cx.get("bars"),
            &cx.get("speed"),
            cx.flag("led_caps"),
        )
    }
    fn from_values(colors: &str, sens: &str, bars: &str, speed: &str, led: bool) -> Self {
        Self {
            color: match colors {
                "Album art" => ColorMode::Art,
                "Frequency" => ColorMode::Frequency,
                _ => ColorMode::Accent,
            },
            gain: match sens {
                "Low" => 0.5,
                "High" => 2.0,
                _ => 1.0,
            },
            bars: match bars {
                "Fewer" => 32,
                "More" => 96,
                _ => 64,
            },
            scroll: match speed {
                "Slow" => 1.0,
                "Fast" => 4.0,
                _ => 2.0,
            },
            led,
        }
    }
}
const NOTES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];

struct Viz {
    tap: tap::Tap,
    an: Option<Analyzer>,
    modes: Vec<Box<dyn modes::Visual>>,
    /// 0 = Disabled, i = modes[i - 1].
    mode: usize,
    last_eff_mode: usize,
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
    art: Art,
    /// [`Art::gen`] of the cover image handed to Slint.
    cover_gen: u32,
    /// Cover slot size last sent to Slint (0 = hidden).
    cover_size: f32,
    spectrum: modes::SpectrumStyle,
    radial: modes::RadialStyle,
}

thread_local! {
    /// Toggled by the plugin registry (Settings → Extensions).
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    /// The plugin settings (Settings → Extensions → Music Visualizer).
    static SETTINGS: Cell<Settings> = const { Cell::new(Settings::DEFAULT) };
    /// Redraw requested while a paused frame had settled (see [`wake`]).
    static WAKE: Cell<bool> = const { Cell::new(false) };
    static TICK: Cell<Option<crate::ticker::Task>> = const { Cell::new(None) };
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
    /// (need_levels_for_neon, bg_mode_index_0_if_none)
    static KARAOKE: Cell<(bool, usize)> = const { Cell::new((false, 0)) };
}

type Ctx = (slint::Weak<AppWindow>, Rc<Player>, Rc<RefCell<Viz>>);

/// The visualizer as a plugin: enabling/disabling shows or hides it; modes are picked
/// in the player (dropdown, `V` / `Shift+V`, the command palette or IPC).
pub struct VizPlugin;

impl crate::plugins::Plugin for VizPlugin {
    /// The visualizer draws from its own ~30 fps frame task (see [`arm`]).
    fn wants_tick(&self, _cx: &crate::plugins::Ctx) -> bool {
        false
    }
    fn manifest(&self) -> crate::plugins::Manifest {
        crate::plugins::Manifest::builtin(
            "visualizer",
            "Music Visualizer",
            "Eight visualizer modes for music and radio (anything without video), drawn from what mpv plays, so the equalizer shows: Spectrometer, Scope, Spectrum (Bars, LED, Mountain, Dots), Envelope, Radial (Rays, Cover), Particles, Pitch and VU Meter. Pick a mode with the dropdown or V (Shift+V goes back); Spectrum and Radial have a style picker. Colors follow the theme accent, the song's album art or the frequency.",
            "visualizer",
        )
        .contribute(contrib::PALETTE_COMMAND, "Next visualizer mode", "next")
        .contribute(contrib::PALETTE_COMMAND, "Previous visualizer mode", "prev")
        .setting(SettingSpec::choice(
            "colors",
            "Colors",
            "Album art: the visuals take up to five main colors of the playing song's cover (a gradient from bass to treble), made readable on the theme; a cover with one clear color gives that color, and covers without one (black and white, grey) keep the theme accent. Frequency: a hue per frequency, warm bass to cool treble.",
            &COLORS,
        ))
        .setting(SettingSpec::choice_default(
            "sensitivity",
            "Sensitivity",
            "How strongly the visuals react to the level: Low for loud masters, High for quiet recordings.",
            &SENSITIVITY,
            "Normal",
        ))
        .setting(SettingSpec::choice_default(
            "bars",
            "Bar density",
            "How many bars or dots the Spectrum styles draw across the window.",
            &DENSITY,
            "Normal",
        ))
        .setting(SettingSpec::choice_default(
            "speed",
            "Waterfall speed",
            "How fast the Spectrometer scrolls.",
            &SPEED,
            "Normal",
        ))
        .setting(SettingSpec::toggle(
            "led_caps",
            "LED peak caps",
            "Spectrum, Bars style: the falling peak caps as glowing LED blocks instead of thin lines.",
            false,
        ))
    }
    fn enable(&mut self, cx: &crate::plugins::Ctx) {
        ENABLED.with(|e| e.set(true));
        SETTINGS.with(|e| e.set(Settings::read(cx)));
        cx.app.global::<crate::Viz>().set_enabled(true);
        cx.app
            .global::<crate::Viz>()
            .set_audio(cx.app.global::<crate::Playback>().get_has_media() && audio_only(cx.player));
        arm();
    }
    fn disable(&mut self, cx: &crate::plugins::Ctx) {
        ENABLED.with(|e| e.set(false));
        cx.app.global::<crate::Viz>().set_enabled(false);
        cx.app.global::<crate::Viz>().set_audio(false);
        let _ = run();
    }
    fn setting_changed(&mut self, cx: &crate::plugins::Ctx, _key: &str) {
        SETTINGS.with(|e| e.set(Settings::read(cx)));
        wake();
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
/// Old names (Classic, Cover, Waveform, …) and bare style names (LED, Bars, Rays)
/// set the matching mode and style.
pub fn select(a: &AppWindow, mode: &str) -> Result<(), String> {
    if !enabled() {
        return Err("the Music Visualizer is turned off (Settings → Extensions)".into());
    }
    let v = a.global::<crate::Viz>();
    let names: Vec<String> = v.get_modes().iter().map(|m| m.to_string()).collect();
    let i = mode_index(&names, mode)
        .or_else(|| {
            mode.trim()
                .parse::<usize>()
                .ok()
                .filter(|&i| i < names.len())
        })
        .ok_or_else(|| {
            format!(
                "unknown mode {mode:?}; one of: {} (old names Classic, Mountain, Dots, Cover and Waveform still work)",
                names.join(", ")
            )
        })?;
    if let Some(r) = modes::resolve(mode) {
        set_pending_styles(r);
    }
    v.invoke_select(i as i32);
    Ok(())
}

/// Select a style of the current mode (Bars / LED / Mountain / Dots, or Rays / Cover).
pub fn select_style(a: &AppWindow, style: &str) -> Result<(), String> {
    if !enabled() {
        return Err("the Music Visualizer is turned off (Settings → Extensions)".into());
    }
    let v = a.global::<crate::Viz>();
    let names: Vec<String> = v.get_modes().iter().map(|m| m.to_string()).collect();
    let mode = names
        .get(v.get_mode() as usize)
        .cloned()
        .unwrap_or_default();
    let styles = modes::styles_for(&mode);
    if styles.is_empty() {
        return Err(format!("{mode} has no styles"));
    }
    let want = modes::norm(style);
    let i = styles
        .iter()
        .position(|s| modes::norm(s) == want)
        .ok_or_else(|| {
            format!(
                "unknown style {style:?} for {mode}; one of: {}",
                styles.join(", ")
            )
        })?;
    v.invoke_select_style(i as i32);
    Ok(())
}

/// Snapshot of the visualizer for IPC / MCP.
#[derive(Clone, Debug)]
pub struct State {
    pub mode: String,
    pub modes: Vec<String>,
    pub style: String,
    pub styles: Vec<String>,
    pub showing: bool,
}

/// Current mode, style, the lists, and whether the visualizer is on screen now.
pub fn state(a: &AppWindow) -> State {
    let v = a.global::<crate::Viz>();
    let names: Vec<String> = v.get_modes().iter().map(|m| m.to_string()).collect();
    let mode = names
        .get(v.get_mode() as usize)
        .cloned()
        .unwrap_or_default();
    let styles: Vec<String> = v.get_styles().iter().map(|s| s.to_string()).collect();
    let style = styles
        .get(v.get_style() as usize)
        .cloned()
        .unwrap_or_default();
    let showing = enabled()
        && v.get_audio()
        && v.get_mode() > 0
        && a.global::<crate::Playback>().get_has_media();
    State {
        mode,
        modes: names,
        style,
        styles,
        showing,
    }
}

/// Index of mode `name` in `names`: case, spaces and dashes do not matter ("vu-meter"
/// is VU Meter) and old names still work ([`modes::ALIASES`]: Waveform is Envelope,
/// Classic is Spectrum / LED, Cover is Radial / Cover).
fn mode_index(names: &[impl AsRef<str>], name: &str) -> Option<usize> {
    let want = if let Some(r) = modes::resolve(name) {
        modes::norm(r.mode)
    } else {
        modes::norm(name)
    };
    if want.is_empty() {
        return None;
    }
    names.iter().position(|n| modes::norm(n.as_ref()) == want)
}

fn set_pending_styles(r: modes::Resolved) {
    CTX.with(|c| {
        if let Some((_, _, viz)) = c.borrow().as_ref() {
            let mut v = viz.borrow_mut();
            if let Some(s) = r.spectrum {
                v.spectrum = s;
            }
            if let Some(s) = r.radial {
                v.radial = s;
            }
        }
    });
}

struct Choice {
    mode: usize,
    spectrum: modes::SpectrumStyle,
    radial: modes::RadialStyle,
}

/// First line is the mode (aliases work). Later lines are the remembered Spectrum
/// and Radial styles (`LED`, `Cover`, or `spectrum:LED` / `radial:Cover`).
fn load_choice(text: &str, names: &[impl AsRef<str>]) -> Choice {
    let mut spectrum = modes::SpectrumStyle::Bars;
    let mut radial = modes::RadialStyle::Rays;
    let mut mode = mode_index(names, "Spectrum").unwrap_or(3);
    let mut lines = text.lines().map(str::trim).filter(|s| !s.is_empty());
    if let Some(first) = lines.next() {
        if let Some(r) = modes::resolve(first) {
            if let Some(i) = mode_index(names, r.mode) {
                mode = i;
            }
            if let Some(s) = r.spectrum {
                spectrum = s;
            }
            if let Some(s) = r.radial {
                radial = s;
            }
        } else if let Some(i) = mode_index(names, first) {
            mode = i;
        }
    }
    for line in lines {
        let (key, val) = line
            .split_once(':')
            .map(|(k, v)| (k.trim(), v.trim()))
            .unwrap_or(("", line));
        let token = if key.is_empty() { line } else { val };
        if key.is_empty() || modes::norm(key) == "spectrum" {
            if let Some(s) = modes::SpectrumStyle::parse(token) {
                spectrum = s;
                continue;
            }
        }
        if key.is_empty() || modes::norm(key) == "radial" {
            if let Some(s) = modes::RadialStyle::parse(token) {
                radial = s;
            }
        }
    }
    Choice {
        mode,
        spectrum,
        radial,
    }
}

fn save_choice(mode: &str, spectrum: modes::SpectrumStyle, radial: modes::RadialStyle) {
    let _ = std::fs::create_dir_all(crate::store::state_dir());
    let _ = std::fs::write(
        mode_file(),
        format!(
            "{mode}
{}
{}
",
            spectrum.name(),
            radial.name()
        ),
    );
}

fn sync_style_ui(
    a: &AppWindow,
    mode: &str,
    spectrum: modes::SpectrumStyle,
    radial: modes::RadialStyle,
) {
    let list: Vec<SharedString> = modes::styles_for(mode)
        .iter()
        .map(|s| SharedString::from(*s))
        .collect();
    let i = match modes::norm(mode).as_str() {
        "spectrum" => spectrum as usize,
        "radial" => radial as usize,
        _ => 0,
    };
    let v = a.global::<crate::Viz>();
    v.set_styles(ModelRc::new(VecModel::from(list)));
    v.set_style(i as i32);
}


pub(crate) fn art_blurred(path: &std::path::Path) -> Option<SharedPixelBuffer<slint::Rgba8Pixel>> {
    art::decode_blurred(path)
}

/// Re-assert `@vivi-viz` at the end of mpv's `af` chain (e.g. after `@vivi-pitch` was added).
pub fn reassert_tap() {
    CTX.with(|c| {
        let Some((_, p, viz)) = c.borrow().clone() else {
            return;
        };
        if let Ok(v) = viz.try_borrow() {
            if v.tap_open {
                if let Some(f) = v.tap.filter() {
                    ensure_tap(&p, &f);
                }
            }
        }
    });
}

/// Update visualizer behaviour for the open Karaoke view (Neon audio levels / dimmed viz bg).
pub fn set_karaoke(a: &AppWindow) {
    let open = a.global::<crate::Overlays>().get_karaoke_open();
    let need_levels = open && a.global::<crate::Lyrics>().get_style() == "Neon";
    let bg_mode = if open {
        let bg = a.global::<crate::Lyrics>().get_background();
        let b = bg.as_str();
        if b != "None" && b != "Cover art" {
            let names: Vec<String> = a
                .global::<crate::Viz>()
                .get_modes()
                .iter()
                .map(|m| m.to_string())
                .collect();
            mode_index(&names, b).unwrap_or(0)
        } else {
            0
        }
    } else {
        0
    };
    let prev = KARAOKE.with(|k| k.replace((need_levels, bg_mode)));
    a.global::<crate::Viz>().set_karaoke_active(bg_mode > 0);
    if !need_levels {
        let l = a.global::<crate::Lyrics>();
        l.set_level(0.0);
        l.set_beat(0.0);
    }
    if prev != (need_levels, bg_mode) {
        wake();
    }
}

/// Redraw the visualizer even if its paused frame had settled and the frame task was
/// stopped: a seek, a window resize, a theme, cover or setting change.
pub fn wake() {
    WAKE.with(|w| w.set(true));
    arm();
}

/// Canvas size for a window of `w`×`h` physical pixels at `scale`: the window's own
/// resolution, capped at 960 × scale (≤ 2×) on the longer side and ~1.6 Mpx, so HiDPI
/// screens get a sharper canvas without unbounded per-pixel work.
fn canvas_size(w: u32, h: u32, scale: f32) -> (usize, usize) {
    let cap = 960.0 * scale.clamp(1.0, 2.0);
    let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
    let d = (wf.max(hf) / cap).max((wf * hf / 1.6e6).sqrt()).max(1.0);
    (((wf / d) as usize).max(16), ((hf / d) as usize).max(16))
}

fn mode_file() -> std::path::PathBuf {
    crate::store::state_dir().join("visualizer")
}

pub fn setup(app: &AppWindow, player: &Rc<Player>) {
    let modes = modes::all();
    let mut names: Vec<SharedString> = vec!["Disabled".into()];
    names.extend(modes.iter().map(|m| SharedString::from(m.name())));
    let saved = std::fs::read_to_string(mode_file()).unwrap_or_default();
    let choice = load_choice(&saved, &names);
    let mode = choice.mode;
    app.global::<crate::Viz>()
        .set_modes(ModelRc::new(VecModel::from(names.clone())));
    app.global::<crate::Viz>().set_mode(mode as i32);
    let mode_name = names.get(mode).map(|s| s.as_str()).unwrap_or("Spectrum");
    sync_style_ui(app, mode_name, choice.spectrum, choice.radial);

    let viz = Rc::new(RefCell::new(Viz {
        tap: tap::Tap::new(),
        an: None,
        modes,
        mode,
        last_eff_mode: mode,
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
        art: Art::default(),
        cover_gen: 0,
        cover_size: 0.0,
        spectrum: choice.spectrum,
        radial: choice.radial,
    }));
    app.global::<crate::Viz>().on_wake(wake);

    let (w, v) = (app.as_weak(), viz.clone());
    app.global::<crate::Viz>().on_select(move |i| {
        let Some(a) = w.upgrade() else { return };
        let n = a.global::<crate::Viz>().get_modes().row_count();
        let i = (i.max(0) as usize).min(n.saturating_sub(1));
        let (spectrum, radial) = {
            let mut v = v.borrow_mut();
            if v.mode != i {
                // Start the new mode on a clean canvas (no trails of the old one), and
                // draw it even while paused (the paused frame is otherwise kept).
                v.canvas = Canvas::new(1, 1);
                v.paused_for = 0.0;
            }
            v.mode = i;
            (v.spectrum, v.radial)
        };
        a.global::<crate::Viz>().set_mode(i as i32);
        a.global::<crate::Viz>().set_note("".into());
        if let Some(name) = a.global::<crate::Viz>().get_modes().row_data(i) {
            sync_style_ui(&a, name.as_str(), spectrum, radial);
            save_choice(name.as_str(), spectrum, radial);
        }
        arm();
    });

    let (w, v) = (app.as_weak(), viz.clone());
    app.global::<crate::Viz>().on_select_style(move |i| {
        let Some(a) = w.upgrade() else { return };
        let n = a.global::<crate::Viz>().get_styles().row_count();
        if n == 0 {
            return;
        }
        let i = (i.max(0) as usize).min(n.saturating_sub(1));
        let Some(style) = a.global::<crate::Viz>().get_styles().row_data(i) else {
            return;
        };
        let mode_name = a
            .global::<crate::Viz>()
            .get_modes()
            .row_data(a.global::<crate::Viz>().get_mode() as usize)
            .unwrap_or_default();
        {
            let mut v = v.borrow_mut();
            if let Some(s) = modes::SpectrumStyle::parse(style.as_str()) {
                if modes::norm(mode_name.as_str()) == "spectrum" && v.spectrum != s {
                    v.spectrum = s;
                    v.canvas = Canvas::new(1, 1);
                    v.paused_for = 0.0;
                }
            }
            if let Some(s) = modes::RadialStyle::parse(style.as_str()) {
                if modes::norm(mode_name.as_str()) == "radial" && v.radial != s {
                    v.radial = s;
                    v.canvas = Canvas::new(1, 1);
                    v.paused_for = 0.0;
                }
            }
            save_choice(mode_name.as_str(), v.spectrum, v.radial);
        }
        a.global::<crate::Viz>().set_style(i as i32);
        arm();
    });

    CTX.with(|c| *c.borrow_mut() = Some((app.as_weak(), player.clone(), viz)));
}

fn run() -> bool {
    CTX.with(|c| {
        let Some((w, p, viz)) = c.borrow().clone() else {
            TICK.with(crate::ticker::clear);
            return false;
        };
        let Some(a) = w.upgrade() else {
            TICK.with(crate::ticker::clear);
            return false;
        };
        let keep = viz.borrow_mut().tick(&a, &p);
        keep
    })
}

/// Start the ~30 fps frame task when a visualizer mode can draw; stop it otherwise.
/// Player events and mode changes call this so the timer does not run while idle.
pub fn arm() {
    if TICK.with(|slot| slot.get().is_some()) {
        return;
    }
    if run() {
        TICK.with(|slot| {
            if slot.get().is_none() {
                slot.set(Some(crate::ticker::frames(FRAME, || {
                    run();
                })));
            }
        });
    }
}

/// What the worker sends back for a cover: the picture for the cover slot and its palette.
type Decoded = Option<(SharedPixelBuffer<slint::Rgba8Pixel>, Vec<Rgb>)>;

/// The playing song's cover for *Colors → Album art* and the cover slot: decoded on a
/// worker thread whenever the shared now-playing cover changes (only while needed), its
/// palette then eased in.
#[derive(Default)]
struct Art {
    /// Now-playing change counter seen last.
    np_id: i64,
    cover: String,
    rx: Option<Receiver<Decoded>>,
    /// The cover picture (None: none / not known yet).
    image: Option<SharedPixelBuffer<slint::Rgba8Pixel>>,
    /// Bumped whenever `image` changes.
    gen: u32,
    /// Raw palette of `cover`, main colour first (empty: none / not known yet).
    colors: Vec<Rgb>,
    /// Stops shown now, moving towards the target.
    shown: Option<[Rgb; STOPS]>,
}

impl Art {
    /// Follow the playing song's cover while `need` (Album art colours or a cover mode).
    fn update(&mut self, need: bool, path: Option<&str>) {
        if !need {
            self.np_id = -1; // look again when needed
            return;
        }
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
                self.colors.clear();
                if self.image.take().is_some() {
                    self.gen = self.gen.wrapping_add(1);
                }
                self.rx = None;
                if !cover.is_empty() {
                    let (tx, rx) = channel();
                    self.rx = Some(rx);
                    let _ = std::thread::Builder::new()
                        .name("vivi-viz-art".into())
                        .spawn(move || {
                            let d = crate::plugins::host::fetch_cover(&cover)
                                .and_then(|p| art::decode(&p));
                            let _ = tx.send(d);
                            // Show it even on a settled paused frame.
                            let _ = slint::invoke_from_event_loop(wake);
                        });
                }
            }
        }
        if let Some(d) = self.rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.rx = None;
            let (img, colors) = d.map_or((None, Vec::new()), |(i, c)| (Some(i), c));
            self.colors = colors;
            self.image = img;
            self.gen = self.gen.wrapping_add(1);
        }
    }

    /// Palette stops to draw with: the cover's colours (Album art) or the theme accent,
    /// eased towards the target in ~0.4 s (song changes, switching the setting).
    fn stops(&mut self, art: bool, theme: Rgb, dark: bool, dt: f32) -> ([Rgb; STOPS], usize) {
        let mut target = [theme; STOPS];
        let n = if art && !self.colors.is_empty() {
            for (t, c) in target.iter_mut().zip(&self.colors) {
                *t = art::usable(*c, dark);
            }
            self.colors.len().min(STOPS)
        } else {
            1
        };
        // Unused stops repeat the last colour, so a change of count eases smoothly.
        let last = target[n - 1];
        target[n..].fill(last);
        let k = (dt * 8.0).min(1.0);
        let mut now = self.shown.unwrap_or(target);
        let mut far = 0.0;
        for (c, t) in now.iter_mut().zip(&target) {
            *c = c.lerp(*t, k);
            far += (c.0 - t.0).abs() + (c.1 - t.1).abs() + (c.2 - t.2).abs();
        }
        if far < 0.01 * STOPS as f32 {
            now = target;
        }
        self.shown = Some(now);
        (now, n)
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

    fn tick(&mut self, a: &AppWindow, p: &Player) -> bool {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.2);
        self.last = now;
        self.tick = self.tick.wrapping_add(1);

        if !enabled() {
            self.remove_tap(p);
            self.path = None;
            TICK.with(crate::ticker::clear);
            return false;
        }
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
                self.paused_for = 0.0;
            }
            self.path = path;
            self.track_count = tc;
            self.audio_only = self.path.is_some() && audio_only(p);
            a.global::<crate::Viz>().set_audio(self.audio_only);
        }
        let (need_levels, k_bg_mode) = KARAOKE.with(|k| k.get());
        let eff_mode = if k_bg_mode > 0 { k_bg_mode } else { self.mode };
        if eff_mode != self.last_eff_mode {
            self.last_eff_mode = eff_mode;
            self.canvas = Canvas::new(1, 1);
            self.paused_for = 0.0;
        }
        let active = self.audio_only
            && (eff_mode > 0 || need_levels)
            && a.global::<crate::Playback>().get_has_media();
        if !active {
            self.remove_tap(p);
            if need_levels {
                let l = a.global::<crate::Lyrics>();
                l.set_level(0.0);
                l.set_beat(0.0);
            }
            TICK.with(crate::ticker::clear);
            return false;
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
        let set = SETTINGS.with(|e| e.get());
        let shows_cover = eff_mode > 0
            && self
                .modes
                .get(eff_mode - 1)
                .is_some_and(|m| m.shows_cover());
        let want_art = set.color == ColorMode::Art;
        self.art
            .update(want_art || shows_cover, self.path.as_deref());
        if self.art.gen != self.cover_gen {
            self.cover_gen = self.art.gen;
            let v = a.global::<crate::Viz>();
            match &self.art.image {
                Some(buf) => {
                    v.set_cover(slint::Image::from_rgba8(buf.clone()));
                    v.set_has_cover(true);
                }
                None => {
                    v.set_has_cover(false);
                    v.set_cover(slint::Image::default());
                }
            }
        }
        let theme = col(th.get_accent());
        let (stops, n) = self.art.stops(want_art, theme, dark, dt);
        let pal = Palette {
            accent: if want_art { stops[0] } else { theme },
            bg: col(th.get_bg_b1()),
            dark,
            mode: set.color,
            stops,
            n,
        };
        let pal_changed = self.pal != Some(pal);
        if self.pal.is_some_and(|o| o.bg != pal.bg) {
            self.canvas = Canvas::new(1, 1); // theme switch: no trails in the old background
        }
        self.pal = Some(pal);

        if WAKE.with(|w| w.replace(false)) {
            self.paused_for = self.paused_for.min(1.0); // redraw a few frames
        }
        let paused = p.mpv.get_property::<bool>("pause").unwrap_or(true);
        self.paused_for = if !paused {
            0.0
        } else if pal_changed {
            self.paused_for.min(1.0) // redraw a few frames in the new colours
        } else {
            self.paused_for + dt
        };
        if self.paused_for > 1.5 && (eff_mode == 0 || self.canvas.w > 1) {
            if need_levels {
                let l = a.global::<crate::Lyrics>();
                l.set_level(0.0);
                l.set_beat(0.0);
            }
            // Settled: keep the last frame and stop the frame task until play, a seek,
            // a mode / setting / theme change or a resize wakes it ([`wake`], [`arm`]).
            TICK.with(crate::ticker::clear);
            return false;
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
        an.gain = set.gain;
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

        if need_levels {
            let l = a.global::<crate::Lyrics>();
            l.set_level(an.f.level.clamp(0.0, 1.0));
            l.set_beat(an.f.beat.clamp(0.0, 1.0));
        }
        if eff_mode == 0 {
            return true;
        }
        let size = a.window().size();
        let (cw, ch) = canvas_size(size.width, size.height, a.window().scale_factor());
        if self.canvas.w != cw || self.canvas.h != ch {
            self.canvas = Canvas::new(cw, ch);
            self.canvas.clear(pal.bg);
        }
        let Some(m) = self.modes.get_mut(eff_mode - 1) else {
            return true;
        };
        let has_cover = shows_cover && self.art.image.is_some();
        let opts = modes::Opts {
            bars: set.bars,
            scroll: set.scroll,
            led: set.led,
            cover: has_cover,
            spectrum: self.spectrum,
            radial: self.radial,
        };
        m.draw(&mut self.canvas, &an.f, &pal, &opts);
        let slot = if has_cover {
            m.cover(&an.f, &opts).unwrap_or(0.0)
        } else {
            0.0
        };
        if (slot - self.cover_size).abs() > 0.001 {
            self.cover_size = slot;
            a.global::<crate::Viz>().set_cover_size(slot);
        }
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
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_are_loose_and_old_names_work() {
        let names = ["Disabled", "Spectrum", "Envelope", "VU Meter"];
        assert_eq!(mode_index(&names, "spectrum"), Some(1));
        assert_eq!(mode_index(&names, "Waveform"), Some(2));
        assert_eq!(mode_index(&names, "waveform"), Some(2));
        assert_eq!(mode_index(&names, "vu-meter"), Some(3));
        assert_eq!(mode_index(&names, " VU meter "), Some(3));
        assert_eq!(mode_index(&names, "Classic"), Some(1));
        assert_eq!(mode_index(&names, ""), None);
        assert_eq!(mode_index(&names, "nope"), None);
        let names = [
            "Disabled",
            "Spectrometer",
            "Scope",
            "Spectrum",
            "Envelope",
            "Radial",
            "Particles",
            "Pitch",
            "VU Meter",
        ];
        assert_eq!(mode_index(&names, "Cover"), Some(5));
        assert_eq!(mode_index(&names, "Mountain"), Some(3));
        assert_eq!(mode_index(&names, "Dots"), Some(3));
        assert_eq!(mode_index(&names, "LED"), Some(3));
        assert_eq!(mode_index(&names, "Classic"), Some(3));
    }

    #[test]
    fn load_choice_migrates_old_mode_names_and_keeps_styles() {
        let names = [
            "Disabled",
            "Spectrometer",
            "Scope",
            "Spectrum",
            "Envelope",
            "Radial",
            "Particles",
            "Pitch",
            "VU Meter",
        ];
        let c = load_choice("Classic", &names);
        assert_eq!(c.mode, 3);
        assert_eq!(c.spectrum, modes::SpectrumStyle::Led);
        let c = load_choice("Cover\n", &names);
        assert_eq!(c.mode, 5);
        assert_eq!(c.radial, modes::RadialStyle::Cover);
        let c = load_choice("Waveform", &names);
        assert_eq!(c.mode, 4);
        let c = load_choice("Spectrum\nLED\nCover\n", &names);
        assert_eq!(c.mode, 3);
        assert_eq!(c.spectrum, modes::SpectrumStyle::Led);
        assert_eq!(c.radial, modes::RadialStyle::Cover);
        let c = load_choice("Radial\nspectrum:Dots\nradial:Rays\n", &names);
        assert_eq!(c.mode, 5);
        assert_eq!(c.spectrum, modes::SpectrumStyle::Dots);
        assert_eq!(c.radial, modes::RadialStyle::Rays);
        let c = load_choice("", &names);
        assert_eq!(c.mode, 3);
        assert_eq!(c.spectrum, modes::SpectrumStyle::Bars);
        assert_eq!(c.radial, modes::RadialStyle::Rays);
    }

    #[test]
    fn canvas_follows_the_window_up_to_a_cap() {
        assert_eq!(canvas_size(800, 600, 1.0), (800, 600));
        assert_eq!(canvas_size(1920, 1080, 1.0), (960, 540));
        // HiDPI: sharper, but bounded by the pixel budget.
        let (w, h) = canvas_size(3840, 2160, 2.0);
        assert!(w > 1600 && w <= 1920 && (w * h) as f32 <= 1.61e6, "{w}x{h}");
        assert_eq!(canvas_size(0, 0, 1.0), (16, 16));
    }

    #[test]
    fn settings_parse() {
        let s = Settings::from_values("Album art", "High", "More", "Fast", true);
        assert_eq!(s.color, ColorMode::Art);
        assert!(s.gain > 1.0 && s.bars == 96 && s.scroll > 2.0 && s.led);
        assert_eq!(
            Settings::from_values("", "", "", "", false),
            Settings::DEFAULT
        );
    }

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
