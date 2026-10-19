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
//!   <job id>` back (a built-in's `command`, a package's `script-message-to`).
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
}

impl State {
    pub fn parse(s: &str) -> Option<State> {
        Some(match s {
            "queued" => State::Queued,
            "active" | "downloading" | "recording" => State::Active,
            "paused" => State::Paused,
            "done" | "finished" => State::Done,
            "failed" | "error" | "cancelled" => State::Failed,
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
    /// Saved file (or the file being written).
    pub path: String,
    /// Thumbnail: a local image file, or an http(s) URL fetched into the plugin cover
    /// cache.
    pub artwork: String,
    pub error: String,
    /// Free text under the title instead of size / ETA (e.g. "Recording · 12:30").
    pub detail: String,
    pub can_pause: bool,
    /// Unix seconds it was added.
    pub added: u64,
    /// Job id of the group row this one belongs to (same plugin), e.g. a playlist.
    pub parent: String,
    /// A group row's items done / in total ("12 / 40"); 0 in total = count its rows.
    pub items_done: u32,
    pub items_total: u32,
}

/// What a plugin reports (fields left `None` keep their value).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Report {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub state: Option<String>,
    pub done: Option<u64>,
    pub total: Option<u64>,
    pub progress: Option<f32>,
    pub speed: Option<u64>,
    pub path: Option<String>,
    pub artwork: Option<String>,
    pub error: Option<String>,
    pub detail: Option<String>,
    pub can_pause: Option<bool>,
    pub parent: Option<String>,
    pub items_done: Option<u32>,
    pub items_total: Option<u32>,
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
        if let Some(v) = self.progress {
            j.progress = v;
        }
        if let Some(v) = self.speed {
            j.speed = v;
        }
        if let Some(v) = self.can_pause {
            j.can_pause = v;
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
    /// Finished / failed since the last refresh: (title, ok, error).
    news: Vec<(String, bool, String)>,
    /// Group rows shown with all their items (keys).
    expanded: std::collections::HashSet<String>,
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
        if changed.is_some() {
            s.save = true;
            let j = &s.jobs[i];
            // A group's items finish quietly; the group row notifies.
            let news = match j.state {
                _ if !j.parent.is_empty() => None,
                State::Done => Some((j.title.clone(), true, String::new())),
                State::Failed => Some((j.title.clone(), false, j.error.clone())),
                _ => None,
            };
            s.news.extend(news);
        }
        s.dirty = true;
    });
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
    if !j.detail.is_empty() && j.state != State::Failed {
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
        State::Active => {
            let mut v = Vec::new();
            if j.total > 0 {
                v.push(format!("{} of {}", size(j.done), size(j.total)));
            } else if j.done > 0 {
                v.push(size(j.done));
            }
            if j.speed > 0 {
                v.push(format!("{}/s", size(j.speed)));
                if j.total > j.done {
                    v.push(eta((j.total - j.done) / j.speed.max(1)));
                }
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
        // A failed group reads "Failed · 38 / 40 · 2 failed".
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
                        state: Some("failed".into()),
                        error: Some("Cancelled".into()),
                        ..Report::default()
                    },
                );
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
    let (jobs, save, news, expanded) = ST.with(|s| {
        let mut s = s.borrow_mut();
        if !s.dirty {
            return (None, false, Vec::new(), Default::default());
        }
        s.dirty = false;
        let save = std::mem::take(&mut s.save);
        (
            Some(s.jobs.clone()),
            save,
            std::mem::take(&mut s.news),
            s.expanded.clone(),
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
    for (title, ok, err) in news {
        let (t, b) = if ok {
            ("Download finished", title)
        } else {
            ("Download failed", format!("{title}: {err}"))
        };
        if !(crate::notify::enabled(&cfg.borrow()) && crate::notify::send(t, &b)) {
            crate::plugins::toast(&a, &crate::notify::toast_text(t, &b));
        }
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
}
