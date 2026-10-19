//! Contribution points: where a plugin shows up outside the Extensions window.
//!
//! A plugin declares a [`Role`] and a list of [`Contribution`]s (point, label, command).
//! The host reads them and fills its surfaces: the sidebar's Sources, the Subtitles /
//! Audio / Video menus, the item and player context menus, the command palette, the
//! player's side drawer, Home widgets, control-bar buttons, the tray and notifications.
//! Choosing an entry runs its `command`: [`super::Plugin::command`] for built-in plugins,
//! `script-message-to <script> <command>` for packages (the key of an `action` setting).
//! An empty command opens the plugin's tab in the Extensions window.
//!
//! Packages declare them in `manifest.toml` (`role = "source"`, `[[contributes]]` with
//! `point`, `label`, `command`, optional `icon`). Packages that predate the fields get
//! the defaults from [`known_defaults`], everything else is a feature with no entries
//! (Extensions window only), so old manifests keep working. Unknown points are ignored.
//! See `docs/plugins-architecture.md` ("Contribution points").

/// Sidebar → Sources: a content browser (`command` opens it).
pub const SIDEBAR_SOURCE: &str = "sidebar.source";
/// Subtitles menu (bottom bar / right-click → Subtitle).
pub const MENU_SUBTITLES: &str = "menu.subtitles";
/// Audio menu.
pub const MENU_AUDIO: &str = "menu.audio";
/// Video menu.
pub const MENU_VIDEO: &str = "menu.video";
/// Right-click on a library / playlist item; `command` gets the item path as its argument.
pub const CONTEXT_ITEM: &str = "context.item";
/// Right-click on the video / player.
pub const CONTEXT_PLAYER: &str = "context.player";
/// Command palette (Ctrl+K).
pub const PALETTE_COMMAND: &str = "palette.command";
/// A panel in the player's right drawer (`command` is sent when it opens).
pub const DRAWER_PANEL: &str = "drawer.panel";
/// A block on Home.
pub const HOME_WIDGET: &str = "home.widget";
/// A button the control-bar customizer offers (off until the user adds it).
pub const CONTROLBAR_BUTTON: &str = "controlbar.button";
/// An entry in the tray menu.
pub const TRAY_ITEM: &str = "tray.item";
/// The plugin may send desktop notifications (`label` describes them).
pub const NOTIFY: &str = "notify";
/// The plugin reports downloads to the shared Downloads panel (`src/downloads.rs`).
pub const DOWNLOADS: &str = "downloads";
/// A button in the Downloads panel's header (e.g. "Record stream").
pub const DOWNLOADS_ACTION: &str = "downloads.action";
/// The Downloads panel's "paste a link" field; `command` gets the URL as its argument.
pub const DOWNLOADS_URL: &str = "downloads.url";

/// Every known point, in documentation order.
pub const POINTS: [&str; 15] = [
    SIDEBAR_SOURCE,
    MENU_SUBTITLES,
    MENU_AUDIO,
    MENU_VIDEO,
    CONTEXT_ITEM,
    CONTEXT_PLAYER,
    PALETTE_COMMAND,
    DRAWER_PANEL,
    HOME_WIDGET,
    CONTROLBAR_BUTTON,
    TRAY_ITEM,
    NOTIFY,
    DOWNLOADS,
    DOWNLOADS_ACTION,
    DOWNLOADS_URL,
];

/// What a plugin is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    /// Provides a browsable catalog (Network Streams, Free Music, YouTube, …).
    Source,
    /// Adds to existing UI (subtitle search, lyrics, tagger, …). The default.
    #[default]
    Feature,
}

impl Role {
    /// `role` value of a manifest ("source" | anything else = feature).
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("source") {
            Role::Source
        } else {
            Role::Feature
        }
    }
}

/// One entry a plugin adds at a contribution point.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct Contribution {
    pub point: String,
    #[serde(default)]
    pub label: String,
    /// Plugin command (built-in) or action setting key (package); "" = open its tab.
    #[serde(default)]
    pub command: String,
    /// Icon key (`ui/core/icons.slint` `Icons.plugin`); "" = the plugin's icon.
    #[serde(default)]
    pub icon: String,
}

impl Contribution {
    pub fn new(point: &str, label: &str, command: &str) -> Self {
        Self {
            point: point.into(),
            label: label.into(),
            command: command.into(),
            icon: String::new(),
        }
    }
}

/// Human description of a point, for the Extensions window ("Where it shows up").
pub fn describe(point: &str) -> &'static str {
    match point {
        SIDEBAR_SOURCE => "Sidebar → Sources",
        MENU_SUBTITLES => "Subtitles menu",
        MENU_AUDIO => "Audio menu",
        MENU_VIDEO => "Video menu",
        CONTEXT_ITEM => "Right-click on a library item",
        CONTEXT_PLAYER => "Right-click on the player",
        PALETTE_COMMAND => "Command palette (Ctrl+K)",
        DRAWER_PANEL => "Player side drawer",
        HOME_WIDGET => "Home",
        CONTROLBAR_BUTTON => "Control bar (Customize Player)",
        TRAY_ITEM => "Tray menu",
        NOTIFY => "Desktop notifications",
        DOWNLOADS => "Downloads panel",
        DOWNLOADS_ACTION => "Downloads panel button",
        DOWNLOADS_URL => "Downloads panel: paste a link",
        _ => "",
    }
}

/// Keep known points, drop the rest (newer manifests on an older Vivi).
pub fn known(v: Vec<Contribution>) -> Vec<Contribution> {
    v.into_iter()
        .filter(|c| {
            let ok = POINTS.contains(&c.point.as_str());
            if !ok {
                eprintln!("vivi: unknown contribution point {:?} ignored", c.point);
            }
            ok
        })
        .collect()
}

/// Entries that moved to another plugin: (old plugin, its command, new plugin, new command).
/// Scene detection moved from Scene Chapters into Chapters & Previews (chapter-thumbnails
/// 1.2.0, `scenes-detect`); the old package keeps its "Detect scene chapters" entries for
/// Vivi with only it installed, and they are hidden while the new one offers the same.
pub const MOVED: [(&str, &str, &str, &str); 1] = [(
    "scene-chapters",
    "detect",
    "chapter-thumbnails",
    "scenes-detect",
)];

/// Indices of `entries` (plugin id, command) at one point to leave out: old entries whose
/// replacement (see [`MOVED`]) is there too.
pub fn superseded(entries: &[(&str, &str)]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, (id, cmd))| {
            MOVED.iter().any(|(oid, ocmd, nid, ncmd)| {
                id == oid && cmd == ocmd && entries.iter().any(|(i, c)| i == nid && c == ncmd)
            })
        })
        .map(|(i, _)| i)
        .collect()
}

/// Role and entries for catalog packages released before manifests had them.
pub fn known_defaults(id: &str) -> (Role, Vec<Contribution>) {
    let c = Contribution::new;
    match id {
        "youtube" => (
            Role::Source,
            vec![
                c(SIDEBAR_SOURCE, "YouTube", ""),
                c(PALETTE_COMMAND, "YouTube: list the playlist URL", "add"),
            ],
        ),
        "offline-downloads" => (
            Role::Feature,
            vec![
                c(
                    PALETTE_COMMAND,
                    "Downloads: add the playing stream",
                    "add-current",
                ),
                c(PALETTE_COMMAND, "Downloads: show the queue", "show"),
                c(CONTEXT_PLAYER, "Download for offline", "add-current"),
                c(TRAY_ITEM, "Show downloads", "show"),
                c(NOTIFY, "Download finished or failed", ""),
            ],
        ),
        "equalizer" => (
            Role::Feature,
            vec![c(MENU_AUDIO, "Show equalizer preset", "show")],
        ),
        "night-mode" => (Role::Feature, vec![]),
        "subtitle-sync" => (
            Role::Feature,
            vec![
                c(MENU_SUBTITLES, "Auto-sync subtitles", "sync"),
                c(PALETTE_COMMAND, "Auto-sync subtitles", "sync"),
            ],
        ),
        "chapter-thumbnails" => (
            Role::Feature,
            vec![c(
                PALETTE_COMMAND,
                "Generate chapter thumbnails",
                "generate",
            )],
        ),
        "scene-chapters" => (
            Role::Feature,
            vec![
                c(MENU_VIDEO, "Detect scene chapters", "detect"),
                c(PALETTE_COMMAND, "Detect scene chapters", "detect"),
            ],
        ),
        "ab-loop" => (
            Role::Feature,
            vec![c(CONTEXT_PLAYER, "Clear A-B loop", "clear")],
        ),
        "cut-export" => (
            Role::Feature,
            vec![
                c(CONTEXT_PLAYER, "Set clip start here", "set-in"),
                c(CONTEXT_PLAYER, "Set clip end here", "set-out"),
                c(PALETTE_COMMAND, "Export clip", "export"),
            ],
        ),
        "playback-stats" => (
            Role::Feature,
            vec![c(PALETTE_COMMAND, "Show playback statistics", "show")],
        ),
        "acoustid" => (
            Role::Feature,
            vec![c(CONTEXT_PLAYER, "Identify with AcoustID", "identify")],
        ),
        "sponsorblock" => (
            Role::Feature,
            vec![c(CONTEXT_PLAYER, "Undo SponsorBlock skip", "undo")],
        ),
        "scrobbler" => (
            Role::Feature,
            vec![c(TRAY_ITEM, "Scrobbler status", "status")],
        ),
        _ => (Role::Feature, vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_entries_hidden_only_with_the_replacement() {
        let both = [
            ("scene-chapters", "detect"),
            ("chapter-thumbnails", "generate"),
            ("chapter-thumbnails", "scenes-detect"),
        ];
        assert_eq!(superseded(&both), vec![0]);
        // Chapter Thumbnails before 1.2 has no scenes-detect: the old entry stays.
        assert!(superseded(&[
            ("scene-chapters", "detect"),
            ("chapter-thumbnails", "generate")
        ])
        .is_empty());
        assert!(superseded(&[("chapter-thumbnails", "scenes-detect")]).is_empty());
    }

    #[test]
    fn points_are_described_and_defaults_known() {
        for p in POINTS {
            assert!(!describe(p).is_empty(), "{p}");
        }
        for id in ["youtube", "offline-downloads", "cut-export", "x"] {
            let (_, v) = known_defaults(id);
            assert_eq!(known(v.clone()), v, "{id}");
        }
        assert_eq!(Role::parse("Source"), Role::Source);
        assert_eq!(Role::parse(""), Role::Feature);
        let v = known(vec![Contribution::new("menu.future", "x", "y")]);
        assert!(v.is_empty());
    }
}
