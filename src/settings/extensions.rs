//! Extensions page: installed plugin cards with their settings and the downloadable catalog.

use super::*;

/// Docs site with the plugin catalog ("Find more extensions").
pub const CATALOG_URL: &str = "https://plugins.viviplayer.app/";

/// Extensions page (Figma "Add-ons & Scripts") with two tabs:
/// * Installed: one card per plugin (built-in, downloaded package, mpv script) with its
///   ON/OFF toggle, followed by the plugin's own settings while it is enabled;
///   packages also get Update / Roll back / Uninstall buttons.
/// * Browse: the downloadable catalog with Install / Update / Uninstall buttons.
pub(crate) fn extensions(r: &mut Rows) {
    use plugins::packages;
    r.cat("Extensions");
    let tab = packages::with_state(|s| s.tab.clone());
    let updates = packages::updates();
    r.seg(
        "ext.tab",
        "Show",
        "Installed plugins or the downloadable catalog.",
        &tab,
        &["Installed", "Browse"],
    );
    let status = packages::with_state(|s| s.status.clone());
    if !status.is_empty() {
        r.push("", &status, "", "info", String::new());
    } else if !updates.is_empty() {
        let names: Vec<&str> = updates.iter().map(|e| e.name.as_str()).collect();
        r.push(
            "",
            &format!("Updates available: {}", names.join(", ")),
            "",
            "info",
            String::new(),
        );
    }
    if tab == "Browse" {
        browse(r);
    } else {
        installed(r, &updates);
    }
}

/// Settings → Extensions: plugins live in the Extensions window now; this page links
/// to it and keeps the global options.
pub(crate) fn extensions_page(r: &mut Rows) {
    r.cat("Extensions").section(
        "Plugins",
        "Each plugin has a tab in the Extensions window (Ctrl+E) with its actions and settings.",
        &[("ext.window", "Open Extensions")],
    );
    let auto =
        r.c.plugins
            .get(plugins::packages::GLOBAL_ID)
            .and_then(|s| s.settings.get("autocheck"))
            .is_none_or(|v| v == "true");
    r.toggle(
        "ext.autocheck",
        "Check for plugin updates daily",
        "Download the plugin catalog at most once a day when Vivi starts. Nothing else is sent.",
        auto,
    );
    r.toggle(
        &format!("ext.{}", crate::notify::KEY),
        "Desktop notifications from extensions",
        "Plugins that can notify (for example when a download finishes) show a system notification. Off: the message shows in the player instead.",
        crate::notify::enabled(r.c),
    );
    let startup = plugins::scripts::load_on_startup(r.c);
    r.toggle(
        "scripts.startup",
        "Load add-ons on startup",
        "Load enabled mpv scripts and downloaded plugins when Vivi starts.",
        startup,
    )
    .section(
        "mpv scripts",
        &config::scripts_dir().display().to_string(),
        &[
            ("scripts.folder", "Open folder"),
            ("scripts.rescan", "Rescan"),
        ],
    );
}

pub(crate) fn plugin_rows(
    r: &mut Rows,
    m: &plugins::Manifest,
    src: &str,
    after: impl FnOnce(&mut Rows),
) {
    use plugins::SettingKind;
    let on = plugins::is_enabled(r.c, m);
    r.push(
        &plugins::toggle_id(&m.id),
        &m.name,
        &m.description,
        "plugin",
        on.to_string(),
    );
    r.last().options = strs(&[&m.version, &m.author, &m.icon, src]);
    after(r);
    if !on {
        return;
    }
    for st in &m.settings {
        let id = plugins::setting_id(&m.id, &st.key);
        let val = plugins::setting_value(r.c, m, &st.key);
        match &st.kind {
            SettingKind::Toggle => {
                r.toggle(&id, &st.title, &st.info, val == "true");
            }
            SettingKind::Choice(o) => {
                let o: Vec<&str> = o.iter().map(String::as_str).collect();
                r.choice(&id, &st.title, &st.info, &val, &o);
            }
            SettingKind::Text(ph) => {
                r.text(&id, &st.title, &st.info, &val, ph);
            }
            SettingKind::Secret => {
                r.text(&id, &st.title, &st.info, &val, "");
                r.last().kind = "secret".into();
            }
            SettingKind::Action(l) => {
                r.action(&id, &st.title, &st.info, l);
            }
        }
    }
}

pub(crate) fn installed(r: &mut Rows, updates: &[plugins::packages::Entry]) {
    use plugins::{packages, Origin};
    let all = plugins::manifests();
    let pk = packages::installed(&packages::root());
    let busy = packages::with_state(|s| s.busy.clone());
    r.header("Built-in");
    for m in all.iter().filter(|m| m.origin == Origin::Builtin) {
        plugin_rows(r, m, "Built-in", |r| {
            // Live state a plugin reports (e.g. YouTube Music: who is signed in).
            if let Some(s) = plugins::query::<String>(&m.id, "status") {
                if plugins::is_enabled(r.c, m) {
                    r.push("", &s, "", "info", String::new());
                }
            }
        });
    }
    r.header("Downloaded");
    if pk.is_empty() {
        r.push(
            "",
            "No downloaded plugins yet. Open Browse to install some.",
            "",
            "info",
            String::new(),
        );
    }
    for m in all.iter().filter(|m| m.origin == Origin::Package) {
        let Some(p) = pk.iter().find(|p| p.manifest.id == m.id) else {
            continue;
        };
        let up = updates.iter().find(|e| e.id == m.id);
        plugin_rows(r, m, "Downloaded", |r| {
            let mut title = format!("Version {}", p.manifest.version);
            if let Some(st) = plugins::script_status(&m.id) {
                title += &format!("  ·  {st}");
            }
            let mut labels = Vec::new();
            if let Some(u) = up {
                title += &format!("  ·  {} available", u.version);
                labels.push("Update");
            }
            if let Some(prev) = p.previous.first() {
                title += &format!("  ·  previous {prev}");
                labels.push("Roll back");
            }
            labels.push("Uninstall");
            r.push(
                &format!("pkg.{}", m.id),
                &title,
                &packages::describe_permissions(&p.manifest.permissions),
                "buttons",
                String::new(),
            );
            r.last().labels = strs(&labels);
            r.last().enabled = busy.is_none();
        });
    }
    r.header("mpv Scripts");
    let scripts: Vec<_> = all.iter().filter(|m| m.origin == Origin::Script).collect();
    if scripts.is_empty() {
        r.push(
            "",
            &format!(
                "No scripts yet. Put mpv .lua or .js scripts in {} and press Rescan.",
                config::scripts_dir().display()
            ),
            "",
            "info",
            String::new(),
        );
    }
    for m in scripts {
        plugin_rows(r, m, "mpv script", |r| {
            if let Some(st) = plugins::script_status(&m.id) {
                r.push("", &st, "", "info", String::new());
            }
        });
    }
    let startup = plugins::scripts::load_on_startup(r.c);
    r.toggle(
        "scripts.startup",
        "Load add-ons on startup",
        "Load enabled mpv scripts and downloaded plugins when Vivi starts.",
        startup,
    )
    .action(
        "scripts.folder",
        "Scripts folder",
        &config::scripts_dir().display().to_string(),
        "Open",
    )
    .action(
        "scripts.rescan",
        "Rescan scripts",
        "Pick up scripts added since Vivi started.",
        "Rescan",
    );
}

/// Catalog tab.
pub(crate) fn browse(r: &mut Rows) {
    use plugins::packages::{self, cmp_version, compatible, describe_permissions};
    let (catalog, fetched, fetching, busy, confirm) = packages::with_state(|s| {
        (
            s.catalog.clone(),
            s.fetched,
            s.fetching,
            s.busy.clone(),
            s.confirm.clone(),
        )
    });
    let inst = packages::installed(&packages::root());
    let when = fetched
        .and_then(|t| t.elapsed().ok())
        .map(|e| match e.as_secs() {
            s if s < 120 => "just now".to_string(),
            s if s < 7200 => format!("{} min ago", s / 60),
            s if s < 172_800 => format!("{} h ago", s / 3600),
            s => format!("{} days ago", s / 86400),
        });
    r.header("Plugin catalog")
        .action(
            "ext.check",
            "Check for updates",
            &format!(
                "Catalog: {}. Last checked {}.",
                packages::index_url(),
                when.as_deref().unwrap_or("never")
            ),
            if fetching { "Checking…" } else { "Check now" },
        )
        .enabled(!fetching);
    let auto =
        r.c.plugins
            .get(packages::GLOBAL_ID)
            .and_then(|s| s.settings.get("autocheck"))
            .is_none_or(|v| v == "true");
    r.toggle(
        "ext.autocheck",
        "Check for plugin updates daily",
        "Download the plugin catalog at most once a day when Vivi starts. Nothing else is sent.",
        auto,
    );
    if catalog.is_empty() {
        r.push(
            "",
            if fetching {
                "Loading the catalog…"
            } else {
                "The catalog has not been downloaded yet. Press Check now."
            },
            "",
            "info",
            String::new(),
        );
    }
    r.header("Available plugins");
    // Recommended plugins (catalog featured_rank, else packages::RECOMMENDED) on top, by rank.
    for e in packages::browse_order(&catalog) {
        let have = inst.iter().find(|i| i.manifest.id == e.id);
        // Deprecated plugins are only listed while installed (to uninstall them).
        if e.deprecated.is_some() && have.is_none() {
            continue;
        }
        let (state, labels): (String, Vec<&str>) = if busy.as_deref() == Some(&e.id) {
            ("Downloading…".into(), vec![])
        } else if confirm.as_deref() == Some(&e.id) {
            (
                format!("{}. Plugins run as mpv scripts with your user's rights; install only ones you trust.", describe_permissions(&e.permissions)),
                vec!["Allow & install", "Cancel"],
            )
        } else if !compatible(&e.min_vivi) {
            (format!("Needs Vivi {} or newer", e.min_vivi), vec![])
        } else if !e.available() && have.is_none() {
            (
                format!("Not available for {} yet", packages::platform()),
                vec![],
            )
        } else {
            match have {
                Some(h) if cmp_version(&e.version, &h.manifest.version).is_gt() => (
                    format!("Installed {} · update available", h.manifest.version),
                    vec!["Update", "Uninstall"],
                ),
                Some(h) => (
                    format!("Installed {}", h.manifest.version),
                    vec!["Uninstall"],
                ),
                None => (String::new(), vec!["Install"]),
            }
        };
        // Pending update: show "What's new" from the catalog changelog instead of the description.
        let news = have
            .filter(|h| cmp_version(&e.version, &h.manifest.version).is_gt())
            .map(|h| e.whats_new(&h.manifest.version))
            .filter(|n| !n.is_empty())
            .map(|n| format!("What's new: {n}"));
        r.push(
            &format!("pkg.{}", e.id),
            &e.name,
            &match &e.deprecated {
                Some(d) => format!("Deprecated: {}", d.text(&e.name)),
                None => news.unwrap_or_else(|| e.description.clone()),
            },
            "package",
            if confirm.as_deref() == Some(&e.id) {
                "confirm".into()
            } else {
                String::new()
            },
        );
        let mut size = packages::human_size(e.download_size());
        if !e.size_note.is_empty() {
            size = format!("{size} {}", e.size_note);
        }
        let perms = describe_permissions(&e.permissions);
        let rec = if packages::recommended_rank(&catalog, e).is_some() {
            "Recommended"
        } else {
            ""
        };
        r.last().options = strs(&[&e.version, &e.author, &e.icon, &perms, &state, &size, rec]);
        r.last().labels = strs(&labels);
        r.last().enabled = busy.is_none();
    }
    r.action(
        "scripts.catalog",
        "Plugin catalog on the web",
        CATALOG_URL,
        "Open catalog",
    );
}

pub(crate) fn open_url(u: &str) {
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(u).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(u).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(u).spawn();
    if let Err(e) = r {
        eprintln!("vivi: open {u}: {e}");
    }
}
