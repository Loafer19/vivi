use super::*;

fn ch(v: &[(&str, f64)]) -> Vec<(String, f64)> {
    v.iter().map(|(t, s)| (t.to_string(), *s)).collect()
}

#[test]
fn chapter_names() {
    for t in [
        "Intro",
        "Opening",
        "OP",
        "OP1",
        "Opening Credits",
        "Prologue / Intro",
    ] {
        assert_eq!(classify(t), Some(Part::Intro), "{t}");
    }
    for t in ["Credits", "Ending", "ED", "ED2", "End Credits", "Outro"] {
        assert_eq!(classify(t), Some(Part::Credits), "{t}");
    }
    for t in ["Chapter 1", "Opportunity", "Episode", "Edward", "Part B"] {
        assert_eq!(classify(t), None, "{t}");
    }
}

#[test]
fn end_markers_are_not_ranges() {
    for t in [
        "End of intro",
        "End of outro",
        "End of Opening",
        "End of muted sponsor",
        "Intro End",
        "Credits end",
        "end: intro",
        "Post-credits scene",
        "Mid-credits",
        "After the credits",
        "Opening scene",
    ] {
        assert_eq!(classify(t), None, "{t}");
    }
    assert!(is_end_marker("End of intro"));
    assert!(!is_end_marker("End Credits"));
    assert!(!is_end_marker("Ending"));
    // SponsorBlock 1.2 chapters on a file: the intro runs to "End of intro" only.
    let r = chapter_ranges(
        &ch(&[
            ("Chapter 1", 0.0),
            ("Intro", 5.0),
            ("End of intro", 35.0),
            ("Outro", 500.0),
            ("End of outro", 560.0),
        ]),
        600.0,
    );
    assert_eq!(r.len(), 2);
    assert_eq!(
        (r[0].start, r[0].end, r[0].kind.as_str()),
        (5.0, 35.0, "intro")
    );
    assert_eq!(
        (r[1].start, r[1].end, r[1].kind.as_str()),
        (500.0, 560.0, "credits")
    );
}

#[test]
fn ranges_from_chapters() {
    let r = chapter_ranges(
        &ch(&[
            ("Prologue", 0.0),
            ("Opening", 60.0),
            ("Part A", 150.0),
            ("Ending", 1300.0),
        ]),
        1420.0,
    );
    assert_eq!(r.len(), 2);
    assert_eq!(
        (r[0].start, r[0].end, r[0].kind.as_str()),
        (60.0, 150.0, "intro")
    );
    assert_eq!(
        (r[1].start, r[1].end, r[1].kind.as_str()),
        (1300.0, 1420.0, "credits")
    );
    assert_eq!(r[0].label, "intro");
    assert_eq!(r[0].source, CHAPTERS);
}

#[test]
fn submitted_ranges() {
    let r = parse_submitted(
        "sponsorblock",
        r#"[{"start":10,"end":40,"category":"sponsor","label":"sponsor","action":"skip"},
            {"start":50,"end":60,"kind":"selfpromo","label":"Skip self-promotion","action":"mute"},
            {"start":70,"end":80,"category":"music_offtopic","action":"ask"},
            {"start":90,"end":95,"action":"off"},
            {"start":9,"end":3},
            {"start":100,"end":110}]"#,
    )
    .unwrap();
    assert_eq!(r.len(), 4);
    assert_eq!(
        (r[0].act, r[0].wish, r[0].label.as_str()),
        (Act::Skip, Some(Mode::Skip), "sponsor")
    );
    assert_eq!(
        (r[1].act, r[1].label.as_str()),
        (Act::Mute, "self-promotion")
    );
    assert_eq!(
        (r[2].wish, r[2].label.as_str()),
        (Some(Mode::Ask), "music offtopic")
    );
    assert_eq!((r[3].kind.as_str(), r[3].wish), ("segment", None));
    assert_eq!(r[0].source, "sponsorblock");
    assert!(parse_submitted("x", "{}").is_err());
    assert!(parse_submitted("x", "[]").unwrap().is_empty());
}

#[test]
fn merging_shows_one_button_per_segment() {
    let sb = parse_submitted(
        "sponsorblock",
        r#"[{"start":61,"end":149,"category":"intro","label":"intro","action":"skip"},
            {"start":300,"end":330,"category":"sponsor","label":"sponsor"},
            {"start":300,"end":330,"category":"sponsor","label":"sponsor","action":"mute"}]"#,
    )
    .unwrap();
    let mut all = chapter_ranges(&ch(&[("Opening", 60.0), ("Part A", 150.0)]), 1400.0);
    all.extend(user_ranges("show = 20:00-end", "show", 1400.0));
    all.extend(user_ranges("show = 1:01-2:30", "show", 1400.0));
    all.extend(sb);
    let m = merge(all);
    // Opening (chapters), the SponsorBlock intro and the per-show 1:01-2:30 are one.
    let intros: Vec<&Range> = m.iter().filter(|r| r.start < 200.0).collect();
    assert_eq!(intros.len(), 1);
    assert_eq!(intros[0].source, SHOW);
    // Skip and mute of the same time are different things.
    assert_eq!(m.iter().filter(|r| r.start == 300.0).count(), 2);
    assert_eq!(m.last().unwrap().kind, "credits");
    // A short overlap is two segments.
    let a = Range::part(0.0, 100.0, CHAPTERS, Part::Intro);
    let b = Range::part(90.0, 200.0, SHOW, Part::Intro);
    assert_eq!(merge(vec![a, b]).len(), 2);
}

#[test]
fn sponsorblock_chapter_markers_ignored_when_submitted() {
    let sb = parse_submitted(
        "sponsorblock",
        r#"[{"start":5,"end":35,"category":"intro","action":"ask"}]"#,
    )
    .unwrap();
    let chapters = ch(&[
        ("Opening", 0.0),
        ("Intro", 5.0),
        ("End of intro", 35.0),
        ("Part A", 90.0),
    ]);
    let left = without_markers(&chapters, &sb);
    assert_eq!(left, ch(&[("Opening", 0.0), ("Part A", 90.0)]));
    let r = chapter_ranges(&left, 1000.0);
    assert_eq!((r[0].start, r[0].end), (0.0, 90.0));
}

#[test]
fn modes() {
    let m = parse_modes(
        "intro=skip, sponsorblock:filler=off; credits=ASK sponsorblock=ask bad=maybe =x",
    );
    assert_eq!(m.len(), 4);
    let intro = Range::part(0.0, 10.0, CHAPTERS, Part::Intro);
    let credits = Range::part(0.0, 10.0, SHOW, Part::Credits);
    assert_eq!(mode_for(&intro, &m, false, false), Mode::Skip);
    assert_eq!(mode_for(&credits, &m, false, true), Mode::Ask);
    assert_eq!(
        mode_for(&credits, &m, true, false),
        Mode::Skip,
        "remembered for the show"
    );
    let mut seg = parse_submitted(
        "sponsorblock",
        r#"[{"start":1,"end":5,"category":"filler","action":"skip"},
            {"start":6,"end":9,"category":"sponsor","action":"skip"},
            {"start":10,"end":19,"category":"intro","action":"ask"}]"#,
    )
    .unwrap();
    assert_eq!(
        mode_for(&seg[0], &m, false, false),
        Mode::Off,
        "source:kind"
    );
    assert_eq!(mode_for(&seg[1], &m, false, false), Mode::Ask, "source");
    assert_eq!(
        mode_for(&seg[2], &m, false, false),
        Mode::Skip,
        "kind before source"
    );
    assert_eq!(
        mode_for(&seg[1], &[], false, false),
        Mode::Skip,
        "the plugin's wish"
    );
    seg[1].wish = None;
    assert_eq!(mode_for(&seg[1], &[], false, true), Mode::Skip, "auto");
    assert_eq!(mode_for(&seg[1], &[], false, false), Mode::Ask);
}

#[test]
fn user_spec_roundtrip() {
    assert_eq!(parse_time("1:02:03"), Some(3723.0));
    assert_eq!(parse_time("90"), Some(90.0));
    assert_eq!(parse_time("x"), None);
    let v = parse_user("The Wand = 0:30-2:00, 21:10-end; bad; other=1-0");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].0, "the wand");
    assert_eq!(v[0].1, vec![(30.0, 120.0), (1270.0, f64::INFINITY)]);
    assert_eq!(format_user(&v), "the wand = 0:30-2:00, 21:10-end");
    let r = user_ranges(&format_user(&v), "the wand", 1400.0);
    assert_eq!(r[0].kind, "intro");
    assert_eq!(r[1].kind, "credits");
    assert_eq!(r[1].end, 1400.0);
    let a = parse_auto_shows("The Wand = intro, Credits; x = ; other=intro");
    assert_eq!(
        a,
        vec![
            (
                "the wand".to_string(),
                vec!["intro".to_string(), "credits".to_string()]
            ),
            ("other".to_string(), vec!["intro".to_string()]),
        ]
    );
    assert_eq!(
        format_auto_shows(&a),
        "the wand = intro, credits; other = intro"
    );
}
