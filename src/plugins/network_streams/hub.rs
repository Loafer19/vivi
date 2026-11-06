//! Streams hub mode (#86): the Radio and TV tabs of the Streams page.

use super::*;

// ---------- Streams hub mode (#86) ----------

impl NetworkStreams {
    /// `hub radio|tv [words]`: show the Radio or TV tab of the Streams hub (a page, see
    /// `crate::streams::open_dialog`). Again on the same tab: only the words search.
    pub(super) fn hub_open(&mut self, cx: &Ctx, args: &[String]) {
        let a = cx.app;
        let tab = if args.first().is_some_and(|t| t == "tv") {
            "tv"
        } else {
            "radio"
        };
        let words = args.get(1).cloned();
        let st = a.global::<crate::MusicDialogState>();
        let same =
            a.global::<crate::Overlays>().get_md_open() && st.get_owner() == ID && self.hub == tab;
        self.hub = tab.into();
        if !same {
            st.set_owner(ID.into());
            st.set_caps(crate::MdCaps {
                logos: true,
                chips: true,
                rich: false,
                grid: true,
                info: false,
            });
            self.grid = cx.get("view") == "Grid";
            st.set_grid(self.grid);
            st.set_title(if tab == "tv" { "TV" } else { "Radio" }.into());
            st.set_search_label("Search".into());
            st.set_primary("Play".into());
            let src: Vec<SharedString> = if tab == "tv" {
                HUB_TV_SOURCES.iter().map(|s| (*s).into()).collect()
            } else {
                Vec::new()
            };
            if tab == "tv" && !HUB_TV_SOURCES.contains(&st.get_source().as_str()) {
                st.set_source(HUB_TV_SOURCES[0].into());
            }
            st.set_sources(ModelRc::new(VecModel::from(src)));
            if tab == "radio" {
                st.set_source("".into());
            }
            st.set_query("".into());
            st.set_detail("".into());
            self.last_source = st.get_source().to_string();
            st.set_placeholder(
                if tab == "tv" {
                    "Channel name; the filters pick a country or category"
                } else {
                    "Station name; the filters narrow it down"
                }
                .into(),
            );
            st.set_secondary("Favorite".into());
            self.hub_filters(cx);
            crate::streams::open_dialog(a);
            std::thread::spawn(|| prune_logos(64 * 1024 * 1024));
        }
        if let Some(w) = &words {
            st.set_query(w.as_str().into());
        }
        if !same || words.is_some() {
            self.hub_run(cx);
        }
    }

    /// The tab's dropdown filters.
    pub(super) fn hub_filters(&self, cx: &Ctx) {
        let f = |name: &str, opts: Vec<String>, i: usize| crate::MdFilter {
            name: name.into(),
            value: opts.get(i).cloned().unwrap_or_default().into(),
            options: ModelRc::new(VecModel::from(
                opts.into_iter().map(SharedString::from).collect::<Vec<_>>(),
            )),
        };
        // "Any country" etc.: the dropdowns have no label of their own.
        let any = |what: &str, l: &mut dyn Iterator<Item = String>| {
            std::iter::once(format!("Any {what}"))
                .chain(l)
                .collect::<Vec<_>>()
        };
        let countries = any(
            "country",
            &mut HUB_COUNTRIES.iter().map(|c| c.0.to_string()),
        );
        let v = if self.hub == "tv" {
            vec![
                f("Country", countries, self.t_country),
                f(
                    "Category",
                    any("group", &mut CATEGORY_CHIPS.iter().map(|c| c.0.to_string())),
                    self.t_category,
                ),
                f(
                    "Guide",
                    vec!["Guide on".into(), "Guide off".into()],
                    usize::from(self.guide_off),
                ),
            ]
        } else {
            vec![
                f("Country", countries, self.r_country),
                f(
                    "Genre",
                    any("genre", &mut HUB_GENRES.iter().map(|g| g.to_string())),
                    self.r_genre,
                ),
                f(
                    "Language",
                    any("language", &mut HUB_LANGUAGES.iter().map(|g| g.to_string())),
                    self.r_lang,
                ),
                f(
                    "Bitrate",
                    HUB_BITRATES.iter().map(|b| b.0.to_string()).collect(),
                    self.r_bitrate,
                ),
                f(
                    "Sort",
                    HUB_SORTS.iter().map(|b| b.0.to_string()).collect(),
                    self.r_sort,
                ),
            ]
        };
        cx.app
            .global::<crate::MusicDialogState>()
            .set_filters(ModelRc::new(VecModel::from(v)));
    }

    /// A filter dropdown changed.
    pub(super) fn hub_filter(&mut self, name: &str, v: &str) {
        let pos = |l: &mut dyn Iterator<Item = &str>, any: bool| {
            for (i, x) in l.enumerate() {
                if x == v {
                    return i + usize::from(any);
                }
            }
            0
        };
        match name {
            "Country" if self.hub == "tv" => {
                self.t_country = pos(&mut HUB_COUNTRIES.iter().map(|c| c.0), true)
            }
            "Country" => self.r_country = pos(&mut HUB_COUNTRIES.iter().map(|c| c.0), true),
            "Category" => self.t_category = pos(&mut CATEGORY_CHIPS.iter().map(|c| c.0), true),
            "Guide" => self.guide_off = v == "Guide off",
            "Genre" => self.r_genre = pos(&mut HUB_GENRES.iter().copied(), true),
            "Language" => self.r_lang = pos(&mut HUB_LANGUAGES.iter().copied(), true),
            "Bitrate" => self.r_bitrate = pos(&mut HUB_BITRATES.iter().map(|b| b.0), false),
            "Sort" => self.r_sort = pos(&mut HUB_SORTS.iter().map(|b| b.0), false),
            _ => {}
        }
    }

    /// The Radio tab's query from its filters and the search words.
    pub(super) fn radio_query(&self, words: &str) -> RadioQuery {
        let pick = |i: usize, l: &[&str]| {
            i.checked_sub(1)
                .and_then(|i| l.get(i))
                .map(|x| x.to_string())
                .unwrap_or_default()
        };
        RadioQuery {
            name: words.trim().to_string(),
            country: self
                .r_country
                .checked_sub(1)
                .and_then(|i| HUB_COUNTRIES.get(i))
                .map(|c| c.1.to_string())
                .unwrap_or_default(),
            tag: pick(self.r_genre, &HUB_GENRES),
            language: pick(self.r_lang, &HUB_LANGUAGES),
            bitrate: HUB_BITRATES.get(self.r_bitrate).map_or(0, |b| b.1),
            order: HUB_SORTS.get(self.r_sort).map_or("votes", |s| s.1).into(),
        }
    }

    /// Load what the tab's filters (and search words) ask for.
    pub(super) fn hub_run(&mut self, cx: &Ctx) {
        self.reload_favs();
        let words = cx
            .app
            .global::<crate::MusicDialogState>()
            .get_query()
            .trim()
            .to_string();
        self.note = None;
        self.loaded_src.clear();
        self.chip_after = None;
        if self.hub == "radio" {
            self.mode = 0;
            self.clear(cx);
            Self::ui(cx, "Searching radio-browser.info…", true);
            let q = self.radio_query(&words);
            self.spawn(move || radio_filtered(&q));
            return;
        }
        let mode = self.cur_mode(cx);
        if mode != TV_COUNTRY {
            // M3U and My playlists work as in the 0.13 browser.
            if mode == M3U && words.is_empty() {
                self.mode = M3U;
                self.clear(cx);
                cx.app
                    .global::<crate::MusicDialogState>()
                    .set_secondary("Choose file…".into());
                Self::ui(
                    cx,
                    "Paste a playlist URL or path (.m3u / .m3u8), or Choose file…",
                    false,
                );
                return;
            }
            let hub = std::mem::take(&mut self.hub);
            cx.app.global::<crate::MusicDialogState>().set_source(
                if mode == M3U {
                    SOURCES[M3U]
                } else {
                    SOURCES[MY_PL]
                }
                .into(),
            );
            self.command(cx, "search", &[]);
            cx.app.global::<crate::MusicDialogState>().set_source(
                if mode == M3U {
                    HUB_TV_SOURCES[1]
                } else {
                    HUB_TV_SOURCES[2]
                }
                .into(),
            );
            self.hub = hub;
            return;
        }
        self.mode = TV_COUNTRY;
        let country = self
            .t_country
            .checked_sub(1)
            .and_then(|i| HUB_COUNTRIES.get(i));
        let category = self
            .t_category
            .checked_sub(1)
            .and_then(|i| CATEGORY_CHIPS.get(i));
        match (country, category) {
            (Some((name, code)), cat) => {
                let url = format!("{IPTV}/countries/{}.m3u", code.to_lowercase());
                self.open_playlist(cx, name, url);
                self.chip_after = cat.map(|c| c.0.to_string());
            }
            (None, Some((label, id))) => {
                self.open_playlist(cx, label, format!("{IPTV}/categories/{id}.m3u"));
            }
            (None, None) if !words.is_empty() => {
                self.mode = M3U;
                self.clear(cx);
                Self::ui(cx, "Searching iptv-org channels…", true);
                self.spawn(move || iptv_search(&words));
            }
            (None, None) => {
                self.clear(cx);
                Self::ui(cx, "Loading countries…", true);
                self.note = Some("{n} countries. Pick one, or use the filters.".into());
                self.spawn(|| iptv_index(true));
            }
        }
    }
}
