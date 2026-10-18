//! Host services for downloadable plugins (packages, mpv Lua/JS scripts).
//!
//! Scripts cannot draw Slint, so Vivi draws for them. A package sends
//! `script-message <name> <id> …` broadcasts and gets the user's answers back as
//! `script-message-to <script> vivi-ui <json>`:
//!
//! * `vivi-dialog <id> <json>`: open / update the shared music dialog (rows with covers,
//!   list / grid, sources, chips, filters, details, actions, paging) owned by `id`.
//! * `vivi-dialog-close <id>`.
//! * `vivi-confirm <id> <key> <json>`: a warning / confirm dialog; the answer comes back
//!   as the `confirm` event with `[key, "yes" | "no"]`.
//! * `vivi-media-meta <id> <json>`: title / artist / album / cover of URLs the package
//!   queued; Vivi shows them on the now-playing card (and as the cover in Recent).
//! * `vivi-protect <id> <path>` / `vivi-wipe <id> <path>`: make a file in the package's
//!   own data folder private (0600) or overwrite and delete it (credentials).
//!
//! Covers are http(s) URLs or local paths; Vivi downloads them into its image cache
//! (`plugin-covers`) and decodes them at row / tile size. Packages that never send these
//! messages are unaffected. See docs/plugins-architecture.md ("Host API for packages").

use serde::Deserialize;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// One result row (`rows` of [`DialogUpdate`]).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Row {
    pub title: String,
    pub sub: String,
    /// Small badge on the row / tile ("Song", "Album", a source name).
    pub badge: String,
    /// Outlined chip after the badge ("♥ Liked", a licence); hidden when empty.
    pub chip: String,
    /// The chip in the success colour.
    pub chip_ok: bool,
    pub dur: String,
    /// Cover: http(s) URL or local path.
    pub cover: String,
    /// The row's download button is enabled.
    pub dl: bool,
    pub selected: bool,
}

/// A dialog filter dropdown.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Filter {
    pub name: String,
    pub options: Vec<String>,
    pub value: String,
}

/// `vivi-dialog` payload: every field is optional; only the given ones change.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct DialogUpdate {
    /// Open the dialog (and take it over from its current owner).
    pub open: bool,
    pub title: Option<String>,
    pub placeholder: Option<String>,
    pub search_label: Option<String>,
    pub query: Option<String>,
    pub sources: Option<Vec<String>>,
    pub source: Option<String>,
    pub chips: Option<Vec<String>>,
    pub chip: Option<String>,
    pub actions: Option<Vec<String>>,
    pub filters: Option<Vec<Filter>>,
    pub rows: Option<Vec<Row>>,
    pub status: Option<String>,
    pub busy: Option<bool>,
    pub detail: Option<String>,
    pub primary: Option<String>,
    pub secondary: Option<String>,
    pub has_more: Option<bool>,
    /// Offer the list / grid toggle.
    pub grid_toggle: Option<bool>,
    /// Show cover tiles instead of rows.
    pub grid: Option<bool>,
    /// Text the *Copy* action puts on the clipboard.
    pub copy: Option<String>,
}

/// `vivi-confirm` payload.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Confirm {
    pub title: String,
    pub heading: String,
    pub text: String,
    pub ok: String,
}

/// `vivi-media-meta` payload: one URL or `{"items": [...]}`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Meta {
    pub url: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: String,
    /// First part of the card's info line (e.g. the service name).
    pub info: String,
    pub items: Vec<Meta>,
}

/// Most URLs remembered for the now-playing card.
const META_MAX: usize = 5000;

#[derive(Default)]
struct Host {
    /// Packages that opened a dialog / confirm this run (their UI events are forwarded).
    owners: HashSet<String>,
    rows: Vec<Row>,
    grid: bool,
    /// The dialog's row model, updated in place (a new model would rebuild every delegate).
    model: Option<Rc<VecModel<crate::FmRow>>>,
    /// Covers being decoded (url, size): asked once, not again on every refresh.
    decoding: HashSet<(String, u32)>,
    /// A coalesced [`refresh_rows`] is scheduled.
    refresh_queued: bool,
    asked: HashSet<String>,
    /// Downloaded cover files by URL.
    files: HashMap<String, PathBuf>,
    meta: HashMap<String, (String, Meta)>,
    np_url: String,
    np_shown: bool,
    timer: Option<slint::Timer>,
}

thread_local! {
    static HOST: RefCell<Host> = RefCell::new(Host::default());
}

/// Whether a package described `url` with `vivi-media-meta` (its now-playing card is
/// drawn here, not by Network Streams).
pub fn has_meta(url: &str) -> bool {
    HOST.with(|h| h.borrow().meta.contains_key(url))
}

/// Whether package `id` owns host UI this run (its dialog callbacks are forwarded).
pub fn owns(id: &str) -> bool {
    HOST.with(|h| h.borrow().owners.contains(id))
}

/// The cached cover file of `url` (Recent / library rows), if already downloaded.
pub fn cached_cover(url: &str) -> Option<PathBuf> {
    if url.is_empty() {
        return None;
    }
    let p = cover_base(url);
    ["jpg", "png"]
        .iter()
        .map(|e| p.with_extension(e))
        .find(|p| p.is_file())
}

fn cover_dir() -> PathBuf {
    crate::store::cache_dir().join("plugin-covers")
}

fn cover_base(url: &str) -> PathBuf {
    cover_dir().join(super::network_streams::hash_name(url))
}

/// Download `url` into the cover cache (local paths are used as they are).
fn fetch_cover(url: &str) -> Option<PathBuf> {
    if let Some(p) = url
        .strip_prefix("file://")
        .or(url.starts_with('/').then_some(url))
    {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    if let Some(p) = cached_cover(url) {
        return Some(p);
    }
    let base = cover_base(url);
    if base.with_extension("none").is_file() {
        return None;
    }
    std::fs::create_dir_all(cover_dir()).ok()?;
    let b = super::web::get_bytes(url, 4 * 1024 * 1024).ok().flatten();
    let ext = match b.as_deref() {
        Some([0x89, b'P', b'N', b'G', ..]) => "png",
        Some([0xFF, 0xD8, ..]) => "jpg",
        Some([b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..]) => {
            // WebP: re-encode (the image cache decodes PNG / JPEG).
            let img = image::load_from_memory(b.as_deref()?).ok()?;
            let p = base.with_extension("png");
            img.save_with_format(&p, image::ImageFormat::Png).ok()?;
            return Some(p);
        }
        _ => {
            let _ = std::fs::write(base.with_extension("none"), b"");
            return None;
        }
    };
    let p = base.with_extension(ext);
    std::fs::write(&p, b?).ok()?;
    Some(p)
}

/// Fetch the covers of `urls` on worker threads, then decode them in the image cache.
fn want_covers(urls: Vec<String>) {
    let urls: Vec<String> = HOST.with(|h| {
        let mut h = h.borrow_mut();
        urls.into_iter()
            .filter(|u| !u.is_empty() && h.asked.insert(u.clone()))
            .collect()
    });
    if urls.is_empty() {
        return;
    }
    let chunk = urls.len().div_ceil(4).max(1);
    for part in urls.chunks(chunk) {
        let part = part.to_vec();
        std::thread::spawn(move || {
            for u in part {
                let Some(p) = fetch_cover(&u) else { continue };
                let _ = slint::invoke_from_event_loop(move || {
                    HOST.with(|h| h.borrow_mut().files.insert(u, p));
                    schedule_refresh();
                });
            }
        });
    }
}

/// One [`refresh_rows`] (and now-playing cover) for any number of covers that arrived
/// within a frame or two. Calling it per cover rebuilt every row per decoded cover, and
/// each rebuild asked again for every cover still decoding: with ~1000 rows that never
/// finished (100% CPU, gigabytes of queued callbacks).
fn schedule_refresh() {
    let first = HOST.with(|h| !std::mem::replace(&mut h.borrow_mut().refresh_queued, true));
    if first {
        slint::Timer::single_shot(std::time::Duration::from_millis(40), || {
            HOST.with(|h| h.borrow_mut().refresh_queued = false);
            refresh_rows();
            refresh_np_cover();
        });
    }
}

/// A decoded cover of `url` at `size` px from the image cache (its byte budget bounds
/// memory), requesting the decode once when missing.
fn cover(url: &str, size: u32) -> Option<slint::Image> {
    let k = (url.to_string(), size);
    let (file, busy) = HOST.with(|h| {
        let h = h.borrow();
        (h.files.get(url).cloned(), h.decoding.contains(&k))
    });
    let file = file?;
    if busy {
        return None;
    }
    let k2 = k.clone();
    let now = crate::images::get(&file, size, move |_| {
        HOST.with(|h| h.borrow_mut().decoding.remove(&k2));
        schedule_refresh();
    });
    if now.is_none() {
        HOST.with(|h| h.borrow_mut().decoding.insert(k));
    }
    now
}

/// Rows whose covers are decoded: the visible window plus one window before and after.
fn cover_range(first: i32, count: i32, len: usize) -> std::ops::Range<usize> {
    let count = count.max(1) as usize;
    let first = first.max(0) as usize;
    let lo = first.saturating_sub(count);
    let hi = (first + 2 * count).min(len);
    lo.min(hi)..hi
}

fn app() -> Option<crate::AppWindow> {
    super::with(|r| r.app.upgrade()).flatten()
}

fn strings(v: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        v.iter()
            .map(|s| s.as_str().into())
            .collect::<Vec<SharedString>>(),
    ))
}

/// Rebuild the dialog's rows from the package's rows and the decoded covers.
fn refresh_rows() {
    let Some(a) = app() else { return };
    let st = a.global::<crate::MusicDialogState>();
    let (rows, grid) = HOST.with(|h| {
        let h = h.borrow();
        (h.rows.clone(), h.grid)
    });
    if rows.is_empty() || !owns(&st.get_owner()) {
        return;
    }
    let size = if grid {
        crate::images::TILE
    } else {
        crate::images::ROW
    };
    let near = cover_range(st.get_win_first(), st.get_win_count(), rows.len());
    let fm: Vec<crate::FmRow> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            // Rows far from the viewport hold no image (bounded memory).
            let img = near.contains(&i).then(|| cover(&r.cover, size)).flatten();
            crate::FmRow {
                title: r.title.as_str().into(),
                sub: r.sub.as_str().into(),
                badge: r.badge.as_str().into(),
                lic: r.chip.as_str().into(),
                free: r.chip_ok,
                dur: r.dur.as_str().into(),
                has_cover: img.is_some(),
                cover: img.unwrap_or_default(),
                dl: r.dl,
                selected: r.selected,
            }
        })
        .collect();
    let model = HOST.with(|h| h.borrow().model.clone());
    match model {
        // Same model still shown: change only the rows that differ, append new ones.
        Some(m) if same_model(&st.get_tracks(), &m) && m.row_count() <= fm.len() => {
            for (i, row) in fm.into_iter().enumerate() {
                if i < m.row_count() {
                    if m.row_data(i).as_ref() != Some(&row) {
                        m.set_row_data(i, row);
                    }
                } else {
                    m.push(row);
                }
            }
        }
        _ => {
            let m = Rc::new(VecModel::from(fm));
            st.set_tracks(ModelRc::from(m.clone()));
            HOST.with(|h| h.borrow_mut().model = Some(m));
        }
    }
}

fn same_model(shown: &ModelRc<crate::FmRow>, m: &Rc<VecModel<crate::FmRow>>) -> bool {
    shown
        .as_any()
        .downcast_ref::<VecModel<crate::FmRow>>()
        .map(|v| v as *const _)
        == Some(Rc::as_ptr(m))
}

/// Apply a `vivi-dialog` update from package `id`.
pub fn dialog(a: &crate::AppWindow, id: &str, u: DialogUpdate) {
    let st = a.global::<crate::MusicDialogState>();
    // Scrolling changes which rows get covers (see `cover_range`).
    st.on_window_changed(schedule_refresh);
    let ov = a.global::<crate::Overlays>();
    if st.get_owner() != id {
        if !u.open {
            return; // someone else's dialog is showing: wait for the next open
        }
        st.set_owner(id.into());
        st.set_query("".into());
        st.set_chip("".into());
        st.set_tracks(ModelRc::new(VecModel::from(Vec::<crate::FmRow>::new())));
        st.set_results(ModelRc::new(VecModel::from(Vec::<crate::Item>::new())));
        st.set_filters(ModelRc::new(VecModel::from(Vec::<crate::MdFilter>::new())));
        st.set_actions(strings(&[]));
        st.set_sources(strings(&[]));
        st.set_chips(strings(&[]));
        st.set_detail("".into());
        st.set_copy("".into());
        st.set_status("".into());
        st.set_has_more(false);
        st.set_title("".into());
        st.set_primary("Play".into());
        st.set_secondary("".into());
        st.set_search_label("Search".into());
        HOST.with(|h| h.borrow_mut().rows.clear());
    }
    HOST.with(|h| h.borrow_mut().owners.insert(id.to_string()));
    st.set_caps(crate::MdCaps {
        logos: false,
        chips: true,
        rich: true,
        grid: u.grid_toggle.unwrap_or(st.get_caps().grid),
    });
    macro_rules! set {
        ($f:ident, $s:ident) => {
            if let Some(v) = u.$f {
                st.$s(v.as_str().into());
            }
        };
    }
    set!(title, set_title);
    set!(placeholder, set_placeholder);
    set!(search_label, set_search_label);
    set!(query, set_query);
    set!(source, set_source);
    set!(chip, set_chip);
    set!(status, set_status);
    set!(detail, set_detail);
    set!(primary, set_primary);
    set!(secondary, set_secondary);
    set!(copy, set_copy);
    if let Some(v) = &u.sources {
        st.set_sources(strings(v));
    }
    if let Some(v) = &u.chips {
        st.set_chips(strings(v));
    }
    if let Some(v) = &u.actions {
        st.set_actions(strings(v));
    }
    if let Some(v) = u.busy {
        st.set_busy(v);
    }
    if let Some(v) = u.has_more {
        st.set_has_more(v);
    }
    if let Some(g) = u.grid {
        st.set_grid(g);
    }
    if let Some(f) = &u.filters {
        let f: Vec<crate::MdFilter> = f
            .iter()
            .map(|f| crate::MdFilter {
                name: f.name.as_str().into(),
                options: strings(&f.options),
                value: f.value.as_str().into(),
            })
            .collect();
        st.set_filters(ModelRc::new(VecModel::from(f)));
    }
    let grid = st.get_caps().grid && st.get_grid();
    HOST.with(|h| h.borrow_mut().grid = grid);
    if let Some(rows) = u.rows {
        // Plain items too: the dialog's selection / accessibility use them.
        let items: Vec<crate::Item> = rows
            .iter()
            .map(|r| crate::Item {
                key: r.title.as_str().into(),
                label: r.sub.as_str().into(),
                selected: r.selected,
            })
            .collect();
        st.set_results(ModelRc::new(VecModel::from(items)));
        let urls = rows.iter().map(|r| r.cover.clone()).collect();
        let empty = rows.is_empty();
        HOST.with(|h| h.borrow_mut().rows = rows);
        if empty {
            st.set_tracks(ModelRc::new(VecModel::from(Vec::<crate::FmRow>::new())));
        }
        refresh_rows();
        want_covers(urls);
    }
    if u.open {
        ov.set_settings_open(false);
        ov.set_md_open(true);
    }
}

/// `vivi-dialog-close <id>`.
pub fn dialog_close(a: &crate::AppWindow, id: &str) {
    if a.global::<crate::MusicDialogState>().get_owner() == id {
        a.global::<crate::Overlays>().set_md_open(false);
    }
}

/// The `vivi-ui` JSON for a dialog / confirm callback.
pub fn event_json(a: Option<&crate::AppWindow>, name: &str, args: &[String]) -> String {
    let mut v = serde_json::json!({ "event": name, "args": args });
    if let Some(a) = a {
        let st = a.global::<crate::MusicDialogState>();
        v["query"] = st.get_query().as_str().into();
        v["source"] = st.get_source().as_str().into();
        v["chip"] = st.get_chip().as_str().into();
        v["grid"] = (st.get_caps().grid && st.get_grid()).into();
    }
    v.to_string()
}

/// A UI callback addressed to package `id` (not a built-in plugin). Forwarded as
/// `script-message-to <script> vivi-ui <json>` when the package owns host UI.
pub fn ui_event(id: &str, name: &str, args: &[String]) {
    if !owns(id) {
        return;
    }
    let a = app();
    if name == "view" {
        let g = args.first().map(String::as_str) == Some("grid");
        HOST.with(|h| h.borrow_mut().grid = g);
        refresh_rows();
    }
    let json = event_json(a.as_ref(), name, args);
    super::run(id, "vivi-ui", &[json]);
}

/// `vivi-confirm <id> <key> <json>`.
pub fn confirm(a: &crate::AppWindow, id: &str, key: &str, c: Confirm) {
    HOST.with(|h| h.borrow_mut().owners.insert(id.to_string()));
    let ok = if c.ok.is_empty() { "OK" } else { &c.ok };
    crate::dialogs::confirm(a, id, key, &c.title, &c.heading, &c.text, ok);
}

/// `vivi-media-meta <id> <json>`.
pub fn media_meta(id: &str, m: Meta) {
    let mut list = m.items.clone();
    if !m.url.is_empty() {
        list.push(Meta {
            items: Vec::new(),
            ..m
        });
    }
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        if h.meta.len() + list.len() > META_MAX {
            h.meta.clear();
        }
        for x in list {
            h.meta.insert(x.url.clone(), (id.to_string(), x));
        }
        h.np_url.clear(); // re-read the playing entry
        if h.timer.is_none() {
            let t = slint::Timer::default();
            t.start(
                slint::TimerMode::Repeated,
                std::time::Duration::from_millis(500),
                now_playing,
            );
            h.timer = Some(t);
        }
    });
}

/// The now-playing card for URLs a package described.
fn now_playing() {
    let Some(a) = app() else { return };
    let Some(player) = super::with(|r| r.player.clone()) else {
        return;
    };
    let path = player.get("path").unwrap_or_default();
    let np = a.global::<crate::NowPlaying>();
    let m = HOST.with(|h| h.borrow().meta.get(&path).map(|m| m.1.clone()));
    let show = m.is_some()
        && a.global::<crate::Playback>().get_has_media()
        && a.global::<crate::Viz>().get_audio();
    let Some(m) = m.filter(|_| show) else {
        let was = HOST.with(|h| std::mem::take(&mut h.borrow_mut().np_shown));
        if was {
            np.set_show(false);
            HOST.with(|h| h.borrow_mut().np_url.clear());
        }
        return;
    };
    let changed = HOST.with(|h| {
        let mut h = h.borrow_mut();
        h.np_shown = true;
        let c = h.np_url != path;
        h.np_url = path.clone();
        c
    });
    if changed {
        np.set_station(m.title.as_str().into());
        let sub: Vec<&str> = [m.artist.as_str(), m.album.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        np.set_track(sub.join(" · ").into());
        np.set_has_logo(false);
        want_covers(vec![m.cover.clone()]);
        refresh_np_cover();
    }
    let g = |k: &str| player.get(k).filter(|v| !v.trim().is_empty());
    let mut info: Vec<String> = Vec::new();
    if !m.info.is_empty() {
        info.push(m.info.clone());
    }
    if let Some(c) = g("audio-codec-name") {
        info.push(c.to_uppercase());
    }
    if let Some(b) = g("audio-bitrate")
        .and_then(|b| b.parse::<f64>().ok())
        .filter(|b| *b > 0.0)
    {
        info.push(format!("{:.0} kbps", b / 1000.0));
    }
    np.set_info(info.join(" · ").into());
    if !np.get_show() {
        np.set_show(true);
    }
}

fn refresh_np_cover() {
    let (url, shown) = HOST.with(|h| {
        let h = h.borrow();
        let url = h
            .meta
            .get(&h.np_url)
            .map(|m| m.1.cover.clone())
            .unwrap_or_default();
        (url, h.np_shown)
    });
    if !shown || url.is_empty() {
        return;
    }
    if let (Some(img), Some(a)) = (cover(&url, crate::images::TILE), app()) {
        let np = a.global::<crate::NowPlaying>();
        np.set_logo(img);
        np.set_has_logo(true);
    }
}

/// The data folder of package `id` (`<data>/plugin-data/<id>`).
pub fn data_dir(id: &str) -> PathBuf {
    crate::store::data_dir().join("plugin-data").join(id)
}

/// `path` when it is inside `root` (no `..` escapes, symlinks resolved).
fn inside(root: &Path, path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    let canon = p.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    canon.starts_with(&root).then_some(canon)
}

/// `vivi-protect <id> <path>`: owner-only permissions (0600) on a file in the package's
/// data folder. No-op on Windows (files there are per-user already).
pub fn protect(id: &str, path: &str) -> bool {
    let Some(p) = inside(&data_dir(id), path) else {
        eprintln!("vivi: {id}: vivi-protect outside its data folder ignored");
        return false;
    };
    private(&p)
}

#[cfg(unix)]
fn private(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let mode = if p.is_dir() { 0o700 } else { 0o600 };
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).is_ok()
}

#[cfg(not(unix))]
fn private(p: &Path) -> bool {
    p.exists()
}

/// `vivi-wipe <id> <path>`: overwrite a file in the package's data folder with zeros,
/// then delete it.
pub fn wipe(id: &str, path: &str) -> bool {
    let Some(p) = inside(&data_dir(id), path).filter(|p| p.is_file()) else {
        return false;
    };
    if let Ok(n) = std::fs::metadata(&p).map(|m| m.len()) {
        let _ = std::fs::write(&p, vec![0u8; n as usize]);
    }
    std::fs::remove_file(&p).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_json() {
        let u: DialogUpdate = serde_json::from_str(
            r#"{"open":true,"title":"T","rows":[{"title":"a","sub":"b","cover":"https://x/y.jpg","dl":true}],
                "filters":[{"name":"Add to","options":["A","B"]}],"grid_toggle":true,"unknown":1}"#,
        )
        .unwrap();
        assert!(u.open);
        assert_eq!(u.title.as_deref(), Some("T"));
        assert_eq!(u.query, None);
        let r = &u.rows.as_ref().unwrap()[0];
        assert_eq!((r.title.as_str(), r.dl, r.chip.as_str()), ("a", true, ""));
        assert_eq!(u.filters.unwrap()[0].options, ["A", "B"]);
        assert_eq!(u.grid_toggle, Some(true));
        let d: DialogUpdate = serde_json::from_str("{}").unwrap();
        assert_eq!(d, DialogUpdate::default());
    }

    #[test]
    fn meta_and_confirm_json() {
        let m: Meta = serde_json::from_str(
            r#"{"items":[{"url":"u1","title":"A"},{"url":"u2","title":"B","artist":"C"}]}"#,
        )
        .unwrap();
        assert_eq!(m.items.len(), 2);
        assert_eq!(m.items[1].artist, "C");
        let c: Confirm = serde_json::from_str(r#"{"title":"t","text":"x"}"#).unwrap();
        assert_eq!((c.title.as_str(), c.ok.as_str()), ("t", ""));
    }

    #[test]
    fn event_payload() {
        let j: serde_json::Value =
            serde_json::from_str(&event_json(None, "select", &["3".into()])).unwrap();
        assert_eq!(j["event"], "select");
        assert_eq!(j["args"][0], "3");
    }

    #[test]
    fn protect_and_wipe_stay_inside_the_data_folder() {
        let root = std::env::temp_dir().join(format!("vivi-host-{}", std::process::id()));
        let own = root.join("own");
        std::fs::create_dir_all(&own).unwrap();
        let f = own.join("cookies.txt");
        std::fs::write(&f, "secret").unwrap();
        let outside = root.join("other.txt");
        std::fs::write(&outside, "x").unwrap();
        assert!(inside(&own, f.to_str().unwrap()).is_some());
        let escape = own.join("../other.txt");
        assert!(inside(&own, escape.to_str().unwrap()).is_none());
        assert!(inside(&own, "/nonexistent/file").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn covers_only_near_the_viewport() {
        // ~1000 liked songs: only the visible window and one window around it.
        assert_eq!(cover_range(500, 30, 1000), 470..560);
        assert_eq!(cover_range(0, 30, 1000), 0..60);
        assert_eq!(cover_range(990, 30, 1000), 960..1000);
        assert_eq!(cover_range(0, 0, 5), 0..2);
        assert_eq!(cover_range(10, 30, 5), 0..5);
        assert!(cover_range(0, 60, 2000).len() <= 120);
    }

    #[test]
    fn covers_only_from_web_or_local_files() {
        assert_eq!(fetch_cover("ftp://x/y.png"), None);
        assert_eq!(fetch_cover("/nonexistent/cover.png"), None);
        assert_eq!(cached_cover(""), None);
    }
}
