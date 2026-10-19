//! Settings rows: the row builder, every category's rows and the search filter.

use super::*;

pub(crate) fn hex(s: &str) -> Color {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0xD37918);
    Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub(crate) fn strs(o: &[&str]) -> ModelRc<SharedString> {
    crate::util::strings(o.iter().copied())
}

/// Builds the row list category by category; rows added after
/// [`Rows::advanced`] are hidden behind the "Advanced Options" bar.
pub(crate) struct Rows<'a> {
    pub(crate) c: &'a Config,
    pub(crate) v: Vec<(SettingRow, bool)>,
    pub(crate) cat: &'static str,
    pub(crate) adv: bool,
}

impl<'a> Rows<'a> {
    pub(crate) fn cat(&mut self, cat: &'static str) -> &mut Self {
        self.cat = cat;
        self.adv = false;
        self
    }
    pub(crate) fn advanced(&mut self) -> &mut Self {
        self.adv = true;
        self
    }
    pub(crate) fn push(
        &mut self,
        id: &str,
        title: &str,
        info: &str,
        kind: &str,
        value: String,
    ) -> &mut Self {
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
    pub(crate) fn last(&mut self) -> &mut SettingRow {
        &mut self.v.last_mut().expect("row").0
    }
    pub(crate) fn header(&mut self, t: &str) -> &mut Self {
        self.push("", t, "", "header", String::new())
    }
    pub(crate) fn toggle(&mut self, id: &str, t: &str, info: &str, on: bool) -> &mut Self {
        self.push(id, t, info, "toggle", on.to_string())
    }
    pub(crate) fn choice(
        &mut self,
        id: &str,
        t: &str,
        info: &str,
        val: &str,
        o: &[&str],
    ) -> &mut Self {
        self.push(id, t, info, "choice", val.into());
        self.last().options = strs(o);
        self
    }
    pub(crate) fn seg(
        &mut self,
        id: &str,
        t: &str,
        info: &str,
        val: &str,
        o: &[&str],
    ) -> &mut Self {
        self.choice(id, t, info, val, o);
        self.last().kind = "segmented".into();
        self
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn slider(
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
    pub(crate) fn text(
        &mut self,
        id: &str,
        t: &str,
        info: &str,
        val: &str,
        placeholder: &str,
    ) -> &mut Self {
        self.push(id, t, info, "text", val.into());
        self.last().options = strs(&[placeholder]);
        self
    }
    pub(crate) fn path(&mut self, id: &str, t: &str, info: &str, val: &str) -> &mut Self {
        self.push(id, t, info, "path", val.into())
    }
    pub(crate) fn action(&mut self, id: &str, t: &str, info: &str, label: &str) -> &mut Self {
        self.push(id, t, info, "action", label.into())
    }
    pub(crate) fn enabled(&mut self, on: bool) -> &mut Self {
        self.last().enabled = on;
        self
    }
    /// A not-yet-implemented setting; its value is kept in `[stubs]`.
    pub(crate) fn stub_value(&self, id: &str, default: &str) -> String {
        self.c
            .stubs
            .get(id)
            .cloned()
            .unwrap_or_else(|| default.into())
    }
    pub(crate) fn stub_toggle(
        &mut self,
        id: &str,
        t: &str,
        info: &str,
        default: bool,
    ) -> &mut Self {
        let v = self.stub_value(id, &default.to_string());
        self.push(id, t, info, "toggle", v)
    }
    pub(crate) fn stub_choice(&mut self, id: &str, t: &str, info: &str, o: &[&str]) -> &mut Self {
        let v = self.stub_value(id, o[0]);
        self.choice(id, t, info, &v, o)
    }
    pub(crate) fn stub_text(&mut self, id: &str, t: &str, info: &str, default: &str) -> &mut Self {
        let v = self.stub_value(id, default);
        self.text(id, t, info, &v, "")
    }
}

pub(crate) const SOON: &str = " (Not available yet.)";

/// All rows; the bool marks "advanced" rows hidden behind "Advanced Options".
/// Settings → Library → Clear image cache: what the cache holds and its size on disk.
fn image_cache_info() -> String {
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    format!(
        "Thumbnails, covers and station logos in ~/.cache/vivi use {:.1} MB (at most {:.0} MB; oldest go first). They are made again when needed.",
        mb(crate::images::disk_usage()),
        mb(crate::images::DISK_BUDGET)
    )
}

pub(crate) fn all_rows(c: &Config, devices: &[String]) -> Vec<(SettingRow, bool)> {
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
        .choice("general.language", "Interface language", "Language of menus and dialogs. Only English is available for now.", &schema::get(c, "general.language"), &["English"])
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
        .header("Window")
        .choice(
            "general.close_button",
            "Close button",
            "Minimize to tray keeps music and radio playing with an icon in the system tray (click it to bring Vivi back). Without a system tray, closing quits.",
            &schema::get(c, "general.close_button"),
            &crate::config::CLOSE_BUTTON,
        )
        .header("Remote control")
        .toggle(
            "general.remote_control",
            "Allow AI agents and scripts",
            "Open a local control socket for vivi-mcp (MCP server for AI agents). Takes effect after restart.",
            schema::get(c, "general.remote_control") == "true",
        )
        .header("Diagnostics")
        .toggle(
            "general.save_log",
            "Save a log",
            "Write mpv's log to a file (mpv log-file).",
            schema::get(c, "general.save_log") == "true",
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
            &schema::get(c, "general.log_level"),
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
        .action("interface.customize", "Customize player", "Choose, order and remove bottom-bar buttons.", "Customize…")
        .slider("interface.autohide", "Hide controls after", "Seconds without mouse movement before bars and cursor hide during playback.", c.interface.autohide_secs as f64, (1.0, 10.0, 1.0), &["1 s", "5 s", "10 s"])
        .toggle("interface.hint_chapter_thumbnails", "Suggest seek-bar frame previews", "Frame previews on the seek bar come from the Chapter Thumbnails extension. Without it, the first hover over the seek bar of a video offers to install it (once per session).", c.interface.hint_chapter_thumbnails)
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
    for (t, k) in crate::keymap::rows() {
        r.push("", t, "", "key", k.into());
    }
    r.header("Mouse");
    for (t, k) in crate::keymap::MOUSE {
        r.push("", t, "", "key", (*k).into());
    }
    r.stub_choice(
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
        .action("library.clear_thumbs", "Clear image cache", &image_cache_info(), "Clear")
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
    extensions_page(&mut r);

    r.v
}

/// "10 s" / "5 %" → 10 / 5.
pub(crate) fn lead_num(v: &str) -> u32 {
    v.split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(5)
}

pub(crate) fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

pub(crate) fn aspect_label(v: &str) -> String {
    if v == "no" || v.is_empty() {
        "Automatic".into()
    } else {
        v.into()
    }
}

pub(crate) fn outline_label(v: f64) -> &'static str {
    match v {
        x if x <= 0.0 => "None",
        x if x < 2.5 => "Thin",
        x if x < 4.5 => "Normal",
        _ => "Thick",
    }
}

pub(crate) fn filtered(
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
