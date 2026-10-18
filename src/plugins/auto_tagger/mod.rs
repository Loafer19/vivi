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

mod files;
mod providers;
mod review;
pub use files::*;
pub use providers::*;
pub use review::*;

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
        a.global::<crate::Overlays>().set_md_open(true);
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
        a.global::<crate::Overlays>().set_md_open(true);
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

/// Apply one review and describe the outcome for the summary.
pub fn apply_one(r: &Review, originals: &Path, session: &Path) -> FileResult {
    let mut out = FileResult {
        orig: r.path.clone(),
        path: r.path.clone(),
        status: "skipped",
        detail: String::new(),
        changes: Vec::new(),
        written: false,
    };
    if !r.on {
        out.detail = "Unticked in the review".into();
        return out;
    }
    if r.changes() == 0 {
        out.detail = "Nothing to change: tags already match".into();
        return out;
    }
    let (n, c) = match apply_review(r, originals, session) {
        Ok(v) => v,
        Err(e) => {
            out.status = "failed";
            out.detail = format!("Tags not written: {e}");
            return out;
        }
    };
    out.written = n > 0 || c;
    for ((f, o, nv), on) in review_fields(&r.old, &r.cand.tags).iter().zip(&r.fields) {
        if *on {
            let (o, nv) = if *f == "Lyrics" {
                (lyrics_summary(o), lyrics_summary(nv))
            } else {
                (o.clone(), nv.clone())
            };
            let o = if o.is_empty() { "—".to_string() } else { o };
            out.changes.push(format!("{f}:  {o}  →  {nv}"));
        }
    }
    if c {
        out.changes.push(if r.old_cover.is_some() {
            "Cover:  replaced".into()
        } else {
            "Cover:  embedded".into()
        });
    }
    out.status = if out.written { "tagged" } else { "skipped" };
    out.detail = format!(
        "{n} field{}{}",
        if n == 1 { "" } else { "s" },
        if c { " + cover" } else { "" }
    );
    if let (true, Some(name)) = (r.rename_on, &r.rename) {
        match rename_file(&r.path, name) {
            Ok(to) => {
                out.changes.push(format!(
                    "File name:  {}  →  {name}",
                    r.path.file_name().unwrap_or_default().to_string_lossy()
                ));
                out.path = to;
                out.status = "renamed";
                out.written = true;
            }
            Err(e) => out.detail += &format!(" · not renamed: {e}"),
        }
    }
    out
}

/// Rows for AcoustID recording ids (score ≥ 50, best three) with rename proposals.
fn external_rows(path: &Path, ids: &[(String, u32)], pattern: &str) -> Result<Vec<Row>, String> {
    let (old, _) = read_tags(path)?;
    let mut rows = Vec::new();
    for (id, score) in ids.iter().filter(|(_, s)| *s >= 50).take(3) {
        let rec = lookup_recording(id)?;
        for cand in candidates_from_recording(&rec, *score, "AcoustID", 4) {
            rows.push(Row {
                path: path.to_path_buf(),
                old: old.clone(),
                rename: rename_to(pattern, &cand.tags, path),
                cand,
                note: "Fingerprint: AcoustID plugin (fpcalc)".into(),
            });
        }
    }
    Ok(rows)
}

/// File name proposal for a fingerprint (AcoustID) match; None for tag/name searches or
/// when renaming is off (empty pattern).
fn proposal(cand: &Candidate, pattern: &str, path: &Path) -> Option<String> {
    cand.source
        .starts_with("AcoustID")
        .then(|| rename_to(pattern, &cand.tags, path))
        .flatten()
}

fn batch(dir: &Path, key: &str, pattern: &str, cancel: &AtomicBool, tx: &Sender<Msg>) {
    let files = audio_files(dir);
    let total = files.len();
    let (mut found, mut skipped) = (0, 0);
    for (i, p) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let _ = tx.send(Msg::Status(format!(
            "{}% · identifying {}/{total}: {} · {found} matched, {skipped} skipped",
            i * 100 / total.max(1),
            i + 1,
            p.file_name().unwrap_or_default().to_string_lossy()
        )));
        match identify(p, key) {
            Ok((old, c, notes)) => match c.into_iter().next() {
                Some(cand) if cand.score >= CONFIDENT && !diff(&old, &cand.tags).is_empty() => {
                    found += 1;
                    let _ = tx.send(Msg::Row(Box::new(Row {
                        path: p.clone(),
                        old,
                        rename: proposal(&cand, pattern, p),
                        cand,
                        note: notes.join(" · "),
                    })));
                }
                _ => skipped += 1,
            },
            Err(_) => skipped += 1,
        }
    }
    let _ = tx.send(Msg::Done(format!(
        "{total} audio files: {found} confident matches to review, {skipped} unchanged or not found"
    )));
}

impl Plugin for AutoTagger {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Auto-tagger",
            "Identifies songs by their audio (AcoustID fingerprint) or tags, looks them up on MusicBrainz, Deezer, iTunes, Last.fm and Discogs and writes tags, hi-res covers and optional LRCLIB synced lyrics after a preview that shows each field's source. Originals are backed up.",
            "tag",
        )
        .contribute(contrib::CONTEXT_ITEM, "Auto-tag…", "open-path")
        .contribute(contrib::CONTEXT_PLAYER, "Auto-tag this file…", "open")
        .contribute(contrib::PALETTE_COMMAND, "Auto-tag the playing file", "open")
        .setting(SettingSpec::secret(
            "acoustid-key",
            "AcoustID API key",
            "Optional. Register a free application at acoustid.org/new-application for audio fingerprint matching; without it songs are matched by tags or file name.",
        ))
        .setting(SettingSpec::toggle(
            "cover",
            "Embed cover art",
            "Download the front cover from the Cover Art Archive.",
            true,
        ))
        .setting(SettingSpec::toggle(
            "deezer",
            "Deezer",
            "Fill album, track, date and add a 1000 px cover from Deezer (no account needed).",
            true,
        ))
        .setting(SettingSpec::toggle(
            "itunes",
            "iTunes",
            "Fill album, track, date, genre and add a 1400 px cover from the iTunes Search API (no account needed).",
            true,
        ))
        .setting(SettingSpec::secret(
            "lastfm-key",
            "Last.fm API key",
            "Optional. Genre from Last.fm's top tags; create a free key at last.fm/api/account/create.",
        ))
        .setting(SettingSpec::secret(
            "discogs-token",
            "Discogs token",
            "Optional. Year, genre/style and cover from Discogs; personal access token from discogs.com/settings/developers.",
        ))
        .setting(SettingSpec::toggle(
            "lyrics",
            "Embed synced lyrics",
            "Adds LRCLIB lyrics to the review: MP3 gets SYLT + USLT, FLAC/Ogg/Opus a LYRICS comment, M4A ©lyr (LRC text).",
            false,
        ))
        .setting(SettingSpec::toggle(
            "rename",
            "Suggest file names",
            "When the audio fingerprint identifies a song, the review offers a new file name (unticked: renaming is opt-in). Never overwrites a file; characters not allowed in file names become _.",
            true,
        ))
        .setting(SettingSpec::text(
            "rename-pattern",
            "File name pattern",
            "Fields: {artist} {title} {album} {albumartist} {year} {date} {track} {genre}.",
            "{artist} - {title}",
            "{artist} - {title}",
        ))
        .setting(SettingSpec::text(
            "folder",
            "Library folder",
            "Folder for Tag library folder; empty = first library folder.",
            "~/Music",
            "",
        ))
        .setting(SettingSpec::action(
            "open",
            "Tag the playing file",
            "Opens the preview dialog for the current file.",
            "Tag file…",
        ))
        .setting(SettingSpec::action(
            "batch",
            "Tag library folder",
            "Finds confident matches for every audio file; you review before writing.",
            "Tag folder…",
        ))
        .setting(SettingSpec::action(
            "restore",
            "Restore original tags",
            "Puts back the tags of the playing file from before the first auto-tag.",
            "Restore",
        ))
    }
    fn disable(&mut self, cx: &Ctx) {
        self.cancel.store(true, Ordering::Relaxed);
        self.rx = None;
        if cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
            cx.app.global::<crate::Overlays>().set_md_open(false);
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let idx = || args.first().and_then(|a| a.parse::<usize>().ok());
        match name {
            "open" => {
                let p = cx
                    .local_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                cx.app.global::<crate::Overlays>().set_settings_open(false);
                self.open(cx, p);
            }
            // `context.item`: tag the right-clicked library file or folder.
            "open-path" => {
                let p = args.first().cloned().unwrap_or_default();
                cx.app.global::<crate::Overlays>().set_settings_open(false);
                self.open(cx, p);
            }
            "batch" => {
                let f = cx.get("folder");
                let f = if f.trim().is_empty() {
                    crate::config::Config::load()
                        .library
                        .folders
                        .first()
                        .cloned()
                        .unwrap_or_default()
                } else {
                    f
                };
                let f = match f.strip_prefix("~/") {
                    Some(r) => crate::store::home().join(r).to_string_lossy().into_owned(),
                    None => f,
                };
                cx.app.global::<crate::Overlays>().set_settings_open(false);
                self.open(cx, f);
            }
            "restore" => match cx.local_path() {
                Some(p) => match restore(&backup_dir(), &p) {
                    Ok(()) => cx.toast("Original tags restored"),
                    Err(e) => cx.toast(&e),
                },
                None => cx.toast("Play a local file first"),
            },
            "search" => self.start(cx),
            "acoustid" => self.external(cx, args),
            "select" => {
                if let Some(i) = idx() {
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_detail(self.detail(i).into());
                }
            }
            "primary" => self.review(cx, idx().into_iter().collect()),
            "secondary" => {
                // One row per file: the first (best) one.
                let mut seen = std::collections::HashSet::new();
                let all = (0..self.rows.len())
                    .filter(|&i| seen.insert(self.rows[i].path.clone()))
                    .collect();
                self.review(cx, all);
            }
            "toggle-file" | "toggle-field" | "cover-pick" | "field-alt" => {
                let a = args.first().map(String::as_str).unwrap_or("");
                let mut it = a.split(':');
                let i = it.next().and_then(|x| x.parse::<usize>().ok());
                let j = it.next().and_then(|x| x.parse::<i64>().ok());
                let Some(r) = i.and_then(|i| self.reviews.get_mut(i)) else {
                    return;
                };
                match (name, j) {
                    ("toggle-file", _) => r.on = !r.on,
                    ("toggle-field", Some(j)) if j as usize >= r.fields.len() => {
                        r.rename_on = !r.rename_on && r.rename.is_some();
                    }
                    ("toggle-field", Some(j)) => {
                        let (o, n) = {
                            let f = review_fields(&r.old, &r.cand.tags);
                            let (_, o, n) = f[j as usize].clone();
                            (o, n)
                        };
                        if field_changes(&o, &n) {
                            if let Some(b) = r.fields.get_mut(j as usize) {
                                *b = !*b;
                            }
                        }
                    }
                    ("field-alt", Some(j)) if j >= 0 => {
                        r.next_alt(j as usize);
                    }
                    ("cover-pick", Some(j)) => {
                        r.cover = (j >= 0 && (j as usize) < r.covers.len()).then_some(j as usize)
                    }
                    _ => {}
                }
                self.render_review(cx);
            }
            "apply-selected" => self.apply_reviews(cx),
            "apply-all" => {
                for r in &mut self.reviews {
                    r.on = true;
                    let f = review_fields(&r.old, &r.cand.tags);
                    r.fields = f.iter().map(|(_, o, n)| field_changes(o, n)).collect();
                    if r.cover.is_none() && !r.covers.is_empty() {
                        r.cover = Some(0);
                    }
                    // Renaming stays opt-in: only files whose rename box is ticked.
                }
                self.render_review(cx);
                self.apply_reviews(cx);
            }
            "undo" => self.undo(cx, None),
            "undo-file" => self.undo(cx, idx()),
            "open-folder" => {
                if let Some(r) = idx().and_then(|i| self.results.get(i)) {
                    if let Some(d) = r.path.parent() {
                        let _ = std::process::Command::new("xdg-open").arg(d).spawn();
                    }
                }
            }
            "review-close" => {
                cx.app.global::<crate::Overlays>().set_tr_open(false);
                self.reviews.clear();
                cx.app
                    .global::<crate::TagReview>()
                    .set_files(ModelRc::default());
                // Thumbnails are only needed while the dialog is open.
                let _ = std::fs::remove_dir_all(cover_cache());
            }
            "close" => {
                self.cancel.store(true, Ordering::Relaxed);
                self.rx = None;
            }
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        let Some(rx) = &self.rx else { return };
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Status(s) => {
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_status(s.as_str().into());
                    cx.app.global::<crate::TagReview>().set_status(s.into());
                }
                Msg::Row(r) => {
                    self.rows.push(*r);
                    self.show_rows(cx, None);
                }
                Msg::Rows(r, note) => {
                    self.rows = r;
                    self.show_rows(cx, None);
                    let s = if self.rows.is_empty() {
                        "No match found on MusicBrainz".to_string()
                    } else {
                        format!(
                            "{} candidates. Select one to preview the changes.",
                            self.rows.len()
                        )
                    };
                    let s = if note.is_empty() {
                        s
                    } else {
                        format!("{s} {note}.")
                    };
                    Self::ui(cx, &s, false);
                    if !self.rows.is_empty() {
                        cx.app
                            .global::<crate::MusicDialogState>()
                            .set_detail(self.detail(0).into());
                    }
                    self.rx = None;
                    // Screenshots/tests: go straight to the review of the best match.
                    if std::env::var_os("VIVI_TAGREVIEW").is_some() && !self.rows.is_empty() {
                        self.review(cx, vec![0]);
                    }
                    return;
                }
                Msg::Done(s) => {
                    Self::ui(cx, &s, false);
                    if self.batch && !self.rows.is_empty() {
                        cx.app
                            .global::<crate::MusicDialogState>()
                            .set_detail(self.detail(0).into());
                    }
                    self.rx = None;
                    return;
                }
                Msg::Reviews(v) => {
                    self.reviews = v;
                    self.rx = None;
                    Self::ui(cx, "", false);
                    cx.app.global::<crate::TagReview>().set_summary("".into());
                    cx.app.global::<crate::TagReview>().set_busy(false);
                    cx.app.global::<crate::TagReview>().set_can_undo(false);
                    self.render_review(cx);
                    cx.app.global::<crate::Overlays>().set_md_open(false);
                    cx.app.global::<crate::Overlays>().set_tr_open(true);
                    if std::env::var("VIVI_TAGREVIEW").as_deref() == Ok("apply") {
                        self.apply_reviews(cx);
                    }
                    return;
                }
                Msg::Applied(results) => {
                    self.rx = None;
                    if let Some(sess) = &mut self.session {
                        sess.1 = results
                            .iter()
                            .filter(|r| r.written)
                            .map(|r| r.orig.clone())
                            .collect();
                    }
                    self.results = results;
                    cx.app.global::<crate::TagReview>().set_busy(false);
                    self.render_summary(cx);
                    cx.app.global::<crate::TagReview>().set_status(
                        "Backups kept in ~/.local/share/vivi/tag-backups · Undo restores tags, covers and names".into(),
                    );
                    let (t, _, _, f) = result_counts(&self.results);
                    cx.toast(&format!(
                        "Auto-tagger: {t} tagged{}",
                        if f > 0 {
                            format!(", {f} failed")
                        } else {
                            String::new()
                        }
                    ));
                    return;
                }
            }
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
mod tests {
    use super::*;

    const MEDIA: &str = "/workspace/vivi-media/forecast.mp3";

    fn tmp_copy(name: &str) -> Option<PathBuf> {
        let src = Path::new(MEDIA);
        if !src.exists() {
            return None;
        }
        let d = std::env::temp_dir().join(format!("vivi-tagger-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("forecast.mp3");
        std::fs::copy(src, &p).unwrap();
        Some(p)
    }

    #[test]
    fn diff_skips_empty_and_equal() {
        let old = Tags {
            title: "a".into(),
            artist: "x".into(),
            ..Default::default()
        };
        let new = Tags {
            title: "a".into(),
            artist: "y".into(),
            album: "".into(),
            date: "2014".into(),
            ..Default::default()
        };
        let d = diff(&old, &new);
        assert_eq!(
            d,
            vec![
                ("Artist", "x".into(), "y".into()),
                ("Date", "".into(), "2014".into())
            ]
        );
    }

    fn cand(title: &str, artist: &str, album: &str, rel: &str, score: u32) -> Candidate {
        Candidate {
            tags: Tags {
                title: title.into(),
                artist: artist.into(),
                album: album.into(),
                mb_recording: "rec".into(),
                mb_release: rel.into(),
                ..Default::default()
            },
            release_group: String::new(),
            score,
            source: "MusicBrainz search",
            sources: Vec::new(),
            extra_covers: Vec::new(),
            alts: Vec::new(),
            agree: Vec::new(),
        }
    }

    #[test]
    fn ranking_merges_duplicates_and_prefers_confirmed_matches() {
        let current = Tags {
            title: "Local Forecast".into(),
            artist: "Kevin MacLeod".into(),
            ..Default::default()
        };
        let mut confirmed = cand("Other Song", "Band", "B", "r3", 88);
        confirmed.agree = vec!["Deezer", "iTunes", "Discogs", "Last.fm"];
        let v = vec![
            cand("Local Forecast", "Kevin MacLeod", "A", "r1", 80),
            cand("Local Forecast", "Kevin MacLeod", "A", "r1", 90), // duplicate, better
            cand("Something", "Else", "C", "r2", 92),
            confirmed,
        ];
        let r = rank_candidates(v, &current);
        assert_eq!(r.len(), 3, "same recording + release collapses");
        // 88 + 12 (three agreeing sources max) = 100 beats 90 + 5 (tags match) and 92.
        assert_eq!(r[0].tags.mb_release, "r3");
        assert_eq!(r[0].confidence(), 100);
        assert_eq!((r[1].tags.mb_release.as_str(), r[1].score), ("r1", 95));
        assert_eq!(r[2].tags.mb_release, "r2");
    }

    #[test]
    fn review_cycles_alternative_values() {
        let old = Tags {
            album: "Old".into(),
            ..Default::default()
        };
        let mut c = cand("T", "A", "MB Album", "r1", 90);
        c.alts.push(Alt {
            field: "Album",
            value: "Old".into(),
            source: "iTunes",
        });
        c.alts.push(Alt {
            field: "Album",
            value: "Deezer Album".into(),
            source: "Deezer",
        });
        let mut r = Review::new(PathBuf::from("/nonexistent.mp3"), old, c, Vec::new(), true);
        assert_eq!(r.alts_of(2), 2);
        assert!(r.fields[2]);
        assert!(r.next_alt(2));
        // "Old" equals the current value: nothing to write, the box unticks.
        assert_eq!(r.cand.tags.album, "Old");
        assert!(!r.fields[2]);
        assert_eq!(
            super::super::tagsources::source_of(&r.cand, "Album"),
            "iTunes"
        );
        assert!(r.next_alt(2));
        assert_eq!(r.cand.tags.album, "Deezer Album");
        assert!(r.fields[2]);
        assert!(r.next_alt(2));
        assert_eq!(r.cand.tags.album, "MB Album");
        assert_eq!(
            super::super::tagsources::source_of(&r.cand, "Album"),
            "MusicBrainz"
        );
        assert!(!r.next_alt(0), "no alternatives for the title");
    }

    #[test]
    fn rename_sanitizes_and_never_overwrites() {
        assert_eq!(
            sanitize_file_name("AC/DC: Back <in> Black?"),
            "AC_DC_ Back _in_ Black_"
        );
        assert_eq!(
            sanitize_file_name("  ..Artist -  - Title.. "),
            "Artist - Title"
        );
        assert_eq!(sanitize_file_name("con"), "con_");
        assert_eq!(sanitize_file_name("COM3.x"), "COM3_.x");
        assert_eq!(sanitize_file_name("Comet"), "Comet");
        assert_eq!(sanitize_file_name("a\tb\u{7}c"), "a b_c");
        let long = "ж".repeat(200);
        let s = sanitize_file_name(&long);
        assert!(s.len() <= MAX_STEM && s.chars().all(|c| c == 'ж'));
        let t = Tags {
            title: "Song/Live".into(),
            artist: "Band".into(),
            track: "3/12".into(),
            ..Default::default()
        };
        assert_eq!(
            rename_to("{track}. {artist} - {title}", &t, Path::new("/m/x.flac")).as_deref(),
            Some("03. Band - Song_Live.flac")
        );
        assert_eq!(
            rename_to("{title}", &t, Path::new("/m/Song_Live.mp3")),
            None
        );
        let d = std::env::temp_dir().join(format!("vivi-rename-safe-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("a.mp3");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(d.join("Taken.mp3"), b"b").unwrap();
        assert!(
            rename_file(&a, "taken.mp3").is_err(),
            "case-insensitive clash"
        );
        assert!(rename_file(&a, "../escape.mp3").is_err());
        assert!(rename_file(&a, ".hidden.mp3").is_err());
        let to = rename_file(&a, "New.mp3").unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"a");
        assert_eq!(std::fs::read(d.join("Taken.mp3")).unwrap(), b"b");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn summary_results_skip_fail_and_count() {
        let c = cand("T", "A", "Al", "r1", 90);
        let mut r = Review::new(
            PathBuf::from("/nonexistent/x.mp3"),
            Tags::default(),
            c,
            Vec::new(),
            true,
        );
        r.on = false;
        let tmp = std::env::temp_dir().join(format!("vivi-sum-{}", std::process::id()));
        assert_eq!(apply_one(&r, &tmp, &tmp).status, "skipped");
        r.on = true;
        let f = apply_one(&r, &tmp, &tmp);
        assert_eq!(f.status, "failed");
        assert!(f.detail.starts_with("Tags not written"), "{}", f.detail);
        let ok = FileResult {
            status: "renamed",
            written: true,
            ..f.clone()
        };
        assert_eq!(result_counts(&[f, ok]), (1, 1, 0, 1));
    }

    #[test]
    fn terms_from_name() {
        let t = Tags::default();
        assert_eq!(
            search_terms(&t, Path::new("/m/03 - Kevin MacLeod - Local Forecast.mp3")),
            ("Local Forecast".into(), "Kevin MacLeod".into())
        );
        assert_eq!(lucene("AC/DC (live)"), "AC\\/DC \\(live\\)");
    }

    #[test]
    fn parse_recording() {
        let rec = serde_json::json!({
            "id": "r1", "title": "Song", "first-release-date": "2010",
            "artist-credit": [{"name": "A", "joinphrase": " feat. "}, {"name": "B"}],
            "genres": [{"name": "jazz", "count": 3}, {"name": "pop", "count": 1}],
            "releases": [
                {"id": "bootleg", "title": "Live", "status": "Bootleg", "date": "2001"},
                {"id": "rel1", "title": "Album", "status": "Official", "date": "2011-02-03",
                 "release-group": {"id": "rg1", "primary-type": "Album"},
                 "media": [{"tracks": [{"number": "7"}]}]}
            ]
        });
        let c = candidates_from_recording(&rec, 95, "AcoustID", 4);
        assert_eq!(c.len(), 2);
        let t = &c[0].tags;
        assert_eq!(t.artist, "A feat. B");
        assert_eq!((t.album.as_str(), t.track.as_str()), ("Album", "7"));
        assert_eq!((t.date.as_str(), t.genre.as_str()), ("2011-02-03", "Jazz"));
        assert_eq!(c[0].release_group, "rg1");
        assert_eq!(c[1].tags.mb_release, "bootleg");
    }

    #[test]
    fn review_apply_and_undo_restores_tags_and_cover() {
        let Some(p) = tmp_copy("review") else { return };
        let png: Vec<u8> = {
            // 1×1 PNG
            let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01".to_vec();
            v.extend_from_slice(&[8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89]);
            v
        };
        assert_eq!(image_size(&png), Some((1, 1)));
        let (old, _) = read_tags(&p).unwrap();
        let mut new = old.clone();
        new.title = "Reviewed Title".into();
        new.album = "Reviewed Album".into();
        let cand = Candidate {
            tags: new,
            release_group: String::new(),
            score: 95,
            source: "AcoustID",
            sources: Vec::new(),
            extra_covers: Vec::new(),
            alts: Vec::new(),
            agree: Vec::new(),
        };
        let cover = CoverOpt {
            label: "Front".into(),
            data: png.clone(),
        };
        let mut r = Review::new(p.clone(), old.clone(), cand, vec![cover], true);
        // Only the title: the album change is unticked.
        r.fields[2] = false;
        r.cover = Some(0);
        let session = std::env::temp_dir().join(format!("vivi-review-{}", std::process::id()));
        let (n, c) = apply_review(&r, &session.join("orig"), &session).unwrap();
        assert_eq!((n, c), (1, true));
        let (now, _) = read_tags(&p).unwrap();
        assert_eq!(now.title, "Reviewed Title");
        assert_eq!(now.album, old.album);
        assert_eq!(embedded_cover(&p).as_deref(), Some(png.as_slice()));
        restore(&session, &p).unwrap();
        let (back, _) = read_tags(&p).unwrap();
        assert_eq!(back.title, old.title);
        assert_ne!(embedded_cover(&p).as_deref(), Some(png.as_slice()));
        let _ = std::fs::remove_dir_all(&session);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn embeds_synced_lyrics() {
        let Some(p) = tmp_copy("lyrics") else { return };
        let lrc = "[00:01.50]First line\n[00:04.00]Second line\n";
        let t = Tags {
            lyrics: lrc.into(),
            ..Default::default()
        };
        write_tags(&p, &t, None).unwrap();
        let (back, _) = read_tags(&p).unwrap();
        assert_eq!(back.lyrics.trim(), lrc.trim(), "USLT holds the LRC text");
        assert_eq!(
            read_sylt(&p),
            vec![
                (1500, "First line".to_string()),
                (4000, "Second line".to_string())
            ]
        );
        // Writing again replaces the SYLT frame instead of adding one.
        write_tags(&p, &t, None).unwrap();
        assert_eq!(read_sylt(&p).len(), 2);
        assert_eq!(lyrics_summary(lrc), "Synced, 2 lines: First line");
        // FLAC / Opus / M4A: one text field (LYRICS / ©lyr) with the LRC.
        for ext in ["flac", "opus", "m4a"] {
            let out = p.with_extension(ext);
            let ok = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-i"])
                .arg(&p)
                .args(["-t", "3", "-map", "0:a"])
                .arg(&out)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                continue;
            }
            write_tags(&out, &t, None).unwrap();
            assert_eq!(
                read_tags(&out).unwrap().0.lyrics.trim(),
                lrc.trim(),
                "{ext}"
            );
            let _ = std::fs::remove_file(&out);
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rename_pattern() {
        let t = Tags {
            title: "Song: Part 1/2".into(),
            artist: "AC/DC".into(),
            album: "Album".into(),
            date: "1980-07-25".into(),
            track: "3/10".into(),
            ..Default::default()
        };
        let p = Path::new("/m/01 track.mp3");
        assert_eq!(
            rename_to("{artist} - {title}", &t, p).as_deref(),
            Some("AC_DC - Song_ Part 1_2.mp3")
        );
        assert_eq!(
            rename_to("{track} {artist} - {album} ({year}) - {title}", &t, p).as_deref(),
            Some("03 AC_DC - Album (1980) - Song_ Part 1_2.mp3")
        );
        // Empty fields leave no dangling separators.
        let bare = Tags {
            title: "T".into(),
            ..Default::default()
        };
        assert_eq!(
            rename_to("{artist} - {title}", &bare, p).as_deref(),
            Some("T.mp3")
        );
        assert_eq!(rename_to("{artist} - {title}", &Tags::default(), p), None);
        assert_eq!(
            rename_to("{title}", &bare, Path::new("/m/T.mp3")),
            None,
            "same name"
        );
        let d = std::env::temp_dir().join(format!("vivi-rename-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.mp3"), b"x").unwrap();
        std::fs::write(d.join("b.mp3"), b"y").unwrap();
        assert!(
            rename_file(&d.join("a.mp3"), "b.mp3").is_err(),
            "never overwrites"
        );
        let to = rename_file(&d.join("a.mp3"), "c.mp3").unwrap();
        assert_eq!(std::fs::read(to).unwrap(), b"x");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parse_acoustid_results() {
        let v = serde_json::json!({"status": "ok", "results": [
            {"score": 0.5, "recordings": [{"id": "b"}]},
            {"score": 0.97, "recordings": [{"id": "a"}, {"id": "b"}]}
        ]});
        assert_eq!(
            parse_acoustid(&v),
            vec![("a".to_string(), 97), ("b".to_string(), 50)]
        );
    }

    #[test]
    fn write_backup_restore_roundtrip() {
        let Some(p) = tmp_copy("rt") else { return };
        let dir = p.parent().unwrap().join("backups");
        let (before, _) = read_tags(&p).unwrap();
        assert_eq!(before.artist, "Kevin MacLeod");
        backup(&dir, &p).unwrap();
        let new = Tags {
            title: "Local Forecast - Elevator".into(),
            album: "Rollin' at 5".into(),
            track: "12".into(),
            mb_recording: "b1d46379-5d1c-4a36-98d3-d7986dd33dc4".into(),
            ..Default::default()
        };
        let jpg = std::fs::read("/workspace/vivi-media/cover.jpg").ok();
        write_tags(&p, &new, jpg.as_deref()).unwrap();
        let (after, _) = read_tags(&p).unwrap();
        assert_eq!(after.title, new.title);
        assert_eq!(after.album, new.album);
        assert_eq!(after.track, "12");
        assert_eq!(after.mb_recording, new.mb_recording);
        assert_eq!(after.artist, "Kevin MacLeod");
        if jpg.is_some() {
            let tf = lofty::read_from_path(&p).unwrap();
            let tag = tf.primary_tag().unwrap();
            assert!(tag
                .pictures()
                .iter()
                .any(|x| x.pic_type() == PictureType::CoverFront));
        }
        // A second backup keeps the original.
        backup(&dir, &p).unwrap();
        restore(&dir, &p).unwrap();
        let (back, _) = read_tags(&p).unwrap();
        assert_eq!(back.title, before.title);
        assert_eq!(back.album, before.album);
        assert_eq!(back.mb_recording, "");
        assert!(restore(&dir, &p).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn builtin_fingerprint_matches_fpcalc() {
        let Some(p) = tmp_copy("fp") else { return };
        let (d, fp) = fingerprint_builtin(&p).unwrap();
        assert!((185..=192).contains(&d), "{d}");
        assert!(fp.len() > 1000 && fp.starts_with("AQAD"));
        // Compare raw sub-fingerprints with fpcalc's (if installed): decoders and
        // resamplers differ slightly, so allow some bit errors (AcoustID does too).
        let out = std::process::Command::new("fpcalc")
            .args(["-raw", "-json"])
            .arg(&p)
            .output();
        if let Some(out) = out.ok().filter(|o| o.status.success()) {
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            let theirs: Vec<u32> = v["fingerprint"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_u64().unwrap() as u32)
                .collect();
            let (_, ours) =
                fingerprint_raw(&p, &rusty_chromaprint::Configuration::preset_test2()).unwrap();
            let best = (0..8)
                .flat_map(|o| [(o, 0), (0, o)])
                .map(|(a, b)| {
                    let n = ours
                        .len()
                        .saturating_sub(a)
                        .min(theirs.len().saturating_sub(b));
                    let errs: u32 = (0..n)
                        .map(|i| (ours[i + a] ^ theirs[i + b]).count_ones())
                        .sum();
                    errs as f64 / (n as f64 * 32.0)
                })
                .fold(1.0, f64::min);
            eprintln!("bit error rate vs fpcalc: {best:.3}");
            assert!(best < 0.15, "{best}");
        }
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    /// Live MusicBrainz + Cover Art Archive (no key needed). `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_musicbrainz_and_cover() {
        let Some(p) = tmp_copy("live") else { return };
        let (old, c, _) = identify(&p, "").unwrap();
        assert_eq!(old.artist, "Kevin MacLeod");
        assert!(!c.is_empty());
        eprintln!("{:#?}", &c[..c.len().min(3)]);
        let best = c.iter().find(|c| !c.tags.mb_release.is_empty()).unwrap();
        let rec = lookup_recording(&best.tags.mb_recording).unwrap();
        assert!(!candidates_from_recording(&rec, 100, "x", 3).is_empty());
        let art = cover_art("a252b0a1-4465-45cf-9292-37ab4321b8af", "").unwrap();
        assert!(art.starts_with(&[0xff, 0xd8]));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
