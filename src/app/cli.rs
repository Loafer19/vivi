//! Command line: `--version` / `--help` (handled before any window or mpv starts),
//! files and URLs to play, and `vivi://` links. Unknown `-`/`--` options are reported
//! and ignored instead of being opened as files.

pub(crate) const USAGE: &str = "\
Usage: vivi [OPTIONS] [FILE | URL | vivi://plugin/ID]...

Plays the given files or URLs: the first one starts, the rest are queued.
vivi://plugin/ID opens that plugin in Settings > Extensions > Browse.

Options:
  -h, --help       Print this help and exit
  -V, --version    Print the version and exit
  --               Treat every following argument as a file, even if it starts with -

Environment:
  VIVI_X11=1        Run under XWayland on Wayland (file drag and drop works there)
  VIVI_LOG=1        Log player events to stderr
  VIVI_MPV_DEBUG=1  Verbose mpv log on the terminal
";

/// `vivi 0.11.1 (commit abc1234, built 2026-10-18)`; the commit part is left out when
/// the build had no git information.
pub(crate) fn version() -> String {
    let v = env!("CARGO_PKG_VERSION");
    let (hash, date) = (env!("VIVI_GIT_HASH"), env!("VIVI_BUILD_DATE"));
    if hash.is_empty() || hash == "unknown" {
        format!("vivi {v} (built {date})")
    } else {
        format!("vivi {v} (commit {hash}, built {date})")
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Cli {
    Version,
    Help,
    Run {
        /// Files and URLs, in order.
        files: Vec<String>,
        /// `vivi://` links.
        links: Vec<String>,
        /// Options vivi does not know (reported, then ignored).
        unknown: Vec<String>,
    },
}

pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Cli {
    let (mut files, mut links, mut unknown) = (Vec::new(), Vec::new(), Vec::new());
    let mut only_files = false;
    for a in args {
        if only_files {
            files.push(a);
            continue;
        }
        match a.as_str() {
            "-V" | "--version" => return Cli::Version,
            "-h" | "--help" => return Cli::Help,
            "--" => only_files = true,
            // A lone "-" is mpv's stdin.
            s if s.starts_with('-') && s.len() > 1 => unknown.push(a),
            s if s.starts_with("vivi://") => links.push(a),
            _ => files.push(a),
        }
    }
    Cli::Run {
        files,
        links,
        unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Cli {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn version_help_files_links_and_unknown_flags() {
        assert_eq!(p(&["--version"]), Cli::Version);
        assert_eq!(p(&["a.mkv", "-V"]), Cli::Version);
        assert_eq!(p(&["-h"]), Cli::Help);
        assert_eq!(p(&["--help", "--version"]), Cli::Help);
        let Cli::Run {
            files,
            links,
            unknown,
        } = p(&[
            "a.mkv",
            "--fullscreen",
            "vivi://plugin/lyrics",
            "-",
            "https://x.test/v",
            "--",
            "--odd-name.mp4",
            "-V",
        ])
        else {
            panic!("expected Run");
        };
        assert_eq!(
            files,
            ["a.mkv", "-", "https://x.test/v", "--odd-name.mp4", "-V"]
        );
        assert_eq!(links, ["vivi://plugin/lyrics"]);
        assert_eq!(unknown, ["--fullscreen"]);
        assert!(version().starts_with(&format!("vivi {} (", env!("CARGO_PKG_VERSION"))));
    }
}
