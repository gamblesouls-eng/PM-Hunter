//! Reading the text recognition's results: the enemy's box, your Pokemon's,
//! the Fight menu's moves, the bag's tabs and search box, the MENU, the game's
//! messages.

use crate::layout;
use crate::px::Pt;
use crate::rules::{norm, same_word, Enc};
use regex::Regex;
use std::sync::LazyLock;

/// Text read off the screen; x, y its center (fractions of what was read, or
/// screen px for screen_texts), top-left origin.
#[derive(Clone, Debug, PartialEq)]
pub struct TextHit {
    pub text: String,
    pub x: f64,
    pub y: f64,
}

impl TextHit {
    pub fn new(text: impl Into<String>, x: f64, y: f64) -> TextHit {
        TextHit { text: text.into(), x, y }
    }
    pub fn pt(&self) -> Pt {
        Pt::new(self.x, self.y)
    }
    pub fn is(&self, word: &str) -> bool {
        self.text.to_lowercase() == word
    }
}

macro_rules! re {
    ($name:ident, $re:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($re).unwrap());
    };
}

re!(LEVEL_CUT, r"(?i)\s+l[vy]\b");
re!(NOT_NAME, r"[^A-Za-z0-9 .'\-:]");
re!(LEVEL, r"(?i)^\s*l[vy][.,:]?\s*(\d+)");
re!(LEVEL_ANY, r"(?i)\bl[vy][.,:]?\s*(\d+)");
re!(LEVEL_WORD, r"(?i)^l[vy]\b");
re!(TWO_LETTERS, r"[A-Za-z]{2}");
re!(HP, r"(\d+)\s*[/|]\s*(\d+)");
re!(HP_SLASH, r"(\d+)\s*/\s*(\d+)");
re!(COUNT, r"(?i)^x[\d,.]+$");
re!(DIGIT_OR_MARK, r"[\d?!]");
re!(DIGIT, r"\d");
// "PP / max" under a move. The slash is read as | or I now and then, or lost
// ("40 40"); nothing else there has numbers.
re!(PP, r"(\d+)\s*(?:[/|Il\\]|\s)\s*(\d+)");
re!(SHOULD, r"should (.+?) do");
re!(SAVE_WAIT, r"(?i)wait\s+(\d+)\s+more\s+second");
re!(SENT_TO, r"sent to (the )?(pc|box)");
re!(TARGET_CUT, r"(?i)\s+-\s+|\s+lv");
re!(WILD_FAINTED, r"(?i)\bwild\b.*\bfainted");
re!(WILD_STATUSED, r"(?i)\bwild\b.*\b(was (badly )?poisoned|hurt by (the )?poison|was burned|hurt by its burn|is paralyzed|was paralyzed|was frozen)");

pub const STATUSES: [&str; 6] = ["SLP", "PSN", "PAR", "BRN", "FRZ", "TOX"];

/// A bag row's count tag: "x985".
pub fn is_count(t: &str) -> bool {
    COUNT.is_match(t)
}

/// Every species' name (PokeAPI's, species.txt), letters only: "typenull", "mrmime".
static SPECIES: LazyLock<std::collections::HashSet<String>> =
    LazyLock::new(|| include_str!("species.txt").lines().map(norm).filter(|n| !n.is_empty()).collect());

/// A real Pokemon's name, or a form of one ("Galarian Articuno", "Hisuian-Qwilfish",
/// "Type: Null"), not a misread ("Me Ill" for "Mew").
pub fn known_species(name: &str) -> bool {
    let n = norm(name);
    !n.is_empty() && (SPECIES.contains(&n)
        || name.split(|c: char| c == ' ' || c == '-').any(|w| w.chars().count() >= 3 && SPECIES.contains(&norm(w))))
}

pub fn two_letters(t: &str) -> bool {
    TWO_LETTERS.is_match(t)
}

/// "Marill Lv. 52" -> "Marill"; a misread gender sign at the end goes.
pub fn clean_name(text: &str) -> String {
    let t = LEVEL_CUT.split(text).next().unwrap_or("");
    let t = NOT_NAME.replace_all(t, " ");
    let mut words: Vec<&str> = t.split_whitespace().collect();
    while words.len() > 1 && words.last().is_some_and(|w| w.chars().count() == 1) {
        words.pop();
    }
    words.join(" ").trim_matches(|c| c == ' ' || c == '.').to_string()
}

/// The enemy's species, level, status, reskin and gradient from the text read
/// in its box (layout::BOX, x and y fractions of it). The icons are separate.
pub fn enemy(texts: &[TextHit]) -> Enc {
    let bh = layout::BOX.h() as f64;
    let hp = (layout::HP_BAR_END - layout::BOX.t) as f64 / bh;
    let end = (layout::BOX_END - layout::BOX.t) as f64 / bh;
    let mut enc = Enc::default();
    let mut sorted: Vec<&TextHit> = texts.iter().collect();
    sorted.sort_by(|a, b| a.x.total_cmp(&b.x));
    let join = |a: &str, b: &str| format!("{a} {b}").trim().to_string();
    for hit in sorted {
        let mut text = hit.text.clone();
        let words: Vec<&str> = hit.text.split_whitespace().collect();
        let badge: Vec<String> =
            words.iter().map(|w| w.to_uppercase()).filter(|w| STATUSES.contains(&w.as_str())).collect();
        if let Some(b) = badge.first() {
            // the SLP / PSN ... badge left of the HP bar
            enc.status = b.clone();
            text = words.iter().filter(|w| !STATUSES.contains(&w.to_uppercase().as_str())).cloned()
                .collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                continue;
            }
        }
        if let Some(m) = LEVEL.captures(&text) {
            enc.level = m[1].to_string();
        } else if hit.y < hp && hit.x < 0.6 {
            enc.species = join(&enc.species, &clean_name(&text));
            if let (true, Some(m)) = (enc.level.is_empty(), LEVEL_ANY.captures(&text)) {
                enc.level = m[1].to_string(); // the name and the level read as one line ("Entei Lv. 20")
            }
        } else if !two_letters(&text) {
            continue; // an icon read as text ("*" for the shiny star)
        } else if hp <= hit.y && hit.y < end {
            enc.reskin = join(&enc.reskin, text.trim());
        } else if hit.y >= end {
            enc.gradient = join(&enc.gradient, text.trim());
        }
    }
    // the gender sign can come as a text of its own ("Sentret" "O"): cleaned once more, joined
    enc.species = clean_name(&enc.species);
    enc
}

/// Your Pokemon in battle, from its box (layout::PLAYER_BOX): "Clodsire Lv. 109", "350 / 376".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lead {
    pub name: String,
    pub level: Option<i64>,
    pub hp: Option<i64>,
    pub max: Option<i64>,
}

impl Lead {
    /// All of it read: something over the box (the level-up stats panel) leaves gaps.
    pub fn complete(&self) -> bool {
        !self.name.is_empty() && self.level.is_some() && matches!((self.hp, self.max), (Some(h), Some(m)) if h <= m)
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"name": self.name, "level": self.level, "hp": self.hp, "max": self.max})
    }
}

pub fn lead(texts: &[TextHit]) -> Lead {
    let mut sorted: Vec<&TextHit> = texts.iter().collect();
    sorted.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let joined = sorted.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" "); // "376 / 376" can come in pieces
    let hp = HP.captures(&joined); // the slash is read as | now and then
    let level = LEVEL_ANY.captures(&joined);
    let name = sorted.iter().map(|t| clean_name(&t.text)).find(|n| {
        two_letters(n) && n.to_lowercase() != "exp" && !LEVEL_WORD.is_match(n)
    });
    Lead {
        name: name.unwrap_or_default(),
        level: level.and_then(|m| m[1].parse().ok()),
        hp: hp.as_ref().and_then(|m| m[1].parse().ok()),
        max: hp.as_ref().and_then(|m| m[2].parse().ok()),
    }
}

/// A move in the Fight menu, with its PP left and its most.
#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub name: String,
    pub pp: Option<i64>,
    pub max: Option<i64>,
}

impl Move {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"name": self.name, "pp": self.pp, "max": self.max})
    }
}

/// The moves in the Fight menu from the text read in layout::MSG (fractions):
/// up to four buttons side by side, the name on top, "PP / max" under it.
pub fn moves(texts: &[TextHit]) -> Vec<Move> {
    let mut by_x: Vec<&TextHit> = texts.iter().collect();
    by_x.sort_by(|a, b| a.x.total_cmp(&b.x));
    let pair = |c: regex::Captures| (c[1].parse::<i64>().ok(), c[2].parse::<i64>().ok());
    let pps: Vec<(i64, (Option<i64>, Option<i64>))> = texts.iter().filter(|t| t.y >= 0.5)
        .filter_map(|t| PP.captures(&t.text).map(|m| ((t.x * 4.0) as i64, pair(m))))
        .collect();
    // left to right; one hit can hold two buttons' PP ("30 / 35 Normal Physical 40 / 40")
    let in_order: Vec<(Option<i64>, Option<i64>)> =
        by_x.iter().filter(|t| t.y >= 0.5).flat_map(|t| PP.captures_iter(&t.text).map(pair).collect::<Vec<_>>()).collect();
    let names: Vec<(String, f64)> = by_x.iter()
        .filter(|t| {
            let s = t.text.trim();
            t.y < 0.5 && two_letters(&t.text) && !DIGIT_OR_MARK.is_match(&t.text) && s.chars().count() <= 22
                && s.to_lowercase() != "back"
        })
        .map(|t| (t.text.trim().to_string(), t.x))
        .collect();
    names.iter().enumerate().map(|(i, (name, x))| {
        // one PP per button: in order; else by the button the text sits in
        let pp = if in_order.len() == names.len() {
            Some(in_order[i])
        } else {
            pps.iter().find(|(col, _)| *col == (x * 4.0) as i64).map(|p| p.1)
        };
        Move { name: name.clone(), pp: pp.and_then(|p| p.0), max: pp.and_then(|p| p.1) }
    }).collect()
}

/// The PP numbers under a row of moves (one hit can hold several).
pub fn pp_numbers(text: &str) -> Vec<i64> {
    // digits read as letters ("0 / IO": Windows, for "0 / 10"): a short word of those and
    // digits is a number
    let text: String = text.split(' ').map(|w| {
        let lookalike = (2..=3).contains(&w.len()) && w.chars().all(|c| c.is_ascii_digit() || "OoIl".contains(c))
            && w.chars().any(|c| !c.is_ascii_digit());
        if lookalike {
            w.chars().map(|c| match c { 'O' | 'o' => '0', 'I' | 'l' => '1', c => c }).collect()
        } else {
            w.to_string()
        }
    }).collect::<Vec<_>>().join(" ");
    PP.captures_iter(&text).filter_map(|m| m[1].parse().ok()).collect()
}

pub fn has_digit(t: &str) -> bool {
    DIGIT.is_match(t)
}

/// Your Pokemon's HP as a fraction, from the text in its box.
pub fn hp(text: &str) -> Option<f64> {
    // the last "a / b" with a <= b ("Lv. 129 412/415" isn't 129/415): anything else is a misread
    HP_SLASH.captures_iter(text).filter_map(|m| {
        let (hp, max): (f64, f64) = (m[1].parse().ok()?, m[2].parse().ok()?);
        (max > 0.0 && hp <= max).then(|| hp / max)
    }).last()
}

/// "What should Clodsire do?" -> "Clodsire"
pub fn should(msg: &str) -> Option<String> {
    SHOULD.captures(msg).map(|m| m[1].to_string())
}

pub const TABS: [&str; 8] =
    ["Recovers", "Pokeballs", "Key Items", "Evolution", "Competitive", "TMs", "Held Items", "Z-Crystals"];

fn tab_index(t: &str) -> Option<usize> {
    TABS.iter().position(|tab| norm(tab) == norm(t) || same_word(tab, t))
}

/// Where the backpack's tab is. On a small window or a phone the labels are
/// tiny and some aren't read, or are misread ("TMB", "Heid Iterme"), but the
/// tabs sit in a fixed order, evenly spaced: one that isn't read is worked out
/// from the ones that are. With one read, or none (Windows read only "Z- Cry St. Is"
/// in a small window, the bag left on TMs), from the "Backpack" title over them:
/// the first tab is TAB_FIRST tab steps under it, the search box TAB_SEARCH (the
/// bag's width doesn't say: it's squatter on some screens). The TextHit is made up
/// in that case (its text the tab's).
pub fn find_tab(texts: &[TextHit], want: &str, split: f64, width: u32) -> Option<TextHit> {
    let Some(i) = TABS.iter().position(|t| t.to_lowercase() == want.to_lowercase()) else {
        return texts.iter().find(|t| t.text.to_lowercase() == want.to_lowercase()).cloned();
    };
    // (the list's heading is the open tab's name too: it's right of the title's middle, the tabs left)
    let left = texts.iter().find(|t| norm(&t.text) == "backpack").map_or(split, |t| t.x.min(split));
    let hits: Vec<(usize, &TextHit)> =
        texts.iter().filter(|t| t.x < left).filter_map(|t| tab_index(&t.text).map(|n| (n, t))).collect();
    if hits.is_empty() {
        return tab_from_title(texts, i, None, split);
    }
    let col = hits.iter().map(|h| h.1.x).fold(f64::INFINITY, f64::min); // the tab column; the list's heading is further right
    let in_col: Vec<&(usize, &TextHit)> = hits.iter().filter(|h| (h.1.x - col).abs() < 0.04 * width as f64).collect();
    if let Some(exact) = in_col.iter().find(|h| h.0 == i) {
        return Some(exact.1.clone());
    }
    let mut kinds: Vec<usize> = in_col.iter().map(|h| h.0).collect();
    kinds.sort();
    kinds.dedup();
    if kinds.len() < 2 {
        return tab_from_title(texts, i, Some(in_col[0]), split);
    }
    let n: Vec<f64> = in_col.iter().map(|h| h.0 as f64).collect();
    let y: Vec<f64> = in_col.iter().map(|h| h.1.y).collect();
    let mn = n.iter().sum::<f64>() / n.len() as f64;
    let my = y.iter().sum::<f64>() / y.len() as f64;
    let step = n.iter().zip(&y).map(|(a, b)| (a - mn) * (b - my)).sum::<f64>()
        / n.iter().map(|a| (a - mn) * (a - mn)).sum::<f64>();
    let mut xs: Vec<f64> = in_col.iter().map(|h| h.1.x).collect();
    xs.sort_by(f64::total_cmp);
    let mid = if xs.len() % 2 == 1 { xs[xs.len() / 2] } else { (xs[xs.len() / 2 - 1] + xs[xs.len() / 2]) / 2.0 };
    Some(TextHit::new(TABS[i], mid, my + step * (i as f64 - mn)))
}

// samples/bag-open.png: "Backpack" at y 287, the tabs at 418 + 75.3 n (x 490, the title's 527),
// the search box at 390
const TAB_FIRST: f64 = 131.0 / 75.3;
const TAB_SEARCH: f64 = 103.0 / 75.3;
const TAB_X: f64 = -37.0 / 75.3;

/// Tab i from the bag's title and one tab that was read, or (none read) the search box.
fn tab_from_title(texts: &[TextHit], i: usize, read: Option<&(usize, &TextHit)>, split: f64) -> Option<TextHit> {
    let title = texts.iter().find(|t| norm(&t.text) == "backpack")?;
    let (step, x) = match read {
        Some((k, t)) => ((t.y - title.y) / (TAB_FIRST + *k as f64), t.x),
        None => {
            let close = texts.iter().find(|t| t.is("close") && t.x > split)?;
            let room = 0.2 * (close.x - title.x); // (the search box is just under Close, above the list)
            let search = texts.iter().filter(|t| t.x > split && t.y > close.y + 5.0 && t.y - close.y < room && !is_count(&t.text))
                .min_by(|a, b| a.y.total_cmp(&b.y))?;
            let step = (search.y - title.y) / TAB_SEARCH;
            (step, title.x + TAB_X * step)
        }
    };
    (step > 0.0).then(|| TextHit::new(TABS[i], x, title.y + step * (TAB_FIRST + i as f64)))
}

/// The bag's search box: just under Close, whatever it shows ("Search", "QUICK
/// BALL", a lowercase "full" left in it), never the menu bar's "A ABC" up top.
pub fn search_box(texts: &[TextHit], split: f64, v: f64) -> Option<Pt> {
    let close = texts.iter().find(|t| t.is("close") && t.x > split)?.y;
    let mut by_y: Vec<&TextHit> = texts.iter().collect();
    by_y.sort_by(|a, b| a.y.total_cmp(&b.y));
    by_y.into_iter()
        .find(|t| t.x > split && 40.0 * v < t.y - close && t.y - close < 170.0 * v && !is_count(&t.text))
        .map(|t| t.pt())
}

/// A search left in the bag's search box ("Dive Ball", typed by an older Hunt): the text
/// in the box, under the "Backpack" title and above the list's rows, that isn't its
/// "Search..." placeholder.
pub fn search_leftover(texts: &[TextHit], split: f64, v: f64) -> Option<TextHit> {
    let title = bag_title(texts)?.y;
    let rows = texts.iter().filter(|t| is_count(&t.text)).map(|t| t.y).fold(f64::INFINITY, f64::min);
    texts.iter()
        .filter(|t| t.x > split && t.y > title + 25.0 * v && t.y < rows - 20.0 * v && !is_count(&t.text)
            && !close_word(t) && two_letters(&t.text))
        .min_by(|a, b| a.y.total_cmp(&b.y))
        .filter(|t| !norm(&t.text).starts_with("search"))
        .cloned()
}

/// The bag's stopped moving (it floats up as it opens): its "Backpack" title
/// read in this read and the one before, in the same place.
pub fn bag_still(texts: &[TextHit], before: &[TextHit]) -> bool {
    let title = |ts: &[TextHit]| ts.iter().find(|t| norm(&t.text) == "backpack").map(|t| (t.x, t.y));
    match (title(texts), title(before)) {
        (Some(a), Some(b)) => (a.0 - b.0).abs() + (a.1 - b.1).abs() <= 4.0,
        _ => false,
    }
}

pub const MENU_ITEMS: [&str; 7] = ["Pokedex", "Party", "Bag", "PokeBoosters", "Save", "Trainer Card", "Codes"];

/// The MENU panel is open: two of its labels are read (forgiving a misread).
pub fn menu_open(texts: &[TextHit]) -> bool {
    texts.iter().filter(|t| MENU_ITEMS.iter().any(|m| norm(m) == norm(&t.text) || same_word(m, &t.text))).count() >= 2
}

/// Where to click a MENU item: its tile, half a row above its label (only the
/// tile takes clicks and taps; the label under it doesn't). The rows' height from
/// the labels read and how many items apart they are in the MENU (a label not
/// read, "Party", doesn't make two rows look like one); with one label read, as
/// the MENU usually is on a screen this tall (screen_h px: a row is 9.3% of it).
pub fn menu_button(hit: Pt, texts: &[TextHit], screen_h: f64) -> Pt {
    let mut seen: std::collections::BTreeMap<usize, f64> = Default::default();
    for t in texts {
        if let Some(i) = MENU_ITEMS.iter().position(|m| norm(m) == norm(&t.text)) {
            seen.entry(i).or_insert(t.y);
        }
    }
    let rows: Vec<(usize, f64)> = seen.into_iter().collect();
    let mut steps: Vec<f64> = rows.windows(2)
        .filter(|w| w[1].1 > w[0].1)
        .map(|w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0) as f64)
        .collect();
    steps.sort_by(f64::total_cmp);
    let row = steps.get(steps.len() / 2).copied().unwrap_or(screen_h * 0.093);
    Pt::new(hit.x, hit.y - row / 2.0)
}

/// The box after Save: "Save successful", "Try again", or "Unable to save.
/// Please wait 95 more seconds." (one save every 2 minutes).
pub fn save_message(t: &str) -> bool {
    let t = t.to_lowercase();
    (t.contains("save") && (t.contains("successful") || t.contains("again") || t.contains("unable")))
        || SAVE_WAIT.is_match(&t)
}

/// The seconds a "Please wait 95 more seconds" says.
pub fn save_wait(t: &str) -> Option<u64> {
    SAVE_WAIT.captures(t).and_then(|m| m[1].parse().ok())
}

/// The battle's text after a catch: "Gotcha! ... was caught!", or only
/// "Rattata was added to the Party." when the first message went by too fast.
pub fn caught_said(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("was caught") || m.contains("gotcha") || m.contains("added to the party") || SENT_TO.is_match(&m)
}

/// "The wild Giratina fainted!" (its poison, say): the battle's over, nothing caught.
pub fn wild_fainted(msg: &str) -> bool {
    WILD_FAINTED.is_match(msg)
}

/// "The wild Giratina was poisoned!", "... was hurt by poison!": it has a status, whether
/// or not its badge is read (Yawn can't work on it).
pub fn wild_statused(msg: &str) -> bool {
    WILD_STATUSED.is_match(msg)
}

/// The Pokemon Party screen: in battle "Choose a Pokemon to use Full Restore on." (or
/// "Choose your next Pokemon." when yours fainted), outside one "Select a pokemon to heal".
pub fn party_up(texts: &[TextHit]) -> bool {
    texts.iter().any(|t| {
        let l = t.text.to_lowercase();
        l.starts_with("choose a pokemon") || l.starts_with("select a pokemon") || l.starts_with("pokemon party")
            || l.starts_with("choose your next")
    })
}

/// Your Pokemon fainted: "Choose your next Pokemon."
pub fn next_asked(texts: &[TextHit]) -> bool {
    texts.iter().any(|t| t.text.to_lowercase().starts_with("choose your next"))
}

/// The backpack is open: its "Backpack" title.
pub fn bag_title(texts: &[TextHit]) -> Option<&TextHit> {
    texts.iter().find(|t| norm(&t.text) == "backpack")
}

/// A Close button's word, whole or in the pieces some PCs read it as ("C", "e", "lose").
pub fn close_word(t: &TextHit) -> bool {
    let l = t.text.to_lowercase();
    l == "close" || ["lose", "clos", "ciose", "c1ose", "cl0se", "close."].contains(&l.as_str())
}

/// The party screen's card for target: "Clodsire - Lv. 109".
pub fn is_target(target: &str, t: &str) -> bool {
    same_word(target, t) || same_word(target, TARGET_CUT.split(t).next().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(t: &str, x: f64, y: f64) -> TextHit {
        TextHit::new(t, x, y)
    }

    #[test]
    fn species_names() {
        for good in ["Mew", "Entei", "Type: Null", "Galarian Articuno", "Hisuian-Qwilfish", "Mr. Mime", "Pichu-Spiky-eared"] {
            assert!(known_species(good), "{good}");
        }
        for bad in ["Me Ill", "", "L\". 20"] {
            assert!(!known_species(bad), "{bad}");
        }
    }

    #[test]
    fn name_and_level_as_one_line() {
        // Windows, on a Coal Entei's box made bigger: the name and the level as one text
        let e = enemy(&[hit("Entei Lv. 20", 0.4, 0.21), hit("COAL", 0.67, 0.66)]);
        assert_eq!((e.species.as_str(), e.level.as_str(), e.reskin.as_str()), ("Entei", "20", "COAL"));
        let e = enemy(&[hit("Entei", 0.15, 0.21), hit("Lv. 20", 0.81, 0.21)]);
        assert_eq!((e.species.as_str(), e.level.as_str()), ("Entei", "20"));
    }

    #[test]
    fn pp_with_digits_read_as_letters() {
        // Windows, on a move list with Yawn out of PP: "0 / 10" read as "0 / IO"
        assert_eq!(pp_numbers("Yawn 0 / IO Normal Status Protect 1/10 Normal Status"), vec![0, 1]);
        assert_eq!(pp_numbers("8 / 1O"), vec![8]);
        assert_eq!(pp_numbers("Normal Status"), Vec::<i64>::new());
    }

    #[test]
    fn names() {
        assert_eq!(clean_name("Marill Lv. 52"), "Marill");
        assert_eq!(clean_name("Sentret O"), "Sentret");
        assert_eq!(clean_name("Type: Null"), "Type: Null");
        assert_eq!(clean_name("Pichu-Spiky-eared ♂"), "Pichu-Spiky-eared");
    }

    #[test]
    fn the_enemys_box() {
        let e = enemy(&[hit("Marill", 0.2, 0.15), hit("Lv. 52", 0.7, 0.15), hit("INFERNO", 0.4, 0.6),
                        hit("SLP", 0.1, 0.4), hit("Mirage", 0.5, 0.9), hit("*", 0.1, 0.6)]);
        assert_eq!((e.species.as_str(), e.level.as_str(), e.reskin.as_str(), e.gradient.as_str(), e.status.as_str()),
                   ("Marill", "52", "INFERNO", "Mirage", "SLP"));
    }

    #[test]
    fn lead_and_moves() {
        let l = lead(&[hit("Clodsire", 0.2, 0.2), hit("Lv. 109", 0.7, 0.2), hit("350 /", 0.6, 0.7), hit("376", 0.8, 0.7)]);
        assert_eq!(l, Lead { name: "Clodsire".into(), level: Some(109), hp: Some(350), max: Some(376) });
        assert!(l.complete());
        let m = moves(&[hit("Yawn", 0.1, 0.3), hit("Protect", 0.35, 0.3), hit("10 / 16", 0.1, 0.7),
                        hit("5 | 16", 0.35, 0.7), hit("Back", 0.9, 0.3)]);
        assert_eq!(m, vec![Move { name: "Yawn".into(), pp: Some(10), max: Some(16) },
                           Move { name: "Protect".into(), pp: Some(5), max: Some(16) }]);
    }

    #[test]
    fn tabs_worked_out() {
        let texts = [hit("Recovers", 100.0, 200.0), hit("Key Items", 102.0, 300.0), hit("Pokeballs!", 600.0, 100.0)];
        let t = find_tab(&texts, "Pokeballs", 500.0, 2000).unwrap();
        assert_eq!((t.x.round(), t.y.round()), (101.0, 250.0));
        assert_eq!(find_tab(&texts, "Recovers", 500.0, 2000).unwrap().y, 200.0);
    }

    #[test]
    fn tabs_from_the_title() {
        // a friend's Windows PC, the bag left on TMs: of the tabs only "Z- Cry St. Is" read
        // (Pokeballs is at (314, 271), the tabs 44.7 px apart)
        let mut texts = vec![hit("Backpack", 333.0, 148.0), hit("Close", 937.0, 148.0), hit("TMs", 380.0, 210.0),
                             hit("Search...", 1000.0, 209.0), hit("Aerial Ace", 494.0, 266.0), hit("Blizzard", 486.0, 316.0),
                             hit("Z- Cry St. Is", 314.0, 539.0)];
        let t = find_tab(&texts, "Pokeballs", 700.0, 1280).unwrap();
        assert!((t.x - 314.0).abs() < 4.0 && (t.y - 271.0).abs() < 4.0, "{t:?}");
        // none read: the search box under Close
        texts.pop();
        let t = find_tab(&texts, "Pokeballs", 700.0, 1280).unwrap();
        assert!((t.x - 314.0).abs() < 10.0 && (t.y - 271.0).abs() < 4.0, "{t:?}");
        // no bag, no tab
        assert!(find_tab(&texts[2..], "Pokeballs", 700.0, 1280).is_none());
    }

    #[test]
    fn menu_rows() {
        let texts = [hit("Pokedex", 100.0, 100.0), hit("Bag", 100.0, 300.0), hit("Save", 100.0, 500.0)];
        assert!(menu_open(&texts));
        // Party (not read) between Pokedex and Bag: rows 100 px apart
        assert_eq!(menu_button(Pt::new(100.0, 500.0), &texts, 1249.0), Pt::new(100.0, 450.0));
        // only Save read (samples/menu-open.png: its label at y 856, its tile 767 to 834)
        let at = menu_button(Pt::new(100.0, 856.0), &[hit("Save", 100.0, 856.0)], 1249.0);
        assert!((767.0..834.0).contains(&at.y), "{at:?}");
        assert!(save_message("Unable to save. Please wait 95 more seconds."));
        assert_eq!(save_wait("Please wait 95 more seconds."), Some(95));
        assert!(caught_said("Rattata was sent to the PC!"));
    }
}
