//! Extra Auto-tagger sources (#79) and synced lyrics (#80), used when the review of a
//! file is built. MusicBrainz stays authoritative: these only fill fields it left empty,
//! add cover candidates and record which source each proposed field comes from.
//!
//! * **Deezer** (`api.deezer.com/search` + `/track/<id>`, no key): album, track number,
//!   release date, 1000×1000 cover.
//! * **iTunes Search** (`itunes.apple.com/search`, no key): album, track number, date,
//!   genre, 1400×1400 cover (`artworkUrl100` resized by the CDN).
//! * **Last.fm** (`track.getTopTags`, the user's API key): genre from the top tag.
//! * **Discogs** (`/database/search`, the user's personal access token): year, genre
//!   (genre / style), cover.
//! * **LRCLIB** (shared with the Lyrics plugin): synced lyrics (LRC) to embed.

use super::auto_tagger::{Alt, Candidate};
use super::lrclib;
use super::web;

/// Which sources to ask (from the Auto-tagger settings).
#[derive(Debug, Clone, Default)]
pub struct Opts {
    pub deezer: bool,
    pub itunes: bool,
    pub lastfm_key: String,
    pub discogs_token: String,
    pub lyrics: bool,
}

/// What one source knows about a track; empty strings are unknown.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub album: String,
    pub track: String,
    pub date: String,
    pub genre: String,
    pub cover: String,
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Same artist and title, ignoring case, spacing and punctuation (one may contain the other,
/// e.g. "Song (Remastered)").
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (norm(a), norm(b));
    !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
}

fn capitalize(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Id of the first Deezer search hit by this artist with this title.
pub fn parse_deezer_search(v: &serde_json::Value, artist: &str, title: &str) -> Option<u64> {
    v["data"].as_array()?.iter().find_map(|t| {
        (same(t["artist"]["name"].as_str()?, artist) && same(t["title"].as_str()?, title))
            .then(|| t["id"].as_u64())
            .flatten()
    })
}

/// Fields of a Deezer `/track/<id>` answer.
pub fn parse_deezer_track(v: &serde_json::Value) -> Found {
    let s = |x: &serde_json::Value| x.as_str().unwrap_or("").to_string();
    Found {
        album: s(&v["album"]["title"]),
        track: v["track_position"]
            .as_u64()
            .map(|n| n.to_string())
            .unwrap_or_default(),
        date: s(&v["release_date"]),
        genre: String::new(),
        cover: s(&v["album"]["cover_xl"]),
    }
}

/// First iTunes Search hit by this artist with this title.
pub fn parse_itunes(v: &serde_json::Value, artist: &str, title: &str) -> Option<Found> {
    let r = v["results"].as_array()?.iter().find(|r| {
        same(r["artistName"].as_str().unwrap_or(""), artist)
            && same(r["trackName"].as_str().unwrap_or(""), title)
    })?;
    let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
    Some(Found {
        album: s("collectionName"),
        track: r["trackNumber"]
            .as_u64()
            .map(|n| n.to_string())
            .unwrap_or_default(),
        date: s("releaseDate").chars().take(10).collect(),
        genre: s("primaryGenreName"),
        cover: s("artworkUrl100").replace("100x100bb", "1400x1400bb"),
    })
}

/// Genre from Last.fm top tags: the most used tag that is not the artist's name or a
/// listening note ("seen live", "favorites").
pub fn parse_lastfm_tags(v: &serde_json::Value, artist: &str) -> Option<String> {
    const SKIP: &[&str] = &[
        "seenlive",
        "favorites",
        "favourite",
        "favorite",
        "love",
        "awesome",
    ];
    v["toptags"]["tag"].as_array()?.iter().find_map(|t| {
        let n = t["name"].as_str()?;
        let k = norm(n);
        (!k.is_empty() && k != norm(artist) && !SKIP.contains(&k.as_str()) && n.len() <= 30)
            .then(|| capitalize(n))
    })
}

/// First Discogs release search hit.
pub fn parse_discogs(v: &serde_json::Value) -> Option<Found> {
    let r = v["results"].as_array()?.first()?;
    let first = |k: &str| {
        r[k].as_array()
            .and_then(|a| a.first())
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string()
    };
    let genre = {
        let st = first("style");
        if st.is_empty() {
            first("genre")
        } else {
            st
        }
    };
    let cover = r["cover_image"].as_str().unwrap_or("");
    Some(Found {
        album: String::new(),
        track: String::new(),
        date: r["year"]
            .as_str()
            .map(String::from)
            .or_else(|| r["year"].as_u64().map(|y| y.to_string()))
            .unwrap_or_default(),
        genre,
        cover: if cover.ends_with("spacer.gif") {
            String::new()
        } else {
            cover.to_string()
        },
    })
}

/// Whether two values of `field` say the same thing (years compare by their first four
/// digits, track numbers without "/total" and leading zeros).
pub fn agrees(field: &str, a: &str, b: &str) -> bool {
    match field {
        "Year" => a.chars().take(4).eq(b.chars().take(4)),
        "Track" => {
            let n = |x: &str| {
                x.split('/')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_start_matches('0')
                    .to_string()
            };
            n(a) == n(b)
        }
        _ => norm(a) == norm(b),
    }
}

/// Merge what source `src` knows into `c`: empty fields are filled (and remembered as
/// coming from `src`), a value that matches counts `src` as agreeing (raises the
/// confidence), a different value becomes an alternative the review can switch to. The
/// cover becomes a candidate labelled "<src>".
pub fn merge(c: &mut Candidate, f: &Found, src: &'static str) {
    let mut agreed = false;
    for (field, v) in [
        ("Album", &f.album),
        ("Track", &f.track),
        ("Year", &f.date),
        ("Genre", &f.genre),
    ] {
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        let Some(dst) = super::auto_tagger::field_mut(&mut c.tags, field) else {
            continue;
        };
        if dst.trim().is_empty() {
            *dst = v.to_string();
            c.sources.push((field, src));
        } else if agrees(field, dst, v) {
            agreed = true;
        } else if !c
            .alts
            .iter()
            .any(|a| a.field == field && agrees(field, &a.value, v))
        {
            c.alts.push(Alt {
                field,
                value: v.to_string(),
                source: src,
            });
        }
    }
    if agreed && !c.agree.contains(&src) {
        c.agree.push(src);
    }
    if !f.cover.is_empty() && !c.extra_covers.iter().any(|(_, u)| u == &f.cover) {
        c.extra_covers.push((src.to_string(), f.cover.clone()));
    }
}

fn deezer(artist: &str, title: &str) -> Result<Option<Found>, String> {
    // Plain query: the advanced `artist:"…" track:"…"` syntax often answers nothing.
    let q = format!("{artist} {title}");
    let v = web::get_json(
        "https://api.deezer.com/search",
        &[("q", &q), ("limit", "10")],
        "Deezer",
    )?;
    let Some(id) = parse_deezer_search(&v, artist, title) else {
        return Ok(None);
    };
    let t = web::get_json(&format!("https://api.deezer.com/track/{id}"), &[], "Deezer")?;
    Ok(Some(parse_deezer_track(&t)))
}

fn itunes(artist: &str, title: &str) -> Result<Option<Found>, String> {
    let term = format!("{artist} {title}");
    let v = web::get_json(
        "https://itunes.apple.com/search",
        &[("term", &term), ("entity", "song"), ("limit", "10")],
        "iTunes",
    )?;
    Ok(parse_itunes(&v, artist, title))
}

fn lastfm(key: &str, artist: &str, title: &str) -> Result<Option<String>, String> {
    let v = web::get_json(
        "https://ws.audioscrobbler.com/2.0/",
        &[
            ("method", "track.gettoptags"),
            ("artist", artist),
            ("track", title),
            ("autocorrect", "1"),
            ("api_key", key),
            ("format", "json"),
        ],
        "Last.fm",
    )?;
    Ok(parse_lastfm_tags(&v, artist))
}

fn discogs(token: &str, artist: &str, title: &str) -> Result<Option<Found>, String> {
    let v = web::get_json(
        "https://api.discogs.com/database/search",
        &[
            ("type", "release"),
            ("artist", artist),
            ("track", title),
            ("per_page", "5"),
            ("token", token),
        ],
        "Discogs",
    )?;
    Ok(parse_discogs(&v))
}

/// Ask the enabled sources about `c` (artist + title) and merge what they know; also
/// fetch synced lyrics. `duration` (s) helps LRCLIB pick the right version. Returns
/// notes about sources that failed (the review still opens).
pub fn enrich(c: &mut Candidate, o: &Opts, duration: f64) -> Vec<String> {
    let (artist, title) = (c.tags.artist.clone(), c.tags.title.clone());
    let mut notes = Vec::new();
    if artist.is_empty() || title.is_empty() {
        return notes;
    }
    let mut note = |r: Result<(), String>| {
        if let Err(e) = r {
            notes.push(e);
        }
    };
    if o.deezer {
        note(deezer(&artist, &title).map(|f| {
            if let Some(f) = f {
                merge(c, &f, "Deezer")
            }
        }));
    }
    if o.itunes {
        note(itunes(&artist, &title).map(|f| {
            if let Some(f) = f {
                merge(c, &f, "iTunes")
            }
        }));
    }
    if !o.lastfm_key.trim().is_empty() {
        note(lastfm(o.lastfm_key.trim(), &artist, &title).map(|g| {
            if let Some(g) = g {
                let f = Found {
                    genre: g,
                    ..Default::default()
                };
                merge(c, &f, "Last.fm")
            }
        }));
    }
    if !o.discogs_token.trim().is_empty() {
        note(discogs(o.discogs_token.trim(), &artist, &title).map(|f| {
            if let Some(f) = f {
                merge(c, &f, "Discogs")
            }
        }));
    }
    if o.lyrics {
        let t = lrclib::Track {
            artist: artist.clone(),
            title: title.clone(),
            album: c.tags.album.clone(),
            duration,
        };
        note(lrclib::fetch(&t).map(|l| {
            if let lrclib::Fetched::Lyrics(l) = l {
                let synced = !super::auto_tagger::lrc_lines(&l).is_empty();
                c.tags.lyrics = l;
                c.sources.push((
                    "Lyrics",
                    if synced {
                        "LRCLIB synced"
                    } else {
                        "LRCLIB plain"
                    },
                ));
            }
        }));
    }
    notes
}

/// Source label of a review field of `c` (MusicBrainz unless an extra source filled it).
pub fn source_of(c: &Candidate, field: &str) -> &'static str {
    c.sources
        .iter()
        .rev()
        .find(|(f, _)| *f == field)
        .map(|(_, s)| *s)
        .unwrap_or(match c.source {
            "Deezer" | "iTunes" => c.source,
            _ => "MusicBrainz",
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::auto_tagger::Tags;
    use serde_json::json;

    fn cand() -> Candidate {
        Candidate {
            tags: Tags {
                title: "Song".into(),
                artist: "Band".into(),
                album: "MB Album".into(),
                ..Default::default()
            },
            release_group: String::new(),
            score: 90,
            source: "AcoustID",
            sources: Vec::new(),
            extra_covers: Vec::new(),
            alts: Vec::new(),
            agree: Vec::new(),
        }
    }

    #[test]
    fn deezer_parse() {
        let s = json!({"data": [
            {"id": 1, "title": "Other", "artist": {"name": "Band"}},
            {"id": 2, "title": "Song (Remastered)", "artist": {"name": "The band"}}
        ]});
        assert_eq!(parse_deezer_search(&s, "The Band", "Song"), Some(2));
        let t = json!({"track_position": 4, "release_date": "2001-02-03",
            "album": {"title": "Deezer Album", "cover_xl": "https://e-cdns/1000x1000.jpg"}});
        let f = parse_deezer_track(&t);
        assert_eq!((f.track.as_str(), f.date.as_str()), ("4", "2001-02-03"));
        assert!(f.cover.contains("1000x1000"));
    }

    #[test]
    fn itunes_parse() {
        let v = json!({"results": [{"artistName": "Band", "trackName": "Song",
            "collectionName": "It Album", "trackNumber": 7, "releaseDate": "1999-05-01T07:00:00Z",
            "primaryGenreName": "Rock", "artworkUrl100": "https://a/100x100bb.jpg"}]});
        let f = parse_itunes(&v, "band", "song").unwrap();
        assert_eq!(f.cover, "https://a/1400x1400bb.jpg");
        assert_eq!((f.date.as_str(), f.genre.as_str()), ("1999-05-01", "Rock"));
        assert!(parse_itunes(&v, "Other", "Song").is_none());
    }

    #[test]
    fn lastfm_and_discogs_parse() {
        let v = json!({"toptags": {"tag": [{"name": "seen live"}, {"name": "Band"}, {"name": "post-rock"}]}});
        assert_eq!(parse_lastfm_tags(&v, "Band").as_deref(), Some("Post-rock"));
        let d = json!({"results": [{"year": "1997", "genre": ["Rock"], "style": ["Shoegaze"],
            "cover_image": "https://i.discogs.com/x.jpg"}]});
        let f = parse_discogs(&d).unwrap();
        assert_eq!((f.date.as_str(), f.genre.as_str()), ("1997", "Shoegaze"));
    }

    #[test]
    fn merge_fills_only_empty_fields() {
        let mut c = cand();
        let f = Found {
            album: "Other".into(),
            track: "3".into(),
            genre: "Rock".into(),
            cover: "https://x/1400.jpg".into(),
            ..Default::default()
        };
        merge(&mut c, &f, "iTunes");
        merge(&mut c, &f, "Deezer");
        assert_eq!(c.tags.album, "MB Album");
        assert_eq!(c.tags.track, "3");
        assert_eq!(source_of(&c, "Track"), "iTunes");
        assert_eq!(source_of(&c, "Genre"), "iTunes");
        assert_eq!(source_of(&c, "Album"), "MusicBrainz");
        assert_eq!(c.extra_covers.len(), 1, "same cover once");
    }

    #[test]
    fn merge_keeps_alternatives_and_counts_agreement() {
        let mut c = cand();
        c.tags.date = "2001".into();
        c.score = 80;
        let deezer = Found {
            album: "MB album".into(), // same album, other case: agrees
            date: "2001-03-12".into(),
            ..Default::default()
        };
        let itunes = Found {
            album: "MB Album (Deluxe)".into(),
            genre: "Electronic".into(),
            ..Default::default()
        };
        let discogs = Found {
            album: "mb album (deluxe)".into(), // same alternative once
            genre: "House".into(),
            ..Default::default()
        };
        merge(&mut c, &deezer, "Deezer");
        merge(&mut c, &itunes, "iTunes");
        merge(&mut c, &discogs, "Discogs");
        assert_eq!(c.agree, vec!["Deezer"]);
        assert_eq!(c.confidence(), 84);
        assert_eq!(c.tags.genre, "Electronic");
        let alts: Vec<(&str, &str, &str)> = c
            .alts
            .iter()
            .map(|a| (a.field, a.value.as_str(), a.source))
            .collect();
        assert_eq!(
            alts,
            vec![
                ("Album", "MB Album (Deluxe)", "iTunes"),
                ("Genre", "House", "Discogs")
            ]
        );
        assert!(agrees("Track", "03/12", "3"));
        assert!(!agrees("Year", "1999", "2001"));
    }

    #[test]
    #[ignore = "network"]
    fn sources_live() {
        let mut c = cand();
        c.tags.artist = "Daft Punk".into();
        c.tags.title = "One More Time".into();
        c.tags.album.clear();
        let o = Opts {
            deezer: true,
            itunes: true,
            lyrics: true,
            ..Default::default()
        };
        let notes = enrich(&mut c, &o, 320.0);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(!c.tags.album.is_empty());
        assert!(c.extra_covers.len() >= 2);
        assert!(c.tags.lyrics.contains('['));
    }
}
