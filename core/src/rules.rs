//! What's wanted: rules ({"reskin": "any", "gradient": "any"}, built in; the tests' own too),
//! matched against what was read, forgiving a misread letter.

use serde_json::Value;

/// An encounter, as read from the enemy's box.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Enc {
    pub species: String,
    pub level: String,
    pub reskin: String,
    pub gradient: String,
    pub status: String,
    pub shiny: bool,
    pub tint: bool,
    /// The little Poke Ball by the HP bar: a species caught before.
    pub owned: bool,
}

pub const FIELDS: [&str; 5] = ["species", "shiny", "tint", "reskin", "gradient"];

impl Enc {
    pub fn get(&self, field: &str) -> Value {
        match field {
            "species" => Value::from(self.species.as_str()),
            "level" => Value::from(self.level.as_str()),
            "reskin" => Value::from(self.reskin.as_str()),
            "gradient" => Value::from(self.gradient.as_str()),
            "status" => Value::from(self.status.as_str()),
            "shiny" => Value::Bool(self.shiny),
            "tint" => Value::Bool(self.tint),
            "owned" => Value::Bool(self.owned),
            _ => Value::Null,
        }
    }

    /// "shiny Quaxly Lv12  (reskin INFERNO, gradient Mirage, tint)"
    pub fn describe(&self) -> String {
        let mut words = Vec::new();
        if self.shiny {
            words.push("shiny".to_string());
        }
        words.push(if self.species.is_empty() { "?".into() } else { self.species.clone() });
        if !self.level.is_empty() {
            words.push(format!("Lv{}", self.level));
        }
        let mut extras = Vec::new();
        if !self.reskin.is_empty() {
            extras.push(format!("reskin {}", self.reskin));
        }
        if !self.gradient.is_empty() {
            extras.push(format!("gradient {}", self.gradient));
        }
        if self.tint {
            extras.push("tint".into());
        }
        words.join(" ") + &if extras.is_empty() { String::new() } else { format!("  ({})", extras.join(", ")) }
    }

    /// "Doodle reskin, Mirage gradient, Shiny, Tinted Lapras"
    pub fn title(&self) -> String {
        let mut traits = Vec::new();
        if !self.reskin.is_empty() {
            traits.push(format!("{} reskin", title_case(&self.reskin)));
        }
        if !self.gradient.is_empty() {
            traits.push(format!("{} gradient", title_case(&self.gradient)));
        }
        if self.shiny {
            traits.push("Shiny".into());
        }
        if self.tint {
            traits.push("Tinted".into());
        }
        format!("{} {}", traits.join(", "), self.species).trim().to_string()
    }
}

/// Python's str.title(): each run of letters starts upper case, the rest lower.
pub fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_letter = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_letter {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_letter = true;
        } else {
            out.push(c);
            prev_letter = false;
        }
    }
    out
}

/// A reskin or gradient seen on screen whose name didn't read.
pub const UNREAD: &str = "?";

/// Lower case letters and digits only.
pub fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect()
}

/// Equal, forgiving one misread letter in longer words.
pub fn same_word(want: &str, got: &str) -> bool {
    let a = norm(want).replace('0', "o");
    let b = norm(got).replace('0', "o");
    a == b || (a.chars().count() >= 5 && ratio(&a, &b) >= 0.8)
}

/// Python's difflib.SequenceMatcher(None, a, b).ratio().
pub fn ratio(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    2.0 * matches(&a, &b) as f64 / (a.len() + b.len()) as f64
}

fn matches(a: &[char], b: &[char]) -> usize {
    // the longest common run, earliest in a, then earliest in b; then either side of it
    let (mut best, mut bi, mut bj) = (0, 0, 0);
    for i in 0..a.len() {
        for j in 0..b.len() {
            let mut k = 0;
            while i + k < a.len() && j + k < b.len() && a[i + k] == b[j + k] {
                k += 1;
            }
            if k > best {
                (best, bi, bj) = (k, i, j);
            }
        }
    }
    if best == 0 {
        return 0;
    }
    best + matches(&a[..bi], &b[..bj]) + matches(&a[bi + best..], &b[bj + best..])
}

pub const LEGENDARIES: &str = "Articuno Zapdos Moltres Mewtwo Mew Raikou Entei Suicune Lugia Ho-Oh Celebi \
Regirock Regice Registeel Latias Latios Kyogre Groudon Rayquaza Jirachi Deoxys \
Uxie Mesprit Azelf Dialga Palkia Heatran Regigigas Giratina Cresselia Phione \
Manaphy Darkrai Shaymin Arceus Victini Cobalion Terrakion Virizion Tornadus \
Thundurus Reshiram Zekrom Landorus Kyurem Keldeo Meloetta Genesect Xerneas \
Yveltal Zygarde Diancie Hoopa Volcanion Type:Null Silvally Tapu-Koko Tapu-Lele \
Tapu-Bulu Tapu-Fini Cosmog Cosmoem Solgaleo Lunala Necrozma Magearna Marshadow \
Zeraora Meltan Melmetal Zacian Zamazenta Eternatus Kubfu Urshifu Zarude \
Regieleki Regidrago Glastrier Spectrier Calyrex Enamorus Wo-Chien Chien-Pao \
Ting-Lu Chi-Yu Koraidon Miraidon Okidogi Munkidori Fezandipiti Ogerpon \
Terapagos Pecharunt";

/// Python's truth of a value read: a non-empty text, true.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Null => false,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// "Galarian-Articuno" -> "Articuno"
fn without_region(s: &str) -> String {
    let lower = s.to_lowercase();
    for region in ["alolan", "galarian", "hisuian", "paldean"] {
        if lower.starts_with(region) && s.is_char_boundary(region.len()) {
            let rest = &s[region.len()..];
            return rest.trim_start_matches(|c: char| !(c.is_alphanumeric() || c == '_')).to_string();
        }
    }
    s.to_string()
}

pub fn field_ok(want: &Value, got: &Value) -> bool {
    match want {
        Value::Bool(b) => return *b == truthy(got),
        Value::String(s) if s == "any" => return truthy(got),
        Value::String(s) if s == "none" => return !truthy(got),
        _ => {}
    }
    let list = match want {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    let mut wants: Vec<String> = Vec::new();
    for w in list {
        if w.as_str() == Some("legendary") {
            wants.extend(LEGENDARIES.split_whitespace().map(String::from));
        } else {
            wants.push(text(&w));
        }
    }
    let g = text(got);
    let plain = without_region(&g);
    wants.iter().any(|w| same_word(w, &g) || same_word(w, &plain))
}

pub type Rule = serde_json::Map<String, Value>;

pub fn rule_ok(rule: &Rule, enc: &Enc) -> bool {
    rule.iter().all(|(k, v)| {
        if k == "not_species" { !field_ok(v, &enc.get("species")) } else { field_ok(v, &enc.get(k)) }
    })
}

/// The first rule this encounter matches, or None.
pub fn wanted<'a>(enc: &Enc, rules: &'a [Rule]) -> Option<&'a Rule> {
    rules.iter().find(|r| rule_ok(r, enc))
}

/// For an encounter whose name never read: a rule naming a species that it matches in
/// everything else (it might be the one).
pub fn might_want(enc: &Enc, rules: &[Rule]) -> bool {
    rules.iter().any(|r| r.contains_key("species")
        && r.iter().all(|(k, v)| k == "species" || k == "not_species" || field_ok(v, &enc.get(k))))
}

/// The reads agree on everything but a name that never comes (the text recognition
/// misses some names every time: "Mew"): the last three, nameless, the same.
pub fn settled_nameless(reads: &[Enc]) -> Option<Enc> {
    if reads.len() < 3 {
        return None;
    }
    let last3 = &reads[reads.len() - 3..];
    let l = &last3[2];
    let same = last3.iter().all(|r| r.species.is_empty() && r.reskin == l.reskin && r.gradient == l.gradient
        && r.status == l.status && (r.level == l.level || r.level.is_empty() || l.level.is_empty()));
    if !same || (l.level.is_empty() && l.reskin.is_empty() && l.gradient.is_empty()) {
        return None; // (nothing read at all: not a box)
    }
    let mut enc = l.clone();
    if enc.level.is_empty() {
        enc.level = last3.iter().map(|r| r.level.clone()).find(|v| !v.is_empty()).unwrap_or_default();
    }
    enc.shiny = last3.iter().any(|r| r.shiny);
    enc.tint = last3.iter().any(|r| r.tint);
    Some(enc)
}

/// "species=Tympole, shiny=true"
pub fn show(rule: &Rule) -> String {
    rule.iter()
        .map(|(k, v)| format!("{k}={}", match v {
            Value::String(s) => s.clone(),
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            Value::Array(a) => format!("[{}]", a.iter().map(|x| match x {
                Value::String(s) => format!("'{s}'"),
                o => o.to_string(),
            }).collect::<Vec<_>>().join(", ")),
            o => o.to_string(),
        }))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The enemy, once the reads of it agree, or None: the last two the same, or
/// four in a row of the same Pokemon and level, with the rows most often read
/// (Windows reads the small rows a little differently each time). The icons,
/// a reskin and a gradient count when any read saw them: a sparkling star comes
/// and goes, and Windows misses the colored rows on some reads.
pub fn settled(reads: &[Enc]) -> Option<Enc> {
    let last = reads.last()?;
    // a read without the level agrees with one that has it (Windows reads a light "Lv. 18"
    // only now and then: a tinted Wave Entei's reads never agreed, and it was waited on)
    let level_ok = |a: &Enc, b: &Enc| a.level == b.level || a.level.is_empty() || b.level.is_empty();
    let run: Vec<&Enc> = {
        let n = reads.iter().rev().take_while(|r| r.species == last.species && level_ok(r, last)).count();
        reads[reads.len() - n..].iter().collect()
    };
    let same = run.len() >= 2 && {
        let p = run[run.len() - 2];
        p.species == last.species && level_ok(p, last) && p.reskin == last.reskin
            && p.gradient == last.gradient && p.status == last.status
    };
    if last.species.is_empty() || (!same && run.len() < 4) {
        return None;
    }
    let mut enc = last.clone();
    if enc.level.is_empty() {
        enc.level = run.iter().rev().map(|r| r.level.clone()).find(|l| !l.is_empty()).unwrap_or_default();
    }
    // the most read, the longer on a tie; given: only names read (an unread one if there's none)
    let most = |f: fn(&Enc) -> &String, given: bool| -> String {
        let seen: Vec<&String> = run.iter().map(|r| f(r)).filter(|v| !given || !(v.is_empty() || *v == UNREAD)).collect();
        if given && seen.is_empty() {
            return if run.iter().any(|r| f(r) == UNREAD) { UNREAD.into() } else { String::new() };
        }
        let mut best: Option<(&String, (usize, usize))> = None;
        for v in &seen {
            let key = (seen.iter().filter(|x| x == &v).count(), v.chars().count());
            if best.is_none_or(|b| key > b.1) {
                best = Some((v, key));
            }
        }
        best.map(|b| b.0.clone()).unwrap_or_default()
    };
    if !same {
        enc.reskin = most(|e| &e.reskin, false);
        enc.gradient = most(|e| &e.gradient, false);
        enc.status = most(|e| &e.status, false);
    }
    // a reskin or gradient any read saw counts, like the icons: Windows misses the colored
    // rows on some reads (an Azurine Latios read once, then twice without: run from)
    if enc.reskin.is_empty() || enc.reskin == UNREAD {
        enc.reskin = most(|e| &e.reskin, true);
    }
    if enc.gradient.is_empty() || enc.gradient == UNREAD {
        enc.gradient = most(|e| &e.gradient, true);
    }
    enc.shiny = run.iter().any(|r| r.shiny);
    enc.tint = run.iter().any(|r| r.tint);
    enc.owned = run.iter().any(|r| r.owned);
    Some(enc)
}

/// Still the same Pokemon, forgiving a misread letter ("Type: Nulll").
pub fn same_mon(a: &Enc, b: &Enc) -> bool {
    if !a.level.is_empty() && !b.level.is_empty() && a.level != b.level {
        return false;
    }
    same_word(&a.species, &b.species)
        && [(&a.reskin, &b.reskin), (&a.gradient, &b.gradient)]
            .iter()
            .all(|(x, y)| [x, y].iter().any(|v| v.is_empty() || v.as_str() == UNREAD) || same_word(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn enc(species: &str) -> Enc {
        Enc { species: species.into(), ..Default::default() }
    }

    #[test]
    fn ratio_is_difflibs() {
        assert!((ratio("abcd", "bcde") - 0.75).abs() < 1e-9);
        assert!((ratio("tympole", "tympo1e") - 2.0 * 6.0 / 14.0).abs() < 1e-9);
        assert_eq!(ratio("", ""), 1.0);
    }

    #[test]
    fn same_word_forgives_one_letter_in_long_words() {
        assert!(same_word("Tympole", "Tympo1e"));
        assert!(same_word("Quick Ball", "QUICK BALL"));
        assert!(!same_word("Mew", "Mewo"));
        assert!(same_word("Type: Null", "Type:Nulll"));
        assert!(same_word("P0ke", "poke"));
    }

    #[test]
    fn fields() {
        assert!(field_ok(&json!(true), &json!(true)));
        assert!(!field_ok(&json!(true), &json!(false)));
        assert!(field_ok(&json!("any"), &json!("INFERNO")));
        assert!(!field_ok(&json!("any"), &json!("")));
        assert!(field_ok(&json!("none"), &json!("")));
        assert!(field_ok(&json!("legendary"), &json!("Galarian Articuno")));
        assert!(field_ok(&json!("legendary"), &json!("Mewtwo")));
        assert!(!field_ok(&json!("legendary"), &json!("Pikachu")));
        assert!(field_ok(&json!(["Tympole", "Marill"]), &json!("Marill")));
    }

    #[test]
    fn rules() {
        let rule: Rule = serde_json::from_value(json!({"species": "Tympole", "shiny": true})).unwrap();
        let mut e = enc("Tympole");
        assert!(!rule_ok(&rule, &e));
        e.shiny = true;
        assert!(rule_ok(&rule, &e));
        let not: Rule = serde_json::from_value(json!({"shiny": true, "not_species": "Tympole"})).unwrap();
        assert!(!rule_ok(&not, &e));
        assert_eq!(show(&rule), "species=Tympole, shiny=True");
    }

    #[test]
    fn settling() {
        let mut a = enc("Marill");
        a.level = "52".into();
        assert_eq!(settled(&[a.clone()]), None);
        let mut b = a.clone();
        b.tint = true;
        let s = settled(&[b, a.clone()]).unwrap();
        assert!(s.tint);
        // four that don't agree on the reskin: the most read
        let mut r = vec![a.clone(); 4];
        r[0].reskin = "INFERNO".into();
        r[1].reskin = "INFERN".into();
        r[2].reskin = "INFERNO".into();
        r[3].reskin = "NFERNO".into();
        assert_eq!(settled(&r).unwrap().reskin, "INFERNO");
        // the level read now and then (Windows): reads with and without it agree, and keep it
        let mut e = enc("Entei");
        e.reskin = "WAVE".into();
        let mut l = e.clone();
        l.level = "18".into();
        assert_eq!(settled(&[l.clone(), e.clone()]).unwrap().level, "18");
        assert_eq!(settled(&[e.clone(), l.clone(), e.clone()]).unwrap().level, "18");
        assert_eq!(settled(&[e.clone(), e]).unwrap().level, "");
        // a gradient read once, then missed twice (Windows): still the gradient
        let mut g = enc("Latios");
        g.level = "22".into();
        let plain = g.clone();
        g.gradient = "Azurine".into();
        assert_eq!(settled(&[g.clone(), plain.clone(), plain.clone()]).unwrap().gradient, "Azurine");
        assert_eq!(settled(&[plain.clone(), g.clone(), plain.clone(), plain.clone()]).unwrap().gradient, "Azurine");
        // seen but not read: named by a read that did
        let mut u = plain.clone();
        u.gradient = UNREAD.into();
        assert_eq!(settled(&[g.clone(), u.clone(), u.clone()]).unwrap().gradient, "Azurine");
        assert_eq!(settled(&[u.clone(), u.clone()]).unwrap().gradient, UNREAD);
        assert!(same_mon(&g, &u) && same_mon(&u, &g));
    }

    #[test]
    fn nameless() {
        let mut e = enc("");
        e.reskin = "TOXIC FLUID".into();
        e.level = "18".into();
        assert!(settled_nameless(&[e.clone(), e.clone()]).is_none());
        assert_eq!(settled_nameless(&[e.clone(), e.clone(), e.clone()]).unwrap().reskin, "TOXIC FLUID");
        assert!(settled_nameless(&[enc(""), enc(""), enc("")]).is_none()); // nothing read: not a box
        // a rule naming a species might want a nameless reskin, not a plain one
        let rule: Rule = json!({"species": "Suicune", "reskin": "Doodle"}).as_object().unwrap().clone();
        let mut d = enc("");
        d.reskin = "DOODLE".into();
        assert!(might_want(&d, &[rule.clone()]));
        assert!(!might_want(&e, &[rule]));
    }

    #[test]
    fn titles() {
        let mut e = enc("Lapras");
        e.reskin = "DOODLE".into();
        e.gradient = "mirage".into();
        e.shiny = true;
        e.tint = true;
        assert_eq!(e.title(), "Doodle reskin, Mirage gradient, Shiny, Tinted Lapras");
        assert_eq!(e.describe(), "shiny Lapras  (reskin DOODLE, gradient mirage, tint)");
    }
}
