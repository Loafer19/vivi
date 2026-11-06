//! Auto-tagger (#69, includes AcoustID tagging #65): identifies audio files and fixes
//! their tags.
//!
//! 1. **Fingerprint** with Chromaprint: the `fpcalc` tool when it is installed, otherwise
//!    the bundled pure-Rust port (`rusty-chromaprint`, audio decoded with symphonia).
//! 2. **AcoustID** lookup of the fingerprint (needs a free application API key from
//!    <https://acoustid.org/new-application>, a plugin setting) gives MusicBrainz
//!    recording ids. Without a key, or when AcoustID knows nothing, the tagger falls
//!    back to a MusicBrainz search by the current title/artist (or the file name).
//! 3. **MusicBrainz** lookups give title, artist, album, album artist, date, track
//!    number and genre per release; **Cover Art Archive** gives the front cover.
//!    MusicBrainz is called at most once per second with a descriptive User-Agent.
//! 4. A **preview** dialog shows the candidates and a diff (current → new) per field.
//!    Writing goes through lofty; the original tags (and pictures) of a file are backed
//!    up once to `$XDG_DATA_HOME/vivi/tag-backups/` and can be restored.
//!
//! *Tag library folder* scans a folder (default: the first library folder), keeps the
//! best confident match per file and lets the user write them all at once.
//!
//! The AcoustID package (#76) fingerprints with `fpcalc` itself and hands its AcoustID
//! answer to this preview with `script-message vivi-tagger-acoustid <path> <json>`
//! (`{"acoustid": <lookup response>, "pattern": "{artist} - {title}"}`): the recordings
//! are looked up on MusicBrainz here, and each candidate also proposes a new file name
//! from the pattern, shown as a "File name" row in the review and renamed on apply
//! (Undo renames back).
//!
//! **Result cache** ([`cache`]): each file's AcoustID answer is kept by path, size and
//! modification time (at most 300 files, `<data>/vivi/tagger-cache.json`, 0600), so
//! identifying a file again, playing it again or tagging its folder does not fingerprint
//! it again. *Forget cached results* clears it.
//!
//! **Identify untagged songs while playing** (setting `auto-identify`, off by default):
//! when a local song Vivi knows nothing about starts (the shared now-playing track says
//! kind `music`, source `file`: no title / artist tags, not in the library, no package
//! metadata), it is fingerprinted in a background thread at low priority a few seconds
//! after it starts. When AcoustID is at least [`AUTO_MIN_SCORE`] sure, the match (title,
//! artist, album, duration, MusicBrainz id) becomes the track's now-playing metadata, like
//! a package's `vivi-media-meta` (player title, now-playing card, MPRIS, extensions), and a
//! toast offers *Review* to open the tag review. The file is never written automatically.

use super::web;
use super::{contrib, Ctx, Manifest, Plugin, SettingSpec};
use crate::Item;
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
use slint::ComponentHandle;
use slint::{ModelRc, VecModel};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub const ID: &str = "auto-tagger";
/// Seconds of audio fingerprinted (same as `fpcalc`'s default).
const FP_SECONDS: u32 = 120;
/// Lowest AcoustID score (0–100) shown as now playing by the automatic identification
/// (the AcoustID package's 0.7).
pub const AUTO_MIN_SCORE: u32 = 70;
/// How long a song plays before it is identified automatically (skipping through a
/// folder does not fingerprint every file).
const AUTO_DELAY: std::time::Duration = std::time::Duration::from_secs(3);
/// Toast button id that opens the review of the song identified last
/// (`crate::plugins::packages::open_replacement`).
pub const REVIEW_ACTION: &str = "builtin:auto-tagger/review";

mod cache;
mod files;
mod providers;
mod review;
pub use cache::{forget as forget_cached, Match};
pub use files::*;
pub use providers::*;
pub use review::*;
mod jobs;
mod plugin;
pub use jobs::*;

enum Msg {
    Status(String),
    Rows(Vec<Row>, String),
    Row(Box<Row>),
    Done(String),
    Reviews(Vec<Review>),
    Applied(Vec<FileResult>),
}

#[derive(Default)]
pub struct AutoTagger {
    rows: Vec<Row>,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    batch: bool,
    reviews: Vec<Review>,
    /// Backup folder of the last apply and the files it changed (Undo).
    session: Option<(PathBuf, Vec<PathBuf>)>,
    /// Per-file results of the last apply (summary, per-file Undo).
    results: Vec<FileResult>,
    /// Automatic identification of untagged songs.
    auto: Auto,
    /// Open the review of the best candidate as soon as the candidates arrive (the
    /// *Review* button of an automatic identification).
    straight_to_review: bool,
}

/// A running automatic lookup: its path, answer channel and cancel flag.
type AutoJob = (
    String,
    Receiver<Result<Option<Match>, String>>,
    Arc<AtomicBool>,
);

/// State of the automatic identification (see the module docs).
#[derive(Default)]
struct Auto {
    /// `nowplaying::current_id` last looked at.
    seen: i64,
    /// Untagged song waiting for [`AUTO_DELAY`] and when it was seen first.
    pending: Option<(String, std::time::Instant)>,
    /// Paths identified (or tried) this run.
    tried: std::collections::HashSet<String>,
    /// The running lookup: its path, answer and cancel flag.
    job: Option<AutoJob>,
    /// The last song identified (path, match), for *Review*.
    last: Option<(String, Match)>,
    /// Songs identified this run, so playing one again gets its title back (the
    /// now-playing metadata stays, mpv's file-local title does not).
    shown: std::collections::HashMap<String, Match>,
    /// "No key" was said once this run.
    warned: bool,
}

fn cover_cache() -> PathBuf {
    crate::store::cache_dir().join("tagger-covers")
}

/// Show image bytes in Slint (through a small cache file; femtovg loads from paths).
fn slint_image(data: &[u8]) -> slint::Image {
    let dir = cover_cache();
    let _ = std::fs::create_dir_all(&dir);
    let ext = if data.starts_with(b"\x89PNG") {
        "png"
    } else {
        "jpg"
    };
    let f = dir.join(format!("{:x}.{ext}", md5::compute(data)));
    if !f.exists() {
        let _ = std::fs::write(&f, data);
    }
    slint::Image::load_from_path(&f).unwrap_or_default()
}

impl AutoTagger {
    /// A key from the plugin settings, else the environment, else (AcoustID) the build-time
    /// default (never logged).
    fn secret(cx: &Ctx, key: &str, env: &[&str]) -> String {
        let v = cx.get(key);
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
        env.iter()
            .find_map(|e| std::env::var(e).ok().filter(|v| !v.trim().is_empty()))
            .or_else(|| {
                // AcoustID application key baked in at build time (CI variable, like the
                // Jamendo client id); an application key, not a user secret.
                (key == "acoustid-key")
                    .then(|| option_env!("VIVI_ACOUSTID_KEY").map(String::from))
                    .flatten()
            })
            .unwrap_or_default()
    }

    /// The rename pattern, empty when "Suggest file names" is off.
    fn pattern(cx: &Ctx) -> String {
        if cx.get("rename") == "false" {
            return String::new();
        }
        let p = cx.get("rename-pattern");
        if p.trim().is_empty() {
            "{artist} - {title}".into()
        } else {
            p
        }
    }

    fn ui(cx: &Ctx, status: &str, busy: bool) {
        cx.app
            .global::<crate::MusicDialogState>()
            .set_status(status.into());
        cx.app.global::<crate::MusicDialogState>().set_busy(busy);
    }

    fn open(&mut self, cx: &Ctx, target: String) {
        let a = cx.app;
        a.global::<crate::MusicDialogState>().set_owner(ID.into());
        a.global::<crate::MusicDialogState>()
            .set_caps(crate::MdCaps::default());
        a.global::<crate::MusicDialogState>()
            .set_title("Auto-tagger".into());
        a.global::<crate::MusicDialogState>()
            .set_placeholder("File or folder to tag".into());
        a.global::<crate::MusicDialogState>()
            .set_search_label("Identify".into());
        a.global::<crate::MusicDialogState>()
            .set_primary("Review changes…".into());
        a.global::<crate::MusicDialogState>()
            .set_secondary("Review all files…".into());
        a.global::<crate::MusicDialogState>()
            .set_sources(ModelRc::new(VecModel::from(
                Vec::<slint::SharedString>::new(),
            )));
        a.global::<crate::MusicDialogState>()
            .set_query(target.into());
        a.global::<crate::MusicDialogState>().set_detail("".into());
        crate::streams::open_dialog(a);
        self.rows.clear();
        self.show_rows(cx, None);
        self.start(cx);
    }

    fn show_rows(&self, cx: &Ctx, selected: Option<usize>) {
        let items: Vec<Item> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let t = &r.cand.tags;
                let key = if self.batch {
                    format!(
                        "{}  →  {} — {}",
                        r.path.file_name().unwrap_or_default().to_string_lossy(),
                        t.artist,
                        t.title
                    )
                } else {
                    format!("{} — {}", t.artist, t.title)
                };
                Item {
                    key: key.into(),
                    label: format!(
                        "{}{}  ·  {}  ·  {}%",
                        t.album,
                        if t.date.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", &t.date[..t.date.len().min(4)])
                        },
                        r.cand.source,
                        r.cand.score
                    )
                    .into(),
                    selected: Some(i) == selected,
                }
            })
            .collect();
        cx.app
            .global::<crate::MusicDialogState>()
            .set_results(ModelRc::new(VecModel::from(items)));
    }

    fn detail(&self, i: usize) -> String {
        let Some(r) = self.rows.get(i) else {
            return String::new();
        };
        let d = diff(&r.old, &r.cand.tags);
        let mut s = format!("{}\n\n", r.path.display());
        if d.is_empty() {
            s += "Tags already match.\n";
        }
        for (f, o, n) in d {
            let o = if o.is_empty() { "—".to_string() } else { o };
            s += &format!("{f}:  {o}  →  {n}\n");
        }
        if let Some(n) = &r.rename {
            s += &format!(
                "File name:  {}  →  {n}\n",
                r.path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        s += "Cover:  front cover from the Cover Art Archive, if there is one\n";
        s
    }

    fn render_review(&self, cx: &Ctx) {
        use crate::{TagCover, TagField, TagFile};
        let files: Vec<TagFile> = self
            .reviews
            .iter()
            .map(|r| {
                let mut fields: Vec<TagField> = review_fields(&r.old, &r.cand.tags)
                    .into_iter()
                    .zip(&r.fields)
                    .enumerate()
                    .map(|(j, ((name, o, n), on))| {
                        let changed = field_changes(&o, &n);
                        let (o, n) = if name == "Lyrics" {
                            (lyrics_summary(&o), lyrics_summary(&n))
                        } else {
                            (o, n)
                        };
                        TagField {
                            name: name.into(),
                            changed,
                            old: o.into(),
                            new: n.into(),
                            checked: *on,
                            source: super::tagsources::source_of(&r.cand, name).into(),
                            alts: r.alts_of(j) as i32,
                        }
                    })
                    .collect();
                // Rename proposal: the row after the tag fields.
                if let Some(n) = &r.rename {
                    fields.push(TagField {
                        name: "File name".into(),
                        changed: true,
                        old: r
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .as_ref()
                            .into(),
                        new: n.as_str().into(),
                        checked: r.rename_on,
                        source: "Your pattern".into(),
                        alts: 0,
                    });
                }
                let covers: Vec<TagCover> = r
                    .covers
                    .iter()
                    .map(|c| TagCover {
                        img: slint_image(&c.data),
                        label: c.label.as_str().into(),
                        info: image_info(&c.data).into(),
                    })
                    .collect();
                TagFile {
                    name: r
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .as_ref()
                        .into(),
                    source: format!(
                        "{} · {}%",
                        if r.cand.source.starts_with("AcoustID") {
                            "AcoustID + MusicBrainz"
                        } else {
                            "MusicBrainz search"
                        },
                        r.cand.confidence()
                    )
                    .into(),
                    confidence: r.cand.confidence() as i32,
                    note: r.note.as_str().into(),
                    checked: r.on,
                    fields: ModelRc::new(VecModel::from(fields)),
                    has_old_cover: r.old_cover.is_some(),
                    old_cover: r.old_cover.as_deref().map(slint_image).unwrap_or_default(),
                    old_cover_info: r
                        .old_cover
                        .as_deref()
                        .map(image_info)
                        .unwrap_or_default()
                        .into(),
                    covers: ModelRc::new(VecModel::from(covers)),
                    cover_pick: r.cover.map(|i| i as i32).unwrap_or(-1),
                }
            })
            .collect();
        cx.app
            .global::<crate::TagReview>()
            .set_files(ModelRc::new(VecModel::from(files)));
        let on: Vec<&Review> = self.reviews.iter().filter(|r| r.on).collect();
        let n: usize = on.iter().map(|r| r.changes()).sum();
        cx.app.global::<crate::TagReview>().set_status(
            format!(
                "{} of {} files selected · {n} changes · ⇄ switches to another source's value · 🔍 enlarges a cover",
                on.len(),
                self.reviews.len()
            )
            .into(),
        );
    }

    /// Build the review for the given rows (one per file) in the background:
    /// reads the current cover and downloads the cover candidates.
    fn review(&mut self, cx: &Ctx, which: Vec<usize>) {
        let picks: Vec<(Row, Vec<Candidate>)> = which
            .iter()
            .filter_map(|&i| self.rows.get(i).cloned())
            .map(|r| {
                let others = self
                    .rows
                    .iter()
                    .filter(|o| o.path == r.path)
                    .map(|o| o.cand.clone())
                    .collect();
                (r, others)
            })
            .collect();
        if picks.is_empty() {
            Self::ui(cx, "Select a match first", false);
            return;
        }
        let embed = cx.flag("cover");
        let opts = super::tagsources::Opts {
            deezer: cx.flag("deezer"),
            itunes: cx.flag("itunes"),
            lastfm_key: Self::secret(cx, "lastfm-key", &["VIVI_LASTFM_KEY", "LASTFM_API_KEY"]),
            discogs_token: Self::secret(
                cx,
                "discogs-token",
                &["VIVI_DISCOGS_TOKEN", "DISCOGS_TOKEN"],
            ),
            lyrics: cx.flag("lyrics"),
        };
        let mut asked: Vec<&str> = vec!["MusicBrainz"];
        if opts.deezer {
            asked.push("Deezer");
        }
        if opts.itunes {
            asked.push("iTunes");
        }
        if !opts.lastfm_key.is_empty() {
            asked.push("Last.fm");
        }
        if !opts.discogs_token.is_empty() {
            asked.push("Discogs");
        }
        let asked = format!(
            "Sources: {}{}",
            asked.join(", "),
            if opts.lyrics {
                " · lyrics: LRCLIB"
            } else {
                " · lyrics embedding off"
            }
        );
        let (tx, rx) = channel();
        self.rx = Some(rx);
        Self::ui(cx, "Asking more sources and fetching cover art…", true);
        std::thread::spawn(move || {
            let total = picks.len();
            let v = picks
                .into_iter()
                .enumerate()
                .map(|(i, (mut r, others))| {
                    let _ = tx.send(Msg::Status(format!(
                        "Deezer / iTunes / Last.fm / Discogs / lyrics {}/{total}…",
                        i + 1
                    )));
                    let dur = read_tags(&r.path).map(|(_, d)| d).unwrap_or(0.0);
                    for n in super::tagsources::enrich(&mut r.cand, &opts, dur) {
                        eprintln!("vivi: tagger source: {n}");
                    }
                    let _ = tx.send(Msg::Status(format!("Fetching covers {}/{total}…", i + 1)));
                    let covers = cover_candidates(&r.cand, &others, 6);
                    let mut rv =
                        Review::new(r.path, r.old, r.cand, covers, embed).with_rename(r.rename);
                    rv.note = [r.note.as_str(), asked.as_str()]
                        .iter()
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    rv
                })
                .collect();
            let _ = tx.send(Msg::Reviews(v));
        });
    }

    fn apply_reviews(&mut self, cx: &Ctx) {
        if !self.reviews.iter().any(|r| r.on && r.changes() > 0) {
            cx.app
                .global::<crate::TagReview>()
                .set_status("Nothing selected to change".into());
            return;
        }
        let all = self.reviews.clone();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let session = backup_dir().join(format!("session-{stamp}"));
        let (tx, rx) = channel();
        self.rx = Some(rx);
        cx.app.global::<crate::TagReview>().set_busy(true);
        cx.app
            .global::<crate::TagReview>()
            .set_status("Writing tags…".into());
        let dir = session.clone();
        self.session = Some((session, Vec::new()));
        std::thread::spawn(move || {
            let total = all.len();
            let results = all
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let _ = tx.send(Msg::Status(format!("Writing tags {}/{total}…", i + 1)));
                    apply_one(r, &backup_dir(), &dir)
                })
                .collect();
            let _ = tx.send(Msg::Applied(results));
        });
    }

    /// Show the summary of [`Self::results`].
    fn render_summary(&self, cx: &Ctx) {
        use crate::TagResult;
        let (tagged, renamed, skipped, failed) = result_counts(&self.results);
        let undone = self.results.iter().filter(|r| r.status == "undone").count();
        let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        let head = if tagged == 0 && undone > 0 {
            "Changes undone".to_string()
        } else if tagged == 0 {
            "Nothing was written".to_string()
        } else {
            format!("{} tagged", plural(tagged, "file"))
        };
        let mut sub = Vec::new();
        if renamed > 0 {
            sub.push(format!("{renamed} renamed"));
        }
        if skipped > 0 {
            sub.push(format!("{skipped} skipped"));
        }
        if failed > 0 {
            sub.push(format!("{failed} failed"));
        }
        if undone > 0 {
            sub.push(format!("{undone} undone"));
        }
        cx.app
            .global::<crate::TagReview>()
            .set_summary(head.as_str().into());
        cx.app
            .global::<crate::TagReview>()
            .set_summary_sub(sub.join(" · ").into());
        cx.app
            .global::<crate::TagReview>()
            .set_summary_ok(failed == 0 && tagged > 0);
        let items: Vec<TagResult> = self
            .results
            .iter()
            .map(|r| TagResult {
                name: r
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref()
                    .into(),
                status: tag_status(r.status),
                detail: r.detail.as_str().into(),
                changes: ModelRc::new(VecModel::from(
                    r.changes
                        .iter()
                        .map(|c| slint::SharedString::from(c.as_str()))
                        .collect::<Vec<_>>(),
                )),
                can_undo: r.written && r.status != "undone",
            })
            .collect();
        cx.app
            .global::<crate::TagReview>()
            .set_results(ModelRc::new(VecModel::from(items)));
        cx.app.global::<crate::TagReview>().set_can_undo(
            self.results
                .iter()
                .any(|r| r.written && r.status != "undone"),
        );
    }

    /// Restore one file of the last apply (name first, then tags and cover).
    fn undo_one(&mut self, i: usize) -> Result<(), String> {
        let Some((dir, _)) = &self.session else {
            return Err("Nothing to undo".into());
        };
        let Some(r) = self.results.get_mut(i) else {
            return Err("No such file".into());
        };
        if !r.written || r.status == "undone" {
            return Ok(());
        }
        if r.path != r.orig {
            if r.orig.exists() {
                return Err(format!("{} exists again", r.orig.display()));
            }
            std::fs::rename(&r.path, &r.orig).map_err(|e| e.to_string())?;
            r.path = r.orig.clone();
        }
        restore(dir, &r.orig)?;
        r.status = "undone";
        r.detail = "Previous tags, cover and name restored".into();
        Ok(())
    }

    fn undo(&mut self, cx: &Ctx, which: Option<usize>) {
        let idx: Vec<usize> = match which {
            Some(i) => vec![i],
            None => (0..self.results.len()).collect(),
        };
        let errs: Vec<String> = idx
            .into_iter()
            .filter_map(|i| self.undo_one(i).err())
            .collect();
        let left = self
            .results
            .iter()
            .any(|r| r.written && r.status != "undone");
        if !left {
            if let Some((dir, _)) = self.session.take() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        let s = if errs.is_empty() {
            "Previous tags, covers and file names restored".to_string()
        } else {
            format!("Undo failed: {}", errs.join("; "))
        };
        self.render_summary(cx);
        cx.app
            .global::<crate::TagReview>()
            .set_status(s.as_str().into());
        cx.toast(&s);
    }

    /// Candidates from the AcoustID package: `args` = [path, json] with the AcoustID lookup
    /// response and the file name pattern. MusicBrainz is queried here (same as
    /// [`identify`]), then the usual candidates list and review follow.
    fn external(&mut self, cx: &Ctx, args: &[String]) {
        let (Some(path), Some(json)) = (args.first(), args.get(1)) else {
            return;
        };
        let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
        let pattern = v["pattern"].as_str().unwrap_or("").to_string();
        let ids = parse_acoustid(&v["acoustid"]);
        let target = PathBuf::from(path);
        // The package's answer serves the next identify / auto-identify / folder run too.
        if v["acoustid"]["status"] == "ok" {
            cache::remember(&target, &parse_acoustid_matches(&v["acoustid"]));
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let a = cx.app;
        a.global::<crate::Overlays>().set_settings_open(false);
        a.global::<crate::MusicDialogState>().set_owner(ID.into());
        a.global::<crate::MusicDialogState>()
            .set_caps(crate::MdCaps::default());
        a.global::<crate::MusicDialogState>()
            .set_title("Auto-tagger".into());
        a.global::<crate::MusicDialogState>()
            .set_placeholder("File or folder to tag".into());
        a.global::<crate::MusicDialogState>()
            .set_search_label("Identify".into());
        a.global::<crate::MusicDialogState>()
            .set_primary("Review changes…".into());
        a.global::<crate::MusicDialogState>()
            .set_secondary("Review all files…".into());
        a.global::<crate::MusicDialogState>()
            .set_sources(ModelRc::new(VecModel::from(
                Vec::<slint::SharedString>::new(),
            )));
        a.global::<crate::MusicDialogState>()
            .set_query(path.as_str().into());
        a.global::<crate::MusicDialogState>().set_detail("".into());
        crate::streams::open_dialog(a);
        self.batch = false;
        self.rows.clear();
        self.show_rows(cx, None);
        Self::ui(
            cx,
            "AcoustID match from the plugin: looking up MusicBrainz…",
            true,
        );
        let (tx, rx) = channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let m = match external_rows(&target, &ids, &pattern) {
                Ok(rows) if rows.is_empty() => {
                    Msg::Done("AcoustID: no match for the fingerprint".into())
                }
                Ok(rows) => Msg::Rows(rows, "Fingerprinted by the AcoustID plugin".into()),
                Err(e) => Msg::Done(e),
            };
            let _ = tx.send(m);
        });
    }

    fn auto_cancel(&mut self) {
        if let Some((_, _, c)) = self.auto.job.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.auto.pending = None;
    }

    /// Automatic identification: collect a finished lookup, then look at the shared
    /// now-playing track for a new untagged song (see the module docs).
    fn auto_tick(&mut self, cx: &Ctx) {
        let playing = cx.player.get("path").unwrap_or_default();
        if let Some((path, rx, cancel)) = &self.auto.job {
            if *path != playing {
                // Skipped to another file: the answer is still cached when it comes.
                cancel.store(true, Ordering::Relaxed);
            }
            match rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => {}
                Ok(Ok(Some(m))) if *path == playing => {
                    let path = path.clone();
                    self.auto_show(cx, &path, m);
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => eprintln!("vivi: auto-tagger: automatic identification: {e}"),
            }
            self.auto.job = None;
            self.auto.seen = -1; // the song may have changed meanwhile
        }
        if !cx.flag("auto-identify") {
            self.auto.pending = None;
            return;
        }
        let id = crate::nowplaying::current_id();
        if id != self.auto.seen {
            self.auto.seen = id;
            let t = crate::nowplaying::current();
            if t.path == playing {
                if let Some(m) = self.auto.shown.get(&t.path).cloned() {
                    // Played again this run: no new lookup, only the title again.
                    Self::set_title(cx, &m);
                    self.auto.last = Some((t.path.clone(), m));
                }
            }
            self.auto.pending = (t.path == playing
                && untagged_song(&t)
                && !self.auto.tried.contains(&t.path)
                && Path::new(&t.path).is_file())
            .then(|| (t.path.clone(), std::time::Instant::now()));
        }
        let Some((path, since)) = self.auto.pending.clone() else {
            return;
        };
        if path != playing {
            self.auto.pending = None;
            return;
        }
        if since.elapsed() < AUTO_DELAY {
            return;
        }
        self.auto.pending = None;
        let key = Self::secret(
            cx,
            "acoustid-key",
            &["VIVI_ACOUSTID_KEY", "ACOUSTID_API_KEY"],
        );
        if key.trim().is_empty() {
            if !std::mem::replace(&mut self.auto.warned, true) {
                cx.toast("Auto-tagger: identifying untagged songs needs an AcoustID key (Extensions → Auto-tagger)");
            }
            return;
        }
        self.auto.tried.insert(path.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        self.auto.job = Some((path.clone(), rx, cancel.clone()));
        std::thread::spawn(move || {
            lower_priority();
            let _ = tx.send(auto_identify(Path::new(&path), &key, &cancel));
        });
    }

    /// Show an automatic match as now playing and offer the review.
    fn auto_show(&mut self, cx: &Ctx, path: &str, m: Match) {
        crate::plugins::host::media_meta(ID, now_playing_meta(path, &m));
        Self::set_title(cx, &m);
        let msg = format!("Identified: {} ({}%)", display_title(&m), m.score);
        toast_with_action(cx.app, &msg, "Review tags", REVIEW_ACTION, 10.0);
        if self.auto.shown.len() >= cache::CACHE_MAX {
            self.auto.shown.clear();
        }
        self.auto.shown.insert(path.to_string(), m.clone());
        self.auto.last = Some((path.to_string(), m));
    }

    /// The player's title line and the window title (they read mpv's media-title), for
    /// the playing file only; left alone when it already reads so.
    fn set_title(cx: &Ctx, m: &Match) {
        let title = display_title(m);
        if cx
            .player
            .mpv
            .get_property::<String>("media-title")
            .ok()
            .as_deref()
            == Some(&title)
        {
            return;
        }
        if let Err(e) = cx
            .player
            .mpv
            .set_property("file-local-options/force-media-title", title.as_str())
        {
            eprintln!("vivi: auto-tagger: set the title: {e:?}");
        }
    }

    fn start(&mut self, cx: &Ctx) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let target = PathBuf::from(
            cx.app
                .global::<crate::MusicDialogState>()
                .get_query()
                .trim()
                .to_string(),
        );
        let key = Self::secret(
            cx,
            "acoustid-key",
            &["VIVI_ACOUSTID_KEY", "ACOUSTID_API_KEY"],
        );
        let pattern = Self::pattern(cx);
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.rows.clear();
        self.show_rows(cx, None);
        cx.app
            .global::<crate::MusicDialogState>()
            .set_detail("".into());
        self.batch = target.is_dir();
        if self.batch {
            Self::ui(cx, "Scanning folder…", true);
            std::thread::spawn(move || batch(&target, &key, &pattern, &cancel, &tx));
        } else if target.is_file() {
            Self::ui(
                cx,
                if key.trim().is_empty() {
                    "Searching MusicBrainz…"
                } else {
                    "Fingerprinting and looking up AcoustID…"
                },
                true,
            );
            std::thread::spawn(move || {
                let m = match identify(&target, &key) {
                    Ok((old, c, notes)) => {
                        let note = notes.join(" · ");
                        let rows = c
                            .into_iter()
                            .map(|cand| Row {
                                path: target.clone(),
                                old: old.clone(),
                                rename: proposal(&cand, &pattern, &target),
                                cand,
                                note: note.clone(),
                            })
                            .collect();
                        Msg::Rows(rows, notes.join(". "))
                    }
                    Err(e) => Msg::Done(e),
                };
                let _ = tx.send(m);
            });
        } else {
            Self::ui(cx, "Choose an audio file or a folder", false);
            self.rx = None;
        }
    }
}

/// The review's status word as the typed UI state.
fn tag_status(s: &str) -> crate::TagStatus {
    match s {
        "tagged" => crate::TagStatus::Tagged,
        "renamed" => crate::TagStatus::Renamed,
        "skipped" => crate::TagStatus::Skipped,
        "undone" => crate::TagStatus::Undone,
        _ => crate::TagStatus::Failed,
    }
}

#[cfg(test)]
mod tests;
