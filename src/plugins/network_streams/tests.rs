use super::*;

#[test]
fn logos_are_sniffed_shrunk_and_migrated() {
    assert_eq!(
        sniff_logo(b"<html><body>404</body></html>"),
        LogoKind::Other
    );
    assert_eq!(
        sniff_logo(b"<?xml version=\"1.0\"?><svg xmlns=\"\"/>"),
        LogoKind::Svg
    );
    assert_eq!(sniff_logo(&[0xFF, 0xD8, 0xFF, 0xE0]), LogoKind::Raster);
    let dir = std::env::temp_dir().join(format!("vivi-logos-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(600, 300, image::Rgba([200, 10, 10, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    assert_eq!(sniff_logo(&png), LogoKind::Raster);
    let out = store_logo(&dir.join("a"), &png).unwrap();
    assert_eq!(out, dir.join("a.png"));
    assert_eq!(image::image_dimensions(&out).unwrap(), (128, 64));
    assert!(store_logo(&dir.join("b"), b"<html>").is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn m3u_iptv() {
    let t = "#EXTM3U\n#EXTINF:-1 tvg-id=\"a\" tvg-logo=\"http://l/a.png\" group-title=\"News, World\",Channel A\nhttp://s/a.m3u8\n#EXTVLCOPT:http-user-agent=x\n#EXTINF:-1,\nhttps://s/b/radio.mp3\nrelative.mp3\nhttp://s/c\n";
    let v = parse_m3u(t);
    assert_eq!(v.len(), 3);
    assert_eq!(v[0].name, "Channel A");
    assert_eq!(v[0].logo, "http://l/a.png");
    assert_eq!(v[0].group, "News, World");
    assert_eq!(v[1].name, "radio.mp3");
    assert_eq!(v[2].url, "http://s/c");
}

#[test]
fn names_and_sources() {
    assert_eq!(
        pct_decode("01.%20J.%20S.%20BACH%20-%20Air.mp3"),
        "01. J. S. BACH - Air.mp3"
    );
    assert_eq!(pct_decode("100%25 %zz %"), "100% %zz %");
    let v = parse_m3u("#EXTM3U\nhttp://h/My%20Song.mp3\n");
    assert_eq!(v[0].name, "My Song.mp3");
    assert!(!is_playlist_source("folk"));
    assert!(is_playlist_source("https://x/a.m3u"));
    let f = std::env::temp_dir().join(format!("vivi-pl-{}.m3u", std::process::id()));
    std::fs::write(&f, "http://a/b\n").unwrap();
    assert!(is_playlist_source(f.to_str().unwrap()));
    let _ = std::fs::remove_file(f);
    assert!(placeholder(M3U).contains("Choose file"));
}

#[test]
fn stations_json() {
    let v = serde_json::json!([
        {"name": " Jazz ", "url": "http://a", "url_resolved": "http://b", "tags": "jazz,smooth",
         "country": "France", "codec": "MP3", "bitrate": 128, "favicon": "", "homepage": "h"},
        {"name": "broken", "url": "", "url_resolved": ""}
    ]);
    let s = parse_stations(&v);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].name, "Jazz");
    assert_eq!(s[0].url, "http://b");
    assert_eq!(s[0].group, "jazz, smooth");
    assert_eq!(s[0].label(), "France  ·  MP3 128 kbps  ·  jazz, smooth");
    assert_eq!(top_groups(&s, 5), ["jazz", "smooth"]);
}

#[test]
fn recent_dedup() {
    let a = Stream {
        url: "a".into(),
        ..Default::default()
    };
    let b = Stream {
        url: "b".into(),
        ..Default::default()
    };
    let mut l = vec![a.clone(), b.clone()];
    push_front(&mut l, &b, 2);
    assert_eq!(l, vec![b, a.clone()]);
    push_front(
        &mut l,
        &Stream {
            url: "c".into(),
            ..Default::default()
        },
        2,
    );
    assert_eq!(l.len(), 2);
    assert_eq!(
        l[1],
        Stream {
            url: "b".into(),
            ..Default::default()
        }
    );
}

#[test]
fn info_text() {
    let s = now_info(|p| match p {
        "metadata/by-key/icy-title" => Some("Artist - Song".into()),
        "audio-codec-name" => Some("mp3".into()),
        "audio-bitrate" => Some("128000".into()),
        _ => None,
    });
    assert_eq!(s, "Now playing: Artist - Song\nAudio: mp3 128 kbps");
    assert_eq!(now_info(|_| None), "Nothing is playing");
}

#[test]
fn pickers_and_filter() {
    let v = serde_json::json!([{"code":"UA","name":"Ukraine","flag":"🇺🇦"},{"code":"AD","name":"Andorra"}]);
    let c = parse_iptv_index(&v, true);
    assert_eq!(c[0].name, "Andorra");
    assert_eq!(c[1].url, format!("{IPTV}/countries/ua.m3u"));
    assert_eq!(c[1].source, PICK);
    let t = parse_tags(
        &serde_json::json!([{"name":"jazz","stationcount":5},{"name":"x","stationcount":3}]),
    );
    assert_eq!(t.len(), 3);
    assert_eq!(t[2].url, "jazz");
    let all = parse_m3u("#EXTINF:-1 group-title=\"News\",BBC One\nhttp://a\n#EXTINF:-1,Euronews\nhttp://b\n#EXTINF:-1 group-title=\"BBC\",Other\nhttp://c\n");
    let f = filter(&all, "bbc");
    assert_eq!(
        f.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
        ["http://a", "http://c"]
    );
    assert_eq!(filter(&all, " ").len(), 3);
    assert!(live_filter(TV_COUNTRY) && !live_filter(M3U) && SOURCES[SAVED] == "Favorites");
}

#[test]
fn epg_groups_favorites_playlists() {
    let t = "#EXTM3U x-tvg-url=\"https://g/a.xml.gz,https://g/b.xml\"\n#EXTINF:-1 tvg-id=\"A.uk@HD\" group-title=\"News;General\",A\nhttp://a\n#EXTINF:-1 group-title=\"News\",B\nhttp://b\n#EXTINF:-1 group-title=\"Kids\",C\nhttp://c\n";
    let v = parse_m3u(t);
    assert_eq!(v[0].tvg_id, "A.uk@HD");
    assert_eq!(v[0].epg, "https://g/a.xml.gz");
    assert_eq!(
        epg_url("#EXTM3U url-tvg=\"http://x/g.xml\""),
        "http://x/g.xml"
    );
    assert_eq!(top_groups(&v, 5), ["News", "General", "Kids"]);
    assert_eq!(by_group(&v, "news").len(), 2);
    assert_eq!(by_group(&v, "").len(), 3);
    let mut w = v.clone();
    let favs = ["http://c".to_string()].into_iter().collect();
    pin_favorites(&mut w, &favs);
    assert_eq!(w[0].url, "http://c");
    assert_eq!(w[1].url, "http://a");
    assert_eq!(ago(0, 100), "never");
    assert_eq!(ago(100, 7300), "2 h ago");
    assert_eq!(playlist_name("https://h/p/My%20TV.m3u8?x=1"), "My TV");
    assert_eq!(
        playlist_picks(&[Playlist {
            name: "P".into(),
            url: "u".into(),
            count: 3,
            error: "404".into(),
            ..Default::default()
        }])[0]
            .url,
        "pl:u"
    );
    // Saved favorites stay readable without the new fields.
    let old: Stream = serde_json::from_str(r#"{"name":"n","url":"u"}"#).unwrap();
    assert!(old.tvg_id.is_empty());
    assert!(!serde_json::to_string(&old).unwrap().contains("tvg_id"));
    assert!(live_filter(MY_PL) && SOURCES[ALL].starts_with("Search all"));
}

#[test]
fn somafm_json() {
    let v = serde_json::json!({"channels":[{"id":"groovesalad","title":"Groove Salad","genre":"ambient|electronica","description":"d","image":"http://i/s.png","largeimage":"",
        "playlists":[{"url":"http://p/gs.pls","format":"aac","quality":"highest"},{"url":"http://p/gs130.pls","format":"mp3","quality":"high"}]}]});
    let s = parse_somafm(&v);
    assert_eq!(s[0].url, "http://p/gs130.pls");
    assert_eq!(s[0].logo, "http://i/s.png");
    assert!(s[0].group.starts_with("ambient, electronica"));
    // Chips are the genres, not the description.
    assert_eq!(top_groups(&s, 5), ["ambient", "electronica"]);
    assert_eq!(by_group(&s, "electronica").len(), 1);
}

#[test]
fn cache_names_are_stable_and_old_ones_migrate() {
    // FNV-1a 64 test vectors: the names must never change between builds.
    assert_eq!(hash_name(""), "cbf29ce484222325");
    assert_eq!(hash_name("a"), "af63dc4c8601ec8c");
    assert_eq!(hash_name("foobar"), "85944171f73967e8");
    // The spelled-out legacy name is what DefaultHasher gave (Vivi 0.12 and older).
    for u in [
        "",
        "a",
        "https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg",
        "1234567",
    ] {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        u.hash(&mut h);
        assert_eq!(legacy_hash_name(u), format!("{:016x}", h.finish()), "{u:?}");
    }
    let dir = std::env::temp_dir().join(format!("vivi-hash-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let url = "https://example.com/logo.png";
    let old = dir.join(format!("{}.svg", legacy_hash_name(url)));
    std::fs::write(&old, b"<svg/>").unwrap();
    let p = cached_with_ext(&dir, url, &["png", "svg"]).unwrap();
    assert_eq!(p, dir.join(format!("{}.svg", hash_name(url))));
    assert_eq!(std::fs::read(&p).unwrap(), b"<svg/>");
    assert!(!old.exists());
    // A new file wins over an old one; nothing found stays None.
    let old2 = dir.join(format!("custom-{}.m3u", legacy_hash_name(url)));
    std::fs::write(&old2, "old").unwrap();
    let new2 = dir.join(format!("custom-{}.m3u", hash_name(url)));
    std::fs::write(&new2, "new").unwrap();
    let f = hashed_file(&dir, url, |h| format!("custom-{h}.m3u"));
    assert_eq!(std::fs::read_to_string(f).unwrap(), "new");
    assert!(cached_with_ext(&dir, "https://other", &["png"]).is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn station_ids_and_click_path() {
    let v = serde_json::json!([{"name": "A", "url": "http://a",
        "stationuuid": "510506E7-6bc0-4b91-b6a1-fc024ccad1a8"}]);
    let s = parse_stations(&v);
    assert_eq!(s[0].stationuuid, "510506E7-6bc0-4b91-b6a1-fc024ccad1a8");
    assert_eq!(
        click_path(&s[0].stationuuid).unwrap(),
        "/json/url/510506e7-6bc0-4b91-b6a1-fc024ccad1a8"
    );
    assert!(click_path("").is_none());
    assert!(click_path("../../json/stations?x=1aaaaaaaaaaaaaaaaaaaaaaaa").is_none());
    // Favorites keep the id; older ones without it still load.
    let j = serde_json::to_string(&s[0]).unwrap();
    assert!(j.contains("stationuuid"));
    let old: Stream = serde_json::from_str(r#"{"name":"n","url":"u"}"#).unwrap();
    assert!(old.stationuuid.is_empty());
}

/// A one-shot HTTP server on localhost answering every request with `status` / `body`;
/// returns its base URL and a counter of requests it got.
fn serve(
    status: u16,
    body: &'static str,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n2 = n.clone();
    std::thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            n2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let _ = c.read(&mut buf);
            let _ = write!(
                c,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (base, n)
}

#[test]
fn radio_servers_fail_over() {
    let v = serde_json::json!([
        {"ip": "1.2.3.4", "name": "de1.api.radio-browser.info"},
        {"ip": "::1", "name": "de1.api.radio-browser.info"},
        {"ip": "5.6.7.8", "name": "NL1.api.radio-browser.info"},
        {"ip": "9.9.9.9", "name": "evil.example.com"},
        {"ip": "9.9.9.9", "name": "x/y.api.radio-browser.info"}
    ]);
    assert_eq!(
        parse_servers(&v),
        [
            "https://de1.api.radio-browser.info",
            "https://nl1.api.radio-browser.info"
        ]
    );
    let mut a: Vec<u32> = (0..20).collect();
    shuffle(&mut a, 42);
    assert_ne!(a, (0..20).collect::<Vec<_>>());
    a.sort();
    assert_eq!(a, (0..20).collect::<Vec<_>>());
    // A refused connection and a busy mirror are skipped; the failed ones are reported.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let (busy, _) = serve(503, "{}");
    let (good, hits) = serve(200, r#"[{"name":"S","url":"http://s"}]"#);
    let mut failed = Vec::new();
    let v = radio_get_from(
        &[dead.clone(), busy.clone(), good.clone()],
        "/json/stations/search",
        &[("name", "s")],
        |s| failed.push(s.to_string()),
    )
    .unwrap();
    assert_eq!(parse_stations(&v)[0].url, "http://s");
    assert_eq!(failed, [dead, busy]);
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    // A 4xx answer is about the request: the next mirror is not asked.
    let (bad, _) = serve(404, "{}");
    let (unused, unused_hits) = serve(200, "[]");
    let e = radio_get_from(&[bad, unused], "/json/url/x", &[], |_| {}).unwrap_err();
    assert!(e.contains("404"), "{e}");
    assert_eq!(unused_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn search_all_merges_somafm_and_splits_kinds() {
    let soma = parse_somafm(
        &serde_json::json!({"channels":[{"id":"groovesalad","title":"Groove Salad","genre":"ambient","description":"d","image":"",
        "playlists":[{"url":"http://p/gs130.pls","format":"mp3","quality":"high"}]}]}),
    );
    let tv = iptv_playlist_from("#EXTINF:-1 tvg-id=\"g.uk\",Groove TV\nhttp://t/g.m3u8\n");
    let mine = parse_m3u(
        "#EXTINF:-1,Groove FM\nhttp://r/live.mp3\n#EXTINF:-1,Groove News HD\nhttp://t/news.m3u8\n",
    );
    let radio = vec![Stream {
        name: "Groove Radio".into(),
        url: "http://rb/1".into(),
        source: "radio-browser.info".into(),
        ..Default::default()
    }];
    let parts = || {
        vec![
            Ok(mine.clone()),
            Ok(tv.clone()),
            Ok(filter(&soma, "groove")),
            Ok(radio.clone()),
            Err("iptv-org: offline".to_string()),
        ]
    };
    let all = merge_results(parts(), None).unwrap();
    assert_eq!(all.len(), 5);
    assert!(all.iter().any(|s| s.source == "SomaFM"));
    let names = |k| {
        merge_results(parts(), Some(k))
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(Kind::Radio),
        ["Groove FM", "Groove Salad", "Groove Radio"]
    );
    assert_eq!(names(Kind::Tv), ["Groove News HD", "Groove TV"]);
    assert_eq!(Kind::parse("TV"), Some(Kind::Tv));
    assert_eq!(Kind::parse("podcast"), None);
    // Duplicates by URL are dropped; nothing found + an error = the error.
    assert_eq!(
        merge_results(vec![Ok(radio.clone()), Ok(radio.clone())], None)
            .unwrap()
            .len(),
        1
    );
    assert!(merge_results(vec![Ok(vec![]), Err("down".into())], None).is_err());
}

fn iptv_playlist_from(t: &str) -> Vec<Stream> {
    let mut v = parse_m3u(t);
    for s in &mut v {
        s.source = "iptv-org".into();
    }
    v
}

#[test]
fn missing_playlist_files_are_errors_not_channel_names() {
    let tmp = std::env::temp_dir().join(format!("vivi-missing-{}", std::process::id()));
    let missing = tmp.join("nope.m3u");
    let q = missing.to_string_lossy().to_string();
    assert_eq!(m3u_query(&q), M3uQuery::Missing(missing.clone()));
    assert!(is_playlist_source(&q));
    let e = load_m3u(&q).unwrap_err();
    assert!(e.starts_with("Playlist file not found"), "{e}");
    for q in [
        "~/no/such.m3u8",
        "./list.m3u",
        "C:\\TV\\list.m3u",
        "file:///no/such.m3u",
        "channels.M3U8",
    ] {
        assert!(matches!(m3u_query(q), M3uQuery::Missing(_)), "{q}");
    }
    for q in ["bbc news", "News 24/7", "Euronews"] {
        assert_eq!(m3u_query(q), M3uQuery::Name, "{q}");
    }
    assert_eq!(m3u_query("https://x/a.m3u"), M3uQuery::Url);
    std::fs::create_dir_all(&tmp).unwrap();
    let f = tmp.join("my list.m3u");
    std::fs::write(&f, "http://a/b\n").unwrap();
    assert_eq!(m3u_query(f.to_str().unwrap()), M3uQuery::File(f.clone()));
    let url = format!("file://{}", f.to_string_lossy().replace(' ', "%20"));
    assert_eq!(load_m3u(&url).unwrap()[0].url, "http://a/b");
    assert!(load_m3u(tmp.to_str().unwrap())
        .unwrap_err()
        .starts_with("Not a playlist file"));
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn songs_on_servers_are_not_stations() {
    let props = |d: Option<&'static str>, icy: bool| {
        move |k: &str| match k {
            "duration" => d.map(String::from),
            "metadata/by-key/icy-title" if icy => Some("A - B".into()),
            _ => None,
        }
    };
    // A Free Music / podcast file: known length, no stream title.
    assert!(!looks_like_station(false, props(Some("215.3"), false)));
    // Live radio: no length (or 0), or a stream title, or one of our stations.
    assert!(looks_like_station(false, props(None, false)));
    assert!(looks_like_station(false, props(Some("0"), false)));
    assert!(looks_like_station(false, props(Some("215.3"), true)));
    assert!(looks_like_station(true, props(Some("215.3"), false)));
}

#[test]
fn drawer_panel_text() {
    let st = Stream {
        name: "Groove%20Salad".into(),
        ..Default::default()
    };
    let t = drawer_text(Some(&st), true, "Now playing: A - B", 1, 3);
    assert_eq!(
        t,
        "Groove Salad\nNow playing: A - B\n\n1 favorite · 3 recent streams"
    );
    let t = drawer_text(None, false, "Nothing is playing", 0, 1);
    assert!(t.starts_with("No network stream is playing."));
    assert!(t.ends_with("0 favorites · 1 recent stream"));
    // The panel asks for it when it opens.
    let m = NetworkStreams::default().manifest();
    assert!(m
        .contributes
        .iter()
        .any(|c| c.point == contrib::DRAWER_PANEL && c.command == "drawer"));
}

/// Live iptv-org and SomaFM. `--ignored`.
#[test]
#[ignore]
fn live_iptv_somafm() {
    let c = iptv_index(true).unwrap();
    let ua = c.iter().find(|s| s.name == "Ukraine").unwrap();
    let ch = iptv_playlist(&ua.url).unwrap();
    eprintln!(
        "UA: {} channels, {} with logos",
        ch.len(),
        ch.iter().filter(|s| !s.logo.is_empty()).count()
    );
    assert!(!ch.is_empty());
    assert!(!iptv_index(false).unwrap().is_empty());
    assert!(!iptv_search("news").unwrap().is_empty());
    assert!(genre_list().unwrap().len() > 10);
    assert!(!genre_stations("@topclick").unwrap().is_empty());
    assert!(somafm().unwrap().len() > 10);
    let all = search_all("groove").unwrap();
    assert!(
        all.iter().any(|s| s.source == "SomaFM"),
        "SomaFM in Search all"
    );
}

/// Live radio-browser.info search (no key). `--ignored`.
#[test]
#[ignore]
fn live_radio_browser() {
    let servers = radio_servers();
    eprintln!("servers: {servers:?}");
    assert!(!servers.is_empty());
    let jazz = radio_search(0, "jazz").unwrap();
    assert!(click_path(&jazz[0].stationuuid).is_some());
    let v = radio_get(&click_path(&jazz[0].stationuuid).unwrap(), &[]).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert!(!radio_any("folk").unwrap().is_empty());
    for (m, q) in [(0, "jazz"), (1, ""), (2, "UA"), (3, "rock")] {
        let v = radio_search(m, q).unwrap();
        assert!(!v.is_empty(), "{m} {q}");
        eprintln!("{m} {q}: {} e.g. {:?}", v.len(), v[0].name);
    }
}
