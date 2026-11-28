//! Library search matcher (#117): one scorer for the Home / Videos / Music search, the docked
//! library, and the `search` MCP tool. It is registered in SQLite as `vmatch(text, query)`
//! (see `library.rs`), so paging, counting and grouping keep running in SQL.
//!
//! - **Normalisation**: both sides are case-folded and stripped of diacritics (`é` → `e`,
//!   `й` → `и`, `ї` → `і`, `ґ` → `г`, `ß` → `ss`, apostrophes dropped). The library keeps the
//!   normalised text in its `find` column, so a search never normalises the library again.
//! - **Words**: every word of the query must match (AND), like the old `LIKE` search.
//! - **Match kinds, best first**: exact substring (whole word > word start > word end >
//!   inside), fuzzy (the letters in order within a short span: `sntl` → Sintel), typo
//!   (Damerau-Levenshtein: 1 edit for words of 4-7 letters, 2 for 8+; a typo may also hit
//!   the start of a longer word while typing).
//! - **Layouts**: when the query is all Cyrillic or all Latin it is also tried as typed on
//!   the other keyboard layout (UA ЙЦУКЕН ↔ QWERTY, so `ghbdsn` finds `Привіт`) and
//!   transliterated (`сінтел` ↔ `sintel`). Those matches rank below every direct match.
//! - **Fields**: the text is `title\nartist\nalbum\ngenre\npath`; title matches rank above
//!   artist / album above genre; fuzzy and typo matching skips the path.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Match kinds, worst to best (see [`Matcher::score`]).
const TYPO: i64 = 1;
const FUZZY: i64 = 2;
const EXACT: i64 = 3;
/// One step between kinds; a word's bonus (field, position, closeness) stays below it.
const STEP: i64 = 200;
/// Base of a direct match and of a layout / transliteration match.
const DIRECT: i64 = 10_000;
const ALT: i64 = 1_000;
/// Bonus per field line (title, artist, album, genre, path).
const FIELD: [i64; 5] = [60, 30, 30, 10, 0];
/// Lines that take fuzzy and typo matches (not the path).
const SOFT_LINES: usize = 4;
/// Longest word, in letters, that takes part in typo matching.
const MAX_TYPO_LEN: usize = 48;

/// `(letters, replacement)`: letters that fold to a base letter or a few.
const FOLDS: &[(&str, &str)] = &[
    ("àáâãäåāăąǎǟǡǻȁȃȧḁạảấầẩẫậắằẳẵặ", "a"),
    ("æǣǽ", "ae"),
    ("çćĉċčḉ", "c"),
    ("ďđḋḍḏḑḓ", "d"),
    ("èéêëēĕėęěȅȇȩḕḗḙḛḝẹẻẽếềểễệ", "e"),
    ("ƒḟ", "f"),
    ("ĝğġģǧǵḡ", "g"),
    ("ĥħḣḥḧḩḫ", "h"),
    ("ìíîïĩīĭįǐȉȋḭḯỉị", "i"),
    ("ĵǰ", "j"),
    ("ķǩḱḳḵ", "k"),
    ("ĺļľŀłḷḹḻḽ", "l"),
    ("ḿṁṃ", "m"),
    ("ñńņňŉǹṅṇṉṋ", "n"),
    ("òóôõöøōŏőơǒǫǭȍȏȫȭȯȱṍṏṑṓọỏốồổỗộớờởỡợ", "o"),
    ("œ", "oe"),
    ("ṕṗ", "p"),
    ("ŕŗřȑȓṙṛṝṟ", "r"),
    ("śŝşšșṡṣṥṧṩ", "s"),
    ("ß", "ss"),
    ("ţťŧțṫṭṯṱ", "t"),
    ("ùúûüũūŭůűųưǔǖǘǚǜȕȗṳṵṷṹṻụủứừửữự", "u"),
    ("ṽṿ", "v"),
    ("ŵẁẃẅẇẉ", "w"),
    ("ẋẍ", "x"),
    ("ýÿŷẏỳỵỷỹ", "y"),
    ("źżžẑẓẕ", "z"),
    ("ı", "i"),
    ("ﬁ", "fi"),
    ("ﬂ", "fl"),
    // Cyrillic letters that are a base letter plus a mark (NFKD), and ґ.
    ("йѝ", "и"),
    ("ёѐ", "е"),
    ("ї", "і"),
    ("ў", "у"),
    ("ґ", "г"),
];

fn fold_table() -> &'static HashMap<char, &'static str> {
    static T: OnceLock<HashMap<char, &'static str>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = HashMap::new();
        for (from, to) in FOLDS {
            for c in from.chars() {
                m.insert(c, *to);
            }
        }
        m
    })
}

/// `s` in lower case without diacritics, apostrophes or combining marks: the form the
/// library stores in its `find` column and every query is folded to.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            if !matches!(c, '\'' | '`') {
                out.push(c.to_ascii_lowercase());
            }
            continue;
        }
        match c {
            // Apostrophes, combining marks, zero-width characters.
            '\u{2019}' | '\u{2018}' | '\u{02BC}' | '\u{0300}'..='\u{036F}' | '\u{200B}'..='\u{200D}' => {}
            // Full-width ASCII (NFKD folds it to plain ASCII).
            '\u{FF01}'..='\u{FF5E}' => {
                out.push(((c as u32 - 0xFF01 + 0x21) as u8).to_ascii_lowercase() as char);
            }
            _ => {
                for l in c.to_lowercase() {
                    match fold_table().get(&l) {
                        Some(r) => out.push_str(r),
                        None => out.push(l),
                    }
                }
            }
        }
    }
    out
}

/// The text the library searches in: the fields on their own lines, folded.
pub fn search_text(title: &str, artist: &str, album: &str, genre: &str, path: &str) -> String {
    fold(&format!("{title}\n{artist}\n{album}\n{genre}\n{path}"))
}

// ---- keyboard layout and transliteration -----------------------------------------------

/// UA layout: (QWERTY key, Cyrillic letter), lower case. The Russian extras (ы э ъ ё) are
/// only used from Cyrillic to Latin.
const LAYOUT: &[(char, char)] = &[
    ('q', 'й'),
    ('w', 'ц'),
    ('e', 'у'),
    ('r', 'к'),
    ('t', 'е'),
    ('y', 'н'),
    ('u', 'г'),
    ('i', 'ш'),
    ('o', 'щ'),
    ('p', 'з'),
    ('[', 'х'),
    (']', 'ї'),
    ('a', 'ф'),
    ('s', 'і'),
    ('d', 'в'),
    ('f', 'а'),
    ('g', 'п'),
    ('h', 'р'),
    ('j', 'о'),
    ('k', 'л'),
    ('l', 'д'),
    (';', 'ж'),
    ('\'', 'є'),
    ('\\', 'ґ'),
    ('z', 'я'),
    ('x', 'ч'),
    ('c', 'с'),
    ('v', 'м'),
    ('b', 'и'),
    ('n', 'т'),
    ('m', 'ь'),
    (',', 'б'),
    ('.', 'ю'),
];
const LAYOUT_RU: &[(char, char)] = &[('s', 'ы'), ('\'', 'э'), (']', 'ъ'), ('`', 'ё')];

fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

fn layout_to_cyrillic(s: &str) -> String {
    s.chars()
        .map(|c| LAYOUT.iter().find(|(k, _)| *k == c).map_or(c, |(_, v)| *v))
        .collect()
}

fn layout_to_latin(s: &str) -> String {
    s.chars()
        .map(|c| {
            LAYOUT
                .iter()
                .chain(LAYOUT_RU)
                .find(|(_, v)| *v == c)
                .map_or(c, |(k, _)| *k)
        })
        .collect()
}

/// Cyrillic → Latin. `alt` picks the second common spelling (г → g, и → i, х → h, ц → c).
fn translit_to_latin(s: &str, alt: bool) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let r: &str = match c {
            'а' => "a",
            'б' => "b",
            'в' => "v",
            'г' if alt => "g",
            'г' => "h",
            'ґ' => "g",
            'д' => "d",
            'е' => "e",
            'є' if alt => "e",
            'є' => "ye",
            'ж' if alt => "j",
            'ж' => "zh",
            'з' => "z",
            'и' if alt => "i",
            'и' => "y",
            'і' => "i",
            'ї' if alt => "i",
            'ї' => "yi",
            'й' if alt => "i",
            'й' => "y",
            'к' => "k",
            'л' => "l",
            'м' => "m",
            'н' => "n",
            'о' => "o",
            'п' => "p",
            'р' => "r",
            'с' => "s",
            'т' => "t",
            'у' => "u",
            'ф' => "f",
            'х' if alt => "h",
            'х' => "kh",
            'ц' if alt => "c",
            'ц' => "ts",
            'ч' => "ch",
            'ш' => "sh",
            'щ' if alt => "sch",
            'щ' => "shch",
            'ю' => "yu",
            'я' => "ya",
            'ы' => "y",
            'э' => "e",
            'ё' => "yo",
            'ь' | 'ъ' | '\'' | '’' | 'ʼ' => "",
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push_str(r);
    }
    out
}

/// Latin → Cyrillic. `alt` reads i as и, h as г and c as ц (the first reading is the
/// Ukrainian one: i → і, h → х, c → к).
fn translit_to_cyrillic(s: &str, alt: bool) -> String {
    const DIGRAPHS: &[(&str, &str)] = &[
        ("shch", "щ"),
        ("sch", "щ"),
        ("sh", "ш"),
        ("ch", "ч"),
        ("zh", "ж"),
        ("kh", "х"),
        ("ts", "ц"),
        ("yo", "йо"),
        ("ya", "я"),
        ("yu", "ю"),
        ("ye", "є"),
        ("yi", "ї"),
        ("ja", "я"),
        ("ju", "ю"),
    ];
    let mut out = String::new();
    let mut rest = s;
    'outer: while let Some(c) = rest.chars().next() {
        for (p, r) in DIGRAPHS {
            if rest.starts_with(p) {
                out.push_str(r);
                rest = &rest[p.len()..];
                continue 'outer;
            }
        }
        let r: &str = match c {
            'a' => "а",
            'b' => "б",
            'c' if alt => "ц",
            'c' => "к",
            'd' => "д",
            'e' => "е",
            'f' => "ф",
            'g' => "г",
            'h' if alt => "г",
            'h' => "х",
            'i' if alt => "и",
            'i' => "і",
            'j' => "й",
            'k' => "к",
            'l' => "л",
            'm' => "м",
            'n' => "н",
            'o' => "о",
            'p' => "п",
            'q' => "к",
            'r' => "р",
            's' => "с",
            't' => "т",
            'u' => "у",
            'v' | 'w' => "в",
            'x' => "кс",
            'y' if alt => "й",
            'y' => "и",
            'z' => "з",
            _ => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
                continue;
            }
        };
        out.push_str(r);
        rest = &rest[c.len_utf8()..];
    }
    out
}

// ---- matching ---------------------------------------------------------------------------

struct Word {
    text: String,
    chars: Vec<char>,
    bag: u64,
    /// Edits a typo may cost: 0 below 4 letters, 1 for 4-7, 2 from 8.
    typo: usize,
}

impl Word {
    fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let typo = match chars.len() {
            0..=3 => 0,
            4..=7 => 1,
            _ if chars.len() <= MAX_TYPO_LEN => 2,
            _ => 0,
        };
        Self {
            bag: bag(&chars),
            text: text.to_string(),
            chars,
            typo,
        }
    }
}

fn bag(chars: &[char]) -> u64 {
    chars.iter().fold(0, |b, c| b | 1 << (*c as u32 % 64))
}

struct Variant {
    base: i64,
    words: Vec<Word>,
}

/// A compiled search query; [`Matcher::score`] rates one library text.
pub struct Matcher {
    variants: Vec<Variant>,
}

impl Matcher {
    /// `None` when the query has no words (everything matches).
    pub fn new(query: &str) -> Option<Self> {
        let lower = query.trim().to_lowercase();
        if lower.is_empty() {
            return None;
        }
        let cyr = lower.chars().any(is_cyrillic);
        let lat = lower.chars().any(|c| c.is_ascii_alphabetic());
        let mut raw = vec![lower.clone()];
        if cyr && !lat {
            raw.push(layout_to_latin(&lower));
            raw.push(translit_to_latin(&lower, false));
            raw.push(translit_to_latin(&lower, true));
        } else if lat && !cyr {
            raw.push(layout_to_cyrillic(&lower));
            raw.push(translit_to_cyrillic(&lower, false));
            raw.push(translit_to_cyrillic(&lower, true));
        }
        let mut seen: Vec<String> = Vec::new();
        let mut variants = Vec::new();
        for r in raw {
            let f = fold(&r);
            if f.split_whitespace().next().is_none() || seen.contains(&f) {
                continue;
            }
            variants.push(Variant {
                base: if seen.is_empty() { DIRECT } else { ALT },
                words: f.split_whitespace().map(Word::new).collect(),
            });
            seen.push(f);
        }
        (!variants.is_empty()).then_some(Self { variants })
    }

    /// 0 = no match; larger is better. `text` is a folded library text (see
    /// [`search_text`]). Direct matches (exact > fuzzy > typo) always outrank layout and
    /// transliteration matches.
    pub fn score(&self, text: &str) -> i64 {
        let mut lines = Lines {
            text: [""; 5],
            bag: [0; 5],
        };
        for (i, l) in text.split('\n').take(5).enumerate() {
            lines.text[i] = l;
            if i < SOFT_LINES {
                lines.bag[i] = l.chars().fold(0, |b, c| b | 1 << (c as u32 % 64));
            }
        }
        for v in &self.variants {
            let (mut kind, mut bonus) = (EXACT, 0);
            let mut ok = true;
            for w in &v.words {
                match score_word(w, &lines) {
                    Some((k, b)) => {
                        kind = kind.min(k);
                        bonus += b;
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                return v.base + kind * STEP + bonus / v.words.len() as i64;
            }
        }
        0
    }
}

/// The five lines of a library text, and a hash of the letters each of the first four has
/// (a cheap way to rule out fuzzy and typo matches).
struct Lines<'a> {
    text: [&'a str; 5],
    bag: [u64; 5],
}

fn score_word(w: &Word, lines: &Lines) -> Option<(i64, i64)> {
    if let Some(b) = exact(w, lines) {
        return Some((EXACT, b));
    }
    if w.chars.len() >= 3 {
        if let Some(b) = fuzzy(w, lines) {
            return Some((FUZZY, b));
        }
    }
    if w.typo > 0 {
        if let Some(b) = typo(w, lines) {
            return Some((TYPO, b));
        }
    }
    None
}

fn exact(w: &Word, lines: &Lines) -> Option<i64> {
    let mut best: Option<i64> = None;
    for (li, line) in lines.text.iter().enumerate() {
        for (i, _) in line.match_indices(&w.text) {
            let before = line[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
            let after = line[i + w.text.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric());
            let b = 10 + if before { 25 } else { 0 } + if after { 15 } else { 0 } + FIELD[li];
            best = Some(best.map_or(b, |x| x.max(b)));
        }
    }
    best
}

/// The letters of `w` in order, within a short span of one field.
fn fuzzy(w: &Word, lines: &Lines) -> Option<i64> {
    let n = w.chars.len();
    let limit = n * 3 / 2 + 2;
    let mut best: Option<i64> = None;
    for (li, line) in lines.text.iter().take(SOFT_LINES).enumerate() {
        if w.bag & !lines.bag[li] != 0 {
            continue;
        }
        let mut prev_alnum = false;
        for (bi, c) in line.char_indices() {
            let boundary = !prev_alnum;
            prev_alnum = c.is_alphanumeric();
            if c != w.chars[0] {
                continue;
            }
            let (mut wi, mut span) = (1, 1);
            for c2 in line[bi + c.len_utf8()..].chars() {
                if wi == n {
                    break;
                }
                span += 1;
                if span > limit {
                    break;
                }
                if c2 == w.chars[wi] {
                    wi += 1;
                }
            }
            if wi == n && span <= limit {
                let gaps = (span - n) as i64;
                let b = 30 - (gaps * 3).min(30) + if boundary { 15 } else { 0 } + FIELD[li];
                best = Some(best.map_or(b, |x| x.max(b)));
            }
        }
    }
    best
}

/// A word of the text within `w.typo` edits of `w` (or of the same-length start of a longer word).
fn typo(w: &Word, lines: &Lines) -> Option<i64> {
    let (n, k) = (w.chars.len(), w.typo);
    let mut best: Option<i64> = None;
    let mut buf = ['\0'; MAX_TYPO_LEN];
    for (li, line) in lines.text.iter().take(SOFT_LINES).enumerate() {
        if (w.bag & !lines.bag[li]).count_ones() as usize > k {
            continue;
        }
        for tok in line.split(|c: char| !c.is_alphanumeric()) {
            if tok.len() + k < n {
                continue;
            }
            let mut m = 0;
            for c in tok.chars() {
                if m == MAX_TYPO_LEN {
                    break;
                }
                buf[m] = c;
                m += 1;
            }
            if m + k < n {
                continue;
            }
            let t = &buf[..m];
            // Every letter of the query missing from the token costs an edit.
            if (w.bag & !bag(t)).count_ones() as usize > k {
                continue;
            }
            let full = osa_within(&w.chars, t, k).map(|d| (d, true));
            let start = if m > n {
                osa_within(&w.chars, &t[..n], k).map(|d| (d, false))
            } else {
                None
            };
            if let Some((d, whole)) = [full, start].into_iter().flatten().min_by_key(|(d, _)| *d) {
                let b = 40 - d as i64 * 10 + if whole { 10 } else { 0 } + FIELD[li];
                best = Some(best.map_or(b, |x| x.max(b)));
            }
        }
    }
    best
}

/// Optimal string alignment (Damerau-Levenshtein with adjacent swaps) when it is at most
/// `k` (≤ 2), else `None`. Only the band of width 2k + 1 is computed.
fn osa_within(a: &[char], b: &[char], k: usize) -> Option<usize> {
    const W: usize = MAX_TYPO_LEN + 1;
    let (m, n) = (a.len(), b.len());
    if m.abs_diff(n) > k || m > MAX_TYPO_LEN || n > MAX_TYPO_LEN {
        return None;
    }
    let inf = (k + 1) as u8;
    let mut p2 = [inf; W];
    let mut p1 = [inf; W];
    for (j, v) in p1.iter_mut().enumerate().take(n.min(k) + 1) {
        *v = j as u8;
    }
    for i in 1..=m {
        let mut cur = [inf; W];
        let (lo, hi) = (i.saturating_sub(k), (i + k).min(n));
        let mut row_min = inf;
        if lo == 0 {
            cur[0] = (i as u8).min(inf);
            row_min = cur[0];
        }
        for j in lo.max(1)..=hi {
            let cost = u8::from(a[i - 1] != b[j - 1]);
            let mut v = p1[j - 1]
                .saturating_add(cost)
                .min(p1[j].saturating_add(1))
                .min(cur[j - 1].saturating_add(1));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(p2[j - 2].saturating_add(1));
            }
            cur[j] = v.min(inf);
            row_min = row_min.min(cur[j]);
        }
        if row_min >= inf {
            return None;
        }
        p2 = p1;
        p1 = cur;
    }
    (p1[n] as usize <= k).then_some(p1[n] as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(title: &str, artist: &str, album: &str) -> String {
        search_text(title, artist, album, "", &format!("/m/{title}.mp3"))
    }

    fn score(q: &str, title: &str) -> i64 {
        Matcher::new(q).map_or(1, |m| m.score(&text(title, "", "")))
    }

    fn hit(q: &str, title: &str) -> bool {
        score(q, title) > 0
    }

    #[test]
    fn folds_case_and_diacritics() {
        assert_eq!(fold("Café Ёлка Їжак Ґанок Й"), "cafe елка іжак ганок и");
        assert_eq!(fold("Zażółć gęślą jaźń"), "zazolc gesla jazn");
        assert_eq!(fold("Straße Æon ŁÓDŹ"), "strasse aeon lodz");
        assert_eq!(fold("Don’t Stop 'Em"), "dont stop em");
        assert_eq!(fold("ＡＢＣ１２"), "abc12");
        // A decomposed é (e + combining acute) folds the same.
        assert_eq!(fold("Cafe\u{301}"), "cafe");
    }

    #[test]
    fn matches_any_case_and_without_marks() {
        assert!(hit("CAFE", "Café del Mar"));
        assert!(hit("café", "cafe del mar"));
        assert!(hit("елка", "Ёлка"));
        assert!(hit("іжак", "Їжак"));
        assert!(hit("strasse", "Straße"));
        assert!(!hit("zzz", "Café del Mar"));
        // LIKE wildcards are plain characters.
        assert!(!hit("%", "Pure"));
        assert!(!hit("_", "Pure"));
        assert!(hit("%", "100% Pure"));
    }

    #[test]
    fn every_word_must_match_in_any_order() {
        assert!(hit("mar del", "Café del Mar"));
        assert!(hit("cafe mar", "Café del Mar"));
        assert!(!hit("cafe moon", "Café del Mar"));
    }

    #[test]
    fn fuzzy_finds_letters_in_order() {
        assert!(hit("sntl", "Sintel"));
        assert!(hit("itl", "Sintel"));
        assert!(!hit("lets", "Sintel"));
        // Too spread out is not a match.
        assert!(!hit("sxl", "S very long gap before l"));
        // Two letters need a real substring.
        assert!(!hit("sl", "Sintel"));
    }

    #[test]
    fn typos_by_word_length() {
        // 4-7 letters: one edit (substitution, swap, missing or extra letter).
        assert!(hit("sintle", "Sintel"), "swap");
        assert!(hit("sontel", "Sintel"), "substitution");
        assert!(hit("sintell", "Sintel"), "extra letter");
        assert!(!hit("sontal", "Sintel"), "two edits in a 6-letter word");
        // Under 4 letters: none.
        assert!(!hit("teh", "The"));
        assert!(hit("tehe", "Thee x"), "swap in a 4-letter word");
        // 8+ letters: two edits.
        assert!(hit("intarstellor", "Interstellar"), "two substitutions");
        assert!(!hit("intarstollor", "Interstellar"), "three substitutions");
        // The start of a longer word, while typing.
        assert!(hit("sintl", "Sintel Trailer"));
        assert!(hit("interstelar", "Interstellar"));
    }

    #[test]
    fn typos_do_not_reach_into_the_path() {
        let m = Matcher::new("sintle").unwrap();
        let hay = search_text("Other", "", "", "", "/videos/Sintel/other.mkv");
        assert_eq!(m.score(&hay), 0);
        // …but an exact path part still matches.
        let m = Matcher::new("videos").unwrap();
        assert!(m.score(&hay) > 0);
    }

    #[test]
    fn layout_and_transliteration() {
        // Typed on the UA layout while meaning Latin letters, and the other way round.
        assert!(hit("іштеуд", "Sintel"), "sintel typed on the UA layout");
        assert!(hit("ghbdsn", "Привіт"), "привіт typed on the QWERTY layout");
        assert!(hit("];fr", "Їжак"), "ї ж а к");
        assert!(hit("'dhj", "Євро"), "є в р о");
        assert!(hit("\\fyjr", "Ґанок"), "ґ а н о к");
        // Phonetic Cyrillic <-> Latin.
        assert!(hit("сінтел", "Sintel"));
        assert!(hit("sintel", "Сінтел"));
        assert!(hit("schuka", "Щука"));
        assert!(hit("щука", "Shchuka"));
        assert!(hit("yurii", "Юрій"), "й read as и");
        assert!(hit("київ", "Kyiv"));
        assert!(!hit("сінтел", "Interstellar"));
    }

    #[test]
    fn ranking_exact_then_fuzzy_then_typo_then_layout() {
        let exact = score("sintel", "Sintel");
        let prefix = score("sint", "Sintel Trailer");
        let inside = score("ntel", "Sintel");
        let fuzzy = score("sntl", "Sintel");
        let typo = score("sintle", "Sintel");
        let layout = score("сінтел", "Sintel");
        assert!(exact > prefix && prefix > inside, "{exact} {prefix} {inside}");
        assert!(inside > fuzzy, "{inside} {fuzzy}");
        assert!(fuzzy > typo, "{fuzzy} {typo}");
        assert!(typo > layout && layout > 0, "{typo} {layout}");
    }

    #[test]
    fn ranking_prefers_whole_words_and_titles() {
        let whole = score("mar", "Café del Mar");
        let inside = score("mar", "Summary");
        assert!(whole > inside);
        let m = Matcher::new("beatles").unwrap();
        let title = m.score(&text("The Beatles", "", ""));
        let artist = m.score(&text("Help", "The Beatles", ""));
        let album = m.score(&search_text("Help", "", "Beatles for Sale", "", "/m/x.mp3"));
        assert!(title > artist, "{title} {artist}");
        assert!(album > 0);
    }

    #[test]
    fn empty_query_has_no_matcher() {
        assert!(Matcher::new("").is_none());
        assert!(Matcher::new("   ").is_none());
    }

    #[test]
    fn osa_distances() {
        let d = |a: &str, b: &str, k| {
            osa_within(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>(), k)
        };
        assert_eq!(d("kitten", "kitten", 1), Some(0));
        assert_eq!(d("ab", "ba", 1), Some(1), "swap");
        assert_eq!(d("abcd", "abdc", 1), Some(1));
        assert_eq!(d("abcd", "abc", 1), Some(1));
        assert_eq!(d("abc", "abcd", 1), Some(1));
        assert_eq!(d("abcd", "axcy", 1), None);
        assert_eq!(d("abcd", "axcy", 2), Some(2));
        assert_eq!(d("abcd", "wxyz", 2), None);
        assert_eq!(d("sintel", "snitle", 2), Some(2));
    }

    #[test]
    fn large_library_stays_fast() {
        let rows: Vec<String> = (0..20_000)
            .map(|i| text(&format!("Track number {i} of the album"), "Some Artist", "Album Name"))
            .collect();
        let m = Matcher::new("sintle trailer").unwrap();
        let t = std::time::Instant::now();
        let hits = rows.iter().filter(|r| m.score(r) > 0).count();
        assert_eq!(hits, 0);
        // Debug builds are slow; the point is that it is nowhere near quadratic.
        assert!(t.elapsed().as_millis() < 3000, "{:?}", t.elapsed());
    }
}
