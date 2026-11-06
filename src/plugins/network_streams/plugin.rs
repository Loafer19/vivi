//! The Network Streams plugin: hooks the host calls (manifest, enable, ticks, commands).

use super::*;

impl Plugin for NetworkStreams {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Network Streams",
            "Internet radio (radio-browser.info, SomaFM), iptv-org TV by country and category, your own M3U/IPTV playlists that refresh themselves, favorites, Search all (TV + radio), group and category chips, channel logos as a list or a grid, an XMLTV program guide (now / next) and stream info.",
            "streams",
        )
        // Its browser is the Streams hub's Radio and TV tabs now (#86): no sidebar
        // source, palette entry or Home widget of its own.
        .contribute(contrib::CONTEXT_PLAYER, "Stream info", "info")
        .contribute(contrib::DRAWER_PANEL, "Network Streams", "drawer")
        .setting(SettingSpec::action(
            "info",
            "Stream info",
            "Shows the now-playing title, codec, bitrate and buffer of the current stream.",
            "Show",
        ))
        .setting(SettingSpec::text(
            "epg",
            "Program guide (XMLTV)",
            "Optional XMLTV guide URL(s) (.xml or .xml.gz) for channels with a tvg-id, in addition to the playlist's own url-tvg. Only the next 12 hours are kept.",
            "https://example.com/guide.xml.gz",
            "",
        ))
        .setting(SettingSpec::choice(
            "view",
            "Browser view",
            "Show stations as a list or as a grid of logos. The buttons in the browser's header switch it too.",
            &["List", "Grid"],
        ))
        .setting(SettingSpec::choice(
            "refresh",
            "Refresh my playlists",
            "How often saved M3U playlist URLs are downloaded again. A broken link keeps the last good copy.",
            &["24 hours", "6 hours", "7 days"],
        ))
    }
    fn disable(&mut self, cx: &Ctx) {
        self.task.cancel();
        if cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
            cx.app.global::<crate::Overlays>().set_md_open(false);
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let idx = || {
            args.first()
                .and_then(|a| a.parse::<i64>().ok())
                .filter(|i| *i >= 0)
                .map(|i| i as usize)
        };
        match name {
            "drawer" => {
                let path = cx.player.get("path").unwrap_or_default();
                let station = path.contains("://").then(|| known_station(&path)).flatten();
                let text = drawer_text(
                    station.as_ref(),
                    path.contains("://"),
                    &now_info(|p| cx.player.get(p)),
                    hub_store::favorites().len(),
                    hub_store::recent().len(),
                );
                cx.app.global::<crate::Drawer>().set_info(text.into());
            }
            "hub" => self.hub_open(cx, args),
            "filter" if !self.hub.is_empty() => {
                let (Some(n), true) = (args.first(), args.len() > 1) else {
                    return;
                };
                let v = args[1..].join(" ");
                self.hub_filter(n, &v);
                self.hub_filters(cx);
                self.hub_run(cx);
            }
            "favorites-changed" => {
                self.reload_favs();
                if self.mode == SAVED {
                    self.local(cx, SAVED);
                } else {
                    self.show_rows(cx);
                    self.buttons(cx);
                }
            }
            "view" => {
                let grid = args.first().is_some_and(|a| a == "grid");
                self.set_view(cx, grid);
            }
            "search" if self.hub == "radio" => self.hub_run(cx),
            "search"
                if self.hub == "tv"
                    && self.cur_mode(cx) == TV_COUNTRY
                    && !self.all.is_empty()
                    && !self.is_picker() =>
            {
                // A channel list is loaded: typing filters it already.
                self.filtered_by.clear();
            }
            "search" => {
                let mode = self.cur_mode(cx);
                if mode == SAVED || mode == RECENT {
                    self.local(cx, mode);
                    return;
                }
                self.reload_favs();
                let q = cx
                    .app
                    .global::<crate::MusicDialogState>()
                    .get_query()
                    .trim()
                    .to_string();
                self.loaded_src.clear();
                if live_filter(mode) {
                    // Go = (re)load the picker; typing filters it.
                    self.filtered_by = q.clone();
                    self.mode = mode;
                    self.clear(cx);
                    Self::ui(
                        cx,
                        if mode == MY_PL {
                            "Refreshing your playlists…"
                        } else {
                            "Loading…"
                        },
                        true,
                    );
                    let ttl = Self::ttl(cx);
                    if mode == MY_PL && playlists().is_empty() {
                        Self::ui(
                            cx,
                            "No playlists yet. Load an M3U URL in the M3U / IPTV source, then Add to My playlists.",
                            false,
                        );
                        return;
                    }
                    self.spawn(move || match mode {
                        TV_COUNTRY => iptv_index(true),
                        TV_CATEGORY => iptv_index(false),
                        GENRES => genre_list(),
                        MY_PL => my_playlists(ttl),
                        _ => somafm(),
                    });
                    return;
                }
                if q.is_empty() && mode != 1 {
                    Self::ui(cx, "Type something first", false);
                    return;
                }
                self.note = None;
                let tv_search = mode == M3U && !is_playlist_source(&q);
                if tv_search {
                    self.note = Some(format!(
                        "TV channels named “{q}” from iptv-org. Paste a URL or use Choose file… to import a playlist."
                    ));
                } else if mode == M3U {
                    self.loaded_src = q.clone();
                }
                self.mode = mode;
                self.clear(cx);
                Self::ui(
                    cx,
                    if tv_search {
                        "Searching iptv-org channels…"
                    } else if mode == M3U {
                        "Loading playlist…"
                    } else if mode == ALL {
                        "Searching TV and radio…"
                    } else {
                        "Searching radio-browser.info…"
                    },
                    true,
                );
                self.spawn(move || match mode {
                    M3U if tv_search => iptv_search(&q),
                    M3U => load_m3u(&q),
                    ALL => search_all(&q),
                    0 => radio_any(&q),
                    _ => radio_search(mode, &q),
                });
            }
            "chip" => {
                let c = args.first().cloned().unwrap_or_default();
                if self.category_chips {
                    if let Some((label, id)) = CATEGORY_CHIPS.iter().find(|x| x.0 == c) {
                        self.chip.clear();
                        self.open_playlist(cx, label, format!("{IPTV}/categories/{id}.m3u"));
                    }
                    return;
                }
                self.chip = if c == "All" { String::new() } else { c };
                self.apply_view(cx);
                cx.app
                    .global::<crate::MusicDialogState>()
                    .set_detail("".into());
                Self::ui(
                    cx,
                    &format!("{} of {}", self.list.len(), self.all.len()),
                    false,
                );
            }
            "select" => {
                self.selected = idx().filter(|i| *i < self.list.len());
                self.buttons(cx);
                if let Some(s) = self.selected.and_then(|i| self.list.get(i)) {
                    let mut d = s.detail();
                    if !s.tvg_id.is_empty() {
                        d += &format!("\nGuide id: {}", s.tvg_id);
                    }
                    if let Some(v) = self.guide_for(s) {
                        let l = super::epg::describe(v, super::epg::now());
                        if !l.is_empty() {
                            d += &format!("\n\n— Program guide —\n{}", l.join("\n"));
                        }
                    }
                    if cx.player.get("path").as_deref() == Some(s.url.as_str()) {
                        d += &format!("\n\n— Playing —\n{}", now_info(|p| cx.player.get(p)));
                    }
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_detail(d.into());
                }
            }
            "primary" => {
                let Some(s) = idx().and_then(|i| self.list.get(i)).cloned() else {
                    Self::ui(cx, "Select a stream first", false);
                    return;
                };
                if s.source == PICK {
                    self.open_playlist(cx, &s.name, s.url.clone());
                    return;
                }
                self.play(cx, &s, 0);
            }
            n if n.starts_with("act:") => {
                let sel = idx().and_then(|i| self.list.get(i)).cloned();
                match &n[4..] {
                    "Add to queue" | "Mini player" => {
                        let Some(s) = sel.filter(|s| s.source != PICK) else {
                            Self::ui(cx, "Select a stream first", false);
                            return;
                        };
                        self.play(cx, &s, if n.ends_with("queue") { 1 } else { 2 });
                    }
                    "Refresh" => {
                        let Some(src) = sel.and_then(|s| s.url.strip_prefix(PL).map(String::from))
                        else {
                            return;
                        };
                        Self::ui(cx, "Refreshing…", true);
                        self.note = Some("Refreshed. {n} playlists.".into());
                        self.clear(cx);
                        let ttl = Self::ttl(cx);
                        self.spawn(move || {
                            let r = fetch_playlist(&src, ttl, true);
                            let picks = playlist_picks(&playlists());
                            r.map(|_| picks)
                        });
                    }
                    "Add to My playlists" => {
                        let src = self.loaded_src.clone();
                        let mut v = playlists();
                        if v.iter().any(|p| p.url == src) {
                            Self::ui(cx, "Already in My playlists", false);
                            return;
                        }
                        v.push(Playlist {
                            name: playlist_name(&src),
                            url: src.clone(),
                            added: now_secs(),
                            ..Default::default()
                        });
                        save_playlists(&v);
                        let ttl = Self::ttl(cx);
                        std::thread::spawn(move || {
                            let _ = fetch_playlist(&src, ttl, true);
                        });
                        Self::ui(
                            cx,
                            "Added to My playlists. It refreshes itself; see the My playlists source.",
                            false,
                        );
                        self.buttons(cx);
                    }
                    _ => {}
                }
            }
            "secondary"
                if cx
                    .app
                    .global::<crate::MusicDialogState>()
                    .get_secondary()
                    .as_str()
                    == "Choose file…" =>
            {
                let Some(f) = rfd::FileDialog::new()
                    .set_title("Import a playlist")
                    .add_filter("Playlists", &["m3u", "m3u8"])
                    .pick_file()
                else {
                    return;
                };
                cx.app
                    .global::<crate::MusicDialogState>()
                    .set_query(f.to_string_lossy().as_ref().into());
                self.command(cx, "search", &[]);
            }
            "secondary" => {
                let Some(i) = idx().filter(|i| *i < self.list.len()) else {
                    Self::ui(cx, "Select a stream first", false);
                    return;
                };
                let s = self.list[i].clone();
                if self.mode == SAVED || self.mode == RECENT {
                    self.all.retain(|o| o.url != s.url);
                    if self.mode == SAVED {
                        hub_store::save_favorites(&self.all);
                    } else {
                        hub_store::save_recent(&self.all);
                    }
                    crate::streams::refresh_later();
                    self.reload_favs();
                    self.apply_view(cx);
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_detail("".into());
                    Self::ui(cx, &format!("Removed {}", s.name), false);
                } else if s.source == PICK {
                    if let Some(src) = s.url.strip_prefix(PL) {
                        let mut v = playlists();
                        v.retain(|p| p.url != src);
                        save_playlists(&v);
                        let _ = std::fs::remove_file(playlist_cache(src));
                        self.all.retain(|o| o.url != s.url);
                        self.apply_view(cx);
                        Self::ui(cx, &format!("Removed playlist {}", s.name), false);
                    } else {
                        Self::ui(cx, "Open it first, then favorite a stream", false);
                    }
                } else {
                    // The hub's ★: the same favorites as every other stream.
                    let was = !hub_store::toggle_favorite(&self.tagged(&s));
                    crate::streams::refresh_later();
                    self.reload_favs();
                    self.show_rows(cx);
                    self.selected = Some(i);
                    self.buttons(cx);
                    Self::ui(
                        cx,
                        &if was {
                            format!("Removed {} from favorites", s.name)
                        } else {
                            format!("★ {} added to favorites", s.name)
                        },
                        false,
                    );
                }
            }
            "info" => cx.toast(&now_info(|p| cx.player.get(p)).replace('\n', " · ")),
            "close" => self.task.cancel(),
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        self.poll = self.poll.wrapping_add(1);
        if cx.app.global::<crate::Overlays>().get_md_open()
            && cx.app.global::<crate::MusicDialogState>().get_owner() == ID
        {
            let src = cx.app.global::<crate::MusicDialogState>().get_source();
            if src.as_str() != self.last_source {
                self.last_source = src.to_string();
                let m = self.cur_mode(cx);
                cx.app
                    .global::<crate::MusicDialogState>()
                    .set_placeholder(placeholder(m).into());
                if self.mode != m {
                    cx.app.global::<crate::MusicDialogState>().set_secondary(
                        if m == M3U {
                            "Choose file…"
                        } else {
                            "Favorite"
                        }
                        .into(),
                    );
                }
            }
        }
        if live_filter(self.mode)
            && !self.all.is_empty()
            && cx.app.global::<crate::Overlays>().get_md_open()
        {
            let q = cx
                .app
                .global::<crate::MusicDialogState>()
                .get_query()
                .to_string();
            if q != self.filtered_by {
                self.filtered_by = q.clone();
                self.apply_view(cx);
                Self::ui(
                    cx,
                    &format!("{} of {}", self.list.len(), self.all.len()),
                    false,
                );
            }
        }
        if let Some(rx) = &self.row_logo_rx {
            let mut got = false;
            while let Ok((url, p)) = rx.try_recv() {
                let size = self.logo_size();
                let img = p.and_then(|p| logo_image(&p, size, false, url.clone()));
                self.logos.insert(url, img.unwrap_or_default());
                got = true;
            }
            self.logos_dirty |= got;
        }
        for (_, key, img) in READY.with(|r| std::mem::take(&mut *r.borrow_mut())) {
            self.logos.insert(key, img);
            self.logos_dirty = true;
        }
        if let Some(rx) = &self.epg_rx {
            loop {
                match rx.try_recv() {
                    Ok(Ok(g)) => {
                        self.epg.extend(g);
                        self.logos_dirty = true;
                    }
                    Ok(Err(e)) => eprintln!("vivi: EPG: {e}"),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        self.epg_rx = None;
                        break;
                    }
                }
            }
        }
        if self.logos_dirty && self.poll.is_multiple_of(10) {
            self.logos_dirty = false;
            self.show_rows(cx);
        }
        let Some(r) = self.task.poll() else { return };
        match r {
            Ok(v) => {
                let n = v.len();
                self.all = v;
                self.chip = self.chip_after.take().unwrap_or_default();
                if live_filter(self.mode) {
                    self.filtered_by = cx
                        .app
                        .global::<crate::MusicDialogState>()
                        .get_query()
                        .to_string();
                }
                self.apply_view(cx);
                self.want_epg(cx);
                Self::ui(
                    cx,
                    &if let Some(note) = self.note.take() {
                        note.replace("{n}", &n.to_string())
                    } else if n == 0 {
                        "Nothing found".to_string()
                    } else {
                        format!("{n} streams. Double-click to play.")
                    },
                    false,
                );
            }
            Err(e) => {
                self.note = None;
                Self::ui(cx, &e, false)
            }
        }
    }
}
