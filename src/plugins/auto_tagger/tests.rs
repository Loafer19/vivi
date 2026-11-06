use super::*;

const MEDIA: &str = "/workspace/vivi-media/forecast.mp3";

fn tmp_copy(name: &str) -> Option<PathBuf> {
    let src = Path::new(MEDIA);
    if !src.exists() {
        return None;
    }
    let d = std::env::temp_dir().join(format!("vivi-tagger-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("forecast.mp3");
    std::fs::copy(src, &p).unwrap();
    Some(p)
}

#[test]
fn diff_skips_empty_and_equal() {
    let old = Tags {
        title: "a".into(),
        artist: "x".into(),
        ..Default::default()
    };
    let new = Tags {
        title: "a".into(),
        artist: "y".into(),
        album: "".into(),
        date: "2014".into(),
        ..Default::default()
    };
    let d = diff(&old, &new);
    assert_eq!(
        d,
        vec![
            ("Artist", "x".into(), "y".into()),
            ("Date", "".into(), "2014".into())
        ]
    );
}

fn cand(title: &str, artist: &str, album: &str, rel: &str, score: u32) -> Candidate {
    Candidate {
        tags: Tags {
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            mb_recording: "rec".into(),
            mb_release: rel.into(),
            ..Default::default()
        },
        release_group: String::new(),
        score,
        source: "MusicBrainz search",
        sources: Vec::new(),
        extra_covers: Vec::new(),
        alts: Vec::new(),
        agree: Vec::new(),
    }
}

#[test]
fn ranking_merges_duplicates_and_prefers_confirmed_matches() {
    let current = Tags {
        title: "Local Forecast".into(),
        artist: "Kevin MacLeod".into(),
        ..Default::default()
    };
    let mut confirmed = cand("Other Song", "Band", "B", "r3", 88);
    confirmed.agree = vec!["Deezer", "iTunes", "Discogs", "Last.fm"];
    let v = vec![
        cand("Local Forecast", "Kevin MacLeod", "A", "r1", 80),
        cand("Local Forecast", "Kevin MacLeod", "A", "r1", 90), // duplicate, better
        cand("Something", "Else", "C", "r2", 92),
        confirmed,
    ];
    let r = rank_candidates(v, &current);
    assert_eq!(r.len(), 3, "same recording + release collapses");
    // 88 + 12 (three agreeing sources max) = 100 beats 90 + 5 (tags match) and 92.
    assert_eq!(r[0].tags.mb_release, "r3");
    assert_eq!(r[0].confidence(), 100);
    assert_eq!((r[1].tags.mb_release.as_str(), r[1].score), ("r1", 95));
    assert_eq!(r[2].tags.mb_release, "r2");
}

#[test]
fn review_cycles_alternative_values() {
    let old = Tags {
        album: "Old".into(),
        ..Default::default()
    };
    let mut c = cand("T", "A", "MB Album", "r1", 90);
    c.alts.push(Alt {
        field: "Album",
        value: "Old".into(),
        source: "iTunes",
    });
    c.alts.push(Alt {
        field: "Album",
        value: "Deezer Album".into(),
        source: "Deezer",
    });
    let mut r = Review::new(PathBuf::from("/nonexistent.mp3"), old, c, Vec::new(), true);
    assert_eq!(r.alts_of(2), 2);
    assert!(r.fields[2]);
    assert!(r.next_alt(2));
    // "Old" equals the current value: nothing to write, the box unticks.
    assert_eq!(r.cand.tags.album, "Old");
    assert!(!r.fields[2]);
    assert_eq!(
        super::super::tagsources::source_of(&r.cand, "Album"),
        "iTunes"
    );
    assert!(r.next_alt(2));
    assert_eq!(r.cand.tags.album, "Deezer Album");
    assert!(r.fields[2]);
    assert!(r.next_alt(2));
    assert_eq!(r.cand.tags.album, "MB Album");
    assert_eq!(
        super::super::tagsources::source_of(&r.cand, "Album"),
        "MusicBrainz"
    );
    assert!(!r.next_alt(0), "no alternatives for the title");
}

#[test]
fn rename_sanitizes_and_never_overwrites() {
    assert_eq!(
        sanitize_file_name("AC/DC: Back <in> Black?"),
        "AC_DC_ Back _in_ Black_"
    );
    assert_eq!(
        sanitize_file_name("  ..Artist -  - Title.. "),
        "Artist - Title"
    );
    assert_eq!(sanitize_file_name("con"), "con_");
    assert_eq!(sanitize_file_name("COM3.x"), "COM3_.x");
    assert_eq!(sanitize_file_name("Comet"), "Comet");
    assert_eq!(sanitize_file_name("a\tb\u{7}c"), "a b_c");
    let long = "ж".repeat(200);
    let s = sanitize_file_name(&long);
    assert!(s.len() <= MAX_STEM && s.chars().all(|c| c == 'ж'));
    let t = Tags {
        title: "Song/Live".into(),
        artist: "Band".into(),
        track: "3/12".into(),
        ..Default::default()
    };
    assert_eq!(
        rename_to("{track}. {artist} - {title}", &t, Path::new("/m/x.flac")).as_deref(),
        Some("03. Band - Song_Live.flac")
    );
    assert_eq!(
        rename_to("{title}", &t, Path::new("/m/Song_Live.mp3")),
        None
    );
    let d = std::env::temp_dir().join(format!("vivi-rename-safe-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let a = d.join("a.mp3");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(d.join("Taken.mp3"), b"b").unwrap();
    assert!(
        rename_file(&a, "taken.mp3").is_err(),
        "case-insensitive clash"
    );
    assert!(rename_file(&a, "../escape.mp3").is_err());
    assert!(rename_file(&a, ".hidden.mp3").is_err());
    let to = rename_file(&a, "New.mp3").unwrap();
    assert_eq!(std::fs::read(&to).unwrap(), b"a");
    assert_eq!(std::fs::read(d.join("Taken.mp3")).unwrap(), b"b");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn summary_results_skip_fail_and_count() {
    let c = cand("T", "A", "Al", "r1", 90);
    let mut r = Review::new(
        PathBuf::from("/nonexistent/x.mp3"),
        Tags::default(),
        c,
        Vec::new(),
        true,
    );
    r.on = false;
    let tmp = std::env::temp_dir().join(format!("vivi-sum-{}", std::process::id()));
    assert_eq!(apply_one(&r, &tmp, &tmp).status, "skipped");
    r.on = true;
    let f = apply_one(&r, &tmp, &tmp);
    assert_eq!(f.status, "failed");
    assert!(f.detail.starts_with("Tags not written"), "{}", f.detail);
    let ok = FileResult {
        status: "renamed",
        written: true,
        ..f.clone()
    };
    assert_eq!(result_counts(&[f, ok]), (1, 1, 0, 1));
}

#[test]
fn terms_from_name() {
    let t = Tags::default();
    assert_eq!(
        search_terms(&t, Path::new("/m/03 - Kevin MacLeod - Local Forecast.mp3")),
        ("Local Forecast".into(), "Kevin MacLeod".into())
    );
    assert_eq!(lucene("AC/DC (live)"), "AC\\/DC \\(live\\)");
}

#[test]
fn parse_recording() {
    let rec = serde_json::json!({
        "id": "r1", "title": "Song", "first-release-date": "2010",
        "artist-credit": [{"name": "A", "joinphrase": " feat. "}, {"name": "B"}],
        "genres": [{"name": "jazz", "count": 3}, {"name": "pop", "count": 1}],
        "releases": [
            {"id": "bootleg", "title": "Live", "status": "Bootleg", "date": "2001"},
            {"id": "rel1", "title": "Album", "status": "Official", "date": "2011-02-03",
             "release-group": {"id": "rg1", "primary-type": "Album"},
             "media": [{"tracks": [{"number": "7"}]}]}
        ]
    });
    let c = candidates_from_recording(&rec, 95, "AcoustID", 4);
    assert_eq!(c.len(), 2);
    let t = &c[0].tags;
    assert_eq!(t.artist, "A feat. B");
    assert_eq!((t.album.as_str(), t.track.as_str()), ("Album", "7"));
    assert_eq!((t.date.as_str(), t.genre.as_str()), ("2011-02-03", "Jazz"));
    assert_eq!(c[0].release_group, "rg1");
    assert_eq!(c[1].tags.mb_release, "bootleg");
}

#[test]
fn review_apply_and_undo_restores_tags_and_cover() {
    let Some(p) = tmp_copy("review") else { return };
    let png: Vec<u8> = {
        // 1×1 PNG
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01".to_vec();
        v.extend_from_slice(&[8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89]);
        v
    };
    assert_eq!(image_size(&png), Some((1, 1)));
    let (old, _) = read_tags(&p).unwrap();
    let mut new = old.clone();
    new.title = "Reviewed Title".into();
    new.album = "Reviewed Album".into();
    let cand = Candidate {
        tags: new,
        release_group: String::new(),
        score: 95,
        source: "AcoustID",
        sources: Vec::new(),
        extra_covers: Vec::new(),
        alts: Vec::new(),
        agree: Vec::new(),
    };
    let cover = CoverOpt {
        label: "Front".into(),
        data: png.clone(),
    };
    let mut r = Review::new(p.clone(), old.clone(), cand, vec![cover], true);
    // Only the title: the album change is unticked.
    r.fields[2] = false;
    r.cover = Some(0);
    let session = std::env::temp_dir().join(format!("vivi-review-{}", std::process::id()));
    let (n, c) = apply_review(&r, &session.join("orig"), &session).unwrap();
    assert_eq!((n, c), (1, true));
    let (now, _) = read_tags(&p).unwrap();
    assert_eq!(now.title, "Reviewed Title");
    assert_eq!(now.album, old.album);
    assert_eq!(embedded_cover(&p).as_deref(), Some(png.as_slice()));
    restore(&session, &p).unwrap();
    let (back, _) = read_tags(&p).unwrap();
    assert_eq!(back.title, old.title);
    assert_ne!(embedded_cover(&p).as_deref(), Some(png.as_slice()));
    let _ = std::fs::remove_dir_all(&session);
    let _ = std::fs::remove_file(&p);
}

#[test]
fn embeds_synced_lyrics() {
    let Some(p) = tmp_copy("lyrics") else { return };
    let lrc = "[00:01.50]First line\n[00:04.00]Second line\n";
    let t = Tags {
        lyrics: lrc.into(),
        ..Default::default()
    };
    write_tags(&p, &t, None).unwrap();
    let (back, _) = read_tags(&p).unwrap();
    assert_eq!(back.lyrics.trim(), lrc.trim(), "USLT holds the LRC text");
    assert_eq!(
        read_sylt(&p),
        vec![
            (1500, "First line".to_string()),
            (4000, "Second line".to_string())
        ]
    );
    // Writing again replaces the SYLT frame instead of adding one.
    write_tags(&p, &t, None).unwrap();
    assert_eq!(read_sylt(&p).len(), 2);
    assert_eq!(lyrics_summary(lrc), "Synced, 2 lines: First line");
    // FLAC / Opus / M4A: one text field (LYRICS / ©lyr) with the LRC.
    for ext in ["flac", "opus", "m4a"] {
        let out = p.with_extension(ext);
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&p)
            .args(["-t", "3", "-map", "0:a"])
            .arg(&out)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            continue;
        }
        write_tags(&out, &t, None).unwrap();
        assert_eq!(
            read_tags(&out).unwrap().0.lyrics.trim(),
            lrc.trim(),
            "{ext}"
        );
        let _ = std::fs::remove_file(&out);
    }
    let _ = std::fs::remove_file(&p);
}

#[test]
fn rename_pattern() {
    let t = Tags {
        title: "Song: Part 1/2".into(),
        artist: "AC/DC".into(),
        album: "Album".into(),
        date: "1980-07-25".into(),
        track: "3/10".into(),
        ..Default::default()
    };
    let p = Path::new("/m/01 track.mp3");
    assert_eq!(
        rename_to("{artist} - {title}", &t, p).as_deref(),
        Some("AC_DC - Song_ Part 1_2.mp3")
    );
    assert_eq!(
        rename_to("{track} {artist} - {album} ({year}) - {title}", &t, p).as_deref(),
        Some("03 AC_DC - Album (1980) - Song_ Part 1_2.mp3")
    );
    // Empty fields leave no dangling separators.
    let bare = Tags {
        title: "T".into(),
        ..Default::default()
    };
    assert_eq!(
        rename_to("{artist} - {title}", &bare, p).as_deref(),
        Some("T.mp3")
    );
    assert_eq!(rename_to("{artist} - {title}", &Tags::default(), p), None);
    assert_eq!(
        rename_to("{title}", &bare, Path::new("/m/T.mp3")),
        None,
        "same name"
    );
    let d = std::env::temp_dir().join(format!("vivi-rename-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("a.mp3"), b"x").unwrap();
    std::fs::write(d.join("b.mp3"), b"y").unwrap();
    assert!(
        rename_file(&d.join("a.mp3"), "b.mp3").is_err(),
        "never overwrites"
    );
    let to = rename_file(&d.join("a.mp3"), "c.mp3").unwrap();
    assert_eq!(std::fs::read(to).unwrap(), b"x");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn parse_acoustid_results() {
    let v = serde_json::json!({"status": "ok", "results": [
        {"score": 0.5, "recordings": [{"id": "b"}]},
        {"score": 0.97, "recordings": [{"id": "a"}, {"id": "b"}]}
    ]});
    assert_eq!(
        parse_acoustid(&v),
        vec![("a".to_string(), 97), ("b".to_string(), 97)]
    );
}

#[test]
fn parse_acoustid_matches_with_metadata() {
    let v = serde_json::json!({"status": "ok", "results": [
        {"score": 0.42, "recordings": [{"id": "low", "title": "Other"}]},
        {"score": 0.931, "recordings": [{
            "id": "rec-1", "title": "Song", "duration": 201.5,
            "artists": [{"name": "A", "joinphrase": " & "}, {"name": "B"}],
            "releasegroups": [{"title": "Single", "type": "Single"}, {"title": "LP", "type": "Album"}]
        }, {"id": "rec-2"}]}
    ]});
    let m = parse_acoustid_matches(&v);
    assert_eq!(m.len(), 3);
    assert_eq!(
        m[0],
        Match {
            id: "rec-1".into(),
            score: 93,
            title: "Song".into(),
            artist: "A & B".into(),
            album: "LP".into(),
            duration: 201.5,
        }
    );
    assert_eq!(
        (m[1].id.as_str(), m[1].score, m[1].title.as_str()),
        ("rec-2", 93, "")
    );
    assert_eq!((m[2].id.as_str(), m[2].score), ("low", 42));
    // Threshold: 0.7 score (70) like the AcoustID plugin.
    assert_eq!(best_confident(&m, AUTO_MIN_SCORE).unwrap().id, "rec-1");
    assert_eq!(best_confident(&m[2..], AUTO_MIN_SCORE), None);
    assert_eq!(best_confident(&[], AUTO_MIN_SCORE), None);
    let edge = [Match {
        id: "e".into(),
        score: 70,
        ..Default::default()
    }];
    assert!(best_confident(&edge, AUTO_MIN_SCORE).is_some());
    // Equal scores: the one with a title wins, wherever it is listed.
    let tie = [m[1].clone(), m[0].clone()];
    assert_eq!(best_confident(&tie, AUTO_MIN_SCORE).unwrap().id, "rec-1");
    assert!(parse_acoustid_matches(&serde_json::json!({"status": "error"})).is_empty());
}

#[test]
fn auto_identify_targets_and_now_playing() {
    use crate::nowplaying::{merge, Sources, Track};
    let local = |kind: &str, source: &str| Track {
        path: "/music/x.mp3".into(),
        kind: kind.into(),
        source: source.into(),
        ..Default::default()
    };
    assert!(untagged_song(&local("music", "file")));
    assert!(!untagged_song(&local("music", "tags")));
    assert!(!untagged_song(&local("music", "library")));
    assert!(!untagged_song(&local("music", "plugin:youtube-music")));
    assert!(!untagged_song(&local("video", "file")));
    assert!(!untagged_song(&local("podcast", "file")));
    assert!(!untagged_song(&Track {
        path: "https://example.com/a.mp3".into(),
        ..local("music", "file")
    }));
    assert!(!untagged_song(&Track::default()));
    let m = Match {
        id: "8f3471b5-7e6a-48da-86a9-c1c07a0f47ae".into(),
        score: 91,
        title: "Song".into(),
        artist: "Band".into(),
        album: "LP".into(),
        duration: 0.0,
    };
    assert_eq!(display_title(&m), "Band – Song");
    assert_eq!(
        display_title(&Match {
            artist: String::new(),
            ..m.clone()
        }),
        "Song"
    );
    // The match becomes the shared now-playing track (tags win nowhere: there are none).
    let t = merge(&Sources {
        path: "/music/x.mp3".into(),
        media_title: "x.mp3".into(),
        duration: 200.0,
        plugin: Some((ID.into(), now_playing_meta("/music/x.mp3", &m))),
        ..Default::default()
    });
    assert_eq!(
        (t.title.as_str(), t.artist.as_str(), t.album.as_str()),
        ("Song", "Band", "LP")
    );
    assert_eq!(t.source, "plugin:auto-tagger");
    assert_eq!(t.kind, "music");
    assert_eq!(t.mbid, m.id);
    assert_eq!(t.duration, 200.0);
}

#[test]
fn redact_masks_the_key() {
    assert_eq!(
        redact("AcoustID: bad client=abc123 abc123", "abc123"),
        "AcoustID: bad client=*** ***"
    );
    assert_eq!(redact("no key here", ""), "no key here");
    assert_eq!(redact("x k y", " k "), "x *** y");
}

#[test]
fn write_backup_restore_roundtrip() {
    let Some(p) = tmp_copy("rt") else { return };
    let dir = p.parent().unwrap().join("backups");
    let (before, _) = read_tags(&p).unwrap();
    assert_eq!(before.artist, "Kevin MacLeod");
    backup(&dir, &p).unwrap();
    let new = Tags {
        title: "Local Forecast - Elevator".into(),
        album: "Rollin' at 5".into(),
        track: "12".into(),
        mb_recording: "b1d46379-5d1c-4a36-98d3-d7986dd33dc4".into(),
        ..Default::default()
    };
    let jpg = std::fs::read("/workspace/vivi-media/cover.jpg").ok();
    write_tags(&p, &new, jpg.as_deref()).unwrap();
    let (after, _) = read_tags(&p).unwrap();
    assert_eq!(after.title, new.title);
    assert_eq!(after.album, new.album);
    assert_eq!(after.track, "12");
    assert_eq!(after.mb_recording, new.mb_recording);
    assert_eq!(after.artist, "Kevin MacLeod");
    if jpg.is_some() {
        let tf = lofty::read_from_path(&p).unwrap();
        let tag = tf.primary_tag().unwrap();
        assert!(tag
            .pictures()
            .iter()
            .any(|x| x.pic_type() == PictureType::CoverFront));
    }
    // A second backup keeps the original.
    backup(&dir, &p).unwrap();
    restore(&dir, &p).unwrap();
    let (back, _) = read_tags(&p).unwrap();
    assert_eq!(back.title, before.title);
    assert_eq!(back.album, before.album);
    assert_eq!(back.mb_recording, "");
    assert!(restore(&dir, &p).is_err());
    let _ = std::fs::remove_dir_all(p.parent().unwrap());
}

#[test]
fn builtin_fingerprint_matches_fpcalc() {
    let Some(p) = tmp_copy("fp") else { return };
    let (d, fp) = fingerprint_builtin(&p).unwrap();
    assert!((185..=192).contains(&d), "{d}");
    assert!(fp.len() > 1000 && fp.starts_with("AQAD"));
    // Compare raw sub-fingerprints with fpcalc's (if installed): decoders and
    // resamplers differ slightly, so allow some bit errors (AcoustID does too).
    let out = std::process::Command::new("fpcalc")
        .args(["-raw", "-json"])
        .arg(&p)
        .output();
    if let Some(out) = out.ok().filter(|o| o.status.success()) {
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let theirs: Vec<u32> = v["fingerprint"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap() as u32)
            .collect();
        let (_, ours) =
            fingerprint_raw(&p, &rusty_chromaprint::Configuration::preset_test2()).unwrap();
        let best = (0..8)
            .flat_map(|o| [(o, 0), (0, o)])
            .map(|(a, b)| {
                let n = ours
                    .len()
                    .saturating_sub(a)
                    .min(theirs.len().saturating_sub(b));
                let errs: u32 = (0..n)
                    .map(|i| (ours[i + a] ^ theirs[i + b]).count_ones())
                    .sum();
                errs as f64 / (n as f64 * 32.0)
            })
            .fold(1.0, f64::min);
        eprintln!("bit error rate vs fpcalc: {best:.3}");
        assert!(best < 0.15, "{best}");
    }
    let _ = std::fs::remove_dir_all(p.parent().unwrap());
}

/// Live MusicBrainz + Cover Art Archive (no key needed). `cargo test -- --ignored`.
#[test]
#[ignore]
fn live_musicbrainz_and_cover() {
    let Some(p) = tmp_copy("live") else { return };
    let (old, c, _) = identify(&p, "").unwrap();
    assert_eq!(old.artist, "Kevin MacLeod");
    assert!(!c.is_empty());
    eprintln!("{:#?}", &c[..c.len().min(3)]);
    let best = c.iter().find(|c| !c.tags.mb_release.is_empty()).unwrap();
    let rec = lookup_recording(&best.tags.mb_recording).unwrap();
    assert!(!candidates_from_recording(&rec, 100, "x", 3).is_empty());
    let art = cover_art("a252b0a1-4465-45cf-9292-37ab4321b8af", "").unwrap();
    assert!(art.starts_with(&[0xff, 0xd8]));
    let _ = std::fs::remove_dir_all(p.parent().unwrap());
}
