//! Shared Downloads panel (Library → Downloads, ui/downloads.slint): every plugin's
//! downloads in one list with progress, speed, size / ETA and actions.
//!
//! Plugins report jobs; the host draws them, keeps the list across restarts
//! (`downloads.json` in the data folder), notifies on finish / failure and calls the
//! plugin back for pause / resume / cancel / retry:
//!
//! * Built-ins call [`report`] / [`remove`] (safe from inside plugin callbacks: the UI
//!   is refreshed from a timer).
//! * Packages send `script-message vivi-download <plugin id> <job id> <json>` (fields of
//!   [`Report`], all optional, merged into the job) and `vivi-download-remove <plugin id>
//!   <job id>`.
//! * The host sends `download-pause|download-resume|download-cancel|download-retry
//!   <job id>` back (a built-in's `command`, a package's `script-message-to`), and
//!   `download-discard <job id>` to a job that reported `can_discard`. Discard on any
//!   other job is a Cancel after which the host deletes the reported file itself.
//!
//! See docs/plugins-architecture.md, "Downloads".

use crate::config::Config;
use crate::player::Player;
use crate::AppWindow;
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

/// Job state (`state` in the JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Queued,
    Active,
    Paused,
    Done,
    Failed,
    /// Stopped by the user: no "Download failed" notification, Retry stays.
    Cancelled,
}

impl State {
    pub fn parse(s: &str) -> Option<State> {
        Some(match s {
            "queued" => State::Queued,
            "active" | "downloading" | "recording" => State::Active,
            "paused" => State::Paused,
            "done" | "finished" => State::Done,
            "failed" | "error" => State::Failed,
            "cancelled" | "canceled" => State::Cancelled,
            _ => return None,
        })
    }
    fn name(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Active => "active",
            State::Paused => "paused",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }
    fn running(self) -> bool {
        matches!(self, State::Queued | State::Active | State::Paused)
    }
}

/// One download, as stored in `downloads.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Job {
    pub plugin: String,
    pub job: String,
    pub title: String,
    /// Second line before the state (e.g. "Artist · Album").
    pub subtitle: String,
    pub state: State,
    /// Bytes so far / in total (0 = unknown).
    pub done: u64,
    pub total: u64,
    /// 0..1 when the plugin knows the share but not the bytes (-1 = unknown).
    pub progress: f32,
    /// Bytes per second (measured by the host from `done`, or reported).
    pub speed: u64,
    /// Seconds left as the plugin reported them (0 = unknown: from the speed).
    pub eta: u64,
    /// Saved file (or the file being written).
    pub path: String,
    /// Thumbnail: a local image file, or an http(s) URL fetched into the plugin cover
    /// cache.
    pub artwork: String,
    pub error: String,
    /// Free text under the title instead of size / ETA (e.g. "Recording · 12:30").
    pub detail: String,
    pub can_pause: bool,
    /// The plugin handles `download-discard` itself (else the host's Discard is a Cancel
    /// and deletes the file when the job ends).
    pub can_discard: bool,
    /// Unix seconds it was added.
    pub added: u64,
    /// Job id of the group row this one belongs to (same plugin), e.g. a playlist.
    pub parent: String,
    /// A group row's items done / in total ("12 / 40"); 0 in total = count its rows.
    pub items_done: u32,
    pub items_total: u32,
    /// Heading of the notification when it finishes (empty = "Download finished"), e.g.
    /// "Export finished" for a clip export.
    pub done_title: String,
}

/// What a plugin reports (fields left `None` keep their value). Numbers may be
/// fractional (yt-dlp's speed and ETA are): whole-number fields are rounded, negative
/// ones read 0, and a number that is not one (`null`, `"n/a"`) is left out instead of
/// dropping the whole report.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Report {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub state: Option<String>,
    #[serde(deserialize_with = "num::whole")]
    pub done: Option<u64>,
    #[serde(deserialize_with = "num::whole")]
    pub total: Option<u64>,
    #[serde(deserialize_with = "num::real")]
    pub progress: Option<f32>,
    /// 0..100, used when `progress` is not given.
    #[serde(deserialize_with = "num::real")]
    pub percent: Option<f32>,
    #[serde(deserialize_with = "num::whole")]
    pub speed: Option<u64>,
    /// Seconds left.
    #[serde(deserialize_with = "num::whole")]
    pub eta: Option<u64>,
    pub path: Option<String>,
    pub artwork: Option<String>,
    pub error: Option<String>,
    pub detail: Option<String>,
    pub can_pause: Option<bool>,
    pub can_discard: Option<bool>,
    pub parent: Option<String>,
    #[serde(deserialize_with = "num::whole")]
    pub items_done: Option<u32>,
    #[serde(deserialize_with = "num::whole")]
    pub items_total: Option<u32>,
    pub done_title: Option<String>,
}

/// Lenient numbers in [`Report`]: any JSON number (or a numeric string).
mod num {
    use serde::de::{Deserializer, Error, Visitor};
    use std::fmt;

    struct Num;

    impl<'de> Visitor<'de> for Num {
        type Value = Option<f64>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a number")
        }
        fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v as f64))
        }
        fn visit_i64<E: Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v as f64))
        }
        fn visit_f64<E: Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(v.is_finite().then_some(v))
        }
        fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
        }
        fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_any(Num)
        }
    }

    /// A whole number field: rounded, negative values 0, saturating at the type's max.
    pub fn whole<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: TryFrom<u64> + Bounded,
    {
        Ok(d.deserialize_option(Num)?.map(|v| {
            // `as` saturates (and maps negatives to 0).
            T::try_from(v.round() as u64).unwrap_or(T::MAX)
        }))
    }

    pub fn real<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f32>, D::Error> {
        Ok(d.deserialize_option(Num)?.map(|v| v as f32))
    }

    pub trait Bounded {
        const MAX: Self;
    }
    impl Bounded for u64 {
        const MAX: u64 = u64::MAX;
    }
    impl Bounded for u32 {
        const MAX: u32 = u32::MAX;
    }
}

impl Report {
    /// Merge into `j`; returns the old state when it changed.
    fn apply(self, j: &mut Job) -> Option<State> {
        let old = j.state;
        let set = |d: &mut String, v: Option<String>| {
            if let Some(v) = v {
                *d = v;
            }
        };
        set(&mut j.title, self.title);
        set(&mut j.subtitle, self.subtitle);
        set(&mut j.path, self.path);
        set(&mut j.artwork, self.artwork);
        set(&mut j.error, self.error);
        set(&mut j.detail, self.detail);
        set(&mut j.parent, self.parent);
        set(&mut j.done_title, self.done_title);
        if let Some(v) = self.items_done {
            j.items_done = v;
        }
        if let Some(v) = self.items_total {
            j.items_total = v;
        }
        if let Some(s) = self.state.as_deref().and_then(State::parse) {
            j.state = s;
        }
        if let Some(v) = self.done {
            j.done = v;
        }
        if let Some(v) = self.total {
            j.total = v;
        }
        if let Some(v) = self.progress.or(self.percent.map(|p| p / 100.0)) {
            j.progress = v;
        }
        if let Some(v) = self.speed {
            j.speed = v;
        }
        if let Some(v) = self.eta {
            j.eta = v;
        }
        if let Some(v) = self.can_pause {
            j.can_pause = v;
        }
        if let Some(v) = self.can_discard {
            j.can_discard = v;
        }
        if j.state == State::Done && j.total > 0 {
            j.done = j.total;
        }
        (old != j.state).then_some(old)
    }
}

struct Ui {
    app: slint::Weak<AppWindow>,
    player: Rc<Player>,
    cfg: Rc<RefCell<Config>>,
    model: Rc<VecModel<crate::DlJob>>,
}

#[derive(Default)]
struct Store {
    jobs: Vec<Job>,
    /// (job key, last instant, bytes then) for the speed.
    rate: Vec<(String, Instant, u64)>,
    dirty: bool,
    save: bool,
    /// Finished / failed since the last refresh: (title, ok, error or the finished heading).
    news: Vec<(String, bool, String)>,
    /// Group rows shown with all their items (keys).
    expanded: std::collections::HashSet<String>,
    /// Running groups: items_done when the run started, and whether an item finished since.
    runs: std::collections::HashMap<String, (u32, bool)>,
    /// Jobs discarded by the host (a Cancel was sent): their file is deleted and the row
    /// removed when the plugin reports that they ended.
    discarding: std::collections::HashSet<String>,
    /// Jobs the host discarded: late reports of them (a length probe after the end) are
    /// ignored until the job id starts again.
    discarded: std::collections::HashSet<String>,
    /// Toasts to show on the next refresh ("Discarded: …").
    toasts: Vec<String>,
}

thread_local! {
    static ST: RefCell<Store> = RefCell::new(Store::default());
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    /// Artwork URLs already handed to a fetch thread this run.
    static ASKED: RefCell<std::collections::HashSet<String>> = RefCell::new(Default::default());
}

/// Redraw the rows on the next refresh tick (an artwork arrived or was decoded).
fn mark_dirty() {
    ST.with(|s| s.borrow_mut().dirty = true);
}

/// The local file of a job's artwork: a path (or `file://` URL) as it is; an http(s)
/// URL from the plugin cover cache, fetched once on a worker thread (the row updates
/// when it arrives).
fn art_file(art: &str) -> Option<std::path::PathBuf> {
    if art.starts_with("https://") || art.starts_with("http://") {
        if let Some(p) = crate::plugins::host::cached_cover(art) {
            return Some(p);
        }
        if ASKED.with(|a| a.borrow_mut().insert(art.to_string())) {
            let u = art.to_string();
            std::thread::spawn(move || {
                if crate::plugins::host::fetch_cover(&u).is_some() {
                    let _ = slint::invoke_from_event_loop(mark_dirty);
                }
            });
        }
        return None;
    }
    let p = std::path::PathBuf::from(art.strip_prefix("file://").unwrap_or(art));
    (!art.is_empty() && p.is_file()).then_some(p)
}

fn key(plugin: &str, job: &str) -> String {
    format!("{plugin}\u{1f}{job}")
}

fn file() -> std::path::PathBuf {
    crate::store::data_dir().join("downloads.json")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Jobs of a previous run that never finished are failed ("interrupted").
fn restore(mut v: Vec<Job>) -> Vec<Job> {
    for j in v.iter_mut().filter(|j| j.state.running()) {
        j.state = State::Failed;
        j.error = "Interrupted when Vivi closed".into();
        j.speed = 0;
    }
    v
}

/// Create or update a job (the UI follows within half a second).
pub fn report(plugin: &str, job: &str, r: Report) {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let k = key(plugin, job);
        if s.discarded.contains(&k) {
            let again = r
                .state
                .as_deref()
                .and_then(State::parse)
                .is_some_and(State::running);
            if !again {
                return;
            }
            s.discarded.remove(&k);
        }
        let i = match s
            .jobs
            .iter()
            .position(|j| j.plugin == plugin && j.job == job)
        {
            Some(i) => i,
            None => {
                s.jobs.insert(
                    0,
                    Job {
                        plugin: plugin.into(),
                        job: job.into(),
                        progress: -1.0,
                        added: now(),
                        ..Job::default()
                    },
                );
                s.save = true;
                0
            }
        };
        let reported_speed = r.speed.is_some();
        let changed = r.apply(&mut s.jobs[i]);
        if !s.jobs[i].state.running() && s.discarding.remove(&k) {
            let j = s.jobs.remove(i);
            discard_files(&j.path);
            s.discarded.insert(k.clone());
            s.toasts.push(format!("Discarded: {}", j.title));
            s.rate.retain(|(x, _, _)| *x != k);
            s.dirty = true;
            s.save = true;
            return;
        }
        let (done, state) = (s.jobs[i].done, s.jobs[i].state);
        // Speed from the byte count (over at least a second).
        if !reported_speed {
            if state != State::Active {
                s.jobs[i].speed = 0;
            } else if let Some(p) = s.rate.iter().position(|(x, _, _)| *x == k) {
                let (t, b) = (s.rate[p].1, s.rate[p].2);
                let dt = t.elapsed().as_secs_f64();
                if dt >= 1.0 {
                    s.jobs[i].speed = (done.saturating_sub(b) as f64 / dt) as u64;
                    s.rate[p] = (k, Instant::now(), done);
                }
            } else {
                s.rate.push((k, Instant::now(), done));
            }
        }
        let (parent, items_done, group) = {
            let j = &s.jobs[i];
            (j.parent.clone(), j.items_done, j.items_total > 0)
        };
        // The item count of a running group when it first has one (a sync reports the
        // track list after "active", without a state change).
        if group && parent.is_empty() && state.running() {
            s.runs
                .entry(key(plugin, job))
                .or_insert((items_done, false));
        }
        if changed.is_some() {
            s.save = true;
            if !parent.is_empty() && state == State::Done {
                if let Some(r) = s.runs.get_mut(&key(plugin, &parent)) {
                    r.1 = true;
                }
            }
            // A sync that found nothing new finishes the group again: no "Download finished".
            let quiet = if !parent.is_empty() || state.running() {
                false
            } else {
                s.runs
                    .remove(&key(plugin, job))
                    .is_some_and(|(base, fresh)| {
                        group && state == State::Done && !fresh && items_done <= base
                    })
            };
            let j = &s.jobs[i];
            // A group's items finish quietly; the group row notifies. A cancel was the
            // user's own doing: no notification.
            let news = match j.state {
                _ if !j.parent.is_empty() || quiet => None,
                State::Done => Some((j.title.clone(), true, j.done_title.clone())),
                State::Failed => Some((j.title.clone(), false, j.error.clone())),
                _ => None,
            };
            s.news.extend(news);
        }
        s.dirty = true;
    });
}

/// What the host's Discard deletes once the job ended: the reported file and its `.part`
/// (yt-dlp, curl), only when they are regular files.
fn discard_targets(path: &str) -> Vec<std::path::PathBuf> {
    if path.trim().is_empty() {
        return Vec::new();
    }
    let p = std::path::PathBuf::from(path.strip_prefix("file://").unwrap_or(path));
    let mut part = p.clone().into_os_string();
    part.push(".part");
    vec![p, part.into()]
}

fn discard_files(path: &str) {
    for f in discard_targets(path) {
        if f.is_file() {
            if let Err(e) = std::fs::remove_file(&f) {
                eprintln!("vivi: discard {}: {e}", f.display());
            }
        }
    }
}

/// Drop a job from the list (a group with its items).
pub fn remove(plugin: &str, job: &str) {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.jobs
            .retain(|j| !(j.plugin == plugin && (j.job == job || j.parent == job)));
        s.dirty = true;
        s.save = true;
    });
}

/// Running (queued, active, paused) jobs and the overall share done (-1 = unknown).
pub fn summary(jobs: &[Job]) -> (usize, f32) {
    // Group rows stand for their items, which are counted themselves.
    let run: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.state.running() && !is_group(jobs, j))
        .collect();
    let (d, t) = run
        .iter()
        .filter(|j| j.total > 0)
        .fold((0u64, 0u64), |(d, t), j| (d + j.done, t + j.total));
    let p = if t > 0 { d as f32 / t as f32 } else { -1.0 };
    (run.len(), p)
}

/// "2 downloading · 1 queued · 45%" (empty when nothing runs).
pub fn summary_line(jobs: &[Job]) -> String {
    let (n, p) = summary(jobs);
    if n == 0 {
        return String::new();
    }
    let count = |s: State| {
        jobs.iter()
            .filter(|j| j.state == s && !is_group(jobs, j))
            .count()
    };
    let mut parts: Vec<String> = [
        (State::Active, "downloading"),
        (State::Queued, "queued"),
        (State::Paused, "paused"),
    ]
    .iter()
    .filter_map(|&(s, w)| {
        let c = count(s);
        (c > 0).then(|| format!("{c} {w}"))
    })
    .collect();
    if p >= 0.0 {
        parts.push(format!("{}%", (p * 100.0).round() as i32));
    }
    parts.join(" · ")
}

pub fn size(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b:.0} B")
    }
}

fn eta(s: u64) -> String {
    if s >= 3600 {
        format!("{}h {}m left", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{}m {}s left", s / 60, s % 60)
    } else {
        format!("{s}s left")
    }
}

/// The line under a job's title.
pub fn detail(j: &Job) -> String {
    if !j.detail.is_empty() && !matches!(j.state, State::Failed | State::Cancelled) {
        return j.detail.clone();
    }
    match j.state {
        State::Queued => "Queued".into(),
        State::Paused if j.total > 0 => {
            format!("Paused · {} of {}", size(j.done), size(j.total))
        }
        State::Paused => "Paused".into(),
        State::Done if j.total > 0 => format!("Done · {}", size(j.total)),
        State::Done => "Done".into(),
        State::Failed if j.error.is_empty() => "Failed".into(),
        State::Failed => format!("Failed · {}", j.error),
        // The plugin's "Cancelled" is the label itself; a group adds its counts.
        State::Cancelled if j.error.is_empty() || j.error.eq_ignore_ascii_case("cancelled") => {
            "Cancelled".into()
        }
        // Discarded (download-discard): what was written is gone.
        State::Cancelled if j.error.eq_ignore_ascii_case("discarded") => "Discarded".into(),
        State::Cancelled => format!("Cancelled · {}", j.error),
        State::Active => {
            let mut v = Vec::new();
            if j.total > 0 {
                v.push(format!("{} of {}", size(j.done), size(j.total)));
            } else if j.done > 0 {
                v.push(size(j.done));
            }
            if j.speed > 0 {
                v.push(format!("{}/s", size(j.speed)));
            }
            if j.eta > 0 {
                v.push(eta(j.eta));
            } else if j.speed > 0 && j.total > j.done {
                v.push(eta((j.total - j.done) / j.speed.max(1)));
            }
            if v.is_empty() {
                "Downloading…".into()
            } else {
                v.join(" · ")
            }
        }
    }
}

/// Whether `j` is a group row (it has items, or counts them itself).
fn is_group(jobs: &[Job], j: &Job) -> bool {
    j.items_total > 0
        || jobs
            .iter()
            .any(|k| k.plugin == j.plugin && k.parent == j.job && !j.job.is_empty())
}

/// A group row as drawn: "12 / 40" (the plugin's counts, or its rows done / all),
/// failures, progress by items, and active while any item runs.
pub fn group_view(g: &Job, items: &[&Job]) -> Job {
    let mut v = g.clone();
    let failed = items.iter().filter(|j| j.state == State::Failed).count();
    let (d, t) = if g.items_total > 0 {
        (g.items_done, g.items_total)
    } else {
        let d = items.iter().filter(|j| j.state == State::Done).count() as u32;
        (d, items.len() as u32)
    };
    if items.iter().any(|j| j.state.running()) {
        v.state = State::Active;
    }
    if t > 0 {
        v.progress = (d as f32 / t as f32).min(1.0);
        v.done = 0;
        v.total = 0;
        v.speed = items.iter().map(|j| j.speed).sum();
        let mut line = vec![format!("{d} / {t}")];
        if failed > 0 {
            line.push(format!("{failed} failed"));
        }
        if v.state == State::Failed && !g.error.is_empty() {
            line.push(g.error.clone());
        } else if v.state.running() && !g.detail.is_empty() {
            line.push(g.detail.clone());
        }
        if v.speed > 0 && v.state == State::Active {
            line.push(format!("{}/s", size(v.speed)));
        }
        v.detail = line.join(" · ");
        // A failed group reads "Failed · 38 / 40 · 2 failed" (a cancelled one
        // "Cancelled · 38 / 40").
        v.error = v.detail.clone();
    }
    v
}

/// What the list shows, in order: each top-level job, then its items oldest first
/// (all of them when the group is expanded, else only the running ones). Items whose
/// group is gone show as top-level jobs. `(job, depth, item count)`.
pub fn visible(
    jobs: &[Job],
    expanded: &std::collections::HashSet<String>,
) -> Vec<(Job, u8, usize)> {
    let has = |j: &Job| {
        !j.parent.is_empty()
            && jobs
                .iter()
                .any(|g| g.plugin == j.plugin && g.job == j.parent && g.parent.is_empty())
    };
    let mut out = Vec::new();
    for g in jobs.iter().filter(|j| !has(j)) {
        let items: Vec<&Job> = jobs
            .iter()
            .rev()
            .filter(|k| k.plugin == g.plugin && k.parent == g.job && has(k))
            .collect();
        if items.is_empty() && g.items_total == 0 {
            out.push((g.clone(), 0, 0));
            continue;
        }
        out.push((group_view(g, &items), 0, items.len()));
        let open = expanded.contains(&key(&g.plugin, &g.job));
        for k in items {
            if open || matches!(k.state, State::Active | State::Paused) {
                out.push((k.clone(), 1, 0));
            }
        }
    }
    out
}

fn share(j: &Job) -> f32 {
    if j.state == State::Done {
        1.0
    } else if j.total > 0 {
        (j.done as f32 / j.total as f32).min(1.0)
    } else {
        j.progress
    }
}

pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>) {
    // Packages check this before handing their queue to the panel (older Vivi: unset).
    // An integer, so `mp.get_property` reads `1` (a string node would read `"1"`, with
    // the quotes, and `== "1"` checks in packages would never see the panel).
    if let Err(e) = player.mpv.set_property("user-data/vivi/downloads", 1i64) {
        eprintln!("vivi: set user-data/vivi/downloads: {e:?}");
    }
    let model: Rc<VecModel<crate::DlJob>> = Rc::new(VecModel::default());
    let g = app.global::<crate::Downloads>();
    g.set_jobs(ModelRc::from(model.clone()));
    UI.with(|u| {
        *u.borrow_mut() = Some(Ui {
            app: app.as_weak(),
            player: player.clone(),
            cfg,
            model,
        })
    });
    let jobs: Vec<Job> = std::fs::read_to_string(file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.jobs = restore(jobs);
        s.dirty = true;
    });
    g.on_act(|id, action| act(&id, &action));
    g.on_clear_finished(|| {
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.jobs.retain(|j| j.state.running());
            s.dirty = true;
            s.save = true;
        });
        refresh();
    });
    g.on_run(|plugin, cmd, arg| {
        let args: Vec<String> = if arg.is_empty() {
            Vec::new()
        } else {
            vec![arg.to_string()]
        };
        crate::plugins::run(&plugin, &cmd, &args);
    });
    let t = Box::leak(Box::new(slint::Timer::default()));
    t.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(500),
        refresh,
    );
    refresh();
}

/// A row button: `id` is "<plugin>\u{1f}<job>".
fn act(id: &str, action: &str) {
    let Some((plugin, job)) = id.split_once('\u{1f}') else {
        return;
    };
    let j = ST.with(|s| {
        s.borrow()
            .jobs
            .iter()
            .find(|j| j.plugin == plugin && j.job == job)
            .cloned()
    });
    let Some(j) = j else { return };
    let ui = UI.with(|u| {
        u.borrow()
            .as_ref()
            .and_then(|u| Some((u.app.upgrade()?, u.player.clone())))
    });
    match action {
        "pause" | "resume" | "cancel" | "retry" => {
            let ok = crate::plugins::run(plugin, &format!("download-{action}"), &[job.to_string()]);
            if !ok {
                if let Some((a, _)) = &ui {
                    crate::plugins::toast(a, "The extension that made this download is off");
                }
            } else if action == "cancel" && j.state == State::Queued {
                report(
                    plugin,
                    job,
                    Report {
                        state: Some("cancelled".into()),
                        error: Some("Cancelled".into()),
                        ..Report::default()
                    },
                );
            }
        }
        "discard" => {
            let k = key(plugin, job);
            let ok = if j.can_discard {
                crate::plugins::run(plugin, "download-discard", &[job.to_string()])
            } else {
                let ok = crate::plugins::run(plugin, "download-cancel", &[job.to_string()]);
                if ok && j.state == State::Queued {
                    // Nothing was written yet: gone at once.
                    remove(plugin, job);
                    if let Some((a, _)) = &ui {
                        crate::plugins::toast(a, &format!("Discarded: {}", j.title));
                    }
                } else if ok {
                    ST.with(|s| s.borrow_mut().discarding.insert(k));
                }
                ok
            };
            if !ok {
                if let Some((a, _)) = &ui {
                    crate::plugins::toast(a, "The extension that made this download is off");
                }
            }
        }
        "play" => {
            if let Some((a, p)) = ui {
                if std::path::Path::new(&j.path).is_file() {
                    a.global::<crate::Overlays>().set_library_open(false);
                    p.open(&j.path);
                } else {
                    crate::plugins::toast(&a, "The file is gone");
                }
            }
        }
        "open" => crate::settings::open_url(&j.path),
        "folder" => {
            let p = j.path.clone();
            std::thread::spawn(move || crate::libview::reveal(&p));
        }
        "remove" => remove(plugin, job),
        "expand" => ST.with(|s| {
            let mut s = s.borrow_mut();
            let k = key(plugin, job);
            if !s.expanded.remove(&k) {
                s.expanded.insert(k);
            }
            s.dirty = true;
        }),
        _ => {}
    }
    refresh();
}

fn to_row(
    (j, depth, kids): &(Job, u8, usize),
    names: &[(String, String, String)],
    expanded: &std::collections::HashSet<String>,
) -> crate::DlJob {
    let (name, icon) = names
        .iter()
        .find(|(id, _, _)| *id == j.plugin)
        .map_or((j.plugin.clone(), "plugins".to_string()), |(_, n, i)| {
            (n.clone(), i.clone())
        });
    // Decoded once at row size through the image cache (not from disk on every refresh).
    let art = art_file(&j.artwork).and_then(|p| crate::images::get(&p, 88, |_| mark_dirty()));
    let detail = match (j.subtitle.is_empty(), detail(j)) {
        (true, d) => d,
        (false, d) => format!("{} · {d}", j.subtitle),
    };
    crate::DlJob {
        id: key(&j.plugin, &j.job).into(),
        title: j.title.as_str().into(),
        plugin: name.into(),
        icon: icon.into(),
        has_art: art.is_some(),
        art: art.unwrap_or_default(),
        state: j.state.name().into(),
        progress: share(j),
        detail: detail.into(),
        can_pause: j.can_pause,
        // Not on a group (its items are discarded one by one) or one of its items.
        can_discard: j.state.running() && *kids == 0 && *depth == 0 && j.parent.is_empty(),
        has_file: !j.path.is_empty() && j.state == State::Done && *kids == 0,
        child: *depth > 0,
        kids: *kids as i32,
        expanded: expanded.contains(&key(&j.plugin, &j.job)),
    }
}

/// Open Library → Downloads (tray menu, `vivi-downloads-show`).
pub fn open(a: &AppWindow) {
    a.global::<crate::Library>()
        .invoke_navigate("downloads".into());
    a.global::<crate::Overlays>().set_library_open(true);
}

/// Push the list to the UI, save, notify and update the tray line.
pub fn refresh() {
    let (jobs, save, news, expanded, toasts) = ST.with(|s| {
        let mut s = s.borrow_mut();
        if !s.dirty {
            return (None, false, Vec::new(), Default::default(), Vec::new());
        }
        s.dirty = false;
        let save = std::mem::take(&mut s.save);
        (
            Some(s.jobs.clone()),
            save,
            std::mem::take(&mut s.news),
            s.expanded.clone(),
            std::mem::take(&mut s.toasts),
        )
    });
    let Some(jobs) = jobs else { return };
    if save {
        if let Ok(t) = serde_json::to_string_pretty(&jobs) {
            let _ = std::fs::create_dir_all(crate::store::data_dir());
            if let Err(e) = std::fs::write(file(), t) {
                eprintln!("vivi: save downloads: {e}");
            }
        }
    }
    let names: Vec<(String, String, String)> = crate::plugins::manifests()
        .into_iter()
        .map(|m| (m.id, m.name, m.icon))
        .collect();
    let line = summary_line(&jobs);
    crate::tray::set_downloads(&line);
    let Some((a, model, cfg)) = UI.with(|u| {
        u.borrow()
            .as_ref()
            .and_then(|u| Some((u.app.upgrade()?, u.model.clone(), u.cfg.clone())))
    }) else {
        return;
    };
    let rows: Vec<crate::DlJob> = visible(&jobs, &expanded)
        .iter()
        .map(|r| to_row(r, &names, &expanded))
        .collect();
    if rows.len() == model.row_count() {
        for (i, r) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&r) {
                model.set_row_data(i, r);
            }
        }
    } else {
        model.set_vec(rows);
    }
    let g = a.global::<crate::Downloads>();
    g.set_active(summary(&jobs).0 as i32);
    g.set_line(line.into());
    g.set_finished(jobs.iter().any(|j| !j.state.running()));
    // Panel actions from plugins (`downloads.action`, `downloads.url`).
    let acts: Vec<crate::ExtAction> =
        crate::plugins::contributions(crate::plugins::contrib::DOWNLOADS_ACTION)
            .into_iter()
            .map(|e| crate::ExtAction {
                id: e.id.into(),
                title: e.command.into(),
                label: e.label.into(),
            })
            .collect();
    g.set_actions(ModelRc::new(VecModel::from(acts)));
    let url = crate::plugins::contributions(crate::plugins::contrib::DOWNLOADS_URL)
        .into_iter()
        .next();
    g.set_url_plugin(url.as_ref().map_or("".into(), |e| e.id.as_str().into()));
    g.set_url_command(
        url.as_ref()
            .map_or("".into(), |e| e.command.as_str().into()),
    );
    g.set_url_label(url.map_or("".into(), |e| e.label.into()));
    for t in toasts {
        crate::plugins::toast(&a, &t);
    }
    for (title, ok, text) in news {
        let (t, b) = if ok {
            (done_heading(&text), title)
        } else {
            ("Download failed", format!("{title}: {text}"))
        };
        if !(crate::notify::enabled(&cfg.borrow()) && crate::notify::send(t, &b)) {
            crate::plugins::toast(&a, &crate::notify::toast_text(t, &b));
        }
    }
}

/// The notification heading of a finished job: its `done_title`, else "Download finished".
fn done_heading(done_title: &str) -> &str {
    match done_title.trim() {
        "" => "Download finished",
        t => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_states() {
        let job = |state, done, total| Job {
            state,
            done,
            total,
            ..Job::default()
        };
        let jobs = [
            job(State::Active, 300, 1000),
            job(State::Queued, 0, 0),
            job(State::Paused, 200, 1000),
            job(State::Done, 5, 5),
        ];
        assert_eq!(
            summary_line(&jobs),
            "1 downloading · 1 queued · 1 paused · 25%"
        );
        assert_eq!(summary_line(&jobs[1..2]), "1 queued");
    }

    #[test]
    fn reports_merge_and_restore() {
        let mut j = Job::default();
        let r: Report = serde_json::from_str(
            r#"{"title":"Song","state":"active","done":500,"total":1000,"can_pause":true}"#,
        )
        .unwrap();
        assert_eq!(r.apply(&mut j), Some(State::Queued));
        assert_eq!(
            (j.state, j.done, j.total, j.can_pause),
            (State::Active, 500, 1000, true)
        );
        assert_eq!(share(&j), 0.5);
        assert_eq!(detail(&j), "500 B of 1 KB");
        j.speed = 100;
        assert_eq!(detail(&j), "500 B of 1 KB · 100 B/s · 5s left");
        assert_eq!(
            summary_line(std::slice::from_ref(&j)),
            "1 downloading · 50%"
        );
        let done = Report {
            state: Some("done".into()),
            ..Report::default()
        };
        assert_eq!(done.apply(&mut j), Some(State::Active));
        assert_eq!((j.done, detail(&j)), (1000, "Done · 1 KB".to_string()));
        assert_eq!(summary_line(std::slice::from_ref(&j)), "");
        let mut q = j.clone();
        q.state = State::Active;
        let r = restore(vec![q]);
        assert_eq!(r[0].state, State::Failed);
        assert_eq!(detail(&r[0]), "Failed · Interrupted when Vivi closed");
        assert_eq!(State::parse("recording"), Some(State::Active));
        assert_eq!(size(1_500_000), "1.5 MB");
    }

    #[test]
    fn subtitle_and_artwork() {
        let mut j = Job::default();
        let r: Report = serde_json::from_str(
            r#"{"title":"Song","subtitle":"Artist · Album","artwork":"https://i.example/c.jpg"}"#,
        )
        .unwrap();
        r.apply(&mut j);
        assert_eq!(j.subtitle, "Artist · Album");
        assert_eq!(j.artwork, "https://i.example/c.jpg");
        // Fields left out keep their value.
        let r: Report = serde_json::from_str(r#"{"state":"active"}"#).unwrap();
        r.apply(&mut j);
        assert_eq!(j.subtitle, "Artist · Album");
        assert_eq!(art_file(""), None);
        assert_eq!(art_file("/no/such/cover.jpg"), None);
        let f = std::env::temp_dir().join("vivi-dl-art-test.jpg");
        std::fs::write(&f, b"x").unwrap();
        let url = format!("file://{}", f.display());
        assert_eq!(art_file(&url), Some(f.clone()));
        assert_eq!(art_file(&f.display().to_string()), Some(f.clone()));
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn groups_count_items_and_collapse() {
        let job = |id: &str, parent: &str, state| Job {
            plugin: "p".into(),
            job: id.into(),
            parent: parent.into(),
            state,
            ..Job::default()
        };
        // Newest first, as `report` inserts them.
        let mut jobs = vec![
            job("t3", "pl", State::Queued),
            job("t2", "pl", State::Active),
            job("t1", "pl", State::Done),
            job("solo", "", State::Done),
            job("pl", "", State::Queued),
            job("lost", "gone", State::Failed),
        ];
        let none = Default::default();
        let v = visible(&jobs, &none);
        let ids: Vec<(&str, u8, usize)> =
            v.iter().map(|(j, d, k)| (j.job.as_str(), *d, *k)).collect();
        // Collapsed: the group, then only its running-now item; orphans stay visible.
        assert_eq!(
            ids,
            [("solo", 0, 0), ("pl", 0, 3), ("t2", 1, 0), ("lost", 0, 0)]
        );
        assert_eq!(v[1].0.state, State::Active);
        assert_eq!(v[1].0.detail, "1 / 3");
        assert!((v[1].0.progress - 1.0 / 3.0).abs() < 1e-6);
        let open: std::collections::HashSet<String> = [key("p", "pl")].into();
        let ids: Vec<String> = visible(&jobs, &open)
            .iter()
            .map(|(j, _, _)| j.job.clone())
            .collect();
        // Expanded: items oldest first (queue order).
        assert_eq!(ids, ["solo", "pl", "t1", "t2", "t3", "lost"]);
        // Group rows are not counted in the summary.
        assert_eq!(summary(&jobs).0, 2);
        // The plugin's own counts win ("37 / 40": most tracks were on disk already).
        jobs[4].items_done = 37;
        jobs[4].items_total = 40;
        jobs[0].state = State::Failed;
        jobs[1].state = State::Done;
        jobs[4].state = State::Failed;
        jobs[4].error = "1 track failed".into();
        let g = &visible(&jobs, &none)[1].0;
        assert_eq!(g.state, State::Failed);
        assert_eq!(detail(g), "Failed · 37 / 40 · 1 failed · 1 track failed");
        // Items finish quietly; removing the group drops its items.
        ST.with(|s| *s.borrow_mut() = Store::default());
        report(
            "p",
            "pl",
            Report {
                title: Some("Mix".into()),
                ..Report::default()
            },
        );
        let item = |st: &str| Report {
            parent: Some("pl".into()),
            state: Some(st.into()),
            ..Report::default()
        };
        report("p", "a", item("done"));
        report(
            "p",
            "pl",
            Report {
                state: Some("done".into()),
                ..Report::default()
            },
        );
        ST.with(|s| {
            let s = s.borrow();
            assert_eq!(s.news.len(), 1);
            assert_eq!(s.news[0].0, "Mix");
        });
        remove("p", "pl");
        ST.with(|s| assert!(s.borrow().jobs.is_empty()));
    }

    #[test]
    fn done_title_names_the_finished_notification() {
        ST.with(|s| *s.borrow_mut() = Store::default());
        let news = || ST.with(|s| std::mem::take(&mut s.borrow_mut().news));
        let rep = |st: &str, done_title: Option<&str>| Report {
            title: Some("clip.mp4".into()),
            state: Some(st.into()),
            done_title: done_title.map(Into::into),
            ..Report::default()
        };
        // Old plugins send no done_title: "Download finished".
        report("yt", "a", rep("active", None));
        report("yt", "a", rep("done", None));
        let n = news();
        assert_eq!(n, [("clip.mp4".to_string(), true, String::new())]);
        assert_eq!(done_heading(&n[0].2), "Download finished");
        // A plugin's own heading, given with any report, is kept for the finish.
        report("cut-export", "1", rep("active", Some("Export finished")));
        report("cut-export", "1", rep("done", None));
        let n = news();
        assert_eq!(done_heading(&n[0].2), "Export finished");
        assert_eq!(
            crate::notify::toast_text(done_heading(&n[0].2), &n[0].0),
            "Export finished: clip.mp4"
        );
        // Failures keep "Download failed" with the error.
        report("cut-export", "2", rep("active", Some("Export finished")));
        report(
            "cut-export",
            "2",
            Report {
                error: Some("ffmpeg failed".into()),
                ..rep("failed", None)
            },
        );
        assert_eq!(news()[0].2, "ffmpeg failed");
        // Saved with the job; downloads.json without the field still loads.
        let j = ST.with(|s| s.borrow().jobs[1].clone());
        let back: Job = serde_json::from_str(&serde_json::to_string(&j).unwrap()).unwrap();
        assert_eq!(back.done_title, "Export finished");
        let old: Job = serde_json::from_str(r#"{"plugin":"p","job":"j","state":"done"}"#).unwrap();
        assert_eq!(done_heading(&old.done_title), "Download finished");
        assert_eq!(done_heading("  "), "Download finished");
    }

    #[test]
    fn resync_without_new_items_is_quiet() {
        ST.with(|s| *s.borrow_mut() = Store::default());
        let group = |st: &str, n: u32| Report {
            title: Some("Mix".into()),
            state: Some(st.into()),
            items_done: Some(n),
            items_total: Some(3),
            ..Report::default()
        };
        let news = || ST.with(|s| std::mem::take(&mut s.borrow_mut().news).len());
        report("p", "pl", group("active", 0));
        report("p", "pl", group("done", 3));
        assert_eq!(news(), 1, "first download notifies");
        report("p", "pl", group("active", 3));
        report("p", "pl", group("done", 3));
        assert_eq!(news(), 0, "nothing new: quiet");
        report("p", "pl", group("active", 3));
        report(
            "p",
            "x",
            Report {
                parent: Some("pl".into()),
                state: Some("done".into()),
                ..Report::default()
            },
        );
        report("p", "pl", group("done", 3));
        assert_eq!(news(), 1, "an item finished: notifies");
        // The plugin's order: "active" with no items yet, then the track list (no state change).
        let fetching = Report {
            state: Some("active".into()),
            items_done: Some(0),
            items_total: Some(0),
            ..Report::default()
        };
        report("p", "pl", fetching);
        report("p", "pl", group("active", 3));
        report("p", "pl", group("done", 3));
        assert_eq!(news(), 0, "track list after active, nothing new: quiet");
        report("p", "pl", group("active", 3));
        report("p", "pl", group("failed", 3));
        assert_eq!(news(), 1, "failures always notify");
        report(
            "p",
            "one",
            Report {
                state: Some("done".into()),
                ..Report::default()
            },
        );
        assert_eq!(news(), 1, "single downloads notify");
    }

    #[test]
    fn fractional_numbers_are_rounded_not_dropped() {
        let r: Report = serde_json::from_str(
            r#"{"state":"active","done":1500.4,"total":"3000","speed":512.6,"eta":2.5,
                "progress":0.5,"items_done":1.0,"items_total":3.7}"#,
        )
        .unwrap();
        assert_eq!(
            (r.done, r.total, r.speed, r.eta, r.items_done, r.items_total),
            (Some(1500), Some(3000), Some(513), Some(3), Some(1), Some(4))
        );
        let mut j = Job::default();
        r.apply(&mut j);
        assert_eq!(j.speed, 513);
        // The reported ETA wins over the one from the speed.
        assert_eq!(detail(&j), "2 KB of 3 KB · 513 B/s · 3s left");
        // Negative, null and non-numbers: 0 / left out, the rest of the report still counts.
        let r: Report = serde_json::from_str(
            r#"{"title":"T","done":-3.2,"total":null,"speed":"n/a","percent":42.5}"#,
        )
        .unwrap();
        assert_eq!((r.done, r.total, r.speed), (Some(0), None, None));
        assert_eq!(r.title.as_deref(), Some("T"));
        let mut j = Job::default();
        r.apply(&mut j);
        assert!((j.progress - 0.425).abs() < 1e-6);
        // `progress` wins over `percent`; huge values saturate.
        let r: Report =
            serde_json::from_str(r#"{"progress":0.1,"percent":90,"items_total":1e12}"#).unwrap();
        assert_eq!(r.items_total, Some(u32::MAX));
        r.apply(&mut j);
        assert!((j.progress - 0.1).abs() < 1e-6);
        // Not a number at all (an object) still fails the report, as before.
        assert!(serde_json::from_str::<Report>(r#"{"done":{}}"#).is_err());
    }

    #[test]
    fn cancel_is_its_own_state_without_a_failure_notice() {
        assert_eq!(State::parse("cancelled"), Some(State::Cancelled));
        assert_eq!(State::parse("canceled"), Some(State::Cancelled));
        assert_eq!(State::parse("failed"), Some(State::Failed));
        assert_eq!(State::parse("error"), Some(State::Failed));
        assert_eq!(State::Cancelled.name(), "cancelled");
        assert!(!State::Cancelled.running());
        let job = |state, error: &str| Job {
            state,
            error: error.into(),
            detail: "Starting…".into(),
            ..Job::default()
        };
        assert_eq!(detail(&job(State::Cancelled, "Cancelled")), "Cancelled");
        assert_eq!(detail(&job(State::Cancelled, "")), "Cancelled");
        assert_eq!(detail(&job(State::Failed, "HTTP 403")), "Failed · HTTP 403");
        // A cancelled group keeps its counts.
        let mut g = job(State::Cancelled, "Cancelled");
        g.items_done = 3;
        g.items_total = 10;
        assert_eq!(detail(&group_view(&g, &[])), "Cancelled · 3 / 10");
        // Saved and restored as cancelled (not turned into "Interrupted").
        let t = serde_json::to_string(&job(State::Cancelled, "Cancelled")).unwrap();
        assert!(t.contains(r#""state":"cancelled""#));
        let back: Job = serde_json::from_str(&t).unwrap();
        assert_eq!(restore(vec![back])[0].state, State::Cancelled);
        // No "Download failed" for a cancel; real failures still notify.
        ST.with(|s| *s.borrow_mut() = Store::default());
        let news = || ST.with(|s| std::mem::take(&mut s.borrow_mut().news));
        let st = |st: &str, e: &str| Report {
            title: Some("Video".into()),
            state: Some(st.into()),
            error: Some(e.into()),
            ..Report::default()
        };
        report("yt", "a", st("active", ""));
        report("yt", "a", st("cancelled", "Cancelled"));
        assert!(news().is_empty());
        report("yt", "a", st("active", ""));
        report("yt", "a", st("failed", "HTTP 403"));
        assert_eq!(
            news(),
            [("Video".to_string(), false, "HTTP 403".to_string())]
        );
        // A cancelled group is quiet too, and its items' cancels do not count as failed.
        report("yt", "pl", st("active", ""));
        report(
            "yt",
            "i1",
            Report {
                parent: Some("pl".into()),
                state: Some("cancelled".into()),
                ..Report::default()
            },
        );
        report("yt", "pl", st("cancelled", "Cancelled"));
        assert!(news().is_empty());
        let jobs = ST.with(|s| s.borrow().jobs.clone());
        let g = &visible(&jobs, &Default::default())[0].0;
        assert_eq!(g.state, State::Cancelled);
        assert_eq!(detail(g), "Cancelled · 0 / 1");
    }

    #[test]
    fn discard_deletes_the_file_and_its_part() {
        assert!(discard_targets("").is_empty());
        let t = discard_targets("/tmp/a b.mp3");
        assert_eq!(t[0], std::path::PathBuf::from("/tmp/a b.mp3"));
        assert_eq!(t[1], std::path::PathBuf::from("/tmp/a b.mp3.part"));
        assert_eq!(discard_targets("file:///x/y.mp3")[0], std::path::PathBuf::from("/x/y.mp3"));
    }

    #[test]
    fn host_discard_removes_the_row_and_file_when_the_job_ends() {
        // A plugin without download-discard (Offline Downloads 1.1.4): the host sent a
        // Cancel; the recording then reports done with its file (Cancel keeps it).
        let dir = std::env::temp_dir().join(format!("vivi-discard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("rec.mp3");
        std::fs::write(&f, b"abc").unwrap();
        std::fs::write(dir.join("rec.mp3.part"), b"x").unwrap();
        let path = f.to_string_lossy().to_string();
        let rep = |state: &str| Report {
            state: Some(state.into()),
            title: Some("Radio".into()),
            path: Some(path.clone()),
            ..Report::default()
        };
        report("t-plugin", "j1", rep("active"));
        ST.with(|s| s.borrow_mut().discarding.insert(key("t-plugin", "j1")));
        report("t-plugin", "j1", rep("active")); // still running: kept
        assert!(f.is_file());
        report("t-plugin", "j1", rep("done"));
        assert!(!f.is_file() && !dir.join("rec.mp3.part").exists());
        let (gone, toast, news) = ST.with(|s| {
            let s = s.borrow();
            (
                !s.jobs.iter().any(|j| j.job == "j1"),
                s.toasts.iter().any(|t| t == "Discarded: Radio"),
                s.news.iter().any(|n| n.0 == "Radio"),
            )
        });
        assert!(gone && toast && !news, "row removed, a toast, no finished notification");
        // A late report (its length probe) does not bring the row back ...
        report("t-plugin", "j1", rep("done"));
        assert!(ST.with(|s| !s.borrow().jobs.iter().any(|j| j.job == "j1")));
        // ... but the same job starting again does.
        report("t-plugin", "j1", rep("queued"));
        assert!(ST.with(|s| s.borrow().jobs.iter().any(|j| j.job == "j1")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_discarded_job_reads_discarded() {
        let j = Job {
            state: State::Cancelled,
            error: "Discarded".into(),
            ..Job::default()
        };
        assert_eq!(detail(&j), "Discarded");
    }
}
