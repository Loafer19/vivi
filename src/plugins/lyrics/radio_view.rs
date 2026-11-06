//! Radio mode: poll the stream title, look the song up, show what was found.

use super::*;

impl Lyrics {
    /// Poll the stream title (twice a second) and look lyrics up when it changes.
    pub(super) fn radio_tick(&mut self, cx: &Ctx) {
        let Some(r) = self.radio.as_mut() else { return };
        if r.last_poll
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        r.last_poll = Some(std::time::Instant::now());
        let raw = cx
            .player
            .get("metadata/by-key/icy-title")
            .filter(|s| !s.trim().is_empty())
            .or_else(|| cx.player.get("media-title"))
            .unwrap_or_default();
        if raw == r.title {
            return;
        }
        r.title = raw.clone();
        r.task.cancel();
        let station = cx
            .player
            .get("metadata/by-key/icy-name")
            .unwrap_or_default();
        let parsed = parse_stream_title(&raw, &station);
        let key = parsed
            .as_ref()
            .map(|(a, t)| crate::plugins::protocol::join(&[a, t]))
            .unwrap_or_default();
        r.key = key.clone();
        r.song = parsed.clone();
        let Some((artist, title)) = parsed else {
            self.key.clear();
            self.show(cx, None); // ad / station ID / talk: hide
            return;
        };
        self.set_key(cx, crate::plugins::protocol::join(&["radio", key.as_str()]));
        if let Some(s) = read_lyrics_file(&manual_file(&self.key)) {
            let it = if is_synced(&s) {
                LrclibItem {
                    synced: Some(s),
                    ..Default::default()
                }
            } else {
                LrclibItem {
                    plain: Some(s),
                    ..Default::default()
                }
            };
            self.show_radio(cx, Some(&it), Source::Manual);
            return;
        }
        let r = self.radio.as_mut().expect("radio");
        if let Some(hit) = r.mem.get(&key).cloned() {
            self.show_radio(cx, hit.as_ref(), Source::Cache);
            return;
        }
        let cache = radio_cache_file(&artist, &title);
        if let Ok(s) = std::fs::read_to_string(&cache) {
            if let Ok(it) = serde_json::from_str::<LrclibItem>(&s) {
                r.mem.insert(key, Some(it.clone()));
                self.show_radio(cx, Some(&it), Source::Cache);
                return;
            }
        }
        let offline = !cx.flag("online") || cached_miss(&cache).is_some();
        if offline {
            r.mem.insert(key.clone(), None);
        }
        if !offline {
            r.task = crate::util::Task::spawn(move || {
                let res = fetch_radio(&artist, &title);
                if let Some(d) = cache.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                match &res {
                    Ok(Some(it)) => {
                        if let Ok(j) = serde_json::to_string(it) {
                            let _ = std::fs::write(&cache, j);
                        }
                    }
                    Ok(None) => write_cache(&cache, &Fetched::NotFound),
                    Err(e) => eprintln!("vivi: radio lyrics: {e}"),
                }
                (key, res.ok().flatten())
            });
        }
        self.show(cx, None);
    }

    pub(super) fn radio_result(&mut self, cx: &Ctx) {
        let Some(r) = self.radio.as_mut() else { return };
        let Some((key, it)) = r.task.poll() else {
            return;
        };
        r.mem.insert(key.clone(), it.clone());
        if key == r.key {
            self.show_radio(cx, it.as_ref(), Source::Online);
        }
    }

    pub(super) fn show_radio(&mut self, cx: &Ctx, it: Option<&LrclibItem>, source: Source) {
        let Some(it) = it.filter(|i| i.has_lyrics()) else {
            self.show(cx, None);
            if it.is_some_and(|i| i.instrumental == Some(true)) {
                self.instrumental = true;
                self.update_info(cx);
            }
            return;
        };
        let synced = non_empty(&it.synced)
            .map(Lrc::parse)
            .filter(|l| !l.lines.is_empty());
        let text = it.best_text().unwrap_or_default();
        match synced {
            Some(l) if click_sync(&cx.get("radio-mode")) => {
                self.show(cx, Some(l));
                self.radio_list(cx);
            }
            _ => {
                let lines: Vec<(f64, String)> =
                    plain_lines(it).into_iter().map(|l| (0.0, l)).collect();
                self.show(
                    cx,
                    Some(Lrc {
                        lines,
                        ..Default::default()
                    }),
                );
                cx.app.global::<crate::Lyrics>().set_plain(true);
            }
        }
        self.current = Found::new(source, text);
        self.update_info(cx);
    }
}
