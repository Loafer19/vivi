//! The Free Music plugin: hooks the host calls (manifest, settings, ticks, commands).

use super::*;

impl Plugin for FreeMusic {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Free Music",
            "Search and browse Creative Commons and public-domain music from Jamendo, the Internet Archive and the Free Music Archive: genres, moods, licence filters, album and artist pages, play similar, the licence on every track and on the now-playing card, and Internet Archive downloads with tags, cover and licence attribution that play from disk once saved (Jamendo tracks link to jamendo.com for downloading).",
            "free-music",
        )
        .source("open")
        .contribute(contrib::PALETTE_COMMAND, "Find free music", "open")
        .contribute(contrib::STREAMS_MUSIC, "Free Music", "open")
        .contribute(contrib::HOME_WIDGET, "Discover free music", "open")
        .contribute(contrib::DOWNLOADS, "Track downloads", "")
        .contribute(contrib::DOWNLOADS_ACTION, "Find free music", "open")
        .setting(SettingSpec::secret(
            "jamendo-client-id",
            "Jamendo client ID",
            "For Jamendo (the biggest catalogue; searched together with the Internet Archive once set). Get a free client ID at https://devportal.jamendo.com (sign up → My Apps → Add application). Only the public client ID is needed, never the secret. The Internet Archive needs no key.",
        ))
        .setting(SettingSpec::text(
            "folder",
            "Download folder",
            "Where downloads go (one folder per artist). Add it to the library to see them there.",
            "~/Music/Free Music",
            "",
        ))
        .setting(SettingSpec::action(
            "choose-folder",
            "Choose download folder",
            "Pick the download folder with a dialog.",
            "Choose…",
        ))
        .setting(SettingSpec::choice(
            "downloader",
            "Download with",
            "Built-in saves tags, cover, licence and attribution. Offline Downloads (plugin) adds the file to its queue instead, without tags.",
            &DOWNLOADERS,
        ))
        .setting(SettingSpec::action(
            "open",
            "Find free music",
            "Opens the search and browse dialog.",
            "Browse…",
        ))
    }
    fn disable(&mut self, cx: &Ctx) {
        self.gen += 1;
        self.pending.clear();
        if cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
            cx.app.global::<crate::Overlays>().set_md_open(false);
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        match name {
            "open" => self.open(cx),
            // Downloads panel buttons (args[0] = track id).
            "download-cancel" => {
                let id = args.first().cloned().unwrap_or_default();
                if let Some((t, p)) = &self.dl_active {
                    if t.id == id {
                        p.cancel.store(true, Ordering::Relaxed);
                    }
                }
                self.dl_queue.retain(|t| t.id != id);
            }
            "download-retry" => {
                let id = args.first().cloned().unwrap_or_default();
                match self.dl_tracks.get(&id).cloned() {
                    Some(t) => self.download(cx, t),
                    None => cx.toast("Find the track in Free Music again to download it"),
                }
            }
            "choose-folder" => {
                let _ = slint::invoke_from_event_loop(|| {
                    if let Some(d) = rfd::FileDialog::new()
                        .set_title("Free Music download folder")
                        .pick_folder()
                    {
                        crate::plugins::command(
                            ID,
                            "set-folder",
                            &[d.to_string_lossy().into_owned()],
                        );
                    }
                });
            }
            "set-folder" => {
                if let Some(d) = args.first().filter(|d| !d.is_empty()) {
                    cx.set("folder", d);
                    self.saved.clear();
                    slint::Timer::single_shot(Duration::ZERO, crate::settings::refresh_ui);
                    cx.toast(&format!("Free Music downloads go to {d}"));
                }
            }
            "search" => self.search(cx),
            // `query <words…>`: fill the search box and search (scripts, screenshots).
            "query" => {
                cx.app
                    .global::<crate::MusicDialogState>()
                    .set_query(args.join(" ").into());
                self.search(cx);
            }
            // Typing: search after a short pause (see `tick`).
            "edited" => self.typed = Some(Instant::now()),
            "more" => {
                if self.pending.is_empty() && self.more.values().any(|m| *m) {
                    self.q.page += 1;
                    self.run(cx);
                }
            }
            "chip" => {
                let c = args.first().map(String::as_str).unwrap_or("");
                let i = CHIPS.iter().position(|x| x.0 == c);
                self.q.chip = if self.q.chip == i && i.is_some_and(|i| i >= 3) {
                    None
                } else {
                    i
                };
                if self.q.chip.is_some_and(|c| c < 3) {
                    // Popular / New / Featured are lists, not text searches.
                    self.q.text.clear();
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_query("".into());
                }
                self.search(cx);
            }
            "filter" => {
                let (Some(n), true) = (args.first(), args.len() > 1) else {
                    return;
                };
                let v = &args[1..].join(" ");
                let pos = |l: &[&str]| l.iter().position(|x| x == v).unwrap_or(0);
                match n.as_str() {
                    "mood" => self.q.mood = pos(&MOODS),
                    "licence" => self.q.lic = pos(&LICENCES),
                    "length" => self.q.len = pos(&LENGTHS),
                    "sort" => self.q.sort = pos(&SORTS),
                    _ => return,
                }
                self.q.page = 0;
                self.filters(cx);
                self.run(cx);
            }
            "select" => {
                if let Some(t) = self.at(args) {
                    self.select(cx, &t);
                    self.show(cx);
                }
            }
            "primary" => match self.at(args).or_else(|| self.selected().cloned()) {
                Some(t) => self.play(cx, &t, false),
                None => Self::ui(cx, "Select a track first", false),
            },
            "secondary" => match self.at(args).or_else(|| self.selected().cloned()) {
                // Jamendo: its page, where the listener downloads it themselves (#85).
                Some(t) if t.source == JAMENDO => crate::settings::open_url(&t.page),
                Some(t) => self.download(cx, t),
                None => Self::ui(cx, "Select a track first", false),
            },
            n if n.starts_with("act:") => {
                if &n[4..] == "‹ Back" {
                    if let Some(q) = self.back.pop() {
                        self.q = q;
                        if self.back.is_empty() {
                            self.title.clear();
                        }
                        self.sync_query_box(cx);
                        self.filters(cx);
                        self.run(cx);
                    }
                    return;
                }
                let Some(t) = self.at(args).or_else(|| self.selected().cloned()) else {
                    return;
                };
                let src = SOURCES.iter().position(|s| *s == t.source).unwrap_or(0);
                match &n[4..] {
                    "Queue" => self.play(cx, &t, true),
                    "Similar" => self.similar(cx, &t),
                    "Album" => {
                        let q = Query {
                            source: src,
                            album: Some((t.source, t.album_id.clone())),
                            ..Default::default()
                        };
                        self.page_view(cx, format!("Album: {}", t.album), q);
                    }
                    "Artist" => {
                        let q = Query {
                            source: src,
                            sort: self.q.sort,
                            artist: Some((t.source, t.artist_id.clone())),
                            ..Default::default()
                        };
                        self.page_view(cx, format!("Artist: {}", t.artist), q);
                    }
                    // The dialog copied the text to the clipboard itself.
                    "Copy credit" => cx.toast("Attribution copied to the clipboard"),
                    a if a == page_label(&t) => crate::settings::open_url(&t.page),
                    _ => {}
                }
            }
            "close" => {
                self.gen += 1;
                self.pending.clear();
            }
            _ => {}
        }
    }
    fn wants_tick(&self, cx: &Ctx) -> bool {
        self.typed.is_some()
            || !self.pending.is_empty()
            || (cx.app.global::<crate::Overlays>().get_md_open()
                && cx.app.global::<crate::MusicDialogState>().get_owner() == ID)
    }
    fn tick(&mut self, cx: &Ctx) {
        if self.typed.is_some_and(|t| {
            t.elapsed() >= DEBOUNCE && cx.app.global::<crate::MusicDialogState>().get_owner() == ID
        }) {
            self.typed = None;
            let q = cx.app.global::<crate::MusicDialogState>().get_query();
            if q.trim().chars().count() >= 2 && q.trim() != self.q.text.trim() {
                self.search(cx);
            }
        }
        // A new source in the dropdown searches right away (like Network Streams).
        if cx.app.global::<crate::Overlays>().get_md_open()
            && cx.app.global::<crate::MusicDialogState>().get_owner() == ID
        {
            let src = cx.app.global::<crate::MusicDialogState>().get_source();
            if src.as_str() != self.last_source && SOURCES.contains(&src.as_str()) {
                self.search(cx);
            }
        }
        for (url, img) in DETAIL.with(|r| std::mem::take(&mut *r.borrow_mut())) {
            self.show_detail_cover(cx, &url, img);
        }
        for (url, img) in READY.with(|r| std::mem::take(&mut *r.borrow_mut())) {
            if is_cover(&img) {
                self.covers.insert(url, img);
                self.covers_dirty = true;
            }
        }
        let mut changed = false;
        let mut status = false;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Page {
                    gen,
                    src,
                    pos,
                    tracks,
                    more,
                    done,
                } if gen == self.gen => {
                    if !tracks.is_empty() {
                        self.page.entry(src).or_default().insert(pos, tracks);
                    }
                    self.more.insert(src, more);
                    if done {
                        self.pending.remove(src);
                    }
                    changed = true;
                }
                Msg::Fail { gen, src, err } if gen == self.gen => {
                    eprintln!("vivi: free music: {err}");
                    self.pending.remove(src);
                    self.errors.push(err);
                    changed = true;
                }
                Msg::Cover(url, p) => {
                    let Some(p) = p else { continue };
                    let key = url.clone();
                    let img = crate::images::get(&p, crate::images::ROW, move |i| {
                        READY.with(|r| r.borrow_mut().push((key, i)))
                    });
                    if let Some(img) = img.filter(is_cover) {
                        self.covers.insert(url, img);
                        self.covers_dirty = true;
                    }
                }
                Msg::Detail(url, p) => {
                    if let Some(p) = p.filter(|_| url == self.detail_url) {
                        self.detail_ready(cx, &url, &p);
                    }
                }
                Msg::Saved(id, title, r) => {
                    // Cancel in the Downloads panel: "Cancelled", not a failure.
                    let cancelled = self
                        .dl_active
                        .take()
                        .is_some_and(|(_, p)| p.cancel.load(Ordering::Relaxed));
                    // The row gets its check mark (or loses it) on the next refresh.
                    self.saved.remove(&id);
                    self.covers_dirty = true;
                    // The Downloads panel notifies (desktop notification or toast).
                    match r {
                        Ok(p) => {
                            self.dl_done += 1;
                            self.dl_last = format!("Saved “{title}”");
                            crate::downloads::report(
                                ID,
                                &id,
                                crate::downloads::Report {
                                    state: Some("done".into()),
                                    path: Some(p.to_string_lossy().into_owned()),
                                    ..Default::default()
                                },
                            );
                        }
                        Err(e) => {
                            self.dl_last = if cancelled {
                                format!("Cancelled “{title}”")
                            } else {
                                format!("“{title}” failed: {e}")
                            };
                            let state = if cancelled { "cancelled" } else { "failed" };
                            crate::downloads::report(
                                ID,
                                &id,
                                crate::downloads::Report {
                                    state: Some(state.into()),
                                    error: Some(e),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                    self.next_download(cx);
                    status = true;
                }
                _ => {}
            }
        }
        if changed {
            self.remerge();
            self.feed_radio(cx);
            self.covers_dirty = false;
            if cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
                self.show(cx);
            }
            status = true;
        } else if self.covers_dirty
            && cx.app.global::<crate::MusicDialogState>().get_owner() == ID
            && cx.app.global::<crate::Overlays>().get_md_open()
        {
            self.covers_dirty = false;
            self.show(cx);
        }
        if let Some((t, p)) = &self.dl_active {
            status = true;
            crate::downloads::report(
                ID,
                &t.id,
                crate::downloads::Report {
                    done: Some(p.got.load(Ordering::Relaxed)),
                    total: Some(p.total.load(Ordering::Relaxed)),
                    ..Default::default()
                },
            );
        }
        if status && cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
            self.status(cx);
        }
    }
}
