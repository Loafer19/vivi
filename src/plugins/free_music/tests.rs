use super::*;

fn fixture(name: &str) -> serde_json::Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/free_music")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

fn t(ti: &str, ar: &str) -> Track {
    Track {
        id: format!("{ar}/{ti}"),
        title: ti.into(),
        artist: ar.into(),
        ..Default::default()
    }
}

#[test]
fn tracks_are_described_as_songs() {
    let mut tr = t("Air", "Bach");
    tr.stream = "https://archive.org/download/x/01.mp3".into();
    tr.album = "Suites".into();
    tr.duration = 215;
    tr.source = FMA;
    let m = track_meta(&tr);
    // The URL plays as a song (music card, shared now playing), not as a radio station.
    assert_eq!(m.url, tr.stream);
    assert_eq!(m.kind, "music");
    assert_eq!(
        (m.title.as_str(), m.artist.as_str(), m.album.as_str()),
        ("Air", "Bach", "Suites")
    );
    assert_eq!(m.service, FMA);
    assert_eq!(m.duration, 215.0);
    // The now-playing card shows the source and the licence.
    tr.license_url = "https://creativecommons.org/licenses/by-sa/4.0/".into();
    assert_eq!(track_meta(&tr).info, "Free Music Archive · CC BY-SA 4.0");
    assert_eq!(page_label(&tr), "Open on Internet Archive");
    tr.source = JAMENDO;
    assert_eq!(page_label(&tr), "Open on Jamendo");
}

#[test]
fn jamendo_tracks_are_not_downloaded_in_app() {
    // #85: Jamendo's API terms forbid offline access in apps.
    let mut tr = t("Song", "Artist");
    tr.download = "https://example.org/song.mp3".into();
    tr.source = IA;
    assert!(downloadable(&tr));
    tr.source = FMA;
    assert!(downloadable(&tr));
    tr.source = JAMENDO;
    assert!(!downloadable(&tr));
    tr.source = IA;
    tr.download.clear();
    assert!(!downloadable(&tr));
}

#[test]
fn client_id_is_not_kept_in_urls() {
    assert_eq!(
        strip_app("https://prod-1.storage.jamendo.com/?trackid=7&format=mp31&from=app-abc123"),
        "https://prod-1.storage.jamendo.com/?trackid=7&format=mp31"
    );
    assert_eq!(
        strip_app("https://x/?from=app-abc&client_id=abc"),
        "https://x/"
    );
    assert_eq!(strip_app("https://x/a.mp3"), "https://x/a.mp3");
    assert_eq!(
        strip_app("https://x/?a=1&fromage=2"),
        "https://x/?a=1&fromage=2"
    );
}

#[test]
fn licences() {
    assert_eq!(
        license_name("http://creativecommons.org/licenses/by-nc-sa/4.0/"),
        "CC BY-NC-SA 4.0"
    );
    assert_eq!(
        license_name("https://creativecommons.org/licenses/by/3.0/us/"),
        "CC BY 3.0"
    );
    assert_eq!(
        license_name("http://creativecommons.org/publicdomain/zero/1.0/"),
        "CC0 1.0"
    );
    assert_eq!(
        license_name("https://creativecommons.org/publicdomain/mark/1.0/"),
        "Public Domain Mark"
    );
    let by = "http://creativecommons.org/licenses/by/3.0/";
    let bysa = "http://creativecommons.org/licenses/by-sa/3.0/";
    let nc = "http://creativecommons.org/licenses/by-nc-nd/3.0/";
    let pd = "http://creativecommons.org/publicdomain/zero/1.0/";
    assert!(commercial_ok(by) && commercial_ok(pd) && !commercial_ok(nc));
    assert!(!commercial_ok("") && !commercial_ok("http://example.com/x"));
    assert!(lic_match(by, 3) && !lic_match(bysa, 3) && lic_match(bysa, 4));
    assert!(lic_match(pd, 5) && !lic_match(by, 5));
    assert!(lic_match(bysa, 2) && !lic_match(nc, 2) && !lic_match(nc, 1));
    assert!(lic_match("", 0) && !lic_match("", 1));
    assert!(lic_summary(nc).contains("non-commercial"));
    assert!(lic_summary(bysa).contains("same licence"));
}

#[test]
fn parse_jamendo_fixture() {
    let v = jamendo_tracks(&fixture("jamendo_tracks.json"));
    assert_eq!(v.len(), 3);
    let a = &v[0];
    assert_eq!(a.title, "That's Life");
    assert_eq!(a.artist, "Yellow Faces");
    assert_eq!(
        (a.artist_id.as_str(), a.album_id.as_str()),
        ("7123", "104336")
    );
    assert_eq!(a.duration, 187);
    assert_eq!(a.genres, ["rock", "pop"]);
    assert_eq!(a.tags, ["happy", "energetic"]);
    assert!(a.cover.contains("width=300") && a.cover_full.contains("width=600"));
    assert_eq!(a.license(), "CC BY-SA 3.0");
    assert!(!a.download.is_empty());
    // The client ID Jamendo puts in stream URLs (`from=app-…`) is dropped.
    assert_eq!(
        a.stream,
        "https://prod-1.storage.jamendo.com/?trackid=1532771&format=mp31"
    );
    assert_eq!(v[1].download, "", "download not allowed");
    let dir = std::env::temp_dir().join("vivi-fm-nodl");
    assert!(download(&v[1], &dir, &DlProgress::default()).is_err());
    // Entries without a stream are dropped.
    assert!(v.iter().all(|t| !t.stream.is_empty()));
}

#[test]
fn parse_ia_fixture() {
    let v = ia_tracks(&fixture("ia_metadata.json"), 10);
    // The FLAC original, the 64 kbps duplicate and the hour-long mix are skipped.
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].title, "Opening");
    assert_eq!(v[1].title, "Night Drive");
    assert_eq!(v[0].artist, "Some Netlabel Artist");
    assert_eq!(v[0].album, "Night Sessions");
    assert_eq!(v[0].date, "2012-12-30");
    assert_eq!(v[0].duration, 245);
    assert_eq!(v[0].source, FMA);
    assert!(v[0].tags.contains(&"electronic".to_string()));
    assert_eq!(
        v[1].download,
        "https://archive.org/download/night_sessions/disc%201/02%20Night%20Drive.mp3"
    );
    assert!(v[0].cover_full.ends_with("/cover.jpg"));
    assert!(v[0].attribution().contains("CC BY 4.0"));
    let s = fixture("ia_search.json");
    let (ids, more) = ia_ids(&s, 0);
    assert_eq!(ids[0], ("night_sessions".to_string(), 1234));
    assert!(more);
    assert!(!ia_ids(&s, 10).1);
}

#[test]
fn parse_ia_item_inline() {
    let m = serde_json::json!({
        "metadata": {"identifier": "my item", "creator": ["Band"], "title": "Live",
            "date": "2012-12-30T00:00:00Z",
            "licenseurl": "http://creativecommons.org/licenses/by/4.0/"},
        "files": [
            {"name": "a.flac", "format": "Flac"},
            {"name": "dir/b c.mp3", "format": "VBR MP3", "title": "Song B"},
            {"name": "c_64kb.mp3", "format": "64Kbps MP3"}
        ]
    });
    let t = ia_tracks(&m, 12);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].title, "Song B");
    assert_eq!(t[0].source, IA);
    assert_eq!(
        t[0].download,
        "https://archive.org/download/my%20item/dir/b%20c.mp3"
    );
    // The waveform PNG derived from an audio file is no cover: the item thumbnail is.
    let m = serde_json::json!({
        "metadata": {"identifier": "w", "creator": "A", "title": "T",
            "licenseurl": "http://creativecommons.org/licenses/by/4.0/"},
        "files": [
            {"name": "T.mp3", "format": "VBR MP3", "source": "original"},
            {"name": "T.png", "format": "PNG", "source": "derivative", "original": "T.mp3"},
            {"name": "T_spectrogram.png", "format": "Spectrogram", "source": "derivative", "original": "T.mp3"}
        ]
    });
    assert_eq!(
        ia_tracks(&m, 12)[0].cover_full,
        "https://archive.org/services/img/w"
    );
    let mut m = m;
    m["files"].as_array_mut().unwrap().push(serde_json::json!(
        {"name": "art_thumb.jpg", "format": "JPEG", "source": "derivative", "original": "art.png"}
    ));
    assert_eq!(
        ia_tracks(&m, 12)[0].cover_full,
        "https://archive.org/download/w/art_thumb.jpg"
    );
    // No licence: nothing.
    let m = serde_json::json!({"metadata": {"identifier": "x"}, "files": []});
    assert!(ia_tracks(&m, 12).is_empty());
}

#[test]
fn query_box_follows_the_page() {
    // Album and artist pages: no stale search text, even if the query carries some.
    let album = Query {
        text: "old search".into(),
        album: Some((JAMENDO, "123".into())),
        ..Default::default()
    };
    assert_eq!(query_box_text(&album), "");
    let artist = Query {
        artist: Some((JAMENDO, "Kevin MacLeod".into())),
        ..Default::default()
    };
    assert_eq!(query_box_text(&artist), "");
    // Similar: the box shows what the similar search really looks for.
    let mut t = t("Song", "Some Artist");
    t.source = JAMENDO;
    let q = similar_query(&t);
    assert_eq!(query_box_text(&q), q.text);
    assert_eq!(q.text, "Some Artist");
    // A plain search keeps its text (Back restores it).
    let plain = Query {
        text: "kevin macleod".into(),
        ..Default::default()
    };
    assert_eq!(query_box_text(&plain), "kevin macleod");
}

#[test]
fn relevance_and_merge() {
    let q = Query {
        text: "kevin macleod".into(),
        ..Default::default()
    };
    let ia = vec![
        t("Lecture 3", "Prof"),
        t("Other", "Kevin MacLeod"),
        t("Kevin MacLeod Medley", "X"),
    ];
    let jam = vec![
        t("Kevin's Song", "Somebody"),
        t("Monkeys Spinning Monkeys", "Kevin MacLeod"),
    ];
    let v = merge(&[jam, ia], &q);
    assert_eq!(v[0].artist, "Kevin MacLeod");
    assert_eq!(v[1].artist, "Kevin MacLeod", "both exact artist hits first");
    // Fewer than five hits: non-matching ones stay, at the end.
    assert_eq!(v.last().unwrap().title, "Lecture 3");
    // Duplicates across sources collapse.
    let a = vec![t("Song", "Band")];
    let mut b = t("song!", "band");
    b.id = "other".into();
    assert_eq!(merge(&[a, vec![b]], &Query::default()).len(), 1);
    // Runs of one artist are broken up.
    let run = vec![
        t("1", "A"),
        t("2", "A"),
        t("3", "A"),
        t("4", "B"),
        t("5", "C"),
    ];
    let names: Vec<String> = spread(run).into_iter().map(|t| t.title).collect();
    assert_eq!(names, ["1", "2", "4", "3", "5"]);
    // Tag matches count.
    let mut g = t("Untitled", "Y");
    g.genres = vec!["jazz".into()];
    assert!(score(&g, "jazz") > score(&t("Untitled", "Z"), "jazz"));
}

#[test]
fn filters_and_sort() {
    let mk = |ti: &str, d: u32, lic: &str, date: &str| Track {
        id: ti.into(),
        title: ti.into(),
        duration: d,
        license_url: lic.into(),
        date: date.into(),
        ..Default::default()
    };
    let by = "http://creativecommons.org/licenses/by/3.0/";
    let nc = "http://creativecommons.org/licenses/by-nc/3.0/";
    let l = vec![
        mk("a", 120, by, "2020-01-01"),
        mk("b", 400, nc, "2022-01-01"),
        mk("c", 200, by, "2021-01-01"),
    ];
    let q = |f: fn(&mut Query)| {
        let mut q = Query::default();
        f(&mut q);
        merge(std::slice::from_ref(&l), &q)
            .iter()
            .map(|t| t.title.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(q(|q| q.lic = 1), ["a", "c"]);
    let mut j = mk("j", 100, by, "");
    j.source = JAMENDO;
    j.genres = vec!["ambient".into()];
    let jazz = Query {
        chip: Some(7),
        ..Default::default()
    };
    assert!(!keep(&j, &jazz) && keep(&j, &Query::default()));
    j.genres.push("jazzfusion".into());
    assert!(keep(&j, &jazz));
    assert_eq!(q(|q| q.len = 1), ["a"]);
    assert_eq!(q(|q| q.len = 3), ["b"]);
    assert_eq!(q(|q| q.sort = 2), ["b", "c", "a"]);
    assert_eq!(q(|q| q.sort = 3), ["a", "c", "b"]);
    assert_eq!(q(|q| q.sort = 4), ["b", "c", "a"]);
}

#[test]
fn query_building() {
    let mut q = Query {
        text: "ac/dc: (live)".into(),
        chip: Some(7),
        mood: 2,
        lic: 1,
        len: 2,
        ..Default::default()
    };
    let s = ia_query(&q);
    assert!(s.contains("title:(ac dc live)"), "{s}");
    assert!(s.contains("subject:(jazz)") && s.contains("subject:(happy)"));
    assert!(s.contains("NOT licenseurl:*nc*") && s.contains("NOT subject:(podcast"));
    assert!(
        s.contains("opensource_audio"),
        "text searches include community uploads"
    );
    assert_eq!(ia_sort(&q), "", "text search keeps IA relevance");
    let p = jamendo_params(&q, JMode::Name);
    let has = |k: &str, v: &str| p.iter().any(|(a, b)| *a == k && b == v);
    assert!(
        has("namesearch", "ac/dc: (live)") && has("fuzzytags", "jazz") && has("vartags", "happy")
    );
    assert!(has("ccnc", "false") && has("durationbetween", "180_360"));
    assert!(!p.iter().any(|(k, _)| *k == "client_id"));
    q.text.clear();
    q.chip = Some(1);
    q.page = 2;
    let p = jamendo_params(&q, JMode::Browse);
    let has = |k: &str, v: &str| p.iter().any(|(a, b)| *a == k && b == v);
    assert!(has("order", "releasedate_desc") && has("offset", "48"));
    assert_eq!(ia_sort(&q), "publicdate desc");
    assert!(
        !ia_query(&q).contains("opensource_audio"),
        "browse: curated only"
    );
    q.source = 3;
    assert!(ia_query(&q).contains("collection:(freemusicarchive)"));
    let a = Query {
        album: Some((JAMENDO, "9".into())),
        ..Default::default()
    };
    let p = jamendo_params(&a, JMode::Browse);
    assert!(p.iter().any(|(k, v)| *k == "album_id" && v == "9"));
    assert!(a.uses(JAMENDO) && !a.uses(IA));
    assert_eq!(tidy_stem("03_-_My_Song", ""), "My Song");
    assert_eq!(
        tidy_stem("deep-and-dirty-by-kevin-macleod", "Kevin MacLeod"),
        "deep and dirty"
    );
    assert_eq!(ia_len(&serde_json::json!("4:05")), Some(245.0));
    assert_eq!(fmt_len(245), "4:05");
    assert_eq!(
        clean_title("{dj-N} Club Step (ID: 396093)"),
        "{dj-N} Club Step"
    );
    assert_eq!(clean_title("Song (Live)"), "Song (Live)");
    assert_eq!(fmt_len(3725), "1:02:05");
    assert_eq!(unescape("That&#039;s"), "That's");
    assert_eq!(
        scrub("url?client_id=abc1".into(), "abc1"),
        "url?client_id=…"
    );
}

#[test]
fn similar_and_tags() {
    let mut x = t("Song", "Band");
    x.source = JAMENDO;
    x.genres = vec!["jazz".into()];
    x.tags = vec!["relaxing".into()];
    x.date = "2019-05-01".into();
    x.license_url = "http://creativecommons.org/licenses/by/3.0/".into();
    let q = similar_query(&x);
    assert_eq!(q.chip.map(|c| CHIPS[c].1), Some("jazz"));
    assert_eq!(MOODS[q.mood], "Relaxing");
    assert_eq!(q.source, 1);
    let tg = tags_for(&x);
    assert_eq!(tg.genre, "Jazz");
    assert!(tg
        .copyright
        .starts_with("2019 Band. Licensed under CC BY 3.0"));
    assert!(tg.comment.contains("licensed under CC BY 3.0"));
    assert!(licence_text(&x).contains("commercial use OK"));
    assert!(target(&x, Path::new("/m")).ends_with("Band/Band - Song.mp3"));
}

/// Live Internet Archive search + download with tags (no key). `--ignored`.
#[test]
#[ignore]
fn live_internet_archive() {
    let q = Query {
        text: "kevin macleod".into(),
        source: 2,
        ..Default::default()
    };
    let t0 = Instant::now();
    let (ids, _) = ia_search_ids(&q).unwrap();
    eprintln!("IA search {:?}: {} items", t0.elapsed(), ids.len());
    let all: Vec<Track> = ids
        .iter()
        .take(6)
        .flat_map(|(id, d)| ia_item(id, 8, *d))
        .collect();
    let v = merge(&[all], &q);
    assert!(!v.is_empty());
    for t in v.iter().take(5) {
        eprintln!(
            "  {} — {} [{}] {}",
            t.artist,
            t.title,
            t.license(),
            t.length()
        );
    }
    let dir = std::env::temp_dir().join(format!("vivi-fm-{}", std::process::id()));
    let small = v
        .iter()
        .min_by_key(|t| {
            if t.duration == 0 {
                u32::MAX
            } else {
                t.duration
            }
        })
        .unwrap();
    let p = download(small, &dir, &DlProgress::default()).unwrap();
    let (tags, dur) = auto_tagger::read_tags(&p).unwrap();
    assert_eq!(tags.title, small.title);
    assert!(
        tags.copyright.contains(&small.license_url),
        "{}",
        tags.copyright
    );
    assert!(tags.comment.contains("licensed under"));
    assert!(dur > 1.0);
    assert!(p.with_extension("licence.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Live Jamendo search; needs `JAMENDO_CLIENT_ID` in the environment. `--ignored`.
#[test]
#[ignore]
fn live_jamendo() {
    let Ok(key) = std::env::var("JAMENDO_CLIENT_ID") else {
        eprintln!("JAMENDO_CLIENT_ID not set, skipped");
        return;
    };
    for text in ["rock", "jazz", "piano", "lofi"] {
        let q = Query {
            text: text.into(),
            source: 1,
            ..Default::default()
        };
        let t0 = Instant::now();
        let (v, more) = jamendo_search(&key, &q).unwrap();
        let v = merge(&[v], &q);
        eprintln!(
            "{text}: {} tracks in {:?} (more {more})",
            v.len(),
            t0.elapsed()
        );
        assert!(!v.is_empty(), "{text}");
        assert!(v[0].stream.starts_with("http"));
    }
    let q = Query {
        chip: Some(0),
        source: 1,
        lic: 1,
        ..Default::default()
    };
    let (v, _) = jamendo_search(&key, &q).unwrap();
    assert!(!v.is_empty());
    assert!(merge(&[v], &q)
        .iter()
        .all(|t| commercial_ok(&t.license_url)));
}
