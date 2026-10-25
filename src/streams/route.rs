//! Where a Streams hub request goes (#86): hub tabs, service pages, the `VIVI_PAGE` /
//! IPC `page` syntax and the 0.13 entry points that stay as hidden aliases for one
//! release (Open Network Stream, Network Streams, the Radio widget, `VIVI_NETWORK`); they are
//! removed in 0.15.0.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Recent,
    Favorites,
    Radio,
    Tv,
    Music,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Recent, Tab::Favorites, Tab::Radio, Tab::Tv, Tab::Music];

    pub fn key(self) -> &'static str {
        match self {
            Tab::Recent => "recent",
            Tab::Favorites => "favorites",
            Tab::Radio => "radio",
            Tab::Tv => "tv",
            Tab::Music => "music",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Recent => "Recent",
            Tab::Favorites => "Favorites",
            Tab::Radio => "Radio",
            Tab::Tv => "TV",
            Tab::Music => "Music",
        }
    }

    pub fn parse(s: &str) -> Option<Tab> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "recent" | "history" => Tab::Recent,
            "favorites" | "favourites" | "fav" | "saved" => Tab::Favorites,
            "radio" => Tab::Radio,
            "tv" | "iptv" => Tab::Tv,
            "music" => Tab::Music,
            _ => return None,
        })
    }

    /// Radio and TV come from the radio/TV directory built-in; when it is off they are
    /// not shown and requests for them land on Recent.
    pub fn needs_directory(self) -> bool {
        matches!(self, Tab::Radio | Tab::Tv)
    }
}

/// A place in the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    /// The hub on a tab. `service` picks the Music tab's service; `focus_url` puts the
    /// cursor in the URL bar (the old "Open URL" entry points).
    Hub {
        tab: Tab,
        service: Option<String>,
        focus_url: bool,
    },
    /// A service's own full-width page (its sidebar icon).
    Service(String),
}

impl Page {
    pub fn tab(tab: Tab) -> Page {
        Page::Hub {
            tab,
            service: None,
            focus_url: false,
        }
    }

    /// The Library navigation key of the page: `streams` or `service:<id>`.
    pub fn nav(&self) -> String {
        match self {
            Page::Hub { .. } => "streams".into(),
            Page::Service(id) => format!("service:{id}"),
        }
    }

    /// The page shown when the radio/TV directory is off (`directory` false).
    pub fn available(self, directory: bool) -> Page {
        match self {
            Page::Hub {
                tab,
                service,
                focus_url,
            } if tab.needs_directory() && !directory => Page::Hub {
                tab: Tab::Recent,
                service,
                focus_url,
            },
            p => p,
        }
    }
}

/// Parse `streams`, `streams:<tab>`, `streams:music:<service>`, `streams:url` and
/// `service:<id>` (the `VIVI_PAGE` test hook, the IPC `page` command, palette entries).
pub fn parse(s: &str) -> Option<Page> {
    let s = s.trim();
    if let Some(id) = s.strip_prefix("service:") {
        let id = id.trim();
        return (!id.is_empty()).then(|| Page::Service(id.to_string()));
    }
    let rest = s.strip_prefix("streams")?;
    if rest.is_empty() {
        return Some(Page::tab(Tab::Recent));
    }
    let rest = rest.strip_prefix(':')?;
    if rest == "url" {
        return Some(Page::Hub {
            tab: Tab::Recent,
            service: None,
            focus_url: true,
        });
    }
    let (tab, svc) = match rest.split_once(':') {
        Some((t, s)) => (t, Some(s.trim())),
        None => (rest, None),
    };
    let tab = Tab::parse(tab)?;
    let service = match svc {
        Some(s) if tab == Tab::Music && !s.is_empty() => Some(s.to_string()),
        Some(_) => return None,
        None => None,
    };
    Some(Page::Hub {
        tab,
        service,
        focus_url: false,
    })
}

/// The 0.13 entry points (palette ids, menu commands, plugin commands, `VIVI_*` hooks)
/// that open the hub for one more release, so keymaps and habits keep working.
pub fn alias(id: &str) -> Option<Page> {
    let url = Page::Hub {
        tab: Tab::Recent,
        service: None,
        focus_url: true,
    };
    Some(match id {
        // Open Network Stream / Open URL (palette, File menu, Library action, Ctrl+N style
        // keymap entries, the welcome screen).
        "network" | "app:network" | "open-network" | "action:network" | "open-url"
        | "app:open-url" => url,
        // The Network Streams browser and its Radio stations widget.
        "network-streams" | "network-streams:open" | "network-streams:radio" | "radio" => {
            Page::tab(Tab::Radio)
        }
        "network-streams:tv" => Page::tab(Tab::Tv),
        // The old Library section.
        "nav:network" => Page::tab(Tab::Recent),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_syntax() {
        assert_eq!(parse("streams"), Some(Page::tab(Tab::Recent)));
        assert_eq!(parse("streams:tv"), Some(Page::tab(Tab::Tv)));
        assert_eq!(parse("streams:Favorites"), Some(Page::tab(Tab::Favorites)));
        assert_eq!(
            parse("streams:music:youtube-music"),
            Some(Page::Hub {
                tab: Tab::Music,
                service: Some("youtube-music".into()),
                focus_url: false
            })
        );
        assert_eq!(
            parse("streams:url"),
            Some(Page::Hub {
                tab: Tab::Recent,
                service: None,
                focus_url: true
            })
        );
        assert_eq!(
            parse("service:free-music"),
            Some(Page::Service("free-music".into()))
        );
        for bad in [
            "",
            "stream",
            "streams:",
            "streams:nope",
            "streams:radio:x",
            "service:",
            "home",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn nav_keys() {
        assert_eq!(Page::tab(Tab::Radio).nav(), "streams");
        assert_eq!(
            Page::Service("youtube-music".into()).nav(),
            "service:youtube-music"
        );
    }

    #[test]
    fn legacy_aliases_open_the_hub() {
        for id in ["network", "app:network", "open-network", "action:network"] {
            assert_eq!(
                alias(id),
                Some(Page::Hub {
                    tab: Tab::Recent,
                    service: None,
                    focus_url: true
                }),
                "{id}"
            );
        }
        assert_eq!(alias("network-streams:open"), Some(Page::tab(Tab::Radio)));
        assert_eq!(alias("network-streams:tv"), Some(Page::tab(Tab::Tv)));
        assert_eq!(alias("nav:network"), Some(Page::tab(Tab::Recent)));
        assert_eq!(alias("app:library"), None);
    }

    #[test]
    fn radio_tv_fall_back_when_the_directory_is_off() {
        assert_eq!(
            Page::tab(Tab::Radio).available(false),
            Page::tab(Tab::Recent)
        );
        assert_eq!(Page::tab(Tab::Tv).available(true), Page::tab(Tab::Tv));
        assert_eq!(
            Page::tab(Tab::Music).available(false),
            Page::tab(Tab::Music)
        );
        assert_eq!(
            Page::Service("x".into()).available(false),
            Page::Service("x".into())
        );
    }

    #[test]
    fn tabs_round_trip() {
        for t in Tab::ALL {
            assert_eq!(Tab::parse(t.key()), Some(t));
        }
    }
}
