//! Desktop notifications for plugins that contribute `notify` (see
//! docs/plugins-architecture.md). Linux: `org.freedesktop.Notifications` over the session
//! bus (zbus, already used for MPRIS), sent from a short-lived thread so the UI never
//! waits on D-Bus. Elsewhere (or without a session bus) callers fall back to the toast.

use crate::config::Config;

/// Config entry (the package manager's global one) and key of the
/// "Desktop notifications from extensions" option; on unless set to `"false"`.
pub const KEY: &str = "notify";

/// Whether the user allows extensions to show desktop notifications.
pub fn enabled(c: &Config) -> bool {
    c.plugins
        .get(crate::plugins::packages::GLOBAL_ID)
        .and_then(|s| s.settings.get(KEY))
        .is_none_or(|v| v == "true")
}

/// Show `title` / `body` as a desktop notification. False when there is no
/// notification service to try (the caller shows a toast instead).
#[cfg(target_os = "linux")]
pub fn send(title: &str, body: &str) -> bool {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        return false;
    }
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::Builder::new()
        .name("vivi-notify".into())
        .spawn(move || {
            let Ok(c) = zbus::blocking::Connection::session() else {
                return;
            };
            let hints: std::collections::HashMap<&str, zbus::zvariant::Value> =
                std::collections::HashMap::new();
            let r = c.call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(
                    "Vivi",
                    0u32,
                    "vivi",
                    title.as_str(),
                    body.as_str(),
                    Vec::<&str>::new(),
                    hints,
                    -1i32,
                ),
            );
            if let Err(e) = r {
                eprintln!("vivi: notification: {e}");
            }
        })
        .is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn send(_title: &str, _body: &str) -> bool {
    false
}

/// Toast text when a notification is shown in the player instead.
pub fn toast_text(title: &str, body: &str) -> String {
    if body.is_empty() {
        title.to_string()
    } else {
        format!("{title}: {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_defaults_on_and_toast_text() {
        let mut c = Config::default();
        assert!(enabled(&c));
        c.plugins
            .entry(crate::plugins::packages::GLOBAL_ID.into())
            .or_default()
            .settings
            .insert(KEY.into(), "false".into());
        assert!(!enabled(&c));
        assert_eq!(toast_text("Done", ""), "Done");
        assert_eq!(toast_text("Done", "a.mp3"), "Done: a.mp3");
    }
}
