use super::*;

#[test]
fn enhanced_lrc_words() {
    let l = Lrc::parse(
        "[00:10.00]<00:10.00>Never <00:10.50>gonna <00:11.00>give<00:11.40>\n[00:12.00]Plain line\n[00:20.00][00:30.00]<00:20.00>Hel<00:20.30>lo",
    );
    assert_eq!(l.lines.len(), 4);
    assert_eq!(l.lines[0].1, "Never gonna give");
    assert_eq!(l.lines[2].1, "Hello");
    assert!(l.has_words());
    assert_eq!(l.words.len(), 4);
    let w: Vec<&str> = l.words[0].iter().map(|w| w.1.as_str()).collect();
    assert_eq!(w, ["Never ", "gonna ", "give", ""], "end marker kept");
    assert!(l.words[1].is_empty(), "untagged line has no words");
    // A repeated line shifts its word times.
    assert_eq!(l.words[3][1].0, 30.3);
    // Word progress.
    assert_eq!(l.word_at(0, 10.25), Some((0, 0.5)));
    assert_eq!(l.word_at(0, 11.2).map(|x| x.0), Some(2));
    assert_eq!(l.word_at(0, 11.2).map(|x| (x.1 * 10.0).round()), Some(5.0));
    assert_eq!(l.word_at(1, 12.5), None);
    // Before the first word: first word at 0 %.
    assert_eq!(l.word_at(2, 19.0), Some((0, 0.0)));
}

#[test]
fn enhanced_offset_and_plain() {
    let l = Lrc::parse("[offset:+1000]\n[00:05.00]<00:05.00>a <00:06.00>b");
    assert_eq!(l.lines[0].0, 4.0);
    assert_eq!(l.words[0][1].0, 5.0);
    let p = Lrc::parse("[00:01.00]x\n[00:02.00]y");
    assert!(!p.has_words() && p.words.is_empty());
    // "<" that is not a time tag stays text.
    assert_eq!(Lrc::parse("[00:01.00]a <3 b").lines[0].1, "a <3 b");
    assert!(!is_synced("just text\nmore"));
    assert!(is_synced("[01:02.03]x"));
}

#[test]
fn offsets_per_track() {
    assert_eq!(clamp_offset(0.1 + 0.2), 0.3);
    assert_eq!(clamp_offset(99.0), 30.0);
    assert_eq!(clamp_offset(-0.04), 0.0);
    assert_eq!(offset_label(0.3), "+0.3 s");
    assert_eq!(offset_label(-1.5), "−1.5 s");
    assert_eq!(offset_label(0.0), "0.0 s");
    let f = std::env::temp_dir().join(format!("vivi-lyr-off-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&f);
    assert!(load_offsets(&f).is_empty());
    save_offset(&f, "/m/a.flac", 0.5);
    save_offset(&f, "/m/b.flac", -1.0);
    let m = load_offsets(&f);
    assert_eq!(m.get("/m/a.flac"), Some(&0.5));
    assert_eq!(m.get("/m/b.flac"), Some(&-1.0));
    save_offset(&f, "/m/a.flac", 0.0);
    assert_eq!(load_offsets(&f).get("/m/a.flac"), None, "0 is forgotten");
    let _ = std::fs::remove_file(&f);
}

#[test]
fn bake_offset() {
    let src = "[ti:x]\n[00:10.00]A\n[00:20.00]B";
    // +0.5 s user offset (later) = LRC offset −500 ms.
    let out = with_offset(src, 0.5);
    assert!(out.starts_with("[offset:-500]\n"));
    assert_eq!(Lrc::parse(&out).lines[0].0, 10.5);
    // Existing tag is replaced and combined.
    let again = with_offset(&out, -0.2);
    assert_eq!(Lrc::parse(&again).lines[0].0, 10.3);
    assert_eq!(again.matches("[offset:").count(), 1);
    assert_eq!(with_offset(src, 0.0), src);
    assert_eq!(with_offset("plain", 1.0), "plain");
    assert_eq!(lrc_time(83.456), "01:23.46");
}

#[test]
fn source_priority() {
    let f = |s, t: &str| Found::new(s, t).unwrap();
    let synced = "[00:01.00]x";
    let plain = "x";
    // Synced beats plain regardless of source.
    let v = [f(Source::LocalFile, plain), f(Source::Online, synced)];
    assert_eq!(choose(&v).unwrap().source, Source::Online);
    // Same kind: source order.
    let v = [
        f(Source::Online, synced),
        f(Source::Cache, synced),
        f(Source::Embedded, synced),
        f(Source::LocalFile, synced),
    ];
    assert_eq!(choose(&v).unwrap().source, Source::LocalFile);
    let v = [f(Source::Cache, synced), f(Source::Embedded, synced)];
    assert_eq!(choose(&v).unwrap().source, Source::Embedded);
    let v = [f(Source::Online, plain), f(Source::Embedded, plain)];
    assert_eq!(choose(&v).unwrap().source, Source::Embedded);
    // A manual pick wins even when plain.
    let v = [f(Source::LocalFile, synced), f(Source::Manual, plain)];
    assert_eq!(choose(&v).unwrap().source, Source::Manual);
    assert!(choose(&[]).is_none());
    assert!(Found::new(Source::Cache, "  \n").is_none());
}

#[test]
fn karaoke_rows() {
    assert_eq!(wrap_rows(&["aaaa ", "bbbb ", "cc"], 10), [0, 0, 1]);
    assert_eq!(wrap_rows(&["a"], 10), [0]);
    assert!(wrap_rows(&[], 10).is_empty());
    // A word longer than a row gets its own row.
    assert_eq!(wrap_rows(&["x", "verylongwordhere", "y"], 8), [0, 1, 2]);
}

#[test]
fn embedded_lyrics_roundtrip() {
    // Plain and synced lyrics written by the tagger are read back as a source.
    let mp3 = std::path::Path::new("/workspace/vivi-media/forecast.mp3");
    if !mp3.exists() {
        return;
    }
    let d = std::env::temp_dir().join(format!("vivi-lyr-emb-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("song.mp3");
    std::fs::copy(mp3, &p).unwrap();
    let lrc = "[00:01.00]First\n[00:02.50]Second";
    let tags = crate::plugins::auto_tagger::Tags {
        lyrics: lrc.into(),
        ..Default::default()
    };
    crate::plugins::auto_tagger::write_tags(&p, &tags, None).unwrap();
    let e = embedded(&p).unwrap();
    assert_eq!(Lrc::parse(&e).texts(), ["First", "Second"]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn parse_lrc() {
    let l = Lrc::parse(
        "[ar:Someone]\n[ti:Song]\n[00:12.00]First\n[00:05.50][01:00.00]Chorus\nno tag\n[00:20.5]  Third  ",
    );
    let t: Vec<f64> = l.lines.iter().map(|x| x.0).collect();
    assert_eq!(t, [5.5, 12.0, 20.5, 60.0]);
    assert_eq!(l.lines[2].1, "Third");
    assert_eq!(l.index_at(0.0), None);
    assert_eq!(l.index_at(12.0), Some(1));
    assert_eq!(l.index_at(30.0), Some(2));
    assert_eq!(l.index_at(1e9), Some(3));
}

#[test]
fn offset_tag() {
    let l = Lrc::parse("[offset:+500]\n[00:10.00]A");
    assert_eq!(l.lines[0].0, 9.5);
}

#[test]
fn local_file() {
    let d = std::env::temp_dir().join(format!("vivi-lrc-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let song = d.join("song.flac");
    assert_eq!(local_lrc(&song), None);
    std::fs::write(d.join("song.lrc"), "[00:01.00]x").unwrap();
    assert_eq!(local_lrc(&song), Some(d.join("song.lrc")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn stream_titles() {
    let p = |s: &str| parse_stream_title(s, "Groove Salad");
    assert_eq!(
        p("Daft Punk - Get Lucky (Radio Edit) [feat. Pharrell]"),
        Some(("Daft Punk".into(), "Get Lucky".into()))
    );
    assert_eq!(
        p("Massive Attack – Teardrop ft. Liz Fraser"),
        Some(("Massive Attack".into(), "Teardrop".into()))
    );
    assert_eq!(p("Groove Salad"), None);
    assert_eq!(p("Groove Salad - Commercial Free"), None);
    assert_eq!(p("Advertisement - Buy now"), None);
    assert_eq!(p("You're listening to Radio X"), None);
    assert_eq!(p("www.radio.fm - best hits"), None);
    assert_eq!(p("Just a title"), None);
    assert_eq!(p(" - "), None);
    assert_eq!(p("Band — Song"), Some(("Band".into(), "Song".into())));
    assert_eq!(p("Band ~ Song"), Some(("Band".into(), "Song".into())));
    assert_eq!(
        p("Band | Song (Live)"),
        Some(("Band".into(), "Song".into()))
    );
    assert_eq!(p("'Band - Song'"), Some(("Band".into(), "Song".into())));
    assert_eq!(p(" - Song"), None);
    assert_eq!(p("Band - (Intro)"), None);
    assert_eq!(p("??? - !!!"), None);
}

#[test]
fn radio_anchor() {
    let mut a = Anchor::default();
    assert!(!a.synced());
    assert_eq!(a.lyrics_time(100.0, 0.0), None, "unsynced: no time");
    // Tuned in an hour ago; the line at 0:42 is being sung now.
    a.set(3600.0, 42.0);
    assert!(a.synced());
    assert_eq!(a.lyrics_time(3600.0, 0.0), Some(42.0));
    assert_eq!(
        a.lyrics_time(3610.0, 0.0),
        Some(52.0),
        "runs with the stream"
    );
    // Fine-tune: +0.5 s shows lines later (lyrics time earlier), like file offsets.
    assert_eq!(a.lyrics_time(3610.0, 0.5), Some(51.5));
    assert_eq!(a.lyrics_time(3610.0, -0.5), Some(52.5));
    // The clicked line is the highlighted one right away.
    let l = Lrc::parse("[00:30.00]A\n[00:42.00]B\n[00:50.00]C");
    let t = a.lyrics_time(3600.0, 0.0).unwrap();
    assert_eq!(l.index_at(t + 0.15), Some(1));
    // Clicking another line re-anchors.
    a.set(3620.0, 50.0);
    assert_eq!(a.lyrics_time(3620.0, 0.0), Some(50.0));
    a.clear();
    assert!(!a.synced());
    // Setting values: everything but plain / off syncs by click (old values too).
    assert!(click_sync("Click a line to sync"));
    assert!(click_sync("Approximate sync"));
    assert!(!click_sync("Plain lyrics") && !click_sync("Off"));
}

#[test]
fn radio_anchor_fixture() {
    // Dummy LRC (tests/fixtures): tuned in mid-song, the 4th line is clicked at 1234.5 s.
    let l = Lrc::parse(include_str!("../../../tests/fixtures/radio-dummy.lrc"));
    let mut a = Anchor::default();
    a.set(1234.5, l.lines[3].0);
    let at = |now: f64, off: f64| a.lyrics_time(now, off).and_then(|t| l.index_at(t + 0.15));
    assert_eq!(at(1234.5, 0.0), Some(3));
    assert_eq!(at(1238.5, 0.0), Some(4), "next line 4 s later");
    // The stream runs 0.5 s ahead of the lyrics: −0.5 s brings the next line in sooner.
    assert_eq!(at(1238.0, 0.0), Some(3));
    assert_eq!(at(1238.0, -0.5), Some(4));
    assert_eq!(at(1238.5, 0.5), Some(3));
}

#[test]
fn plain_from_synced() {
    let it = LrclibItem {
        synced: Some("[00:01.00]A\n[00:02.00]B".into()),
        ..Default::default()
    };
    assert_eq!(plain_lines(&it), ["A", "B"]);
    assert!(it.has_lyrics());
    assert!(!LrclibItem::default().has_lyrics());
}

#[test]
fn lyrics_file_size_cap() {
    let d = std::env::temp_dir().join(format!("vivi-lyr-cap-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let ok = d.join("a.lrc");
    std::fs::write(&ok, "[00:01.00]x").unwrap();
    assert_eq!(read_lyrics_file(&ok).as_deref(), Some("[00:01.00]x"));
    let big = d.join("b.lrc");
    std::fs::write(&big, vec![b'a'; MAX_LYRICS_FILE as usize + 1]).unwrap();
    assert_eq!(read_lyrics_file(&big), None);
    assert_eq!(read_lyrics_file(&d.join("none.lrc")), None);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn now_playing_songs() {
    let np = crate::nowplaying::Track {
        title: "Song ".into(),
        artist: "Band".into(),
        album: "LP".into(),
        duration: 201.0,
        kind: "music".into(),
        source: "plugin:youtube-music".into(),
        path: "https://example.org/v".into(),
        ..Default::default()
    };
    let t = np_track(&np, Some("https://example.org/v")).unwrap();
    assert_eq!(
        (t.artist.as_str(), t.title.as_str(), t.duration),
        ("Band", "Song", 201.0)
    );
    assert!(
        np_track(&np, Some("https://example.org/other")).is_none(),
        "stale path"
    );
    assert!(np_track(&np, None).is_none());
    let live = crate::nowplaying::Track {
        duration: 0.0,
        ..np.clone()
    };
    assert!(
        np_track(&live, Some("https://example.org/v")).is_none(),
        "live radio"
    );
    let tags = crate::nowplaying::Track {
        source: "tags".into(),
        ..np.clone()
    };
    assert!(
        np_track(&tags, Some("https://example.org/v")).is_none(),
        "tags: track_info"
    );
    let video = crate::nowplaying::Track {
        kind: "video".into(),
        ..np.clone()
    };
    assert!(np_track(&video, Some("https://example.org/v")).is_none());
    let no_artist = crate::nowplaying::Track {
        artist: " ".into(),
        ..np
    };
    assert!(np_track(&no_artist, Some("https://example.org/v")).is_none());
}

#[test]
fn seek_to_line() {
    let l = Lrc::parse("[00:10.00]A\n[00:20.00]B");
    // No offset: the line's own time; +0.5 s (lyrics later): half a second later.
    assert_eq!(line_seek(l.lines[1].0, 0.0), 20.0);
    assert_eq!(line_seek(l.lines[1].0, 0.5), 20.5);
    assert_eq!(line_seek(0.2, -1.0), 0.0, "never before the start");
    // After the seek the clicked line is the one highlighted.
    let now = line_seek(l.lines[1].0, 0.5);
    assert_eq!(l.index_at(now - 0.5 + 0.15), Some(1));
}

/// Network test; run with `cargo test -- --ignored`.
#[test]
#[ignore]
fn lrclib_radio_live() {
    let it = fetch_radio("Daft Punk", "Get Lucky").unwrap().unwrap();
    assert!(!plain_lines(&it).is_empty());
    // Swapped "Title - Artist" still found through search.
    assert!(fetch_radio("Get Lucky", "Daft Punk").unwrap().is_some());
}
