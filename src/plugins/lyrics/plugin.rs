//! The Lyrics plugin: hooks the host calls (manifest, settings, ticks, commands).

use super::*;

impl Plugin for Lyrics {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Lyrics",
            "Synced lyrics as a karaoke overlay, a full-screen karaoke view with word highlight or a side-drawer list, from your pick, a .lrc file, the file's tags or LRCLIB; click a line to jump there, per-track timing offset, save as .lrc or embed, radio lyrics by stream title, instrumental tracks recognised.",
            "lyrics",
        )
        .contribute(contrib::PALETTE_COMMAND, "Find lyrics", "search-open")
        .contribute(contrib::PALETTE_COMMAND, "Karaoke lyrics view", "karaoke")
        .contribute(contrib::CONTROLBAR_BUTTON, "Karaoke lyrics view", "karaoke")
        .contribute(contrib::CONTEXT_PLAYER, "Find lyrics…", "search-open")
        .contribute(contrib::DRAWER_PANEL, "Lyrics", "")
        .setting(SettingSpec::toggle(
            "online",
            "Search lyrics online (LRCLIB)",
            "Sends artist, title, album and duration to lrclib.net (radio: the stream title). Results are cached; songs not found are asked again after a week.",
            true,
        ))
        .setting(SettingSpec::choice(
            "save-mode",
            "Keep downloaded lyrics",
            "Also store lyrics found on LRCLIB with the song: a .lrc file next to it (never replaces one) or embedded in its tags (a tag backup is kept, Auto-tagger → Restore).",
            &["Don't save", "Save .lrc next to the song", "Embed in the file's tags"],
        ))
        .setting(SettingSpec::choice(
            "radio-mode",
            "Radio lyrics",
            "Network streams: lyrics are looked up when the stream title changes. The song's start is unknown, so they start as a list; with synced lyrics, click the line being sung to sync them.",
            &["Click a line to sync", "Plain lyrics", "Off"],
        ))
        .setting(SettingSpec::choice(
            "size",
            "Text size",
            "",
            &["Medium", "Small", "Large"],
        ))
        .setting(SettingSpec::action(
            "clear-cache",
            "Downloaded lyrics",
            "Delete cached lyrics, including songs remembered as not found and your manual picks.",
            "Clear cache",
        ))
    }
    fn enable(&mut self, cx: &Ctx) {
        cx.app.global::<crate::Lyrics>().set_enabled(true);
        // Side drawer lines (a click seeks there).
        cx.app
            .global::<crate::Lyrics>()
            .on_cmd(|n, a| crate::plugins::command(ID, &n, &[a.to_string()]));
        self.apply_size(cx);
        if cx.app.global::<crate::Playback>().get_has_media() {
            self.load(cx);
        }
    }
    fn disable(&mut self, cx: &Ctx) {
        cx.app.global::<crate::Lyrics>().set_enabled(false);
        cx.app.global::<crate::Overlays>().set_karaoke_open(false);
        cx.app.global::<crate::Overlays>().set_ls_open(false);
        self.task.cancel();
        self.show(cx, None);
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.load(cx);
    }
    fn setting_changed(&mut self, cx: &Ctx, key: &str) {
        match key {
            "size" => self.apply_size(cx),
            "online" | "radio-mode" => self.load(cx),
            _ => {}
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let arg = args.first().map(String::as_str).unwrap_or("");
        let idx = arg.parse::<usize>().ok();
        match name {
            "clear-cache" => {
                let _ = std::fs::remove_dir_all(lyrics_dir());
                cx.toast("Lyrics cache cleared");
                self.load(cx);
            }
            "karaoke" => {
                let on = match arg {
                    "on" => true,
                    "off" => false,
                    _ => !cx.app.global::<crate::Overlays>().get_karaoke_open(),
                };
                cx.app
                    .global::<crate::Overlays>()
                    .set_karaoke_open(on && cx.app.global::<crate::Playback>().get_has_media());
                self.word_line = None;
            }
            "offset" => match arg {
                "reset" => self.set_offset(cx, 0.0),
                d => {
                    if let Ok(d) = d.parse::<f64>() {
                        self.set_offset(cx, self.offset + d);
                    }
                }
            },
            "sync-line" => {
                if let Some(i) = idx {
                    self.sync_line(cx, i);
                }
            }
            "seek-line" => {
                if let Some(i) = idx {
                    self.seek_line(cx, i);
                }
            }
            "search-open" => self.search_open(cx),
            "search" => self.search_run(cx),
            "search-select" => {
                if let Some(i) = idx {
                    self.search_select(cx, i);
                }
            }
            "search-use" | "search-plain" => {
                if let Some(i) = idx {
                    self.search_use(cx, i, name == "search-plain");
                }
            }
            "search-close" => {
                self.search.task.cancel();
                cx.app.global::<crate::Lyrics>().set_search_busy(false);
                cx.app.global::<crate::Overlays>().set_ls_open(false);
            }
            "forget" => {
                let _ = std::fs::remove_file(manual_file(&self.key));
                if let Some(r) = self.radio.as_mut() {
                    r.title.clear();
                    r.mem.remove(&r.key);
                } else {
                    self.load(cx);
                }
                cx.toast("Manual lyrics removed");
            }
            "save-lrc" => self.save_lrc(cx, false),
            "embed" => self.embed(cx, false),
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        self.search_tick(cx);
        let np = crate::nowplaying::current_id();
        if np != self.np_id {
            self.np_id = np;
            self.now_playing(cx);
        }
        if self.radio.is_some() && cx.get("radio-mode") != "Off" {
            self.radio_result(cx);
            self.radio_tick(cx);
        }
        if let Some((key, res)) = self.task.poll() {
            if Some(&key) == self.path.as_ref() {
                self.online_result(cx, res);
            }
        }
        if self.lrc.is_none() || self.plain() {
            return; // plain lyrics: nothing to highlight
        }
        let now: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(0.0);
        let t = if self.radio.is_some() {
            match self.anchor.lyrics_time(now, self.offset) {
                Some(t) => t,
                None => return,
            }
        } else {
            now - self.offset
        };
        let Some(l) = &self.lrc else { return };
        let i = l.index_at(t + 0.15);
        if i != self.index {
            self.index = i;
            cx.app
                .global::<crate::Lyrics>()
                .set_index(i.map(|i| i as i32).unwrap_or(-1));
        }
        self.update_words(cx, t + 0.05);
    }
}
