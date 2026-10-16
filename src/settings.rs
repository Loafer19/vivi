//! Settings dialog model, live application of settings and per-show presets (#16).

use crate::config::{self, Config, Preset};
use crate::player::Player;
use crate::plugins;
use crate::{store, AppWindow, SettingRow, Theme};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Seconds of inactivity before the controls hide (read by the auto-hide timer).
pub static AUTOHIDE_SECS: AtomicU32 = AtomicU32::new(2);

const CATEGORIES: [&str; 13] = [
    "General",
    "Interface",
    "Privacy",
    "Controls",
    "Playback",
    "Video",
    "Audio",
    "Subtitles",
    "Library",
    "Network",
    "Performance",
    "Recording",
    "Extensions",
];
const ACCENTS: [&str; 9] = [
    "#D37918", "#C8452E", "#A68A1C", "#2E9A44", "#1E9AAE", "#2155C4", "#7C3FD1", "#C23B98",
    "#8C8C8C",
];

pub const SHORTCUTS: [(&str, &str); 18] = [
    ("Play / pause", "Space  K"),
    ("Seek (short skip)", "←  →"),
    ("Seek (long skip)", "Shift ← →"),
    ("Volume (volume step)", "↑  ↓"),
    ("Mute", "M"),
    ("Fullscreen", "F  Double-click"),
    ("Leave fullscreen", "Esc"),
    ("Open file", "Ctrl O"),
    ("Add bookmark", "Ctrl B"),
    ("Settings", "Ctrl ,"),
    ("Karaoke lyrics view", "L"),
    ("A-B loop: set A / set B / clear", "Shift+L"),
    ("Find lyrics", "Ctrl L"),
    ("Lyrics timing offset", "[  ]  (Shift ±0.5 s)"),
    ("Subtitle offset (per file)", "Z  X  (Shift ±1 s)"),
    ("Previous / next in queue", "Buttons"),
    ("Drop files", "Open or enqueue"),
    ("Side panel", "Panel button"),
];

fn hex(s: &str) -> Color {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0xD37918);
    Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn strs(o: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        o.iter().map(|s| SharedString::from(*s)).collect::<Vec<_>>(),
    ))
}

/// Builds the row list category by category; rows added after
/// [`Rows::advanced`] are hidden behind the "Advanced Options" bar.
struct Rows<'a> {
    c: &'a Config,
    v: Vec<(SettingRow, bool)>,
    cat: &'static str,
    adv: bool,
}

impl<'a> Rows<'a> {
    fn cat(&mut self, cat: &'static str) -> &mut Self {
        self.cat = cat;
        self.adv = false;
        self
    }
    fn advanced(&mut self) -> &mut Self {
        self.adv = true;
        self
    }
    fn push(&mut self, id: &str, title: &str, info: &str, kind: &str, value: String) -> &mut Self {
        self.v.push((
            SettingRow {
                id: id.into(),
                category: self.cat.into(),
                title: title.into(),
                info: info.into(),
                kind: kind.into(),
                value: value.into(),
                options: ModelRc::default(),
                swatches: ModelRc::default(),
                labels: ModelRc::default(),
                minimum: 0.0,
                maximum: 1.0,
                step: 1.0,
                enabled: true,
            },
            self.adv,
        ));
        self
    }
    fn last(&mut self) -> &mut SettingRow {
        &mut self.v.last_mut().expect("row").0
    }
    fn header(&mut self, t: &str) -> &mut Self {
        self.push("", t, "", "header", String::new())
    }
    fn toggle(&mut self, id: &str, t: &str, info: &str, on: bool) -> &mut Self {
        self.push(id, t, info, "toggle", on.to_string())
    }
    fn choice(&mut self, id: &str, t: &str, info: &str, val: &str, o: &[&str]) -> &mut Self {
        self.push(id, t, info, "choice", val.into());
        self.last().options = strs(o);
        self
    }
    fn seg(&mut self, id: &str, t: &str, info: &str, val: &str, o: &[&str]) -> &mut Self {
        self.choice(id, t, info, val, o);
        self.last().kind = "segmented".into();
        self
    }
    #[allow(clippy::too_many_arguments)]
    fn slider(
        &mut self,
        id: &str,
        t: &str,
        info: &str,
        val: f64,
        (min, max, step): (f32, f32, f32),
        labels: &[&str],
    ) -> &mut Self {
        self.push(id, t, info, "slider", format!("{val}"));
        let r = self.last();
        r.minimum = min;
        r.maximum = max;
        r.step = step;
        r.labels = strs(labels);
        self
    }
    fn text(&mut self, id: &str, t: &str, info: &str, val: &str, placeholder: &str) -> &mut Self {
        self.push(id, t, info, "text", val.into());
        self.last().options = strs(&[placeholder]);
        self
    }
    fn path(&mut self, id: &str, t: &str, info: &str, val: &str) -> &mut Self {
        self.push(id, t, info, "path", val.into())
    }
    fn action(&mut self, id: &str, t: &str, info: &str, label: &str) -> &mut Self {
        self.push(id, t, info, "action", label.into())
    }
    fn enabled(&mut self, on: bool) -> &mut Self {
        self.last().enabled = on;
        self
    }
    /// A not-yet-implemented setting; its value is kept in `[stubs]`.
    fn stub_value(&self, id: &str, default: &str) -> String {
        self.c
            .stubs
            .get(id)
            .cloned()
            .unwrap_or_else(|| default.into())
    }
    fn stub_toggle(&mut self, id: &str, t: &str, info: &str, default: bool) -> &mut Self {
        let v = self.stub_value(id, &default.to_string());
        self.push(id, t, info, "toggle", v)
    }
    fn stub_choice(&mut self, id: &str, t: &str, info: &str, o: &[&str]) -> &mut Self {
        let v = self.stub_value(id, o[0]);
        self.choice(id, t, info, &v, o)
    }
    fn stub_text(&mut self, id: &str, t: &str, info: &str, default: &str) -> &mut Self {
        let v = self.stub_value(id, default);
        self.text(id, t, info, &v, "")
    }
}

const SOON: &str = " (Not available yet.)";

/// All rows; the bool marks "advanced" rows hidden behind "Advanced Options".
fn all_rows(c: &Config, devices: &[String]) -> Vec<(SettingRow, bool)> {
    let mut r = Rows {
        c,
        v: Vec::new(),
        cat: "General",
        adv: false,
    };
    let s = |k: &str| format!("Stored only.{SOON} {k}");
    let pct = ["-100", "0", "100"];

    // ---------- General ----------
    r.cat("General")
        .header("Appearance")
        .choice("general.language", "Interface language", "Language of menus and dialogs. Only English is available for now.", &c.general.language, &["English"])
        .seg("interface.theme", "Theme", "Dark or light interface colors. System follows the GTK/desktop color scheme when it can be detected.", &cap(&c.interface.theme), &["Dark", "Light", "System"]);
    r.push(
        "interface.accent",
        "Accent color",
        "Highlight color for sliders, toggles and selection.",
        "color",
        c.interface.accent.clone(),
    );
    r.last().options = strs(&ACCENTS);
    r.last().swatches = ModelRc::new(VecModel::from(
        ACCENTS.iter().map(|a| hex(a)).collect::<Vec<_>>(),
    ));
    r.header("Startup")
        .stub_toggle(
            "stub.general.dialogs",
            "Show confirmation and error dialogs",
            &s(""),
            false,
        )
        .stub_toggle(
            "stub.general.updates",
            "Check for updates automatically",
            &s(""),
            false,
        )
        .header("Remote control")
        .toggle(
            "general.remote_control",
            "Allow AI agents and scripts",
            "Open a local control socket for vivi-mcp (MCP server for AI agents). Takes effect after restart.",
            c.general.remote_control,
        )
        .header("Diagnostics")
        .toggle(
            "general.save_log",
            "Save a log",
            "Write mpv's log to a file (mpv log-file).",
            c.general.save_log,
        )
        .path(
            "general.log_file",
            "Log file",
            "Where the log is written.",
            &c.general.log_file,
        )
        .enabled(c.general.save_log)
        .choice(
            "general.log_level",
            "Log details",
            "How much detail the log file gets (mpv msg-level).",
            &c.general.log_level,
            &["Errors only", "Warnings", "Everything"],
        )
        .enabled(c.general.save_log)
        .advanced()
        .stub_toggle("stub.general.devmode", "Developer mode", &s(""), false)
        .action(
            "interface.onboarding",
            "Welcome tour",
            "Show the first-run onboarding again.",
            "Show",
        );

    // ---------- Interface ----------
    r.cat("Interface")
        .header("Playback Controls")
        .choice("interface.profile", "Toolbar profile", "Which buttons appear in the bottom bar. Minimal, Default and Power are built in; Custom is your own layout.", &c.interface.profile, &["Minimal", "Default", "Power", "Custom"])
        .choice("interface.bar_layout", "Control bar layout", "Compact puts transport, seekbar and buttons on one row; Classic keeps the seekbar above the buttons.", &c.interface.bar_layout, &["Compact", "Classic"])
        .toggle("interface.skip_buttons", "Skip buttons", "Show −10 s / +10 s buttons beside play in the compact bar.", c.interface.skip_buttons)
        .toggle("interface.waveform", "Waveform seekbar", "For audio files, draw the waveform on the seekbar (decoded once, cached in ~/.cache/vivi/peaks).", c.interface.waveform)
        .action("interface.customize", "Customize player", "Choose, order and remove bottom-bar buttons.", "Customize…")
        .slider("interface.autohide", "Hide controls after", "Seconds without mouse movement before bars and cursor hide during playback.", c.interface.autohide_secs as f64, (1.0, 10.0, 1.0), &["1 s", "5 s", "10 s"])
        .header("Window")
        .stub_toggle("stub.interface.remember_window", "Remember window size and position", &s(""), true)
        .stub_toggle("stub.interface.ontop", "Keep window above other windows", &s(""), false)
        .stub_choice("stub.interface.fit", "Resize window to fit video", &s(""), &["Never", "Always", "First video only"])
        .advanced()
        .header("Skin")
        .stub_toggle("stub.interface.skin", "Use a custom skin", &s(""), false);

    // ---------- Privacy ----------
    r.cat("Privacy")
        .header("History")
        .toggle(
            "playback.resume",
            "Remember the playback position",
            "Remember the position of every file and continue from it next time (mpv watch-later).",
            c.playback.resume,
        )
        .action(
            "privacy.recent",
            "Clear recent files",
            "Forget the list on the start screen.",
            "Clear",
        )
        .action(
            "privacy.watch_later",
            "Clear resume positions",
            "Delete every saved playback position (watch-later).",
            "Clear",
        )
        .header("Network")
        .stub_toggle(
            "stub.privacy.metadata",
            "Fetch titles and cover art from the internet",
            &s(""),
            false,
        )
        .stub_toggle(
            "stub.privacy.lastfm",
            "Submit played tracks to last.fm",
            &s(""),
            false,
        )
        .stub_text("stub.privacy.lastfm_user", "Last.fm username", &s(""), "");

    // ---------- Controls ----------
    r.cat("Controls")
        .header("Skip Amounts")
        .choice(
            "controls.short_skip",
            "Short skip",
            "Seek step for ← and →.",
            &format!("{} s", c.controls.short_skip),
            &["3 s", "5 s", "10 s", "15 s", "30 s"],
        )
        .choice(
            "controls.long_skip",
            "Long skip",
            "Seek step for Shift + ← and →.",
            &format!("{} s", c.controls.long_skip),
            &["30 s", "60 s", "120 s", "300 s"],
        )
        .choice(
            "audio.volume_step",
            "Volume step",
            "Volume change for ↑ and ↓.",
            &format!("{} %", c.audio.volume_step),
            &["1 %", "2 %", "5 %", "10 %"],
        )
        .header("Keyboard Shortcuts");
    for (t, k) in SHORTCUTS {
        r.push("", t, "", "key", k.into());
    }
    r.header("Mouse")
        .stub_choice(
            "stub.controls.wheel",
            "Scroll wheel",
            &s(""),
            &["Volume", "Seek", "Nothing"],
        )
        .stub_toggle(
            "stub.controls.reverse",
            "Reverse scroll direction",
            &s(""),
            false,
        )
        .advanced()
        .header("Remotes")
        .stub_toggle("stub.controls.web", "Web remote control", &s(""), false);

    // ---------- Playback ----------
    let speed_labels = ["0.25", "0.5", "0.75", "1", "1.25", "1.5", "1.75", "2"];
    r.cat("Playback")
        .header("Startup")
        .slider("playback.speed", "Default playback speed", "Speed applied to every new file (mpv speed).", c.playback.speed, (0.25, 2.0, 0.05), &speed_labels)
        .stub_toggle("stub.playback.autoplay", "Start playing as soon as a file opens", &s(""), true)
        .header("Tracks")
        .choice("playback.quality", "Preferred video quality", "Stream quality picked by yt-dlp for network URLs (mpv ytdl-format).", &c.playback.quality, &["Best available", "2160p", "1080p", "720p", "480p", "Audio only"])
        .header("Order")
        .seg("playback.repeat", "Repeat", "Off, repeat the current file (mpv loop-file) or the whole queue (loop-playlist).", &c.playback.repeat, &["Off", "One", "All"])
        .header("Seeking")
        .toggle("playback.precise_seek", "Seek precisely", "Seek to the exact frame instead of the nearest keyframe (mpv hr-seek). Slower on some files.", c.playback.precise_seek)
        .header("Per-show presets");
    if c.presets.is_empty() {
        r.push(
            "",
            "No presets yet. Save one from the side panel → Subtitles.",
            "",
            "info",
            String::new(),
        );
    }
    for (k, p) in &c.presets {
        let info = format!(
            "sub delay {:+.2}s · audio delay {:+.2}s · sub scale {:.1} · sub pos {} · sub lang {} · audio lang {}",
            p.sub_delay, p.audio_delay, p.sub_scale, p.sub_pos,
            if p.sub_lang.is_empty() { "-" } else { &p.sub_lang },
            if p.audio_lang.is_empty() { "-" } else { &p.audio_lang },
        );
        r.action(&format!("preset.delete:{k}"), k, &info, "Delete");
    }

    // ---------- Video ----------
    r.cat("Video")
        .header("Output")
        .toggle(
            "video.enabled",
            "Enable video",
            "Turn off to play only the audio of video files (mpv vid=no).",
            c.video.enabled,
        )
        .toggle(
            "video.stop_screensaver",
            "Stop the screensaver during playback",
            "mpv stop-screensaver.",
            c.video.stop_screensaver,
        )
        .header("Picture")
        .slider(
            "video.hue",
            "Hue",
            "Default hue (mpv hue).",
            c.video.hue as f64,
            (-100.0, 100.0, 1.0),
            &pct,
        )
        .slider(
            "video.brightness",
            "Brightness",
            "Default brightness (mpv brightness).",
            c.video.brightness as f64,
            (-100.0, 100.0, 1.0),
            &pct,
        )
        .slider(
            "video.contrast",
            "Contrast",
            "Default contrast (mpv contrast).",
            c.video.contrast as f64,
            (-100.0, 100.0, 1.0),
            &pct,
        )
        .slider(
            "video.saturation",
            "Saturation",
            "Default saturation (mpv saturation).",
            c.video.saturation as f64,
            (-100.0, 100.0, 1.0),
            &pct,
        )
        .slider(
            "video.gamma",
            "Gamma",
            "Default gamma (mpv gamma).",
            c.video.gamma as f64,
            (-100.0, 100.0, 1.0),
            &pct,
        )
        .action(
            "video.reset_picture",
            "Reset picture",
            "Set hue, brightness, contrast, saturation and gamma back to 0.",
            "Reset",
        )
        .header("Deinterlacing")
        .choice(
            "video.deinterlace",
            "Deinterlacing",
            "mpv deinterlace: auto only deinterlaces interlaced content.",
            &c.video.deinterlace,
            &["auto", "yes", "no"],
        )
        .header("Framing")
        .choice(
            "video.aspect",
            "Aspect ratio",
            "Force a display aspect ratio (mpv video-aspect-override).",
            &aspect_label(&c.video.aspect),
            &["Automatic", "16:9", "4:3", "21:9", "2.35:1", "1:1"],
        )
        .header("Snapshots")
        .path(
            "video.screenshot_dir",
            "Save snapshots to",
            "Folder for snapshots (mpv screenshot-directory).",
            &c.video.screenshot_dir,
        )
        .seg(
            "video.screenshot_format",
            "Format",
            "mpv screenshot-format.",
            &c.video.screenshot_format,
            &["png", "jpg", "webp"],
        );

    // ---------- Audio ----------
    let mut devs: Vec<&str> = vec!["auto"];
    devs.extend(devices.iter().map(String::as_str).filter(|d| *d != "auto"));
    r.cat("Audio")
        .header("Output")
        .toggle(
            "audio.enabled",
            "Enable audio",
            "Turn off to play video without sound (mpv aid=no).",
            c.audio.enabled,
        )
        .choice(
            "audio.device",
            "Output device",
            "mpv audio-device; auto uses the system default.",
            &c.audio.device,
            &devs,
        )
        .header("Volume")
        .slider(
            "audio.volume_max",
            "Maximum volume",
            "Maximum volume (mpv volume-max); above 100% amplifies and may distort.",
            c.audio.volume_max as f64,
            (100.0, 300.0, 10.0),
            &["100%", "200%", "300%"],
        )
        .slider(
            "audio.startup_volume",
            "Startup volume",
            "Volume when Vivi starts.",
            c.audio.startup_volume as f64,
            (0.0, 100.0, 5.0),
            &["0%", "50%", "100%"],
        )
        .header("Loudness")
        .toggle(
            "audio.normalize",
            "Normalize loudness",
            "Even out quiet and loud passages (lavfi dynaudnorm filter).",
            c.audio.normalize,
        )
        .header("Tracks")
        .text(
            "audio.language",
            "Preferred audio language",
            "Comma-separated language codes in priority order (mpv alang).",
            &c.audio.language,
            "e.g. uk,en",
        )
        .advanced()
        .slider(
            "audio.delay",
            "Default audio delay (s)",
            "mpv audio-delay applied to every file; presets can override it.",
            c.audio.delay,
            (-2.0, 2.0, 0.05),
            &[],
        )
        .stub_toggle("stub.audio.spatial", "Spatial audio", &s(""), false);

    // ---------- Subtitles ----------
    let sub_on = c.subtitles.enabled;
    r.cat("Subtitles")
        .header("Tracks & Languages")
        .toggle("subtitles.enabled", "Enable subtitles", "Show subtitles (mpv sub-visibility).", sub_on)
        .text("subtitles.language", "Preferred language", "Comma-separated language codes in priority order (mpv slang).", &c.subtitles.language, "e.g. uk,en")
        .choice("subtitles.encoding", "Text encoding", "Character set of text subtitles (mpv sub-codepage).", &c.subtitles.encoding, &["auto", "utf8", "cp1251", "cp1252", "latin1", "koi8-u", "shift_jis", "gb18030"])
        .header("Loading Subtitles")
        .choice("subtitles.auto_load", "Load subtitle files automatically", "Load .srt/.ass files next to the video: exact name, fuzzy match or every file in the folder (mpv sub-auto).", &c.subtitles.auto_load, &["no", "exact", "fuzzy", "all"])
        .header("Appearance")
        .text("subtitles.font", "Font", "mpv sub-font (text subtitles only).", &c.subtitles.font, "sans-serif")
        .slider("subtitles.scale", "Font size", "Subtitle scale factor (mpv sub-scale, 1.0 = default).", c.subtitles.scale, (0.5, 3.0, 0.1), &[])
        .toggle("subtitles.bold", "Bold", "mpv sub-bold.", c.subtitles.bold)
        .text("subtitles.color", "Text color", "#RRGGBB (mpv sub-color).", &c.subtitles.color, "#FFFFFF")
        .slider("subtitles.opacity", "Text opacity", "Opacity of the text color.", c.subtitles.opacity as f64, (0.0, 100.0, 5.0), &["0%", "50%", "100%"])
        .choice("subtitles.outline", "Outline thickness", "mpv sub-border-size.", outline_label(c.subtitles.outline), &["None", "Thin", "Normal", "Thick"])
        .text("subtitles.outline_color", "Outline color", "#RRGGBB (mpv sub-border-color).", &c.subtitles.outline_color, "#000000")
        .toggle("subtitles.shadow", "Shadow", "Drop shadow under the text (mpv sub-shadow-offset).", c.subtitles.shadow)
        .advanced()
        .slider("subtitles.position", "Vertical position", "mpv sub-pos: 100 is the bottom edge.", c.subtitles.position as f64, (50.0, 150.0, 1.0), &[]);

    // ---------- Library ----------
    r.cat("Library").header("Library folders");
    if c.library.folders.is_empty() {
        r.push("", "No folders yet.", "", "info", String::new());
    }
    for f in &c.library.folders {
        r.action(
            &format!("library.remove:{f}"),
            f,
            "Scanned for video and music (sub-folders included).",
            "Remove",
        );
    }
    r.action("library.add", "Add a folder", "Pick a folder to include in the library.", "Add folder…")
        .action("library.rescan", "Rescan library", "Look for new, changed and deleted files now. Vivi also rescans on start.", "Rescan")
        .header("Metadata & Artwork")
        .toggle("library.thumbnails", "Thumbnails", "Generate video thumbnails and album covers with ffmpeg (if installed), cached in ~/.cache/vivi/thumbs.", c.library.thumbnails)
        .advanced()
        .action("library.clear_thumbs", "Clear thumbnail cache", "Delete cached thumbnails; they are regenerated when needed.", "Clear")
        .stub_toggle("stub.library.hidden", "Show hidden files", &s(""), false);

    // ---------- Network ----------
    r.cat("Network")
        .header("Buffering")
        .choice("network.buffering", "Buffering", "Size of the read-ahead cache: Low 32 MiB, Normal 150 MiB, High 512 MiB (mpv demuxer-max-bytes).", &c.network.buffering, &["Low", "Normal", "High"])
        .choice("network.cache", "Cache", "mpv cache: auto enables it for network streams only.", &c.network.cache, &["auto", "yes", "no"])
        .slider("network.cache_secs", "Read ahead (s)", "mpv cache-secs.", c.network.cache_secs as f64, (5.0, 600.0, 5.0), &[])
        .header("Proxy")
        .text("network.http_proxy", "HTTP proxy", "mpv http-proxy, e.g. http://host:3128. Empty uses no proxy.", &c.network.http_proxy, "System proxy")
        .header("Security")
        .toggle("network.tls_verify", "Verify certificates against the system trust store", "mpv tls-verify.", c.network.tls_verify)
        .advanced()
        .text("network.user_agent", "User agent", "mpv user-agent for HTTP requests. Empty uses mpv's default.", &c.network.user_agent, "");

    // ---------- Performance ----------
    r.cat("Performance")
        .header("Decoding")
        .choice("playback.hwdec", "Hardware-accelerated decoding", "auto-safe uses the GPU when it is known to work; no forces software decoding (mpv hwdec).", &c.playback.hwdec, &["auto-safe", "auto", "vaapi", "nvdec", "vulkan", "videotoolbox", "d3d11va", "no"])
        .choice("performance.skip_loop_filter", "Skip the H.264 loop filter", "Faster decoding at some quality cost (mpv vd-lavc-skiploopfilter).", &c.performance.skip_loop_filter, &["default", "nonref", "bidir", "nonkey", "all"])
        .slider("performance.threads", "Decoding threads", "mpv vd-lavc-threads; 0 = automatic.", c.performance.threads as f64, (0.0, 16.0, 1.0), &[])
        .header("Smooth Playback")
        .toggle("performance.framedrop", "Drop frames when the system can't keep up", "mpv framedrop=vo.", c.performance.framedrop)
        .advanced()
        .choice("performance.video_sync", "Clock synchronisation", "mpv video-sync.", &c.performance.video_sync, &["audio", "display-resample", "display-vdrop", "desync"]);

    // ---------- Recording ----------
    r.cat("Recording")
        .header("Recording")
        .path(
            "recording.dir",
            "Save recordings to",
            &s(""),
            &c.recording.dir,
        )
        .stub_toggle(
            "stub.recording.remux",
            "Record without re-encoding when possible",
            &s(""),
            true,
        )
        .header("Casting")
        .stub_toggle(
            "stub.casting.discover",
            "Discover cast devices",
            &s(""),
            false,
        )
        .stub_choice(
            "stub.casting.quality",
            "Cast quality",
            &s(""),
            &["Automatic", "High", "Medium", "Low"],
        )
        .header("Streaming")
        .stub_toggle(
            "stub.streaming.local",
            "Play locally while streaming",
            &s(""),
            true,
        )
        .stub_choice(
            "stub.streaming.preset",
            "Encoding preset",
            &s(""),
            &["Medium", "Fast", "Slow"],
        )
        .advanced()
        .stub_choice(
            "stub.streaming.tuning",
            "Tuning",
            &s(""),
            &["None", "Film", "Animation"],
        );

    // ---------- Extensions ----------
    extensions(&mut r);

    r.v
}

thread_local! {
    static REFRESH: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Rebuild the visible settings rows (e.g. after a background plugin job finished).
pub fn refresh_ui() {
    if let Some(f) = REFRESH.with(|r| r.borrow().clone()) {
        f();
    }
}

/// Docs site with the plugin catalog ("Find more extensions").
pub const CATALOG_URL: &str = "https://plugins.viviplayer.app/";

/// Extensions page (Figma "Add-ons & Scripts") with two tabs:
/// * Installed: one card per plugin (built-in, downloaded package, mpv script) with its
///   ON/OFF toggle, followed by the plugin's own settings while it is enabled;
///   packages also get Update / Roll back / Uninstall buttons.
/// * Browse: the downloadable catalog with Install / Update / Uninstall buttons.
fn extensions(r: &mut Rows) {
    use plugins::packages;
    r.cat("Extensions");
    let tab = packages::with_state(|s| s.tab.clone());
    let updates = packages::updates();
    r.seg(
        "ext.tab",
        "Show",
        "Installed plugins or the downloadable catalog.",
        &tab,
        &["Installed", "Browse"],
    );
    let status = packages::with_state(|s| s.status.clone());
    if !status.is_empty() {
        r.push("", &status, "", "info", String::new());
    } else if !updates.is_empty() {
        let names: Vec<&str> = updates.iter().map(|e| e.name.as_str()).collect();
        r.push(
            "",
            &format!("Updates available: {}", names.join(", ")),
            "",
            "info",
            String::new(),
        );
    }
    if tab == "Browse" {
        browse(r);
    } else {
        installed(r, &updates);
    }
}

fn plugin_rows(r: &mut Rows, m: &plugins::Manifest, src: &str, after: impl FnOnce(&mut Rows)) {
    use plugins::SettingKind;
    let on = plugins::is_enabled(r.c, m);
    r.push(
        &plugins::toggle_id(&m.id),
        &m.name,
        &m.description,
        "plugin",
        on.to_string(),
    );
    r.last().options = strs(&[&m.version, &m.author, &m.icon, src]);
    after(r);
    if !on {
        return;
    }
    for st in &m.settings {
        let id = plugins::setting_id(&m.id, &st.key);
        let val = plugins::setting_value(r.c, m, &st.key);
        match &st.kind {
            SettingKind::Toggle => {
                r.toggle(&id, &st.title, &st.info, val == "true");
            }
            SettingKind::Choice(o) => {
                let o: Vec<&str> = o.iter().map(String::as_str).collect();
                r.choice(&id, &st.title, &st.info, &val, &o);
            }
            SettingKind::Text(ph) => {
                r.text(&id, &st.title, &st.info, &val, ph);
            }
            SettingKind::Secret => {
                r.text(&id, &st.title, &st.info, &val, "");
                r.last().kind = "secret".into();
            }
            SettingKind::Action(l) => {
                r.action(&id, &st.title, &st.info, l);
            }
        }
    }
}

fn installed(r: &mut Rows, updates: &[plugins::packages::Entry]) {
    use plugins::{packages, Origin};
    let all = plugins::manifests();
    let pk = packages::installed(&packages::root());
    let busy = packages::with_state(|s| s.busy.clone());
    r.header("Built-in");
    for m in all.iter().filter(|m| m.origin == Origin::Builtin) {
        plugin_rows(r, m, "Built-in", |_| {});
    }
    r.header("Downloaded");
    if pk.is_empty() {
        r.push(
            "",
            "No downloaded plugins yet. Open Browse to install some.",
            "",
            "info",
            String::new(),
        );
    }
    for m in all.iter().filter(|m| m.origin == Origin::Package) {
        let Some(p) = pk.iter().find(|p| p.manifest.id == m.id) else {
            continue;
        };
        let up = updates.iter().find(|e| e.id == m.id);
        plugin_rows(r, m, "Downloaded", |r| {
            let mut title = format!("Version {}", p.manifest.version);
            if let Some(st) = plugins::script_status(&m.id) {
                title += &format!("  ·  {st}");
            }
            let mut labels = Vec::new();
            if let Some(u) = up {
                title += &format!("  ·  {} available", u.version);
                labels.push("Update");
            }
            if let Some(prev) = p.previous.first() {
                title += &format!("  ·  previous {prev}");
                labels.push("Roll back");
            }
            labels.push("Uninstall");
            r.push(
                &format!("pkg.{}", m.id),
                &title,
                &packages::describe_permissions(&p.manifest.permissions),
                "buttons",
                String::new(),
            );
            r.last().labels = strs(&labels);
            r.last().enabled = busy.is_none();
        });
    }
    r.header("mpv Scripts");
    let scripts: Vec<_> = all.iter().filter(|m| m.origin == Origin::Script).collect();
    if scripts.is_empty() {
        r.push(
            "",
            &format!(
                "No scripts yet. Put mpv .lua or .js scripts in {} and press Rescan.",
                config::scripts_dir().display()
            ),
            "",
            "info",
            String::new(),
        );
    }
    for m in scripts {
        plugin_rows(r, m, "mpv script", |r| {
            if let Some(st) = plugins::script_status(&m.id) {
                r.push("", &st, "", "info", String::new());
            }
        });
    }
    let startup = plugins::scripts::load_on_startup(r.c);
    r.toggle(
        "scripts.startup",
        "Load add-ons on startup",
        "Load enabled mpv scripts and downloaded plugins when Vivi starts.",
        startup,
    )
    .action(
        "scripts.folder",
        "Scripts folder",
        &config::scripts_dir().display().to_string(),
        "Open",
    )
    .action(
        "scripts.rescan",
        "Rescan scripts",
        "Pick up scripts added since Vivi started.",
        "Rescan",
    );
}

/// Catalog tab.
fn browse(r: &mut Rows) {
    use plugins::packages::{self, cmp_version, compatible, describe_permissions};
    let (catalog, fetched, fetching, busy, confirm) = packages::with_state(|s| {
        (
            s.catalog.clone(),
            s.fetched,
            s.fetching,
            s.busy.clone(),
            s.confirm.clone(),
        )
    });
    let inst = packages::installed(&packages::root());
    let when = fetched
        .and_then(|t| t.elapsed().ok())
        .map(|e| match e.as_secs() {
            s if s < 120 => "just now".to_string(),
            s if s < 7200 => format!("{} min ago", s / 60),
            s if s < 172_800 => format!("{} h ago", s / 3600),
            s => format!("{} days ago", s / 86400),
        });
    r.header("Plugin catalog")
        .action(
            "ext.check",
            "Check for updates",
            &format!(
                "Catalog: {}. Last checked {}.",
                packages::index_url(),
                when.as_deref().unwrap_or("never")
            ),
            if fetching { "Checking…" } else { "Check now" },
        )
        .enabled(!fetching);
    let auto =
        r.c.plugins
            .get(packages::GLOBAL_ID)
            .and_then(|s| s.settings.get("autocheck"))
            .is_none_or(|v| v == "true");
    r.toggle(
        "ext.autocheck",
        "Check for plugin updates daily",
        "Download the plugin catalog at most once a day when Vivi starts. Nothing else is sent.",
        auto,
    );
    if catalog.is_empty() {
        r.push(
            "",
            if fetching {
                "Loading the catalog…"
            } else {
                "The catalog has not been downloaded yet. Press Check now."
            },
            "",
            "info",
            String::new(),
        );
    }
    r.header("Available plugins");
    for e in &catalog {
        let have = inst.iter().find(|i| i.manifest.id == e.id);
        let (state, labels): (String, Vec<&str>) = if busy.as_deref() == Some(&e.id) {
            ("Downloading…".into(), vec![])
        } else if confirm.as_deref() == Some(&e.id) {
            (
                format!("{}. Plugins run as mpv scripts with your user's rights; install only ones you trust.", describe_permissions(&e.permissions)),
                vec!["Allow & install", "Cancel"],
            )
        } else if !compatible(&e.min_vivi) {
            (format!("Needs Vivi {} or newer", e.min_vivi), vec![])
        } else if !e.available() && have.is_none() {
            (
                format!("Not available for {} yet", packages::platform()),
                vec![],
            )
        } else {
            match have {
                Some(h) if cmp_version(&e.version, &h.manifest.version).is_gt() => (
                    format!("Installed {} · update available", h.manifest.version),
                    vec!["Update", "Uninstall"],
                ),
                Some(h) => (
                    format!("Installed {}", h.manifest.version),
                    vec!["Uninstall"],
                ),
                None => (String::new(), vec!["Install"]),
            }
        };
        // Pending update: show "What's new" from the catalog changelog instead of the description.
        let news = have
            .filter(|h| cmp_version(&e.version, &h.manifest.version).is_gt())
            .map(|h| e.whats_new(&h.manifest.version))
            .filter(|n| !n.is_empty())
            .map(|n| format!("What's new: {n}"));
        r.push(
            &format!("pkg.{}", e.id),
            &e.name,
            news.as_deref().unwrap_or(&e.description),
            "package",
            if confirm.as_deref() == Some(&e.id) {
                "confirm".into()
            } else {
                String::new()
            },
        );
        let mut size = packages::human_size(e.download_size());
        if !e.size_note.is_empty() {
            size = format!("{size} {}", e.size_note);
        }
        let perms = describe_permissions(&e.permissions);
        r.last().options = strs(&[&e.version, &e.author, &e.icon, &perms, &state, &size]);
        r.last().labels = strs(&labels);
        r.last().enabled = busy.is_none();
    }
    r.action(
        "scripts.catalog",
        "Plugin catalog on the web",
        CATALOG_URL,
        "Open catalog",
    );
}

pub(crate) fn open_url(u: &str) {
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(u).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(u).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(u).spawn();
    if let Err(e) = r {
        eprintln!("vivi: open {u}: {e}");
    }
}

/// "10 s" / "5 %" → 10 / 5.
fn lead_num(v: &str) -> u32 {
    v.split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(5)
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn aspect_label(v: &str) -> String {
    if v == "no" || v.is_empty() {
        "Automatic".into()
    } else {
        v.into()
    }
}

fn outline_label(v: f64) -> &'static str {
    match v {
        x if x <= 0.0 => "None",
        x if x < 2.5 => "Thin",
        x if x < 4.5 => "Normal",
        _ => "Thick",
    }
}

fn filtered(
    c: &Config,
    devices: &[String],
    cat: i32,
    search: &str,
    advanced: bool,
) -> Vec<SettingRow> {
    let q = search.trim().to_lowercase();
    let catname = CATEGORIES.get(cat as usize).copied().unwrap_or("General");
    let all = all_rows(c, devices);
    if !q.is_empty() {
        return all
            .into_iter()
            .map(|(r, _)| r)
            .filter(|r| {
                !matches!(r.kind.as_str(), "header" | "info")
                    && (r.title.to_lowercase().contains(&q)
                        || r.info.to_lowercase().contains(&q)
                        || r.category.to_lowercase().contains(&q))
            })
            .collect();
    }
    let rows: Vec<_> = all
        .into_iter()
        .filter(|(r, _)| r.category == catname)
        .collect();
    let mut out: Vec<SettingRow> = rows
        .iter()
        .filter(|(_, a)| !a)
        .map(|(r, _)| r.clone())
        .collect();
    if rows.iter().any(|(_, a)| *a) {
        let mut bar = rows[0].0.clone();
        bar.id = "".into();
        bar.title = "Advanced Options".into();
        bar.info = "".into();
        bar.kind = "advanced".into();
        out.push(bar);
        if advanced {
            out.extend(rows.iter().filter(|(_, a)| *a).map(|(r, _)| r.clone()));
        }
    }
    out
}

/// Arrow-key steps (read by the key handler in main.rs).
pub static SHORT_SKIP: AtomicU32 = AtomicU32::new(5);
pub static LONG_SKIP: AtomicU32 = AtomicU32::new(60);
pub static VOLUME_STEP: AtomicU32 = AtomicU32::new(5);

/// "system" theme: follow GTK_THEME or the GNOME color-scheme when available.
fn system_dark() -> bool {
    if let Ok(t) = std::env::var("GTK_THEME") {
        return !t.to_lowercase().contains("light") || t.to_lowercase().contains("dark");
    }
    #[cfg(target_os = "linux")]
    if let Ok(o) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
    {
        let s = String::from_utf8_lossy(&o.stdout);
        if s.contains("light") {
            return false;
        }
    }
    true
}

pub fn apply_theme(a: &AppWindow, c: &Config) {
    let t = a.global::<Theme>();
    t.set_dark(match c.interface.theme.as_str() {
        "light" => false,
        "system" => system_dark(),
        _ => true,
    });
    t.set_accent(hex(&c.interface.accent));
    a.set_accent_name(c.interface.accent.clone().into());
    AUTOHIDE_SECS.store(c.interface.autohide_secs.max(1), Ordering::Relaxed);
    SHORT_SKIP.store(c.controls.short_skip.max(1), Ordering::Relaxed);
    LONG_SKIP.store(c.controls.long_skip.max(1), Ordering::Relaxed);
    VOLUME_STEP.store(c.audio.volume_step.clamp(1, 50) as u32, Ordering::Relaxed);
}

fn yes(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// `#RRGGBB` + opacity % → mpv `#AARRGGBB`.
fn mpv_color(rgb: &str, opacity: i64) -> String {
    let h = rgb.trim().trim_start_matches('#');
    let h = if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        h
    } else {
        "FFFFFF"
    };
    format!("#{:02X}{h}", (opacity.clamp(0, 100) * 255 / 100) as u8)
}

fn ytdl_format(q: &str) -> String {
    match q.trim_end_matches('p').parse::<u32>() {
        Ok(h) => format!("bestvideo[height<=?{h}]+bestaudio/best[height<=?{h}]/best"),
        Err(_) if q == "Audio only" => "bestaudio/best".into(),
        Err(_) => "bestvideo+bestaudio/best".into(),
    }
}

/// mpv options driven by settings (applied at startup and on change).
pub fn apply_player(p: &Player, c: &Config) {
    // Playback
    p.set("hwdec", &c.playback.hwdec);
    p.set("speed", &c.playback.speed.to_string());
    p.set("resume-playback", yes(c.playback.resume));
    p.set("save-position-on-quit", yes(c.playback.resume));
    p.set(
        "hr-seek",
        if c.playback.precise_seek {
            "yes"
        } else {
            "default"
        },
    );
    p.set("ytdl-format", &ytdl_format(&c.playback.quality));
    let (file, list) = match c.playback.repeat.as_str() {
        "One" => ("inf", "no"),
        "All" => ("no", "inf"),
        _ => ("no", "no"),
    };
    p.set("loop-file", file);
    p.set("loop-playlist", list);
    // Video
    p.set("vid", if c.video.enabled { "auto" } else { "no" });
    p.set("stop-screensaver", yes(c.video.stop_screensaver));
    p.set("hue", &c.video.hue.to_string());
    p.set("brightness", &c.video.brightness.to_string());
    p.set("contrast", &c.video.contrast.to_string());
    p.set("saturation", &c.video.saturation.to_string());
    p.set("gamma", &c.video.gamma.to_string());
    p.set("deinterlace", &c.video.deinterlace);
    p.set("video-aspect-override", &c.video.aspect);
    if !c.video.screenshot_dir.is_empty() {
        p.set("screenshot-directory", &c.video.screenshot_dir);
    }
    p.set("screenshot-format", &c.video.screenshot_format);
    // Audio
    p.set("aid", if c.audio.enabled { "auto" } else { "no" });
    p.set("audio-device", &c.audio.device);
    p.set("alang", &c.audio.language);
    p.set("volume-max", &c.audio.volume_max.to_string());
    p.set("audio-delay", &c.audio.delay.to_string());
    // Labelled filter, so other audio filters are left alone.
    let has_norm = p.get("af").is_some_and(|f| f.contains("vivi-norm"));
    if has_norm && !c.audio.normalize {
        p.cmd("af", &["remove", "@vivi-norm"]);
    }
    if c.audio.normalize && !has_norm {
        p.cmd("af", &["add", "@vivi-norm:lavfi=[dynaudnorm]"]);
    }
    // Subtitles
    p.set("sub-visibility", yes(c.subtitles.enabled));
    p.set("slang", &c.subtitles.language);
    p.set("sub-scale", &c.subtitles.scale.to_string());
    p.set("sub-auto", &c.subtitles.auto_load);
    p.set("sub-codepage", &c.subtitles.encoding);
    if !c.subtitles.font.trim().is_empty() {
        p.set("sub-font", c.subtitles.font.trim());
    }
    p.set("sub-bold", yes(c.subtitles.bold));
    p.set(
        "sub-color",
        &mpv_color(&c.subtitles.color, c.subtitles.opacity),
    );
    p.set("sub-border-size", &c.subtitles.outline.to_string());
    p.set(
        "sub-border-color",
        &mpv_color(&c.subtitles.outline_color, 100),
    );
    p.set(
        "sub-shadow-offset",
        if c.subtitles.shadow { "2" } else { "0" },
    );
    p.set("sub-pos", &c.subtitles.position.to_string());
    // Network
    let bytes = match c.network.buffering.as_str() {
        "Low" => "32MiB",
        "High" => "512MiB",
        _ => "150MiB",
    };
    p.set("demuxer-max-bytes", bytes);
    p.set("cache", &c.network.cache);
    p.set("cache-secs", &c.network.cache_secs.to_string());
    p.set("http-proxy", c.network.http_proxy.trim());
    p.set("tls-verify", yes(c.network.tls_verify));
    if !c.network.user_agent.trim().is_empty() {
        p.set("user-agent", c.network.user_agent.trim());
    }
    // Performance
    p.set("vd-lavc-threads", &c.performance.threads.to_string());
    p.set("vd-lavc-skiploopfilter", &c.performance.skip_loop_filter);
    p.set(
        "framedrop",
        if c.performance.framedrop { "vo" } else { "no" },
    );
    p.set("video-sync", &c.performance.video_sync);
    // Diagnostics
    if c.general.save_log && !c.general.log_file.is_empty() {
        let lvl = match c.general.log_level.as_str() {
            "Warnings" => "all=warn",
            "Everything" => "all=v",
            _ => "all=error",
        };
        p.set("log-file", &c.general.log_file);
        p.set("msg-level", lvl);
    } else {
        p.set("log-file", "");
    }
}

/// Apply a saved preset for the current file's show, if any.
pub fn apply_preset(p: &Player, cfg: &Config, log: bool) -> Option<String> {
    let path = p.get("path")?;
    let key = config::show_key(&path);
    let pr = cfg.presets.get(&key)?;
    p.set("sub-delay", &pr.sub_delay.to_string());
    p.set("audio-delay", &pr.audio_delay.to_string());
    if pr.sub_scale > 0.0 {
        p.set("sub-scale", &pr.sub_scale.to_string());
    }
    p.set("sub-pos", &pr.sub_pos.to_string());
    pick_lang(p, "sub", "sid", &pr.sub_lang);
    pick_lang(p, "audio", "aid", &pr.audio_lang);
    if log {
        eprintln!("preset applied: {key}");
    }
    Some(key)
}

fn pick_lang(p: &Player, kind: &str, prop: &str, lang: &str) {
    if lang.is_empty() {
        return;
    }
    let n: i64 = p.mpv.get_property("track-list/count").unwrap_or(0);
    for i in 0..n {
        let g = |f: &str| p.get(&format!("track-list/{i}/{f}")).unwrap_or_default();
        if g("type") == kind && g("lang") == lang {
            p.set(prop, &g("id"));
            return;
        }
    }
}

fn current_preset(p: &Player) -> Preset {
    let f = |k: &str| p.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let lang = |prop: &str| {
        p.get(&format!("current-tracks/{prop}/lang"))
            .unwrap_or_default()
    };
    Preset {
        sub_delay: f("sub-delay"),
        audio_delay: f("audio-delay"),
        sub_scale: f("sub-scale"),
        sub_pos: f("sub-pos") as i64,
        sub_lang: lang("sub"),
        audio_lang: lang("audio"),
    }
}

fn strings(v: impl IntoIterator<Item = impl Into<SharedString>>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        v.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
}

/// Push the configured bottom-bar layout to the UI.
pub fn apply_toolbar(a: &AppWindow, c: &Config) {
    let t = config::sanitize_toolbar(&c.interface.toolbar);
    let mid = t.iter().position(|s| s == "|").unwrap_or(0);
    a.set_bar_left(strings(t[..mid].iter().cloned()));
    a.set_bar_right(strings(t[mid + 1..].iter().cloned()));
    a.set_profile(c.interface.profile.clone().into());
    a.set_customize_available(strings(
        config::TOOL_ITEMS
            .iter()
            .filter(|i| !t.iter().any(|s| s == *i))
            .copied(),
    ));
    a.set_volume_max(c.audio.volume_max as f32);
    a.set_bar_classic(c.interface.bar_layout == "Classic");
    a.set_bar_skip(c.interface.skip_buttons);
}

/// Mirror mpv's current values into the side-panel sliders.
pub fn sync_panel(a: &AppWindow, p: &Player) {
    let f = |k: &str, d: f64| p.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d) as f32;
    a.set_pv_sub_delay(f("sub-delay", 0.0));
    a.set_pv_sub_scale(f("sub-scale", 1.0));
    a.set_pv_sub_pos(f("sub-pos", 100.0));
    a.set_pv_audio_delay(f("audio-delay", 0.0));
    a.set_pv_boost(f("volume", 100.0));
    a.set_pv_rotate(f("video-rotate", 0.0) as i32);
    a.set_pv_zoom(f("video-zoom", 0.0));
    a.set_pv_panscan(f("panscan", 0.0) * 100.0);
    a.set_pv_brightness(f("brightness", 0.0));
    a.set_pv_contrast(f("contrast", 0.0));
    a.set_pv_saturation(f("saturation", 0.0));
    a.set_pv_gamma(f("gamma", 0.0));
    a.set_pv_speed(f("speed", 1.0));
}

/// Bottom-bar actions that need the player.
fn bar_action(a: &AppWindow, p: &Player, id: &str) {
    match id {
        "back10" => p.seek_rel(-10.0),
        "fwd10" => p.seek_rel(10.0),
        "stop" => {
            p.set("pause", "yes");
            p.seek_abs(0.0);
        }
        "snapshot" => {
            p.run("screenshot");
            a.set_resume_text("Snapshot saved".into());
        }
        "repeat" => {
            let on = !a.get_repeat_on();
            p.set("loop-file", if on { "inf" } else { "no" });
            a.set_repeat_on(on);
        }
        "shuffle" => {
            let on = !a.get_shuffle_on();
            p.run(if on {
                "playlist-shuffle"
            } else {
                "playlist-unshuffle"
            });
            a.set_shuffle_on(on);
        }
        _ => {}
    }
}

fn customize(
    app: &AppWindow,
    player: &Rc<Player>,
    cfg: &Rc<RefCell<Config>>,
    save: impl Fn() + Clone + 'static,
    log: bool,
) {
    let backup: Rc<RefCell<Option<config::Interface>>> = Rc::new(RefCell::new(None));
    let edit = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        move |f: &dyn Fn(&mut Vec<String>)| {
            let Some(a) = w.upgrade() else { return };
            let mut c = cfg.borrow_mut();
            let mut t = config::sanitize_toolbar(&c.interface.toolbar);
            f(&mut t);
            c.interface.profile = config::PROFILES
                .iter()
                .find(|p| config::profile_toolbar(p).as_ref() == Some(&t))
                .map_or("Custom".into(), |p| p.to_string());
            c.interface.toolbar = t;
            apply_toolbar(&a, &c);
        }
    };
    {
        let w = app.as_weak();
        let cfg = cfg.clone();
        let backup = backup.clone();
        app.on_customize_profile(move |p| {
            let Some(a) = w.upgrade() else { return };
            if !a.get_customize_open() {
                *backup.borrow_mut() = Some(cfg.borrow().interface.clone());
                a.set_customize_open(true);
            }
            if let Some(t) = config::profile_toolbar(&p) {
                let mut c = cfg.borrow_mut();
                c.interface.toolbar = t;
                c.interface.profile = p.to_string();
            }
            apply_toolbar(&a, &cfg.borrow());
        });
    }
    {
        let edit = edit.clone();
        app.on_customize_move(move |i, d| {
            edit(&|t| {
                let (i, j) = (i as usize, i as usize as i64 + d as i64);
                if j >= 0 && (j as usize) < t.len() && i < t.len() {
                    t.swap(i, j as usize);
                }
            })
        });
    }
    {
        let edit = edit.clone();
        app.on_customize_remove(move |i| {
            edit(&|t| {
                if t.get(i as usize).is_some_and(|s| s != "|") {
                    t.remove(i as usize);
                }
            })
        });
    }
    app.on_customize_add(move |id| edit(&|t| t.push(id.to_string())));
    {
        let w = app.as_weak();
        let cfg = cfg.clone();
        let p = player.clone();
        app.on_customize_done(move |keep| {
            let Some(a) = w.upgrade() else { return };
            a.set_customize_open(false);
            let old = backup.borrow_mut().take();
            if keep {
                save();
                if log {
                    eprintln!("toolbar saved: {:?}", cfg.borrow().interface.toolbar);
                }
            } else if let Some(old) = old {
                cfg.borrow_mut().interface = old;
            }
            apply_toolbar(&a, &cfg.borrow());
        });
        let w = app.as_weak();
        app.on_bar_action(move |id| {
            if let Some(a) = w.upgrade() {
                bar_action(&a, &p, &id);
            }
        });
    }
}

fn onboarding(app: &AppWindow, cfg: &Rc<RefCell<Config>>, save: impl Fn() + 'static) {
    let keys = [
        ("Play / pause", "Space"),
        ("Seek ±5 s (Shift: 60 s)", "← →"),
        ("Volume", "↑ ↓"),
        ("Fullscreen", "F"),
        ("Open file", "Ctrl O"),
        ("Settings", "Ctrl ,"),
    ];
    app.set_onboarding_actions(strings(keys.iter().map(|k| k.0)));
    app.set_onboarding_keys(strings(keys.iter().map(|k| k.1)));
    app.set_onboarding_open(!cfg.borrow().interface.onboarded);
    let w = app.as_weak();
    let cfg = cfg.clone();
    app.on_onboarding_done(move || {
        if let Some(a) = w.upgrade() {
            a.set_onboarding_open(false);
        }
        cfg.borrow_mut().interface.onboarded = true;
        save();
    });
}

pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>, log: bool) {
    app.set_settings_categories(ModelRc::new(VecModel::from(
        CATEGORIES
            .iter()
            .map(|s| SharedString::from(*s))
            .collect::<Vec<_>>(),
    )));
    app.set_accent_names(strings(ACCENTS));
    app.set_accent_colors(ModelRc::new(VecModel::from(
        ACCENTS.iter().map(|a| hex(a)).collect::<Vec<_>>(),
    )));
    app.set_profiles(strings(config::PROFILES));
    apply_theme(app, &cfg.borrow());
    apply_toolbar(app, &cfg.borrow());
    apply_player(player, &cfg.borrow());
    player.set_volume(cfg.borrow().audio.startup_volume as f64);
    let devices: Rc<Vec<String>> = Rc::new(
        player
            .audio_devices()
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
    );

    let query = Rc::new(RefCell::new((0i32, String::new(), false)));
    let rows: Rc<VecModel<SettingRow>> = Rc::new(VecModel::default());
    app.set_settings_rows(ModelRc::from(rows.clone()));
    // Update rows in place when the set of rows is unchanged, so focused
    // widgets (e.g. the language text fields) are not recreated.
    let refresh = {
        let cfg = cfg.clone();
        let query = query.clone();
        move || {
            let q = query.borrow();
            let new = filtered(&cfg.borrow(), &devices, q.0, &q.1, q.2);
            let same = new.len() == rows.row_count()
                && new
                    .iter()
                    .zip(rows.iter())
                    .all(|(a, b)| a.id == b.id && a.title == b.title && a.kind == b.kind);
            if same {
                for (i, r) in new.into_iter().enumerate() {
                    if rows
                        .row_data(i)
                        .is_some_and(|o| o.value != r.value || o.info != r.info)
                    {
                        rows.set_row_data(i, r);
                    }
                }
            } else {
                rows.set_vec(new);
            }
        }
    };
    let save = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        move || {
            let res = cfg.borrow().save();
            if let Some(a) = w.upgrade() {
                a.set_settings_status(match res {
                    Ok(()) => format!("Saved to {}", config::path().display()).into(),
                    Err(e) => format!("Could not save: {e}").into(),
                });
            }
        }
    };

    {
        let refresh = refresh.clone();
        REFRESH.with(|r| *r.borrow_mut() = Some(Rc::new(refresh)));
    }
    customize(app, player, &cfg, save.clone(), log);
    onboarding(app, &cfg, save.clone());
    {
        let refresh = refresh.clone();
        let query = query.clone();
        app.on_settings_refresh(move |c, s, adv| {
            *query.borrow_mut() = (c, s.to_string(), adv);
            refresh();
        });
    }
    {
        let w = app.as_weak();
        let p = player.clone();
        let cfg = cfg.clone();
        let refresh = refresh.clone();
        let save = save.clone();
        app.on_setting_changed(move |id, value| {
            let Some(a) = w.upgrade() else { return };
            let v = value.as_str();
            let num = v.parse::<f64>().unwrap_or(0.0);
            if log {
                eprintln!("setting {id} = {v}");
            }
            if id.starts_with("plugin.") {
                let msg = plugins::setting_changed(&id, v);
                a.set_settings_status(msg.unwrap_or_default().into());
                refresh();
                return;
            }
            if let Some(pid) = id.strip_prefix("pkg.") {
                plugins::packages::action(pid, v);
                refresh();
                return;
            }
            if let Some(what) = id.strip_prefix("ext.") {
                match what {
                    "tab" => plugins::packages::with_state(|s| {
                        s.tab = v.into();
                        s.status.clear();
                    }),
                    "check" => plugins::packages::check(true),
                    "autocheck" => {
                        cfg.borrow_mut()
                            .plugins
                            .entry(plugins::packages::GLOBAL_ID.into())
                            .or_default()
                            .settings
                            .insert("autocheck".into(), v.into());
                        save();
                    }
                    _ => {}
                }
                refresh();
                return;
            }
            if let Some(what) = id.strip_prefix("scripts.") {
                match what {
                    "startup" => {
                        cfg.borrow_mut()
                            .plugins
                            .entry(plugins::scripts::GLOBAL_ID.into())
                            .or_default()
                            .settings
                            .insert("startup".into(), v.into());
                        save();
                    }
                    "folder" => {
                        let d = config::scripts_dir();
                        let _ = std::fs::create_dir_all(&d);
                        open_url(&d.to_string_lossy());
                    }
                    "rescan" => {
                        let n = plugins::rescan();
                        a.set_settings_status(format!("{n} mpv scripts found").into());
                    }
                    "catalog" => open_url(CATALOG_URL),
                    _ => {}
                }
                refresh();
                return;
            }
            let mut rescan = false;
            {
                let mut c = cfg.borrow_mut();
                match id.as_str() {
                    "interface.theme" => c.interface.theme = v.to_lowercase(),
                    "general.language" => c.general.language = v.into(),
                    "general.save_log" => c.general.save_log = v == "true",
                    "general.remote_control" => c.general.remote_control = v == "true",
                    "general.log_level" => c.general.log_level = v.into(),
                    "general.log_file" | "video.screenshot_dir" | "recording.dir" => {
                        let mut val = v.to_string();
                        if v == "::browse" {
                            let picked = if id.as_str() == "general.log_file" {
                                rfd::FileDialog::new()
                                    .set_title("Log file")
                                    .set_file_name("vivi.log")
                                    .save_file()
                            } else {
                                rfd::FileDialog::new()
                                    .set_title("Choose folder")
                                    .pick_folder()
                            };
                            let Some(d) = picked else { return };
                            val = d.to_string_lossy().into_owned();
                        }
                        match id.as_str() {
                            "general.log_file" => c.general.log_file = val,
                            "video.screenshot_dir" => c.video.screenshot_dir = val,
                            _ => c.recording.dir = val,
                        }
                    }
                    "interface.accent" => c.interface.accent = v.into(),
                    "interface.profile" => {
                        if let Some(t) = config::profile_toolbar(v) {
                            c.interface.toolbar = t;
                        }
                        c.interface.profile = v.into();
                    }
                    "interface.customize" => {
                        drop(c);
                        a.set_settings_open(false);
                        a.invoke_customize_profile("".into());
                        return;
                    }
                    "interface.onboarding" => {
                        a.set_settings_open(false);
                        a.set_onboarding_step(0);
                        a.set_onboarding_open(true);
                        return;
                    }
                    "interface.bar_layout" => c.interface.bar_layout = v.into(),
                    "interface.waveform" => c.interface.waveform = v == "true",
                    "interface.skip_buttons" => c.interface.skip_buttons = v == "true",
                    "interface.autohide" => c.interface.autohide_secs = num as u32,
                    "playback.resume" => c.playback.resume = v == "true",
                    "playback.speed" => c.playback.speed = (num * 100.0).round() / 100.0,
                    "playback.hwdec" => c.playback.hwdec = v.into(),
                    "audio.language" => c.audio.language = v.trim().into(),
                    "audio.volume_max" => c.audio.volume_max = num as i64,
                    "subtitles.language" => c.subtitles.language = v.trim().into(),
                    "subtitles.scale" => c.subtitles.scale = (num * 10.0).round() / 10.0,
                    "subtitles.auto_load" => c.subtitles.auto_load = v.into(),
                    "subtitles.enabled" => c.subtitles.enabled = v == "true",
                    "subtitles.encoding" => c.subtitles.encoding = v.into(),
                    "subtitles.font" => c.subtitles.font = v.into(),
                    "subtitles.bold" => c.subtitles.bold = v == "true",
                    "subtitles.color" => c.subtitles.color = v.trim().into(),
                    "subtitles.opacity" => c.subtitles.opacity = num as i64,
                    "subtitles.outline" => {
                        c.subtitles.outline = match v {
                            "None" => 0.0,
                            "Thin" => 1.5,
                            "Thick" => 5.0,
                            _ => 3.0,
                        }
                    }
                    "subtitles.outline_color" => c.subtitles.outline_color = v.trim().into(),
                    "subtitles.shadow" => c.subtitles.shadow = v == "true",
                    "subtitles.position" => c.subtitles.position = num as i64,
                    "playback.repeat" => c.playback.repeat = v.into(),
                    "playback.precise_seek" => c.playback.precise_seek = v == "true",
                    "playback.quality" => c.playback.quality = v.into(),
                    "controls.short_skip" => c.controls.short_skip = lead_num(v),
                    "controls.long_skip" => c.controls.long_skip = lead_num(v),
                    "audio.volume_step" => c.audio.volume_step = lead_num(v) as i64,
                    "audio.enabled" => c.audio.enabled = v == "true",
                    "audio.device" => c.audio.device = v.into(),
                    "audio.startup_volume" => c.audio.startup_volume = num as i64,
                    "audio.normalize" => c.audio.normalize = v == "true",
                    "audio.delay" => c.audio.delay = (num * 100.0).round() / 100.0,
                    "video.enabled" => c.video.enabled = v == "true",
                    "video.stop_screensaver" => c.video.stop_screensaver = v == "true",
                    "video.hue" => c.video.hue = num as i64,
                    "video.brightness" => c.video.brightness = num as i64,
                    "video.contrast" => c.video.contrast = num as i64,
                    "video.saturation" => c.video.saturation = num as i64,
                    "video.gamma" => c.video.gamma = num as i64,
                    "video.reset_picture" => {
                        let v = &mut c.video;
                        (v.hue, v.brightness, v.contrast, v.saturation, v.gamma) = (0, 0, 0, 0, 0);
                    }
                    "video.deinterlace" => c.video.deinterlace = v.into(),
                    "video.aspect" => {
                        c.video.aspect = if v == "Automatic" {
                            "no".into()
                        } else {
                            v.into()
                        }
                    }
                    "video.screenshot_format" => c.video.screenshot_format = v.into(),
                    "network.buffering" => c.network.buffering = v.into(),
                    "network.cache" => c.network.cache = v.into(),
                    "network.cache_secs" => c.network.cache_secs = num as i64,
                    "network.http_proxy" => c.network.http_proxy = v.trim().into(),
                    "network.tls_verify" => c.network.tls_verify = v == "true",
                    "network.user_agent" => c.network.user_agent = v.into(),
                    "performance.skip_loop_filter" => c.performance.skip_loop_filter = v.into(),
                    "performance.threads" => c.performance.threads = num as i64,
                    "performance.framedrop" => c.performance.framedrop = v == "true",
                    "performance.video_sync" => c.performance.video_sync = v.into(),
                    k if k.starts_with("stub.") => {
                        c.stubs.insert(k.into(), v.into());
                    }
                    "privacy.recent" => {
                        a.invoke_clear_recent();
                        a.set_settings_status("Recent files cleared".into());
                        return;
                    }
                    "privacy.watch_later" => {
                        let _ = std::fs::remove_dir_all(store::watch_later_dir());
                        let _ = std::fs::create_dir_all(store::watch_later_dir());
                        a.set_settings_status("Resume positions cleared".into());
                        return;
                    }
                    "library.add" => {
                        if let Some(d) = rfd::FileDialog::new()
                            .set_title("Add library folder")
                            .pick_folder()
                        {
                            let d = d
                                .to_string_lossy()
                                .replace('\\', "/")
                                .trim_end_matches('/')
                                .to_string();
                            if !c.library.folders.contains(&d) {
                                c.library.folders.push(d);
                            }
                        }
                        rescan = true;
                    }
                    "library.rescan" => rescan = true,
                    "library.thumbnails" => c.library.thumbnails = v == "true",
                    "library.clear_thumbs" => {
                        let _ = std::fs::remove_dir_all(store::cache_dir().join("thumbs"));
                        a.set_settings_status("Thumbnail cache cleared".into());
                        return;
                    }
                    k if k.starts_with("library.remove:") => {
                        let f = &k["library.remove:".len()..];
                        c.library.folders.retain(|x| x != f);
                        rescan = true;
                    }
                    k if k.starts_with("preset.delete:") => {
                        c.presets.remove(&k["preset.delete:".len()..]);
                    }
                    _ => return,
                }
            }
            apply_theme(&a, &cfg.borrow());
            apply_toolbar(&a, &cfg.borrow());
            apply_player(&p, &cfg.borrow());
            sync_panel(&a, &p);
            save();
            refresh();
            if rescan {
                a.invoke_lib_rescan();
            }
        });
    }
    {
        let w = app.as_weak();
        let p = player.clone();
        app.on_save_preset(move || {
            let Some(a) = w.upgrade() else { return };
            let Some(path) = p.get("path") else { return };
            let key = config::show_key(&path);
            cfg.borrow_mut()
                .presets
                .insert(key.clone(), current_preset(&p));
            save();
            refresh();
            a.set_resume_text(format!("Preset saved for “{key}”").into());
            if log {
                eprintln!("preset saved: {key} {:?}", cfg.borrow().presets.get(&key));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_spans_categories() {
        let c = Config::default();
        let r = filtered(&c, &[], 0, "language", false);
        assert!(r.len() >= 2 && r.iter().all(|r| r.kind != "header"));
        // Performance: hwdec is basic, video-sync is behind "Advanced Options".
        let basic = filtered(&c, &[], 10, "", false);
        assert!(basic.iter().any(|r| r.id.as_str() == "playback.hwdec"));
        assert!(basic.iter().any(|r| r.kind.as_str() == "advanced"));
        assert!(basic
            .iter()
            .all(|r| r.id.as_str() != "performance.video_sync"));
        let adv = filtered(&c, &[], 10, "", true);
        assert!(adv
            .iter()
            .any(|r| r.id.as_str() == "performance.video_sync"));
    }
    #[test]
    fn every_category_has_rows() {
        let c = Config::default();
        for i in 0..CATEGORIES.len() {
            assert!(!filtered(&c, &[], i as i32, "", false).is_empty(), "{i}");
        }
        assert_eq!(mpv_color("#FF0000", 50), "#7FFF0000");
        assert_eq!(lead_num("10 s"), 10);
    }
}
