//! Play history and smart playlists (#87).
//!
//! * **History.** The core records a play in `library.db` (`plays`) once a local file
//!   has been listened to / watched for half its length or 4 minutes, whichever comes
//!   first ([`Tracker`]). Seeks do not count as listening. History stays on this
//!   computer; Settings → Privacy → *Clear play history* empties it.
//! * **Smart playlists.** Rules over the history and the library tags, stored as JSON in
//!   `$XDG_DATA_HOME/vivi/smart.json` and evaluated as one SQL query ([`evaluate`]).
//!   *Similar to this track* is a temporary smart list seeded by a file ([`similar`]).

use crate::library::{self, Field, Media, MediaKind};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

const DAY: i64 = 86_400;

// ---------- history ----------

/// A play counts after this much listening, or half the file when that is shorter.
pub const COUNT_AFTER: f64 = 240.0;

/// Listening of the file that is playing (UI thread, fed by player events).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tracker {
    path: String,
    started_at: i64,
    duration: f64,
    last_pos: Option<f64>,
    listened: f64,
    /// Row id once the play is written.
    row: Option<i64>,
}

/// What the tracker wants written.
#[derive(Debug, Clone, PartialEq)]
pub enum Write {
    /// The play just started counting: insert it.
    Insert {
        path: String,
        started_at: i64,
        seconds: f64,
    },
    /// The file ended after counting: final listening time and whether it was finished.
    Finish {
        row: i64,
        seconds: f64,
        completed: bool,
    },
}

impl Tracker {
    /// A new file is loaded (`path` empty or not a local file: nothing is tracked).
    pub fn start(&mut self, path: &str, duration: f64, now: i64) {
        *self = Self {
            path: path.into(),
            started_at: now,
            duration,
            ..Default::default()
        };
    }

    pub fn duration(&mut self, d: f64) {
        self.duration = d;
    }

    /// Listening needed before the play counts.
    fn threshold(&self) -> f64 {
        if self.duration > 0.0 {
            (self.duration * 0.5).min(COUNT_AFTER)
        } else {
            COUNT_AFTER
        }
    }

    /// The position moved to `t`. Steps of up to 2 s count as listening (time-pos
    /// arrives several times a second); larger jumps are seeks. Returns the insert once
    /// the threshold is reached.
    pub fn position(&mut self, t: f64) -> Option<Write> {
        if self.path.is_empty() {
            return None;
        }
        if let Some(prev) = self.last_pos {
            let d = t - prev;
            if d > 0.0 && d <= 2.0 {
                self.listened += d;
            }
        }
        self.last_pos = Some(t);
        if self.row.is_none() && self.listened >= self.threshold() {
            // Marked as written; the caller stores the real row id with `written`.
            self.row = Some(-1);
            return Some(Write::Insert {
                path: self.path.clone(),
                started_at: self.started_at,
                seconds: self.listened,
            });
        }
        None
    }

    /// The insert of [`Tracker::position`] got row `id`.
    pub fn written(&mut self, id: i64) {
        self.row = Some(id);
    }

    /// The file ended (or another one starts): the final update of a counted play.
    pub fn end(&mut self) -> Option<Write> {
        let row = self.row.filter(|r| *r >= 0)?;
        let w = Write::Finish {
            row,
            seconds: self.listened,
            completed: self.duration > 0.0 && self.listened >= self.duration * 0.9,
        };
        self.path.clear();
        self.row = None;
        Some(w)
    }
}

/// Apply a tracker write; returns the new row id for an insert.
pub fn apply(conn: &Connection, w: &Write) -> rusqlite::Result<Option<i64>> {
    match w {
        Write::Insert {
            path,
            started_at,
            seconds,
        } => {
            conn.execute(
                "INSERT INTO plays(path, started_at, seconds, completed) VALUES(?1, ?2, ?3, 0)",
                rusqlite::params![path, started_at, seconds],
            )?;
            Ok(Some(conn.last_insert_rowid()))
        }
        Write::Finish {
            row,
            seconds,
            completed,
        } => {
            conn.execute(
                "UPDATE plays SET seconds = ?2, completed = ?3 WHERE rowid = ?1",
                rusqlite::params![row, seconds, *completed as i64],
            )?;
            Ok(None)
        }
    }
}

/// Paths played lately, newest first, each once.
pub fn recent_paths(conn: &Connection, limit: usize) -> Vec<String> {
    let Ok(mut st) =
        conn.prepare("SELECT path FROM plays GROUP BY path ORDER BY MAX(started_at) DESC LIMIT ?1")
    else {
        return Vec::new();
    };
    st.query_map([limit as i64], |r| r.get(0))
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Number of recorded plays.
pub fn play_count(conn: &Connection) -> usize {
    conn.query_row("SELECT COUNT(*) FROM plays", [], |r| r.get::<_, i64>(0))
        .map_or(0, |n| n as usize)
}

/// Forget the whole play history (Settings → Privacy). Returns the plays removed.
pub fn clear_history(conn: &Connection) -> usize {
    conn.execute("DELETE FROM plays", []).unwrap_or(0)
}

// ---------- rules ----------

/// One condition of a smart playlist; a list matches files that meet all of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum Rule {
    /// Played before, but not in the last `days` days.
    NotPlayedFor { days: u32 },
    /// Never played.
    NeverPlayed,
    /// Played at least `times` times (ever).
    PlayedAtLeast { times: u32 },
    /// Played in the last `days` days (with [`Order::TopRecent`]: "top this week").
    PlayedWithin { days: u32 },
    /// A tag equals `value` (any case); `value` "Unknown" matches untagged files.
    Tag { field: Field, value: String },
    /// Length between `min` and `max` seconds (0 = no bound).
    Length {
        #[serde(default)]
        min: u32,
        #[serde(default)]
        max: u32,
    },
    /// Only songs or only videos.
    Kind { kind: String },
    /// Shares the artist, album or genre of the file at `path` (not the file itself).
    SimilarTo { path: String },
}

/// Order of a smart playlist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    #[default]
    MostPlayed,
    /// Most plays inside the [`Rule::PlayedWithin`] window.
    TopRecent,
    RecentlyPlayed,
    LeastRecentlyPlayed,
    Title,
    Newest,
    Random,
    /// Closest first to the [`Rule::SimilarTo`] seed (same artist, then album, then genre).
    Similarity,
}

/// A saved smart playlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmartList {
    pub id: String,
    pub name: String,
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub order: Order,
    /// At most this many items (0 = no limit).
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Shown on Home.
    #[serde(default)]
    pub home: bool,
}

fn default_limit() -> u32 {
    100
}

/// The lists a new install starts with (and that come back when smart.json is gone).
pub fn defaults() -> Vec<SmartList> {
    let audio = || Rule::Kind {
        kind: "audio".into(),
    };
    vec![
        SmartList {
            id: "top-week".into(),
            name: "Top this week".into(),
            rules: vec![audio(), Rule::PlayedWithin { days: 7 }],
            order: Order::TopRecent,
            limit: 50,
            home: true,
        },
        SmartList {
            id: "most-played".into(),
            name: "Most played".into(),
            rules: vec![audio(), Rule::PlayedAtLeast { times: 2 }],
            order: Order::MostPlayed,
            limit: 100,
            home: true,
        },
        SmartList {
            id: "forgotten".into(),
            name: "Not played for a month".into(),
            rules: vec![audio(), Rule::NotPlayedFor { days: 30 }],
            order: Order::LeastRecentlyPlayed,
            limit: 100,
            home: true,
        },
        SmartList {
            id: "never-played".into(),
            name: "Never played".into(),
            rules: vec![audio(), Rule::NeverPlayed],
            order: Order::Random,
            limit: 100,
            home: false,
        },
    ]
}

pub fn path() -> std::path::PathBuf {
    crate::store::data_dir().join("smart.json")
}

/// The saved smart playlists ([`defaults`] when there is no file yet).
pub fn load() -> Vec<SmartList> {
    match std::fs::read_to_string(path()) {
        Ok(s) => parse(&s),
        Err(_) => defaults(),
    }
}

/// Lists from smart.json text; a broken file logs and falls back to the defaults (it is
/// not overwritten until the user saves a list again).
pub fn parse(s: &str) -> Vec<SmartList> {
    #[derive(Deserialize)]
    struct File {
        lists: Vec<SmartList>,
    }
    match serde_json::from_str::<File>(s) {
        Ok(f) => f.lists,
        Err(e) => {
            eprintln!("vivi: smart.json: {e}; using the default smart playlists");
            defaults()
        }
    }
}

pub fn save(lists: &[SmartList]) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(&serde_json::json!({ "version": 1, "lists": lists }))
        .map_err(std::io::Error::other)?;
    crate::store::write_atomic(&path(), &text)
}

/// The SQL of a list: the query text and its string parameters.
fn sql(conn: &Connection, l: &SmartList, now: i64) -> (String, Vec<String>) {
    let mut w: Vec<String> = Vec::new();
    let mut p: Vec<String> = Vec::new();
    let mut window = None;
    let mut seed: Option<Media> = None;
    for r in &l.rules {
        match r {
            Rule::NotPlayedFor { days } => {
                w.push(format!("a.last < {}", now - *days as i64 * DAY));
            }
            Rule::NeverPlayed => w.push("a.n IS NULL".into()),
            Rule::PlayedAtLeast { times } => w.push(format!("IFNULL(a.n, 0) >= {times}")),
            Rule::PlayedWithin { days } => {
                let from = now - *days as i64 * DAY;
                window = Some(window.map_or(from, |f: i64| f.max(from)));
                w.push("r.n > 0".into());
            }
            Rule::Tag { field, value } => {
                p.push(value.clone());
                let col = field.col();
                if value == library::UNKNOWN {
                    w.push(format!("IFNULL(m.{col}, '') IN ('', ?{})", p.len()));
                } else {
                    w.push(format!("lower(m.{col}) = lower(?{})", p.len()));
                }
            }
            Rule::Length { min, max } => {
                if *min > 0 {
                    w.push(format!("m.duration >= {min}"));
                }
                if *max > 0 {
                    w.push(format!("m.duration <= {max}"));
                }
            }
            Rule::Kind { kind } => {
                let k = MediaKind::parse(kind).unwrap_or_default();
                w.push(format!("m.kind = '{}'", k.as_str()));
            }
            Rule::SimilarTo { path } => {
                let m = library::by_path(conn, path);
                p.push(path.clone());
                w.push(format!("m.path <> ?{}", p.len()));
                let mut any = Vec::new();
                if let Some(m) = &m {
                    for (col, v) in [
                        ("artist", &m.artist),
                        ("album", &m.album),
                        ("genre", &m.genre),
                    ] {
                        if !v.is_empty() {
                            p.push(v.clone());
                            any.push(format!("lower(m.{col}) = lower(?{})", p.len()));
                        }
                    }
                }
                // A seed without tags (or outside the library) has no similar files.
                w.push(if any.is_empty() {
                    "0".into()
                } else {
                    format!("({})", any.join(" OR "))
                });
                seed = m;
            }
        }
    }
    let recent_join = match window {
        Some(from) => format!(
            " LEFT JOIN (SELECT path, COUNT(*) AS n FROM plays WHERE started_at >= {from}
              GROUP BY path) r ON r.path = m.path"
        ),
        None => " LEFT JOIN (SELECT NULL AS path, 0 AS n) r ON 0".into(),
    };
    let order = match l.order {
        Order::MostPlayed => "IFNULL(a.n, 0) DESC, a.last DESC, m.nt".to_string(),
        Order::TopRecent => "IFNULL(r.n, 0) DESC, a.last DESC, m.nt".into(),
        Order::RecentlyPlayed => "a.last IS NULL, a.last DESC, m.nt".into(),
        Order::LeastRecentlyPlayed => "a.last IS NULL, a.last, m.nt".into(),
        Order::Title => "m.nt, m.np".into(),
        Order::Newest => "m.mtime DESC, m.nt".into(),
        Order::Random => "random()".into(),
        Order::Similarity => {
            let mut score = Vec::new();
            if let Some(s) = &seed {
                for (col, v, pts) in [
                    ("artist", &s.artist, 4),
                    ("album", &s.album, 2),
                    ("genre", &s.genre, 1),
                ] {
                    if !v.is_empty() {
                        p.push(v.clone());
                        score.push(format!(
                            "(CASE WHEN lower(m.{col}) = lower(?{}) THEN {pts} ELSE 0 END)",
                            p.len()
                        ));
                    }
                }
            }
            if score.is_empty() {
                "random()".into()
            } else {
                format!("{} DESC, random()", score.join(" + "))
            }
        }
    };
    let cols = library::COLS
        .split(',')
        .map(|c| format!("m.{c}"))
        .collect::<Vec<_>>()
        .join(",");
    let clause = if w.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", w.join(" AND "))
    };
    let limit = if l.limit == 0 { -1 } else { l.limit as i64 };
    (
        format!(
            "SELECT {cols} FROM media m
             LEFT JOIN (SELECT path, COUNT(*) AS n, MAX(started_at) AS last FROM plays
                        GROUP BY path) a ON a.path = m.path{recent_join}{clause}
             ORDER BY {order} LIMIT {limit}"
        ),
        p,
    )
}

/// The files of smart playlist `l` now (library files only).
pub fn evaluate(conn: &Connection, l: &SmartList, now: i64) -> Vec<Media> {
    let (q, p) = sql(conn, l, now);
    library::select(conn, &q, &p)
}

/// *Similar to this track*: a temporary list seeded by `path`.
pub fn similar(path: &str) -> SmartList {
    SmartList {
        id: format!("similar:{path}"),
        name: format!("Similar to {}", crate::store::display_name(path)),
        rules: vec![Rule::SimilarTo { path: path.into() }],
        order: Order::Similarity,
        limit: 50,
        home: false,
    }
}

/// A short line describing a list's rules ("Songs · played in the last 7 days").
pub fn describe(l: &SmartList) -> String {
    let parts: Vec<String> = l
        .rules
        .iter()
        .map(|r| match r {
            Rule::NotPlayedFor { days } => format!("not played for {days} days"),
            Rule::NeverPlayed => "never played".into(),
            Rule::PlayedAtLeast { times } => format!("played {times}+ times"),
            Rule::PlayedWithin { days: 7 } => "played this week".into(),
            Rule::PlayedWithin { days: 30 } => "played this month".into(),
            Rule::PlayedWithin { days } => format!("played in the last {days} days"),
            Rule::Tag { field, value } => format!("{}: {value}", field.col()),
            Rule::Length { min, max } => match (min, max) {
                (0, 0) => "any length".into(),
                (m, 0) => format!("longer than {}", crate::util::clock(*m as i64)),
                (0, x) => format!("shorter than {}", crate::util::clock(*x as i64)),
                (m, x) => format!(
                    "{}–{}",
                    crate::util::clock(*m as i64),
                    crate::util::clock(*x as i64)
                ),
            },
            Rule::Kind { kind } if kind == "audio" => "songs".into(),
            Rule::Kind { .. } => "videos".into(),
            Rule::SimilarTo { path } => format!("like {}", crate::store::display_name(path)),
        })
        .collect();
    let mut s = parts.join(" · ");
    if let Some(c) = s.get(..1) {
        s = c.to_uppercase() + &s[1..];
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let dir = std::env::temp_dir().join(format!(
            "vivi-smart-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let c = library::open_db(&dir.join("library.db")).unwrap();
        for (i, (p, artist, album, genre)) in [
            ("/a/1.mp3", "Band", "LP", "Rock"),
            ("/a/2.mp3", "Band", "EP", "Rock"),
            ("/a/3.mp3", "Other", "X", "Rock"),
            ("/a/4.mp3", "Solo", "Y", "Jazz"),
            ("/v/5.mkv", "", "", ""),
        ]
        .into_iter()
        .enumerate()
        {
            let kind = if p.ends_with(".mkv") {
                "video"
            } else {
                "audio"
            };
            c.execute(
                "INSERT INTO media(path,kind,title,artist,album,genre,track,duration,size,mtime,
                                   dir,nt,nar,nal,np,find)
                 VALUES(?1,?2,?3,?4,?5,?6,0,300,1,?7,'',?3,'','',?1,'')",
                rusqlite::params![p, kind, format!("t{i}"), artist, album, genre, i as i64],
            )
            .unwrap();
        }
        c
    }

    fn play(c: &Connection, p: &str, at: i64) {
        apply(
            c,
            &Write::Insert {
                path: p.into(),
                started_at: at,
                seconds: 200.0,
            },
        )
        .unwrap();
    }

    fn paths(v: Vec<Media>) -> Vec<String> {
        v.into_iter().map(|m| m.path).collect()
    }

    #[test]
    fn tracker_counts_listening_not_seeks() {
        let mut t = Tracker::default();
        t.start("/a/1.mp3", 100.0, 1000);
        // A seek to the middle is not listening.
        assert_eq!(t.position(0.0), None);
        assert_eq!(t.position(60.0), None);
        let mut hit = None;
        for i in 1..=200 {
            if let Some(w) = t.position(60.0 + i as f64 * 0.5) {
                hit = Some((i, w));
                break;
            }
        }
        // Half of a 100 s file = 50 s of listening = 100 steps of 0.5 s.
        let (i, w) = hit.unwrap();
        assert_eq!(i, 100);
        assert!(matches!(
            w,
            Write::Insert {
                started_at: 1000,
                ..
            }
        ));
        t.written(7);
        assert_eq!(t.position(200.0), None, "written once");
        assert!(matches!(
            t.end(),
            Some(Write::Finish {
                row: 7,
                completed: false,
                ..
            })
        ));
        assert_eq!(t.end(), None);
        // Long files count after 4 minutes; nothing is tracked without a path.
        let mut t = Tracker::default();
        t.start("/v/long.mkv", 7200.0, 0);
        assert_eq!(t.threshold(), COUNT_AFTER);
        let mut t = Tracker::default();
        assert_eq!(t.position(1.0), None);
    }

    #[test]
    fn rules_run_in_sql() {
        let c = db();
        let now = 100 * DAY;
        play(&c, "/a/1.mp3", now - DAY);
        play(&c, "/a/1.mp3", now - 2 * DAY);
        play(&c, "/a/2.mp3", now - 40 * DAY);
        play(&c, "/a/3.mp3", now - 3 * DAY);
        let l = |rules, order| SmartList {
            id: "t".into(),
            name: "T".into(),
            rules,
            order,
            limit: 0,
            home: false,
        };
        let audio = Rule::Kind {
            kind: "audio".into(),
        };
        let top = l(
            vec![audio.clone(), Rule::PlayedWithin { days: 7 }],
            Order::TopRecent,
        );
        assert_eq!(paths(evaluate(&c, &top, now)), ["/a/1.mp3", "/a/3.mp3"]);
        let old = l(
            vec![audio.clone(), Rule::NotPlayedFor { days: 30 }],
            Order::LeastRecentlyPlayed,
        );
        assert_eq!(paths(evaluate(&c, &old, now)), ["/a/2.mp3"]);
        let never = l(vec![audio.clone(), Rule::NeverPlayed], Order::Title);
        assert_eq!(paths(evaluate(&c, &never, now)), ["/a/4.mp3"]);
        let most = l(vec![Rule::PlayedAtLeast { times: 2 }], Order::MostPlayed);
        assert_eq!(paths(evaluate(&c, &most, now)), ["/a/1.mp3"]);
        let rock = l(
            vec![Rule::Tag {
                field: Field::Genre,
                value: "rock".into(),
            }],
            Order::Title,
        );
        assert_eq!(evaluate(&c, &rock, now).len(), 3);
        let video = l(
            vec![Rule::Kind {
                kind: "video".into(),
            }],
            Order::Title,
        );
        assert_eq!(paths(evaluate(&c, &video, now)), ["/v/5.mkv"]);
        // Similar: same artist first, then the same genre; never the seed.
        assert_eq!(
            paths(evaluate(&c, &similar("/a/1.mp3"), now)),
            ["/a/2.mp3", "/a/3.mp3"]
        );
        assert!(evaluate(&c, &similar("/v/5.mkv"), now).is_empty());
        assert_eq!(recent_paths(&c, 10), ["/a/1.mp3", "/a/3.mp3", "/a/2.mp3"]);
        assert_eq!(clear_history(&c), 4);
        assert_eq!(play_count(&c), 0);
    }

    #[test]
    fn smart_json_round_trip_and_fallback() {
        let lists = defaults();
        let text =
            serde_json::to_string(&serde_json::json!({ "version": 1, "lists": lists })).unwrap();
        assert_eq!(parse(&text), lists);
        assert_eq!(parse("{ not json"), defaults());
        let one = r#"{"lists":[{"id":"a","name":"A","rules":[{"rule":"tag","field":"artist","value":"X"},{"rule":"length","max":300}]}]}"#;
        let l = &parse(one)[0];
        assert_eq!(l.limit, 100);
        assert_eq!(l.order, Order::MostPlayed);
        assert_eq!(describe(l), "Artist: X · shorter than 5:00");
    }
}
