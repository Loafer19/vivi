//! The Skip Intro plugin: hooks the host calls (manifest, settings, ticks, commands).

use super::*;

impl Plugin for SkipIntro {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Skip Intro & Credits",
            "One Skip button for intros, credits and the segments extensions such as SponsorBlock find: from chapter names, your per-show ranges or the extension. Skip, ask or off per category, Undo after automatic skips, Alt+K / Alt+Shift+K.",
            "skip",
        )
        .contribute(contrib::PALETTE_COMMAND, "Skip segment", "key")
        .contribute(contrib::PALETTE_COMMAND, "Undo last skip", "undo")
        .contribute(contrib::PALETTE_COMMAND, "Mark intro / credits here", "mark")
        .contribute(contrib::CONTROLBAR_BUTTON, "Mark intro / credits here", "mark")
        .setting(SettingSpec::toggle(
            "auto",
            "Skip automatically",
            "Jump over intros and credits without asking; an Undo button shows for a few seconds (Alt+Shift+K undoes later too). Segments from extensions follow the extension's own setting.",
            false,
        ))
        .setting(SettingSpec::toggle(
            "chapters",
            "Detect from chapter names",
            "Chapters named Intro, Opening, OP, Credits, Ending or ED. Markers like “End of intro” and post-credits scenes are not skipped.",
            true,
        ))
        .setting(SettingSpec::text(
            "modes",
            "Per category or source",
            "skip, ask or off; overrides the rest. Categories: intro, credits and the ones extensions send (sponsor, selfpromo, outro, filler, …). Sources: chapters, show, or an extension id (sponsorblock). Both: sponsorblock:intro=off.",
            "intro=skip credits=ask sponsorblock:filler=off",
            "",
        ))
        .setting(SettingSpec::toggle(
            "offer",
            "Offer to always skip for a show",
            "After you skip an intro or credits by hand, the button offers to skip them automatically for that show from now on.",
            true,
        ))
        .setting(SettingSpec::text(
            "auto-shows",
            "Shows skipped automatically",
            "Filled in by that offer. Format: show name = intro, credits; other show = intro",
            "the wand = intro, credits",
            "",
        ))
        .setting(SettingSpec::text(
            "ranges",
            "Per-show ranges",
            "Format: show name = 0:30-2:00, 21:10-end; other show = …  The show name is the file name without the episode number.",
            "the wand = 0:30-2:00",
            "",
        ))
        .setting(SettingSpec::action(
            "mark",
            "Mark range for this show",
            "Press once at the start of the intro (or credits) and again at its end.",
            "Mark",
        ))
        .setting(SettingSpec::action(
            "clear",
            "Forget this show",
            "Clears its marked ranges and automatic skipping.",
            "Clear",
        ))
    }

    fn enable(&mut self, cx: &Ctx) {
        Self::set_capability(cx, true);
        self.reload(cx);
    }
    fn disable(&mut self, cx: &Ctx) {
        // Packages fall back to their own skipping while the button is off.
        Self::set_capability(cx, false);
        self.restore_sound(cx);
        self.ranges.clear();
        self.submitted.clear();
        cx.app.global::<crate::SkipIntro>().set_label("".into());
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.mark = None;
        // A new start even when the same file plays again (ranges it submitted already
        // for this path stay).
        self.path.clear();
        self.reload(cx);
    }
    fn setting_changed(&mut self, cx: &Ctx, _key: &str) {
        self.reload(cx);
    }

    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        match name {
            // `vivi-skip-ranges <source> <json>` / `vivi-skip-ranges-clear <source>`.
            "ranges" | "ranges-clear" => {
                let Some(source) = args.first().map(|s| s.trim().to_ascii_lowercase()) else {
                    return;
                };
                if source.is_empty() || source == CHAPTERS || source == SHOW {
                    eprintln!("vivi: vivi-skip-ranges: invalid source {source:?}");
                    return;
                }
                let list = match (name, args.get(1)) {
                    ("ranges", Some(json)) => match parse_submitted(&source, json) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!("vivi: vivi-skip-ranges {source}: bad json: {e}");
                            return;
                        }
                    },
                    _ => Vec::new(),
                };
                let path = cx.player.get("path").unwrap_or_default();
                if path != self.path {
                    self.reload(cx);
                }
                eprintln!("vivi: {} skip range(s) from {source}", list.len());
                self.submitted.retain(|(s, _, _)| *s != source);
                if !list.is_empty() {
                    self.submitted.push((source, path, list));
                }
                self.reload(cx);
                return;
            }
            "skip" => return self.press(cx, false),
            "key" => return self.press(cx, true),
            "undo" => {
                self.undo(cx);
                return;
            }
            "dismiss" => return self.dismiss(cx),
            _ => {}
        }
        let Some(path) = cx.player.get("path") else {
            cx.toast("Open a file first");
            return;
        };
        let show = show_key(&path);
        let t: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(0.0);
        let mut all = parse_user(&cx.get("ranges"));
        match name {
            "mark" => match self.mark.take() {
                None => {
                    self.mark = Some(t);
                    cx.toast(&format!(
                        "Range starts at {}; mark again at its end",
                        fmt_time(t)
                    ));
                    return;
                }
                Some(a) if t > a => {
                    let end = if self.duration > 0.0 && self.duration - t < 2.0 {
                        f64::INFINITY
                    } else {
                        t
                    };
                    match all.iter_mut().find(|(s, _)| *s == show) {
                        Some((_, r)) => r.push((a, end)),
                        None => all.push((show.clone(), vec![(a, end)])),
                    }
                    cx.toast(&format!(
                        "Saved {}–{} for “{show}”",
                        fmt_time(a),
                        fmt_time(end)
                    ));
                }
                Some(_) => {
                    cx.toast("The end must be after the start");
                    return;
                }
            },
            "clear" => {
                all.retain(|(s, _)| *s != show);
                let mut auto = parse_auto_shows(&cx.get("auto-shows"));
                auto.retain(|(s, _)| *s != show);
                cx.set("auto-shows", &format_auto_shows(&auto));
                self.offered.retain(|o| !o.starts_with(&format!("{show}|")));
                cx.toast(&format!(
                    "Forgot ranges and automatic skipping for “{show}”"
                ));
            }
            _ => return,
        }
        cx.set("ranges", &format_user(&all));
        self.reload(cx);
    }

    fn wants_tick(&self, cx: &Ctx) -> bool {
        cx.app.global::<crate::Playback>().get_playing()
            && cx.app.global::<crate::Playback>().get_has_media()
    }
    fn tick(&mut self, cx: &Ctx) {
        // Chapters added later (SponsorBlock, scene detection), another file, or the
        // duration of a stream known only now.
        let n: i64 = cx
            .player
            .mpv
            .get_property("chapter-list/count")
            .unwrap_or(0);
        let stale = n != self.chapter_count
            || cx.player.get("path").unwrap_or_default() != self.path
            || (self.duration <= 0.0
                && cx.player.mpv.get_property::<f64>("duration").unwrap_or(0.0) > 0.0);
        if stale {
            self.reload(cx);
        }
        let Ok(t) = cx.player.mpv.get_property::<f64>("time-pos") else {
            self.render(cx, None);
            return;
        };
        // Sound back after the range we muted.
        if let Some(k) = self.muted {
            let inside = self
                .ranges
                .iter()
                .any(|r| r.key() == k && t >= r.start && t < r.end);
            if !inside {
                self.restore_sound(cx);
            }
        }
        if let Some(i) = self.current(t, Act::Skip) {
            if self.mode(&self.ranges[i]) == Mode::Skip {
                self.skip(cx, i, true);
            }
        }
        if self.muted.is_none() {
            if let Some(i) = self.current(t, Act::Mute) {
                if self.mode(&self.ranges[i]) == Mode::Skip {
                    self.mute(cx, i);
                }
            }
        }
        let b = self.button(t);
        if b.is_none() && self.after.as_ref().is_some_and(|a| Instant::now() >= a.2) {
            self.after = None;
        }
        self.render(cx, b.as_ref());
    }
}
