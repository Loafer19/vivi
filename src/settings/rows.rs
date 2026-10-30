//! Settings rows: the row builder, every category's rows and the search filter.

use super::*;

pub(crate) fn hex(s: &str) -> Color {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0xD37918);
    Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Swatches for the subtitle text: white, yellow, light gray, light blue, light green, orange.
const SUB_TEXT_COLORS: [&str; 6] = [
    "#FFFFFF", "#FFE94D", "#C8C8C8", "#8FD3FF", "#A6E38A", "#FFB070",
];
/// Swatches for the subtitle outline: black, dark gray, gray, white, navy, dark red.
const SUB_OUTLINE_COLORS: [&str; 6] = [
    "#000000", "#333333", "#666666", "#FFFFFF", "#1A2A6C", "#7A1C1C",
];

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
                card: "".into(),
                card_end: false,
                nested: false,
            },
            self.adv,
        ));
        self
    }
    pub(crate) fn last(&mut self) -> &mut SettingRow {
        &mut self.v.last_mut().expect("row").0
    }
    /// A section: a card titled `t` holding the rows that follow, until the next
    /// section or the Advanced Options bar (see [`cards`]).
    pub(crate) fn header(&mut self, t: &str) -> &mut Self {
        self.section(t, "", &[])
    }
    /// A section with a subtitle and buttons on the right of its title; a button sends
    /// `(id, "run")` like an action row.
    pub(crate) fn section(&mut self, t: &str, sub: &str, actions: &[(&str, &str)]) -> &mut Self {
        self.push("", t, sub, "header", String::new());
        let r = self.last();
        r.options = crate::util::strings(actions.iter().map(|(id, _)| *id));
        r.labels = crate::util::strings(actions.iter().map(|(_, l)| *l));
        self
    }
    /// The last row only matters while `on` (e.g. *Log file* under *Save a log*): it
    /// is indented under the row it depends on and folded away while `on` is false.
    pub(crate) fn depends(&mut self, on: bool) -> &mut Self {
        let r = self.last();
        r.nested = true;
        r.enabled = on;
        self
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
    /// A row of color swatches; `palette` holds #RRGGBB values. A stored color
    /// outside the palette still applies; no swatch is selected then.
    pub(crate) fn color(
        &mut self,
        id: &str,
        t: &str,
        info: &str,
        val: &str,
        palette: &[&str],
    ) -> &mut Self {
        self.push(id, t, info, "color", val.trim().to_uppercase());
        self.last().options = strs(palette);
        self.last().swatches = ModelRc::new(VecModel::from(
            palette.iter().map(|a| hex(a)).collect::<Vec<_>>(),
        ));
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
    /// A setting kept in `[stubs]` (see [`Config::stub`]).
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
}

/// Scrobbling is the Scrobbler extension's job: Settings → Privacy only points to it.
pub(crate) const SCROBBLER: &str = "scrobbler";

/// State of the Scrobbler extension: (installed, enabled).
fn scrobbler_state(c: &Config) -> (bool, bool) {
    let m = crate::plugins::manifests()
        .into_iter()
        .find(|m| m.id == SCROBBLER);
    let on = m.as_ref().is_some_and(|m| crate::plugins::is_enabled(c, m));
    (m.is_some(), on)
}

/// Button of the Privacy row: Install / Enable / Settings.
pub(crate) fn scrobbler_action(c: &Config) -> &'static str {
    match scrobbler_state(c) {
        (false, _) => "Install",
        (true, false) => "Enable",
        (true, true) => "Settings",
    }
}

fn scrobbler_info(c: &Config) -> String {
    match scrobbler_state(c) {
        (false, _) => "Played tracks are sent by the Scrobbler extension (Last.fm, ListenBrainz). It is not installed: Install opens it in Extensions, where it asks for its permissions first.".into(),
        (true, false) => "Played tracks are sent by the Scrobbler extension (Last.fm, ListenBrainz). It is installed but off.".into(),
        (true, true) => "The Scrobbler extension is on: accounts, filters and the offline queue are in its settings.".into(),
    }
}

/// Settings → Library → Clear image cache: what the cache holds and its size on disk.
fn image_cache_info() -> String {
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    format!(
        "Thumbnails, covers and station logos in ~/.cache/vivi use {:.1} MB (at most {:.0} MB; oldest go first). They are made again when needed.",
        mb(crate::images::disk_usage()),
        mb(crate::images::DISK_BUDGET)
    )
}

/// Settings → Library: the folder list (one row per folder with its file count) or
/// its empty state; *Add folder…* and *Rescan* sit in the card's title.
fn library_folders(r: &mut Rows) {
    let empty = r.c.library.folders.is_empty();
    // The empty state has its own Add folder button.
    let actions: &[(&str, &str)] = if empty {
        &[]
    } else {
        &[("library.add", "Add folder…"), ("library.rescan", "Rescan")]
    };
    r.cat("Library").section(
        "Library folders",
        "Scanned for video and music, sub-folders included. Vivi also rescans on start.",
        actions,
    );
    if empty {
        r.push(
            "library.add",
            "No folders yet. Add the folders that hold your videos and music.",
            "",
            "empty",
            "Add folder".into(),
        );
    }
    for f in &r.c.library.folders {
        let count = match crate::libview::folder_count(f) {
            Some(1) => "1 file".to_string(),
            Some(n) => format!("{n} files"),
            None => String::new(),
        };
        r.push(&format!("library.remove:{f}"), f, "", "folder", count);
    }
}

/// All rows; the bool marks "advanced" rows hidden behind "Advanced Options".
/// Every page is a list of sections (cards, see [`cards`]); placeholders that do
/// nothing yet are not shown (their `[stubs]` values are kept).
pub(crate) fn all_rows(c: &Config, devices: &[String]) -> Vec<(SettingRow, bool)> {
    let mut r = Rows {
        c,
        v: Vec::new(),
        cat: "General",
        adv: false,
    };
    let pct = ["-100", "0", "100"];
    let wayland = crate::winstate::wayland();

    // ---------- General ----------
    r.cat("General")
        .header("Language")
        .choice("general.language", "Interface language", "Language of menus and dialogs. Only English is available for now.", &schema::get(c, "general.language"), &["English"])
        .header("Startup")
        .action("interface.onboarding", "Welcome tour", "Show the first-run introduction again.", "Show")
        .toggle("general.check_updates", "Check for updates automatically", "Ask GitHub for a newer Vivi release when Vivi starts. Nothing else is sent. Settings → About can also check now.", c.general.check_updates)
        .header("Diagnostics")
        .toggle("general.save_log", "Save a log", "Write a log of the player to a file, for bug reports (mpv log-file).", c.general.save_log)
        .path("general.log_file", "Log file", "Where the log is written.", &c.general.log_file)
        .depends(c.general.save_log)
        .choice("general.log_level", "Log details", "How much the log records.", &schema::get(c, "general.log_level"), &["Errors only", "Warnings", "Everything"])
        .depends(c.general.save_log);

    // ---------- Interface ----------
    r.cat("Interface")
        .header("Appearance")
        .seg("interface.theme", "Theme", "Dark or light interface colors. System follows the desktop's color scheme when it can be detected.", &cap(&c.interface.theme), &["Dark", "Light", "System"]);
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
    let (pos, top) = if wayland {
        (
            "Open Vivi at the size it had when it was closed. Wayland compositors choose where windows open.",
            "Wayland does not let apps keep themselves on top: use your compositor's pin or always-on-top rule instead. Works on X11, Windows and macOS.",
        )
    } else {
        (
            "Open Vivi at the size and place it had when it was closed.",
            "Stay in front of other windows. The miniplayer always does.",
        )
    };
    let hides = c.interface.autohide_mode != "never";
    r.seg("interface.bar_icons", "Player bar icons", "Icons on the player's top and bottom bars: two-tone with an accent detail, or single-tone. Other icons stay two-tone; small icons are always single-tone.", &schema::get(c, "interface.bar_icons"), &crate::config::BAR_ICONS)
        .choice("interface.panel_glass", "Panel transparency", "How much of the video shows through the player's top and bottom bars, the library, the side panel and plugin drawers. Solid hides the video behind them.", &schema::get(c, "interface.panel_glass"), &schema::labels(schema::PANEL_GLASS))
        .header("Toolbar")
        .choice("interface.profile", "Toolbar profile", "Which buttons appear in the bottom bar. Minimal, Default and Power are built in; Custom is your own layout.", &c.interface.profile, &["Minimal", "Default", "Power", "Custom"])
        .action("interface.customize", "Customize player", "Choose, order and remove bottom-bar buttons.", "Customize…")
        .header("Auto-hide")
        .choice("interface.autohide_mode", "Hide controls", "When the bars and the mouse pointer get out of the way of the video. Never keeps them on screen.", &schema::get(c, "interface.autohide_mode"), &schema::labels(schema::AUTOHIDE_MODE))
        .slider("interface.autohide", "Delay", "Time without mouse movement before the controls hide.", crate::app::wiring::autohide::delay_secs(c.interface.autohide_secs), (0.5, 30.0, 0.5), &["0.5 s", "15 s", "30 s"])
        .depends(hides)
        .toggle("interface.autohide_top", "Hide the top bar", "The bar with the title, Open, Library and Settings. Pointing at the top edge still shows it.", c.interface.autohide_top)
        .depends(hides)
        .toggle("interface.autohide_bottom", "Hide the bottom bar", "The seek bar and the playback buttons.", c.interface.autohide_bottom)
        .depends(hides)
        .toggle("interface.autohide_cursor", "Hide the mouse pointer", "Over the video, together with the bars.", c.interface.autohide_cursor)
        .depends(hides)
        .toggle("interface.autohide_keep_paused", "Keep shown while paused", "Off hides the controls of a paused video too.", c.interface.autohide_keep_paused)
        .depends(hides)
        .toggle("interface.autohide_keep_panel", "Keep shown while a panel or menu is open", "The side panel, the library, a plugin drawer or a menu.", c.interface.autohide_keep_panel)
        .depends(hides)
        .toggle("interface.autohide_keep_hover", "Keep shown while the pointer is over a bar", "The bars stay while you read the seek bar or a button's tooltip.", c.interface.autohide_keep_hover)
        .depends(hides)
        .toggle("interface.autohide_keep_audio", "Keep shown for audio only", "Music and radio without a video track.", c.interface.autohide_keep_audio)
        .depends(hides)
        .choice("interface.autohide_wake", "Show with the mouse", "What brings hidden controls back. Ignoring small bumps helps with a shaky desk or touchpad.", &schema::get(c, "interface.autohide_wake"), &schema::labels(schema::AUTOHIDE_WAKE))
        .depends(hides)
        .choice("interface.autohide_keys", "Show with the keyboard", "Any key shows the controls for the whole delay. Seek and volume keys can show them for about a second and a half instead, and other keys not at all.", &schema::get(c, "interface.autohide_keys"), &schema::labels(schema::AUTOHIDE_KEYS))
        .depends(hides)
        .seg("interface.autohide_anim", "Animation", "How the bars hide and come back.", &schema::get(c, "interface.autohide_anim"), &schema::labels(schema::AUTOHIDE_ANIM))
        .depends(hides)
        .header("Window")
        .choice("general.close_button", "Close button", "Minimize to tray keeps music and radio playing with an icon in the system tray (click it to bring Vivi back). Without a system tray, closing quits.", &schema::get(c, "general.close_button"), &crate::config::CLOSE_BUTTON)
        .stub_toggle(config::REMEMBER_WINDOW.0, "Remember window size and position", pos, config::REMEMBER_WINDOW.1)
        .stub_toggle(config::ON_TOP.0, "Keep window above other windows", top, config::ON_TOP.1)
        .choice(config::FIT, "Resize window to fit video", "When a video starts, size the window to the video (at most 90 % of the screen). Not while full screen or maximized, and not for windows your window manager tiles.", c.fit(), &config::FIT_CHOICES)
        .header("Hints")
        .toggle("interface.hint_chapter_thumbnails", "Suggest seek-bar frame previews", "Frame previews on the seek bar come from the Chapter Thumbnails extension. Without it, the first hover over the seek bar of a video offers to install it (once per session).", c.interface.hint_chapter_thumbnails);

    // ---------- Privacy ----------
    r.cat("Privacy")
        .header("History")
        .toggle(
            "playback.resume",
            "Remember the playback position",
            "Continue every file where you left off.",
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
            "Forget where you left off in every file.",
            "Clear",
        )
        .action(
            "privacy.bookmarks",
            "Clear bookmarks",
            "Delete every saved bookmark and its saved frame (asks first).",
            "Clear",
        )
        .header("Scrobbling")
        .action(
            "privacy.scrobbler",
            "Submit played tracks to Last.fm / ListenBrainz",
            &scrobbler_info(c),
            scrobbler_action(c),
        );

    // ---------- Controls (#92): Seeking & Steps, Keyboard Shortcuts, Mouse, Remotes ----------
    let speed_steps: Vec<String> = crate::config::SPEED_STEPS
        .iter()
        .map(|s| schema::speed_step_label(*s))
        .collect();
    let speed_steps: Vec<&str> = speed_steps.iter().map(String::as_str).collect();
    r.cat("Seeking & Steps")
        .header("Seeking")
        .choice("controls.short_skip", "Short skip", "Seek step for ← and →.", &format!("{} s", c.controls.short_skip), &["3 s", "5 s", "10 s", "15 s", "30 s"])
        .choice("controls.long_skip", "Long skip", "Seek step for Shift + ← and →.", &format!("{} s", c.controls.long_skip), &["30 s", "60 s", "120 s", "300 s"])
        .toggle("playback.precise_seek", "Seek precisely", "Land on the exact frame instead of the nearest keyframe. Slower on some files.", c.playback.precise_seek)
        .header("Volume and speed")
        .choice("audio.volume_step", "Volume step", "Volume change for ↑ and ↓.", &format!("{} %", c.audio.volume_step), &["1 %", "2 %", "5 %", "10 %"])
        .choice("controls.speed_step", "Speed step", "Speed change of the side panel's speed slider and of the scroll wheel when it is set to Speed (Mouse).", &schema::get(c, "controls.speed_step"), &speed_steps);

    r.cat("Keyboard Shortcuts").header("Player");
    for (t, k) in crate::keymap::rows() {
        r.push("", t, "", "key", k.into());
    }

    let actions: Vec<&str> = crate::mouse::ACTIONS.iter().map(|(_, l)| *l).collect();
    let wheel: Vec<&str> = crate::mouse::WHEEL.iter().map(|(_, l)| *l).collect();
    r.cat("Mouse").header("Buttons on the video");
    for (_, id, title) in crate::mouse::BUTTONS {
        let info = match id {
            "controls.mouse_click" => "A double-click also counts as two clicks.",
            "controls.mouse_back" | "controls.mouse_forward" => {
                "The side buttons of mice that have them."
            }
            _ => "",
        };
        r.choice(id, title, info, &schema::get(c, id), &actions);
    }
    r.header("Scroll wheel")
        .choice("controls.wheel", "Scroll wheel", "What turning the wheel over the video does: volume by the volume step, speed by the speed step, seeking by the short skip (Seeking & Steps).", &schema::get(c, "controls.wheel"), &wheel)
        .toggle("controls.wheel_reverse", "Reverse scroll direction", "Scrolling down raises the volume or speed and seeks forward.", c.controls.wheel_reverse)
        .depends(c.controls.wheel != "none")
        .header("Fixed actions");
    for (t, k) in crate::keymap::MOUSE
        .iter()
        .filter(|(_, k)| !matches!(*k, "Click" | "Double-click"))
    {
        r.push("", t, "", "key", (*k).into());
    }

    // The phone remote is the Remote Control extension: a pointer with Install / Enable /
    // Settings, like the seek-bar hint for Chapter Thumbnails.
    let (rc_info, rc_label) = match remote_control_action(c) {
        Some("Install") => ("Control Vivi from a phone or tablet browser on the same network (QR pairing, PIN). Installs the Remote Control extension.", "Install"),
        Some(_) => ("Control Vivi from a phone or tablet browser on the same network (QR pairing, PIN). The Remote Control extension is installed but off.", "Enable"),
        None => ("Remote Control is on: pair a phone, change the port or the PIN in its settings.", "Settings…"),
    };
    r.cat("Remotes")
        .header("Phone or tablet")
        .action(REMOTE_CONTROL_ROW, "Phone remote control", rc_info, rc_label)
        .header("AI agents and scripts")
        .toggle("general.remote_control", "Allow AI agents and scripts", "Open a local control socket for vivi-mcp (MCP server for AI agents). Off: agents and scripts are refused at once.", schema::get(c, "general.remote_control") == "true");

    // ---------- Playback ----------
    let speed_labels = ["0.25", "0.5", "0.75", "1", "1.25", "1.5", "1.75", "2"];
    r.cat("Playback")
        .header("Startup")
        .slider("playback.speed", "Default playback speed", "Speed of every file when it opens.", c.playback.speed, (0.25, 2.0, 0.05), &speed_labels)
        .stub_toggle(config::AUTOPLAY.0, "Start playing as soon as a file opens", "Off: files open paused, ready to play.", config::AUTOPLAY.1)
        .header("Online video")
        .choice("playback.quality", "Preferred video quality", "Quality picked for online videos (links and streams played with yt-dlp).", &c.playback.quality, &["Best available", "2160p", "1080p", "720p", "480p", "Audio only"])
        .header("Order")
        .seg("playback.repeat", "Repeat", "Off, repeat the current file, or repeat the whole queue.", &c.playback.repeat, &["Off", "One", "All"])
        .header("Sleep timer")
        .slider("playback.sleep_fade", "Fade out before pausing", "The sleep timer (side panel → Playback) lowers the volume over this many seconds, then pauses and restores it. Off pauses at once.", c.playback.sleep_fade as f64, (0.0, 120.0, 5.0), &["Off", "60 s", "120 s"])
        .section("Per-show presets", "Subtitle and audio adjustments saved for a show from the side panel → Subtitles.", &[]);
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
            "Subtitle delay {:+.2} s · audio delay {:+.2} s · subtitle size {:.1} · subtitle position {} · subtitle language {} · audio language {}",
            p.sub_delay, p.audio_delay, p.sub_scale, p.sub_pos,
            if p.sub_lang.is_empty() { "-" } else { &p.sub_lang },
            if p.audio_lang.is_empty() { "-" } else { &p.audio_lang },
        );
        r.action(&format!("preset.delete:{k}"), k, &info, "Delete");
    }

    // ---------- Video ----------
    r.cat("Video")
        .header("Output")
        .toggle("video.enabled", "Enable video", "Off plays only the sound of video files.", c.video.enabled)
        .toggle("video.stop_screensaver", "Stop the screensaver during playback", "Keep the screen on while a video plays.", c.video.stop_screensaver)
        .section("Picture", "Starting values for every video. 0 leaves the picture as it is.", &[("video.reset_picture", "Reset picture")])
        .slider("video.hue", "Hue", "Shifts all colors around the color wheel.", c.video.hue as f64, (-100.0, 100.0, 1.0), &pct)
        .slider("video.brightness", "Brightness", "", c.video.brightness as f64, (-100.0, 100.0, 1.0), &pct)
        .slider("video.contrast", "Contrast", "", c.video.contrast as f64, (-100.0, 100.0, 1.0), &pct)
        .slider("video.saturation", "Saturation", "How strong the colors are; -100 is black and white.", c.video.saturation as f64, (-100.0, 100.0, 1.0), &pct)
        .slider("video.gamma", "Gamma", "Brightens or darkens the mid-tones.", c.video.gamma as f64, (-100.0, 100.0, 1.0), &pct)
        .header("Framing")
        .choice("video.aspect", "Aspect ratio", "Show the picture in this shape. Automatic uses the video's own.", &aspect_label(&c.video.aspect), &["Automatic", "16:9", "4:3", "21:9", "2.35:1", "1:1"])
        .choice("video.deinterlace", "Deinterlacing", "Remove the comb-like lines of interlaced video (TV recordings, DVDs). Automatic only does it for interlaced video.", &schema::get(c, "video.deinterlace"), &schema::labels(schema::AUTO_ALWAYS_OFF))
        .header("Snapshots")
        .path("video.screenshot_dir", "Save snapshots to", "Folder for snapshots of the video.", &c.video.screenshot_dir)
        .seg("video.screenshot_format", "Format", "PNG keeps every pixel; JPG and WebP files are smaller.", &schema::get(c, "video.screenshot_format"), &schema::labels(schema::SNAPSHOT_FORMAT));

    // ---------- Audio ----------
    let mut devs: Vec<&str> = vec![schema::AUDIO_DEVICE[0].1];
    devs.extend(devices.iter().map(String::as_str).filter(|d| *d != "auto"));
    let rg_on = c.audio.replaygain != "off";
    r.cat("Audio")
        .header("Output")
        .toggle("audio.enabled", "Enable audio", "Off plays video without sound.", c.audio.enabled)
        .choice("audio.device", "Output device", "Where the sound plays. System default follows your desktop's choice.", &schema::get(c, "audio.device"), &devs)
        .header("Volume")
        .slider("audio.volume_max", "Maximum volume", "How far the volume goes. Above 100% makes quiet files louder but may distort.", c.audio.volume_max as f64, (100.0, 300.0, 10.0), &["100%", "200%", "300%"])
        .slider("audio.startup_volume", "Startup volume", "Volume when Vivi starts.", c.audio.startup_volume as f64, (0.0, 100.0, 5.0), &["0%", "50%", "100%"])
        .header("Volume leveling")
        .choice("audio.replaygain", "ReplayGain", "Even out the volume between tracks with the ReplayGain tags in the files: per track, or per album so an album keeps its own dynamics. Album uses the track value where a file has no album value.", &schema::get(c, "audio.replaygain"), &config::REPLAYGAIN.map(|(_, l)| l))
        .slider("audio.preamp", "Preamp (dB)", "Added to the ReplayGain value.", c.audio.preamp, (-15.0, 15.0, 0.5), &["−15 dB", "0 dB", "+15 dB"])
        .depends(rg_on)
        .toggle("audio.prevent_clipping", "Prevent clipping", "Lower the gain where ReplayGain would push the peaks of a track past full scale.", c.audio.prevent_clipping)
        .depends(rg_on)
        .slider("audio.untagged_gain", "Files without tags (dB)", "Gain for files that have no ReplayGain tags.", c.audio.untagged_gain, (-15.0, 15.0, 0.5), &["−15 dB", "0 dB", "+15 dB"])
        .depends(rg_on)
        .toggle("audio.normalize", "Normalize loudness", "Make quiet and loud passages more even while playing (dynamic range compression).", c.audio.normalize)
        .header("Between tracks")
        .toggle("audio.gapless", "Gapless playback", "Play music tracks back to back without a gap. Video and streams are not affected.", c.audio.gapless)
        .slider("audio.crossfade_secs", "Crossfade (s)", "Fade the ending track out and the next one in, half of this time each (a short dip, not an overlap). Music only; skipped between tracks of the same album while gapless playback is on.", c.audio.crossfade_secs as f64, (0.0, config::CROSSFADE_MAX as f32, 1.0), &["Off", "6 s", "12 s"])
        .header("Tracks")
        .text("audio.language", "Preferred audio language", "Language codes in order of preference, separated by commas, e.g. uk,en.", &c.audio.language, "e.g. uk,en")
        .advanced()
        .slider("audio.delay", "Default audio delay (s)", "Shift the sound of every file by this much; per-show presets can override it.", c.audio.delay, (-2.0, 2.0, 0.05), &[]);

    // ---------- Subtitles ----------
    let sub_on = c.subtitles.enabled;
    r.cat("Subtitles")
        .header("Tracks and languages")
        .toggle("subtitles.enabled", "Enable subtitles", "Show subtitles when a file has them.", sub_on)
        .text("subtitles.language", "Preferred language", "Language codes in order of preference, separated by commas, e.g. uk,en.", &c.subtitles.language, "e.g. uk,en")
        .choice("subtitles.encoding", "Text encoding", "Character set of text subtitles; Detect automatically suits most files. Change it when letters show up garbled.", &schema::get(c, "subtitles.encoding"), &schema::labels(schema::SUB_ENCODING))
        .header("Loading subtitles")
        .choice("subtitles.auto_load", "Load subtitle files automatically", "Load subtitle files (.srt, .ass, …) found next to the video.", &schema::get(c, "subtitles.auto_load"), &schema::labels(schema::SUB_AUTO))
        .section("Appearance", "Text subtitles only; styled (ASS) subtitles keep their own look.", &[])
        .text("subtitles.font", "Font", "Font family name, e.g. Noto Sans.", &c.subtitles.font, "sans-serif")
        .slider("subtitles.scale", "Font size", "1.0 is the normal size.", c.subtitles.scale, (0.5, 3.0, 0.1), &[])
        .toggle("subtitles.bold", "Bold", "", c.subtitles.bold)
        .color("subtitles.color", "Text color", "Other colors can be set as #RRGGBB in config.toml.", &c.subtitles.color, &SUB_TEXT_COLORS)
        .slider("subtitles.opacity", "Text opacity", "Opacity of the text color.", c.subtitles.opacity as f64, (0.0, 100.0, 5.0), &["0%", "50%", "100%"])
        .choice("subtitles.outline", "Outline thickness", "Border around the letters.", outline_label(c.subtitles.outline), &["None", "Thin", "Normal", "Thick"])
        .color("subtitles.outline_color", "Outline color", "Other colors can be set as #RRGGBB in config.toml.", &c.subtitles.outline_color, &SUB_OUTLINE_COLORS)
        .toggle("subtitles.shadow", "Shadow", "Drop shadow under the text.", c.subtitles.shadow)
        .advanced()
        .slider("subtitles.position", "Vertical position", "100 is the bottom edge of the picture; lower values move subtitles up.", c.subtitles.position as f64, (50.0, 150.0, 1.0), &[]);

    // ---------- Library ----------
    library_folders(&mut r);
    r.header("Metadata and artwork")
        .toggle("library.thumbnails", "Thumbnails", "Make video thumbnails and album covers with ffmpeg (if installed), cached in ~/.cache/vivi/thumbs.", c.library.thumbnails)
        .advanced()
        .action("library.clear_thumbs", "Clear image cache", &image_cache_info(), "Clear")
        .stub_toggle(config::HIDDEN_FILES.0, "Include hidden files", "Also add files whose names start with a dot.", config::HIDDEN_FILES.1);

    // ---------- Network ----------
    r.cat("Network")
        .header("Buffering")
        .choice(
            "network.buffering",
            "Buffering",
            "How much of a stream is loaded ahead: Low 32 MiB, Normal 150 MiB, High 512 MiB.",
            &c.network.buffering,
            &["Low", "Normal", "High"],
        )
        .choice(
            "network.cache",
            "Cache",
            "Keep a read-ahead cache. Automatic uses it for network streams only.",
            &schema::get(c, "network.cache"),
            &schema::labels(schema::AUTO_ALWAYS_OFF),
        )
        .slider(
            "network.cache_secs",
            "Read ahead (s)",
            "Seconds of a stream to load ahead.",
            c.network.cache_secs as f64,
            (5.0, 600.0, 5.0),
            &[],
        )
        .header("Proxy")
        .text(
            "network.http_proxy",
            "HTTP proxy",
            "e.g. http://host:3128. Empty uses no proxy.",
            &c.network.http_proxy,
            "System proxy",
        )
        .header("Security")
        .toggle(
            "network.tls_verify",
            "Verify certificates against the system trust store",
            "Refuse HTTPS streams whose certificate the system does not trust.",
            c.network.tls_verify,
        )
        .advanced()
        .text(
            "network.user_agent",
            "User agent",
            "How Vivi introduces itself to web servers. Empty uses mpv's default.",
            &c.network.user_agent,
            "",
        );

    // ---------- Performance ----------
    r.cat("Performance")
        .header("Decoding")
        .choice("playback.hwdec", "Hardware-accelerated decoding", "Decode video on the graphics card. Automatic (safe) uses it where it is known to work; Off decodes on the processor.", &schema::get(c, "playback.hwdec"), &schema::labels(schema::HWDEC))
        .choice("performance.skip_loop_filter", "Skip the H.264 loop filter", "Faster decoding of H.264 video on slow systems, at some cost in picture quality.", &schema::get(c, "performance.skip_loop_filter"), &schema::labels(schema::LOOP_FILTER))
        .slider("performance.threads", "Decoding threads", "0 picks the number automatically.", c.performance.threads as f64, (0.0, 16.0, 1.0), &[])
        .header("Smooth playback")
        .toggle("performance.framedrop", "Drop frames when the system can't keep up", "Keeps the picture in time with the sound on slow systems.", c.performance.framedrop)
        .advanced()
        .choice("performance.video_sync", "Clock synchronisation", "What keeps the video in time: the sound, or the display's refresh rate.", &schema::get(c, "performance.video_sync"), &schema::labels(schema::VIDEO_SYNC));

    // ---------- Recording ----------
    r.cat("Recording").header("Recording").path(
        "recording.dir",
        "Save recordings to",
        "Where recordings of live streams (R) are saved. Empty: your Videos folder.",
        &c.recording.dir,
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
        // Matches grouped by page, one card each (the page name is the card's title).
        let mut out: Vec<SettingRow> = Vec::new();
        for (r, _) in all {
            let hit = !matches!(r.kind.as_str(), "header" | "info" | "empty")
                && (r.title.to_lowercase().contains(&q)
                    || r.info.to_lowercase().contains(&q)
                    || r.category.to_lowercase().contains(&q));
            if !hit {
                continue;
            }
            if out.last().is_none_or(|l| l.category != r.category) {
                let mut h = r.clone();
                (h.id, h.title, h.info, h.kind) =
                    ("".into(), r.category.clone(), "".into(), "header".into());
                h.options = ModelRc::default();
                h.labels = ModelRc::default();
                out.push(h);
            }
            let mut r = r;
            r.nested = false; // shown on its own, not under its parent
            out.push(r);
        }
        return cards(out);
    }
    let rows: Vec<_> = all
        .into_iter()
        .filter(|(r, _)| r.category == catname && !(r.nested && !r.enabled))
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
        bar.options = ModelRc::default();
        bar.labels = ModelRc::default();
        out.push(bar);
        if advanced {
            out.extend(rows.iter().filter(|(_, a)| *a).map(|(r, _)| r.clone()));
        }
    }
    cards(out)
}

/// Group rows into cards: a header opens one ("head"), the rows after it are its body
/// ("row"; a body without a header starts with "first"), and the last row before the
/// next header, the Advanced Options bar or the end closes it (`card_end`).
/// `ui/settings.slint` (`Row`) draws the card from these.
pub(crate) fn cards(mut rows: Vec<SettingRow>) -> Vec<SettingRow> {
    let mut open = false;
    for i in 0..rows.len() {
        let kind = rows[i].kind.clone();
        let (card, closes) = match kind.as_str() {
            "header" => ("head", true),
            "advanced" | "plugin" | "package" => ("", true),
            _ if open => ("row", false),
            _ => ("first", false),
        };
        if closes && open && i > 0 {
            rows[i - 1].card_end = true;
        }
        rows[i].card = card.into();
        rows[i].card_end = false;
        open = !card.is_empty();
    }
    if open {
        if let Some(l) = rows.last_mut() {
            l.card_end = true;
        }
    }
    rows
}
