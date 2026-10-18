//! EGL vendor selection on NVIDIA-only Linux machines.
//!
//! libglvnd loads every EGL vendor listed in `egl_vendor.d` (`10_nvidia.json` and
//! `50_mesa.json`), so on a machine whose only GPU is NVIDIA, Mesa's libgallium and
//! libLLVM are mapped into every window for nothing (~20 MB RSS / ~9 MB PSS). When every
//! render node is driven by `nvidia`, vivi points glvnd at the NVIDIA vendor file only.
//! Hybrid machines (any non-NVIDIA render node), an explicit user choice and the Flatpak
//! (its own GL runtime) are left alone.

use std::path::{Path, PathBuf};

/// Vendor files checked, in order.
const NVIDIA_VENDOR_JSON: [&str; 2] = [
    "/etc/glvnd/egl_vendor.d/10_nvidia.json",
    "/usr/share/glvnd/egl_vendor.d/10_nvidia.json",
];

/// What the decision looks at; gathered by [`probe`], kept separate so it can be tested.
#[derive(Debug, Default)]
pub(crate) struct EglEnv {
    /// `__EGL_VENDOR_LIBRARY_FILENAMES` or `__EGL_VENDOR_LIBRARY_DIRS` is set.
    pub user_override: bool,
    /// `FLATPAK_ID` is set or `/.flatpak-info` exists.
    pub flatpak: bool,
    /// Driver name of each `/sys/class/drm/renderD*` node (`None` when it could not be read).
    pub render_drivers: Vec<Option<String>>,
    /// The first NVIDIA EGL vendor file that exists.
    pub nvidia_json: Option<PathBuf>,
}

/// The vendor file to force, or `None` to leave glvnd's default (load all vendors).
pub(crate) fn nvidia_only_vendor(env: &EglEnv) -> Option<&Path> {
    if env.user_override || env.flatpak || env.render_drivers.is_empty() {
        return None;
    }
    let all_nvidia = env
        .render_drivers
        .iter()
        .all(|d| d.as_deref() == Some("nvidia"));
    if !all_nvidia {
        return None;
    }
    env.nvidia_json.as_deref()
}

fn probe() -> EglEnv {
    let var = |k: &str| std::env::var_os(k).is_some();
    let mut render_drivers = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/sys/class/drm") {
        for e in dir.flatten() {
            if !e.file_name().to_string_lossy().starts_with("renderD") {
                continue;
            }
            let driver = std::fs::read_link(e.path().join("device/driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
            render_drivers.push(driver);
        }
    }
    EglEnv {
        user_override: var("__EGL_VENDOR_LIBRARY_FILENAMES") || var("__EGL_VENDOR_LIBRARY_DIRS"),
        flatpak: var("FLATPAK_ID") || Path::new("/.flatpak-info").exists(),
        render_drivers,
        nvidia_json: NVIDIA_VENDOR_JSON
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file()),
    }
}

/// Call first thing in `main()`, before any thread, mpv or the Slint backend starts.
pub(crate) fn select_vendor() {
    let env = probe();
    if let Some(json) = nvidia_only_vendor(&env) {
        // SAFETY: called at the top of main(), before any other thread exists, so nothing
        // can read the environment concurrently. (`set_var` is `unsafe` from Rust 2024.)
        std::env::set_var("__EGL_VENDOR_LIBRARY_FILENAMES", json);
        if std::env::var_os("VIVI_LOG").is_some() {
            eprintln!("[egl] NVIDIA-only GPU: EGL vendor {}", json.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(drivers: &[Option<&str>]) -> EglEnv {
        EglEnv {
            render_drivers: drivers.iter().map(|d| d.map(String::from)).collect(),
            nvidia_json: Some(PathBuf::from(NVIDIA_VENDOR_JSON[1])),
            ..Default::default()
        }
    }

    #[test]
    fn nvidia_only_picks_the_nvidia_vendor() {
        let e = env(&[Some("nvidia")]);
        assert_eq!(
            nvidia_only_vendor(&e),
            Some(Path::new(NVIDIA_VENDOR_JSON[1]))
        );
        let e = env(&[Some("nvidia"), Some("nvidia")]);
        assert!(nvidia_only_vendor(&e).is_some());
    }

    #[test]
    fn hybrid_or_other_gpus_are_left_alone() {
        assert!(nvidia_only_vendor(&env(&[Some("nvidia"), Some("i915")])).is_none());
        assert!(nvidia_only_vendor(&env(&[Some("amdgpu"), Some("nvidia")])).is_none());
        assert!(nvidia_only_vendor(&env(&[Some("nvidia"), None])).is_none());
        assert!(nvidia_only_vendor(&env(&[Some("xe")])).is_none());
        assert!(nvidia_only_vendor(&env(&[])).is_none());
    }

    #[test]
    fn overrides_flatpak_and_missing_json_are_respected() {
        let mut e = env(&[Some("nvidia")]);
        e.user_override = true;
        assert!(nvidia_only_vendor(&e).is_none());
        let mut e = env(&[Some("nvidia")]);
        e.flatpak = true;
        assert!(nvidia_only_vendor(&e).is_none());
        let mut e = env(&[Some("nvidia")]);
        e.nvidia_json = None;
        assert!(nvidia_only_vendor(&e).is_none());
    }
}
