//! Background work: automatic identification, applying reviews, folder batches.

use super::*;

/// Whether the shared now-playing track is a local song Vivi knows nothing about: music,
/// title / artist from the file name only (no tags, library row or package metadata).
pub fn untagged_song(t: &crate::nowplaying::Track) -> bool {
    !t.path.is_empty() && !t.path.contains("://") && t.kind == "music" && t.source == "file"
}

/// The background half of the automatic identification: the cached or fresh AcoustID
/// answer, its best match when it scores at least [`AUTO_MIN_SCORE`], completed from
/// MusicBrainz (rate limited) when AcoustID has no title for it.
pub(crate) fn auto_identify(
    path: &Path,
    key: &str,
    cancel: &AtomicBool,
) -> Result<Option<Match>, String> {
    let (ms, _) = acoustid_matches(path, key).map_err(|e| redact(&e, key))?;
    let Some(mut m) = best_confident(&ms, AUTO_MIN_SCORE) else {
        return Ok(None);
    };
    if m.title.is_empty() || m.artist.is_empty() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let rec = lookup_recording(&m.id)?;
        let Some(c) = candidates_from_recording(&rec, m.score, "AcoustID", 1)
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        m.title = c.tags.title;
        m.artist = c.tags.artist;
        if m.album.is_empty() {
            m.album = c.tags.album;
        }
        if m.duration <= 0.0 {
            m.duration = rec["length"].as_f64().unwrap_or(0.0) / 1000.0;
        }
    }
    Ok((!m.title.is_empty()).then_some(m))
}

/// "Artist – Title" (or the title alone).
pub fn display_title(m: &Match) -> String {
    if m.artist.is_empty() {
        m.title.clone()
    } else {
        format!("{} – {}", m.artist, m.title)
    }
}

/// The now-playing metadata of an automatic match (as a package's `vivi-media-meta`).
pub fn now_playing_meta(path: &str, m: &Match) -> crate::plugins::host::Meta {
    crate::plugins::host::Meta {
        url: path.to_string(),
        title: m.title.clone(),
        artist: m.artist.clone(),
        album: m.album.clone(),
        mbid: m.id.clone(),
        duration: m.duration.max(0.0),
        kind: "music".into(),
        ..Default::default()
    }
}

/// A toast with a button (`Notices.toast-run(id)`), shown for `secs` seconds.
pub(crate) fn toast_with_action(
    app: &crate::AppWindow,
    msg: &str,
    label: &str,
    id: &str,
    secs: f64,
) {
    let n = app.global::<crate::Notices>();
    n.set_toast_action(label.into());
    n.set_toast_action_id(id.into());
    n.set_toast_action_text(msg.into());
    crate::plugins::toast_for(app, msg, secs);
}

/// Run the calling thread (and the `fpcalc` it starts) at a lower CPU priority, so the
/// background fingerprint never competes with playback. Linux only (the nice value is
/// per thread there); elsewhere a no-op.
pub(crate) fn lower_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain syscalls on the calling thread; failure only leaves the priority as is.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
}

/// Apply one review and describe the outcome for the summary.
pub fn apply_one(r: &Review, originals: &Path, session: &Path) -> FileResult {
    let mut out = FileResult {
        orig: r.path.clone(),
        path: r.path.clone(),
        status: "skipped",
        detail: String::new(),
        changes: Vec::new(),
        written: false,
    };
    if !r.on {
        out.detail = "Unticked in the review".into();
        return out;
    }
    if r.changes() == 0 {
        out.detail = "Nothing to change: tags already match".into();
        return out;
    }
    let (n, c) = match apply_review(r, originals, session) {
        Ok(v) => v,
        Err(e) => {
            out.status = "failed";
            out.detail = format!("Tags not written: {e}");
            return out;
        }
    };
    out.written = n > 0 || c;
    for ((f, o, nv), on) in review_fields(&r.old, &r.cand.tags).iter().zip(&r.fields) {
        if *on {
            let (o, nv) = if *f == "Lyrics" {
                (lyrics_summary(o), lyrics_summary(nv))
            } else {
                (o.clone(), nv.clone())
            };
            let o = if o.is_empty() { "—".to_string() } else { o };
            out.changes.push(format!("{f}:  {o}  →  {nv}"));
        }
    }
    if c {
        out.changes.push(if r.old_cover.is_some() {
            "Cover:  replaced".into()
        } else {
            "Cover:  embedded".into()
        });
    }
    out.status = if out.written { "tagged" } else { "skipped" };
    out.detail = format!(
        "{n} field{}{}",
        if n == 1 { "" } else { "s" },
        if c { " + cover" } else { "" }
    );
    if let (true, Some(name)) = (r.rename_on, &r.rename) {
        match rename_file(&r.path, name) {
            Ok(to) => {
                out.changes.push(format!(
                    "File name:  {}  →  {name}",
                    r.path.file_name().unwrap_or_default().to_string_lossy()
                ));
                out.path = to;
                out.status = "renamed";
                out.written = true;
            }
            Err(e) => out.detail += &format!(" · not renamed: {e}"),
        }
    }
    out
}

/// Rows for AcoustID recording ids (score ≥ 50, best three) with rename proposals.
pub(crate) fn external_rows(
    path: &Path,
    ids: &[(String, u32)],
    pattern: &str,
) -> Result<Vec<Row>, String> {
    let (old, _) = read_tags(path)?;
    let mut rows = Vec::new();
    for (id, score) in ids.iter().filter(|(_, s)| *s >= 50).take(3) {
        let rec = lookup_recording(id)?;
        for cand in candidates_from_recording(&rec, *score, "AcoustID", 4) {
            rows.push(Row {
                path: path.to_path_buf(),
                old: old.clone(),
                rename: rename_to(pattern, &cand.tags, path),
                cand,
                note: "Fingerprint: AcoustID plugin (fpcalc)".into(),
            });
        }
    }
    Ok(rows)
}

/// File name proposal for a fingerprint (AcoustID) match; None for tag/name searches or
/// when renaming is off (empty pattern).
pub(crate) fn proposal(cand: &Candidate, pattern: &str, path: &Path) -> Option<String> {
    cand.source
        .starts_with("AcoustID")
        .then(|| rename_to(pattern, &cand.tags, path))
        .flatten()
}

pub(super) fn batch(dir: &Path, key: &str, pattern: &str, cancel: &AtomicBool, tx: &Sender<Msg>) {
    let files = audio_files(dir);
    let total = files.len();
    let (mut found, mut skipped) = (0, 0);
    for (i, p) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let _ = tx.send(Msg::Status(format!(
            "{}% · identifying {}/{total}: {} · {found} matched, {skipped} skipped",
            i * 100 / total.max(1),
            i + 1,
            p.file_name().unwrap_or_default().to_string_lossy()
        )));
        match identify(p, key) {
            Ok((old, c, notes)) => match c.into_iter().next() {
                Some(cand) if cand.score >= CONFIDENT && !diff(&old, &cand.tags).is_empty() => {
                    found += 1;
                    let _ = tx.send(Msg::Row(Box::new(Row {
                        path: p.clone(),
                        old,
                        rename: proposal(&cand, pattern, p),
                        cand,
                        note: notes.join(" · "),
                    })));
                }
                _ => skipped += 1,
            },
            Err(_) => skipped += 1,
        }
    }
    let pl = |n: usize| if n == 1 { "" } else { "es" };
    let _ = tx.send(Msg::Done(format!(
        "{total} audio file{}: {found} confident match{} to review, {skipped} unchanged or not found",
        if total == 1 { "" } else { "s" },
        pl(found)
    )));
}
