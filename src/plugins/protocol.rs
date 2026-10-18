//! Wire formats between the UI, plugins and mpv scripts:
//!
//! * [`PluginCommand`]: the Slint `plugin-command(id, name, args)` callback. A UI component
//!   packs several arguments into one string, separated by [`SEP`] (U+001F, the ASCII
//!   unit separator, which never appears in user text); [`split_args`] unpacks them.
//! * [`ScriptMessage`]: the `script-message vivi-* …` broadcasts that mpv scripts
//!   (plugin packages) send to Vivi's mpv client.
//! * [`join`]: lookup / cache keys built from several fields with the same separator.

/// Separator between packed arguments and key fields (U+001F, unit separator).
pub const SEP: char = '\u{1f}';

/// `"a\u{1f}b"` → `["a", "b"]`; an empty string is one empty argument.
pub fn split_args(s: &str) -> Vec<String> {
    s.split(SEP).map(String::from).collect()
}

/// Fields joined with [`SEP`] (the inverse of [`split_args`]).
pub fn join<S: AsRef<str>>(parts: &[S]) -> String {
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push(SEP);
        }
        out.push_str(p.as_ref());
    }
    out
}

/// A UI callback addressed to one plugin: `plugin-command(id, name, packed args)`.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginCommand {
    pub id: String,
    pub name: String,
    pub args: Vec<String>,
}

impl PluginCommand {
    pub fn from_ui(id: &str, name: &str, packed: &str) -> Self {
        PluginCommand {
            id: id.into(),
            name: name.into(),
            args: split_args(packed),
        }
    }
}

/// A `script-message` broadcast from an mpv script that Vivi understands.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptMessage<'a> {
    /// `vivi-preview-frames`: the Chapter Thumbnails package wrote new preview frames.
    PreviewFrames,
    /// `vivi-chapter-thumbs`: new chapter thumbnails.
    ChapterThumbs,
    /// `vivi-range <a> <b> [label]` / `vivi-range clear`: a range drawn on the seekbar
    /// (unparsable ends are -1, i.e. cleared).
    Range { a: f32, b: f32, label: &'a str },
    /// `vivi-fullscreen [toggle|on|off]`: `None` toggles.
    Fullscreen(Option<bool>),
    /// `vivi-setting <id> <key> <value>`: a package saves one of its own settings.
    Setting {
        id: &'a str,
        key: &'a str,
        value: &'a str,
    },
    /// `vivi-pick-folder <id> <key>`: folder dialog for a package setting.
    PickFolder { id: &'a str, key: &'a str },
    /// `vivi-sub-offset <value>` (Subtitle Auto-Sync 1.2+).
    SubOffset(&'a str),
    /// `vivi-tagger-acoustid <path> <json>` (AcoustID package).
    TaggerAcoustid { path: &'a str, json: &'a str },
    /// `vivi-qr <title> <text> [caption]`: show `text` as a QR code.
    Qr {
        title: &'a str,
        text: &'a str,
        caption: &'a str,
    },
    /// `vivi-notify <plugin id> <title> [body]`: a desktop notification from a package
    /// that contributes `notify` (a toast when it does not, or notifications are off).
    Notify {
        id: &'a str,
        title: &'a str,
        body: &'a str,
    },
    /// `vivi-download <plugin id> <job id> <json>`: create / update a job in the shared
    /// Downloads panel (fields of [`crate::downloads::Report`]).
    Download {
        id: &'a str,
        job: &'a str,
        json: &'a str,
    },
    /// `vivi-download-remove <plugin id> <job id>`.
    DownloadRemove { id: &'a str, job: &'a str },
    /// `vivi-downloads-show`: open Library → Downloads.
    DownloadsShow,
    /// `vivi-toast <text> [seconds]` with non-blank text (trimmed).
    Toast(&'a str),
    /// `vivi-dialog <id> <json>`: open / update the shared music dialog (see `host`).
    Dialog { id: &'a str, json: &'a str },
    /// `vivi-dialog-close <id>`.
    DialogClose(&'a str),
    /// `vivi-confirm <id> <key> <json>`: a confirm / warning dialog.
    Confirm {
        id: &'a str,
        key: &'a str,
        json: &'a str,
    },
    /// `vivi-media-meta <id> <json>`: now-playing metadata of queued URLs.
    MediaMeta { id: &'a str, json: &'a str },
    /// `vivi-protect <id> <path>`: owner-only permissions on a file in the package's data folder.
    Protect { id: &'a str, path: &'a str },
    /// `vivi-wipe <id> <path>`: overwrite and delete a file in the package's data folder.
    Wipe { id: &'a str, path: &'a str },
}

impl<'a> ScriptMessage<'a> {
    /// The message in `args` (`args[0]` is the message name), if it is one of Vivi's.
    pub fn parse(args: &'a [String]) -> Option<Self> {
        use ScriptMessage as M;
        let s: Vec<&'a str> = args.iter().map(String::as_str).collect();
        let num = |v: Option<&&str>| v.and_then(|v| v.parse::<f32>().ok()).unwrap_or(-1.0);
        Some(match s.as_slice() {
            ["vivi-preview-frames", ..] => M::PreviewFrames,
            ["vivi-chapter-thumbs", ..] => M::ChapterThumbs,
            ["vivi-range", rest @ ..] => M::Range {
                a: num(rest.first()),
                b: num(rest.get(1)),
                label: rest.get(2).copied().unwrap_or(""),
            },
            ["vivi-fullscreen", rest @ ..] => M::Fullscreen(match rest.first() {
                Some(&"on") => Some(true),
                Some(&"off") => Some(false),
                _ => None,
            }),
            ["vivi-setting", id, key, value] => M::Setting { id, key, value },
            ["vivi-pick-folder", id, key] => M::PickFolder { id, key },
            ["vivi-sub-offset", v] => M::SubOffset(v),
            ["vivi-tagger-acoustid", path, json] => M::TaggerAcoustid { path, json },
            ["vivi-qr", title, text, rest @ ..] => M::Qr {
                title,
                text,
                caption: rest.first().copied().unwrap_or(""),
            },
            ["vivi-notify", id, title, rest @ ..] if !title.trim().is_empty() => M::Notify {
                id,
                title: title.trim(),
                body: rest.first().copied().unwrap_or(""),
            },
            ["vivi-download", id, job, json] => M::Download { id, job, json },
            ["vivi-download-remove", id, job] => M::DownloadRemove { id, job },
            ["vivi-downloads-show", ..] => M::DownloadsShow,
            ["vivi-toast", text, ..] if !text.trim().is_empty() => M::Toast(text.trim()),
            ["vivi-dialog", id, json] => M::Dialog { id, json },
            ["vivi-dialog-close", id] => M::DialogClose(id),
            ["vivi-confirm", id, key, json] => M::Confirm { id, key, json },
            ["vivi-media-meta", id, json] => M::MediaMeta { id, json },
            ["vivi-protect", id, path] => M::Protect { id, path },
            ["vivi-wipe", id, path] => M::Wipe { id, path },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn args_round_trip() {
        assert_eq!(split_args(""), v(&[""]));
        assert_eq!(split_args("a\u{1f}b c"), v(&["a", "b c"]));
        assert_eq!(join(&["a", "b c"]), "a\u{1f}b c");
        assert_eq!(split_args(&join(&["x", "", "y"])), v(&["x", "", "y"]));
        let c = PluginCommand::from_ui("free-music", "filter", "genre\u{1f}jazz");
        assert_eq!((c.id.as_str(), c.name.as_str()), ("free-music", "filter"));
        assert_eq!(c.args, v(&["genre", "jazz"]));
    }

    #[test]
    fn parses_script_messages() {
        use ScriptMessage as M;
        let p = |a: &[&str]| ScriptMessage::parse(&v(a)).map(|m| format!("{m:?}"));
        let is = |a: &[&str], m: M| assert_eq!(p(a), Some(format!("{m:?}")));
        is(&["vivi-preview-frames"], M::PreviewFrames);
        is(
            &["vivi-range", "1.5", "4", "In/Out"],
            M::Range {
                a: 1.5,
                b: 4.0,
                label: "In/Out",
            },
        );
        is(
            &["vivi-range", "clear"],
            M::Range {
                a: -1.0,
                b: -1.0,
                label: "",
            },
        );
        is(&["vivi-fullscreen"], M::Fullscreen(None));
        is(&["vivi-fullscreen", "off"], M::Fullscreen(Some(false)));
        is(
            &["vivi-setting", "p", "k", "v"],
            M::Setting {
                id: "p",
                key: "k",
                value: "v",
            },
        );
        is(
            &["vivi-pick-folder", "p", "k"],
            M::PickFolder { id: "p", key: "k" },
        );
        is(&["vivi-sub-offset", "0.5"], M::SubOffset("0.5"));
        is(
            &["vivi-qr", "T", "x"],
            M::Qr {
                title: "T",
                text: "x",
                caption: "",
            },
        );
        is(&["vivi-toast", " hi ", "3"], M::Toast("hi"));
        is(
            &["vivi-notify", "p", "Done", "a.mp3"],
            M::Notify {
                id: "p",
                title: "Done",
                body: "a.mp3",
            },
        );
        assert_eq!(p(&["vivi-notify", "p", " "]), None);
        is(
            &["vivi-download", "p", "j1", "{}"],
            M::Download {
                id: "p",
                job: "j1",
                json: "{}",
            },
        );
        is(
            &["vivi-download-remove", "p", "j1"],
            M::DownloadRemove { id: "p", job: "j1" },
        );
        is(&["vivi-downloads-show"], M::DownloadsShow);
        assert_eq!(p(&["vivi-toast", "  "]), None);
        assert_eq!(p(&["vivi-setting", "p", "k"]), None);
        assert_eq!(p(&["other", "x"]), None);
        is(
            &["vivi-dialog", "p", "{}"],
            M::Dialog {
                id: "p",
                json: "{}",
            },
        );
        is(&["vivi-dialog-close", "p"], M::DialogClose("p"));
        is(
            &["vivi-confirm", "p", "k", "{}"],
            M::Confirm {
                id: "p",
                key: "k",
                json: "{}",
            },
        );
        is(
            &["vivi-media-meta", "p", "{}"],
            M::MediaMeta {
                id: "p",
                json: "{}",
            },
        );
        is(
            &["vivi-protect", "p", "/x"],
            M::Protect {
                id: "p",
                path: "/x",
            },
        );
        is(
            &["vivi-wipe", "p", "/x"],
            M::Wipe {
                id: "p",
                path: "/x",
            },
        );
        assert_eq!(p(&["vivi-dialog", "p"]), None);
    }
}
