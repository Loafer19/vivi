//! The Auto-Tagger plugin: hooks the host calls (manifest, settings, ticks, commands).

use super::*;

impl Plugin for AutoTagger {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Auto-tagger",
            "Identifies songs by their audio (AcoustID fingerprint) or tags, looks them up on MusicBrainz, Deezer, iTunes, Last.fm and Discogs and writes tags, hi-res covers and optional LRCLIB synced lyrics after a preview that shows each field's source. Originals are backed up.",
            "tag",
        )
        .contribute(contrib::CONTEXT_ITEM, "Auto-tag…", "open-path")
        .contribute(contrib::CONTEXT_PLAYER, "Auto-tag this file…", "open")
        .when(&["local", "audio"])
        .contribute(contrib::PALETTE_COMMAND, "Auto-tag the playing file", "open")
        .setting(SettingSpec::secret(
            "acoustid-key",
            "AcoustID API key",
            "Optional. Register a free application at acoustid.org/new-application for audio fingerprint matching; without it songs are matched by tags or file name.",
        ))
        .setting(SettingSpec::toggle(
            "cover",
            "Embed cover art",
            "Download the front cover from the Cover Art Archive.",
            true,
        ))
        .setting(SettingSpec::toggle(
            "deezer",
            "Deezer",
            "Fill album, track, date and add a 1000 px cover from Deezer (no account needed).",
            true,
        ))
        .setting(SettingSpec::toggle(
            "itunes",
            "iTunes",
            "Fill album, track, date, genre and add a 1400 px cover from the iTunes Search API (no account needed).",
            true,
        ))
        .setting(SettingSpec::secret(
            "lastfm-key",
            "Last.fm API key",
            "Optional. Genre from Last.fm's top tags; create a free key at last.fm/api/account/create.",
        ))
        .setting(SettingSpec::secret(
            "discogs-token",
            "Discogs token",
            "Optional. Year, genre/style and cover from Discogs; personal access token from discogs.com/settings/developers.",
        ))
        .setting(SettingSpec::toggle(
            "lyrics",
            "Embed synced lyrics",
            "Adds LRCLIB lyrics to the review: MP3 gets SYLT + USLT, FLAC/Ogg/Opus a LYRICS comment, M4A ©lyr (LRC text).",
            false,
        ))
        .setting(SettingSpec::toggle(
            "rename",
            "Suggest file names",
            "When the audio fingerprint identifies a song, the review offers a new file name (unticked: renaming is opt-in). Never overwrites a file; characters not allowed in file names become _.",
            true,
        ))
        .setting(SettingSpec::text(
            "rename-pattern",
            "File name pattern",
            "Fields: {artist} {title} {album} {albumartist} {year} {date} {track} {genre}.",
            "{artist} - {title}",
            "{artist} - {title}",
        ))
        .setting(SettingSpec::text(
            "folder",
            "Library folder",
            "Folder for Tag library folder; empty = first library folder.",
            "~/Music",
            "",
        ))
        .setting(SettingSpec::action(
            "open",
            "Tag the playing file",
            "Opens the preview dialog for the current file.",
            "Tag file…",
        ))
        .setting(SettingSpec::action(
            "batch",
            "Tag library folder",
            "Finds confident matches for every audio file; you review before writing.",
            "Tag folder…",
        ))
        .setting(SettingSpec::action(
            "restore",
            "Restore original tags",
            "Puts back the tags of the playing file from before the first auto-tag.",
            "Restore",
        ))
        .setting(SettingSpec::toggle(
            "auto-identify",
            "Identify untagged songs while playing",
            "When a local song without title / artist tags starts, identify it in the background by its audio fingerprint (AcoustID). When the match is at least 70% sure, the player, the now-playing card, MPRIS and extensions show its title, artist and album; the file is not changed until you review and apply the tags.",
            false,
        ))
        .setting(SettingSpec::action(
            "forget",
            "Forget cached results",
            "Each file's AcoustID answer is remembered (by path, size and modification time, at most 300 files) so identifying it again, playing it again or tagging its folder needs no new fingerprint. Clear it to look every file up again.",
            "Forget",
        ))
    }
    fn setting_changed(&mut self, _cx: &Ctx, key: &str) {
        if key == "auto-identify" {
            // Look at the playing song again (it may be untagged).
            self.auto.seen = -1;
            self.auto.warned = false;
        }
    }
    fn disable(&mut self, cx: &Ctx) {
        self.auto_cancel();
        self.cancel.store(true, Ordering::Relaxed);
        self.rx = None;
        if cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
            cx.app.global::<crate::Overlays>().set_md_open(false);
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let idx = || args.first().and_then(|a| a.parse::<usize>().ok());
        match name {
            "open" => {
                let p = cx
                    .local_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                cx.app.global::<crate::Overlays>().set_settings_open(false);
                self.open(cx, p);
            }
            // `context.item`: tag the right-clicked library file or folder.
            "open-path" => {
                let p = args.first().cloned().unwrap_or_default();
                cx.app.global::<crate::Overlays>().set_settings_open(false);
                self.open(cx, p);
            }
            "batch" => {
                let f = cx.get("folder");
                let f = if f.trim().is_empty() {
                    crate::config::Config::load()
                        .library
                        .folders
                        .first()
                        .cloned()
                        .unwrap_or_default()
                } else {
                    f
                };
                let f = match f.strip_prefix("~/") {
                    Some(r) => crate::store::home().join(r).to_string_lossy().into_owned(),
                    None => f,
                };
                cx.app.global::<crate::Overlays>().set_settings_open(false);
                self.open(cx, f);
            }
            "restore" => match cx.local_path() {
                Some(p) => match restore(&backup_dir(), &p) {
                    Ok(()) => cx.toast("Original tags restored"),
                    Err(e) => cx.toast(&e),
                },
                None => cx.toast("Play a local file first"),
            },
            "search" => self.start(cx),
            "acoustid" => self.external(cx, args),
            // Toast button of an automatic identification: the tag review of that song.
            "review-identified" => {
                let path = self.auto.last.as_ref().map(|(p, _)| p.clone());
                match path.filter(|p| Path::new(p).is_file()) {
                    Some(p) => {
                        cx.app.global::<crate::Overlays>().set_settings_open(false);
                        self.straight_to_review = true;
                        self.open(cx, p);
                    }
                    None => cx.toast("Auto-tagger: the identified file is gone"),
                }
            }
            "forget" => {
                self.auto.tried.clear();
                match forget_cached() {
                    Ok(n) => cx.toast(&format!(
                        "Auto-tagger: forgot the cached results of {n} file{}",
                        if n == 1 { "" } else { "s" }
                    )),
                    Err(e) => cx.toast(&format!("Auto-tagger: could not clear the cache: {e}")),
                }
            }
            "select" => {
                if let Some(i) = idx() {
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_detail(self.detail(i).into());
                }
            }
            "primary" => self.review(cx, idx().into_iter().collect()),
            "secondary" => {
                // One row per file: the first (best) one.
                let mut seen = std::collections::HashSet::new();
                let all = (0..self.rows.len())
                    .filter(|&i| seen.insert(self.rows[i].path.clone()))
                    .collect();
                self.review(cx, all);
            }
            "toggle-file" | "toggle-field" | "cover-pick" | "field-alt" => {
                let a = args.first().map(String::as_str).unwrap_or("");
                let mut it = a.split(':');
                let i = it.next().and_then(|x| x.parse::<usize>().ok());
                let j = it.next().and_then(|x| x.parse::<i64>().ok());
                let Some(r) = i.and_then(|i| self.reviews.get_mut(i)) else {
                    return;
                };
                match (name, j) {
                    ("toggle-file", _) => r.on = !r.on,
                    ("toggle-field", Some(j)) if j as usize >= r.fields.len() => {
                        r.rename_on = !r.rename_on && r.rename.is_some();
                    }
                    ("toggle-field", Some(j)) => {
                        let (o, n) = {
                            let f = review_fields(&r.old, &r.cand.tags);
                            let (_, o, n) = f[j as usize].clone();
                            (o, n)
                        };
                        if field_changes(&o, &n) {
                            if let Some(b) = r.fields.get_mut(j as usize) {
                                *b = !*b;
                            }
                        }
                    }
                    ("field-alt", Some(j)) if j >= 0 => {
                        r.next_alt(j as usize);
                    }
                    ("cover-pick", Some(j)) => {
                        r.cover = (j >= 0 && (j as usize) < r.covers.len()).then_some(j as usize)
                    }
                    _ => {}
                }
                self.render_review(cx);
            }
            "apply-selected" => self.apply_reviews(cx),
            "apply-all" => {
                for r in &mut self.reviews {
                    r.on = true;
                    let f = review_fields(&r.old, &r.cand.tags);
                    r.fields = f.iter().map(|(_, o, n)| field_changes(o, n)).collect();
                    if r.cover.is_none() && !r.covers.is_empty() {
                        r.cover = Some(0);
                    }
                    // Renaming stays opt-in: only files whose rename box is ticked.
                }
                self.render_review(cx);
                self.apply_reviews(cx);
            }
            "undo" => self.undo(cx, None),
            "undo-file" => self.undo(cx, idx()),
            "open-folder" => {
                if let Some(r) = idx().and_then(|i| self.results.get(i)) {
                    if let Some(d) = r.path.parent() {
                        let _ = std::process::Command::new("xdg-open").arg(d).spawn();
                    }
                }
            }
            "review-close" => {
                cx.app.global::<crate::Overlays>().set_tr_open(false);
                self.reviews.clear();
                cx.app
                    .global::<crate::TagReview>()
                    .set_files(ModelRc::default());
                // Thumbnails are only needed while the dialog is open.
                let _ = std::fs::remove_dir_all(cover_cache());
            }
            "close" => {
                self.cancel.store(true, Ordering::Relaxed);
                self.rx = None;
            }
            _ => {}
        }
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.auto_tick(cx);
    }
    fn wants_tick(&self, _cx: &Ctx) -> bool {
        self.rx.is_some() || self.auto.job.is_some() || self.auto.pending.is_some()
    }
    fn tick(&mut self, cx: &Ctx) {
        self.auto_tick(cx);
        let Some(rx) = &self.rx else { return };
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Status(s) => {
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_status(s.as_str().into());
                    cx.app.global::<crate::TagReview>().set_status(s.into());
                }
                Msg::Row(r) => {
                    self.rows.push(*r);
                    self.show_rows(cx, None);
                }
                Msg::Rows(r, note) => {
                    self.rows = r;
                    self.show_rows(cx, None);
                    let s = if self.rows.is_empty() {
                        "No match found on MusicBrainz".to_string()
                    } else {
                        format!(
                            "{} candidate{}. Select one to preview the changes.",
                            self.rows.len(),
                            if self.rows.len() == 1 { "" } else { "s" }
                        )
                    };
                    let s = if note.is_empty() {
                        s
                    } else {
                        format!("{s} {note}.")
                    };
                    Self::ui(cx, &s, false);
                    if !self.rows.is_empty() {
                        cx.app
                            .global::<crate::MusicDialogState>()
                            .set_detail(self.detail(0).into());
                    }
                    self.rx = None;
                    // *Review* of an automatic identification, screenshots/tests: go
                    // straight to the review of the best match.
                    let straight = std::mem::take(&mut self.straight_to_review);
                    if (straight || std::env::var_os("VIVI_TAGREVIEW").is_some())
                        && !self.rows.is_empty()
                    {
                        self.review(cx, vec![0]);
                    }
                    return;
                }
                Msg::Done(s) => {
                    Self::ui(cx, &s, false);
                    if self.batch && !self.rows.is_empty() {
                        cx.app
                            .global::<crate::MusicDialogState>()
                            .set_detail(self.detail(0).into());
                    }
                    self.rx = None;
                    return;
                }
                Msg::Reviews(v) => {
                    self.reviews = v;
                    self.rx = None;
                    Self::ui(cx, "", false);
                    cx.app.global::<crate::TagReview>().set_summary("".into());
                    cx.app.global::<crate::TagReview>().set_busy(false);
                    cx.app.global::<crate::TagReview>().set_can_undo(false);
                    self.render_review(cx);
                    cx.app.global::<crate::Overlays>().set_md_open(false);
                    cx.app.global::<crate::Overlays>().set_tr_open(true);
                    if std::env::var("VIVI_TAGREVIEW").as_deref() == Ok("apply") {
                        self.apply_reviews(cx);
                    }
                    return;
                }
                Msg::Applied(results) => {
                    self.rx = None;
                    if let Some(sess) = &mut self.session {
                        sess.1 = results
                            .iter()
                            .filter(|r| r.written)
                            .map(|r| r.orig.clone())
                            .collect();
                    }
                    self.results = results;
                    cx.app.global::<crate::TagReview>().set_busy(false);
                    self.render_summary(cx);
                    cx.app.global::<crate::TagReview>().set_status(
                        "Backups kept in ~/.local/share/vivi/tag-backups · Undo restores tags, covers and names".into(),
                    );
                    let (t, _, _, f) = result_counts(&self.results);
                    cx.toast(&format!(
                        "Auto-tagger: {t} tagged{}",
                        if f > 0 {
                            format!(", {f} failed")
                        } else {
                            String::new()
                        }
                    ));
                    return;
                }
            }
        }
    }
}
