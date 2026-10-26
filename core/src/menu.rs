//! Between encounters: the MENU and the Bag for a PP item, what's left open
//! closed, and the window's Debug mode, which goes through the bag workflow once.

use crate::battle::favorite_rows;
use crate::hunter::{Halt, Hunt, R};
use crate::layout::{self, Layout};
use crate::picture::game_shot;
use crate::px::{Frame, Pt};
use crate::reader::{self, find_tab, is_count, menu_button, menu_open, TextHit};
use crate::rules::{norm, same_word, Enc};
use crate::store::Store;
use crate::util::{clip, now, write_atomic};
use crate::vision::{find_menu_tab, tab_look};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

static AFTER_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)restored|recovered|effect|healed|was used|can't be used").unwrap());

fn size_key(f: &Frame) -> String {
    format!("{}x{}", f.width(), f.height())
}

fn as_pt(v: &Value) -> Option<Pt> {
    let a = v.as_array()?;
    Some(Pt::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?))
}

impl Hunt {
    // ------------------------------------------------------------ where the MENU's parts were

    /// Where MENU's tab and its Bag were last clicked on this screen
    /// size (menu.json): clicked at once the next time (an encounter can come in
    /// the second it takes to read them again).
    fn menu_spot(&mut self, f: &Frame, name: &str) -> Option<Value> {
        let path = self.c.store.path("menu.json");
        let spots = self.menu_spots.get_or_insert_with(|| Store::read_json(&path));
        spots.get(&size_key(f)).and_then(|s| s.get(name)).cloned()
    }

    /// MENU's tab (if it was read) and every item in the open MENU, for quick_menu.
    fn remember_menu(&mut self, f: &Frame, texts: &[TextHit], tab: Option<Pt>) {
        self.menu_spot(f, "menu"); // (what's saved, first)
        let look = tab.map(|t| tab_look(f, t));
        let spots = self.menu_spots.as_mut().unwrap();
        let mine = spots.entry(size_key(f)).or_insert_with(|| Value::Object(Map::new()));
        let Some(mine) = mine.as_object_mut() else { return };
        if let (Some(t), Some(look)) = (tab, look) {
            mine.insert("menu".into(), json!([t.x, t.y]));
            mine.insert("menu_look".into(), json!(look));
        }
        for t in texts {
            let n = norm(&t.text);
            if n == "bag" {
                let at = menu_button(t.pt(), texts, false);
                mine.insert(n, json!([at.x, at.y]));
            }
        }
        let text = Value::Object(spots.clone()).to_string();
        let _ = write_atomic(&self.c.store.path("menu.json"), text.as_bytes());
    }

    /// MENU's tab, then Bag where they were last; true if both were known
    /// (whether it worked shows in what comes next).
    fn quick_menu(&mut self, item: &str, why: &str) -> R<bool> {
        let mut f = self.grab();
        let (Some(tab), Some(at)) = (self.menu_spot(&f, "menu").as_ref().and_then(as_pt), self.menu_spot(&f, item).as_ref().and_then(as_pt))
        else { return Ok(false) };
        if self.find_mute(&f, None).is_some() {
            return Ok(false);
        }
        let looks: Option<[f64; 3]> = self.menu_spot(&f, "menu_look").and_then(|v| serde_json::from_value(v).ok());
        let end = now() + 3.0; // the battle fading out: MENU's tab back as it looks first
        while let Some(looks) = looks.filter(|_| now() < end) {
            if self.find_mute(&f, None).is_some() {
                return Ok(false);
            }
            let l = tab_look(&f, tab);
            if (0..3).all(|i| (l[i] - looks[i]).abs() < 20.0) {
                break;
            }
            self.nap(0.08)?;
            f = self.grab();
        }
        let mut word = item.to_string();
        word[..1].make_ascii_uppercase();
        self.trace(&format!("{why}: MENU and {word} where they were"));
        self.click(tab)?;
        self.nap(0.35)?;
        self.click(at)?;
        Ok(true)
    }

    /// MENU's tab, between encounters, or None if it isn't read in seconds. A
    /// battle on screen waits (its last words: "You have won the battle!") till
    /// then; with no seconds, it's None at once (an encounter came first).
    fn menu_tab(&mut self, seconds: f64) -> R<Option<Pt>> {
        let end = now() + seconds;
        loop {
            let f = self.grab();
            let battle = self.find_mute(&f, None).is_some();
            if battle && seconds <= 0.0 {
                return Ok(None);
            }
            let tab = if battle { None } else {
                let texts = self.screen_texts(&f);
                find_menu_tab(&f, &texts)
            };
            if tab.is_some() || now() >= end {
                return Ok(tab);
            }
            self.nap(self.traits.poll)?;
        }
    }

    /// The bag is open (its Close and its tabs read) within seconds.
    fn bag_up(&mut self, seconds: f64) -> R<bool> {
        let end = now() + seconds;
        while now() < end {
            let f = self.grab();
            let texts = self.screen_texts(&f);
            if texts.iter().any(|t| t.is("close")) && texts.iter().any(|t| ["recover", "recovers", "backpack"].contains(&norm(&t.text).as_str())) {
                return Ok(true);
            }
            self.nap(0.2)?;
        }
        Ok(false)
    }

    /// MENU -> Bag, between encounters, as quickly as it can (an encounter can
    /// come any moment). True once the bag's open or on its way.
    pub(crate) fn open_bag(&mut self, why: &str) -> R<bool> {
        let end = now() + 12.0; // out of the battle first (its last words can take seconds): watched, not read
        while now() < end {
            let f = self.grab();
            if self.find_mute(&f, None).is_none() {
                break;
            }
            self.nap(0.15)?;
        }
        for _ in 0..2 {
            // (right as the battle fades out the game can miss the clicks: again)
            if !self.quick_menu("bag", why)? {
                break;
            }
            if self.bag_up(1.5)? {
                return Ok(true);
            }
            let f = self.grab();
            let texts = self.screen_texts(&f);
            if menu_open(&texts) {
                break; // MENU took it, Bag didn't: Bag, read
            }
        }
        let f = self.grab();
        let texts = self.screen_texts(&f);
        let mut tab = None;
        if !menu_open(&texts) {
            let Some(t) = self.menu_tab(6.0)? else { return Ok(false) };
            self.trace(&format!("{why}: MENU at ({:.0}, {:.0})", t.x, t.y));
            self.click(t)?;
            tab = Some(t);
        }
        let end = now() + 8.0;
        let mut last = f;
        while now() < end {
            self.nap(0.3)?;
            let f = self.grab();
            let texts = self.screen_texts(&f);
            if self.find_mute(&f, None).is_some() {
                return Ok(false); // an encounter came first
            }
            if self.close_chat(&f, &texts)? {
                continue;
            }
            if let Some(hit) = texts.iter().find(|t| norm(&t.text) == "bag").map(|t| t.pt()).filter(|_| menu_open(&texts)) {
                let at = menu_button(hit, &texts, false);
                self.trace(&format!("{why}: clicking Bag at ({:.0}, {:.0})", at.x, at.y));
                self.click(at)?;
                self.remember_menu(&f, &texts, tab);
                return Ok(true);
            }
            last = f;
        }
        let shot = self.snapshot(&last, &format!("{why}-menu"));
        self.say(&format!("  (couldn't open the Bag from the MENU: {shot})"));
        self.clear_leftovers()?;
        Ok(false)
    }

    /// The bag's open: whether tab's items are favorites (at its top, a yellow
    /// tag), without changing any. False too for one not in sight.
    fn favorites_in_bag(&mut self, lay: &Layout, tab: &str, items: &[String]) -> R<Vec<(String, bool)>> {
        let split = lay.xy(layout::BAG_SPLIT, 0.0).x;
        let (mut seen, mut tabbed, end, mut texts): (Vec<(String, bool)>, bool, f64, Vec<TextHit>) =
            (Vec::new(), false, now() + 8.0, Vec::new());
        while now() < end {
            self.nap(self.traits.poll)?;
            let f = self.grab();
            let before = std::mem::take(&mut texts);
            texts = self.screen_texts(&f);
            if !self.traits.phone && !reader::bag_still(&texts, &before, split) {
                continue; // (still floating up)
            }
            if !tabbed {
                if let Some(hit) = find_tab(&texts, tab, split, f.width()) {
                    self.click(hit.pt())?;
                    tabbed = true;
                }
                continue;
            }
            if !texts.iter().any(|t| is_count(&t.text)) {
                continue; // the list isn't up yet
            }
            seen = favorite_rows(&f, &texts, items, split, lay);
            if seen.iter().all(|s| s.1) || now() > end - 5.0 {
                break;
            }
        }
        Ok(items.iter().map(|i| (i.clone(), seen.iter().any(|s| &s.0 == i && s.1))).collect())
    }

    /// After an item outside battle: what the game said ("... HP was restored",
    /// "It won't have any effect"), clicked away, and the party screen Cancelled
    /// if it stays, till the bag's list is back. What it said, or "".
    fn back_to_bag(&mut self, seconds: f64) -> R<String> {
        let (mut said, end, mut cancelled) = (String::new(), now() + seconds, 0.0);
        while now() < end {
            self.nap(self.traits.poll)?;
            let f = self.grab();
            let texts = self.screen_texts(&f);
            let shown: Vec<&str> = texts.iter().map(|t| t.text.as_str()).filter(|t| t.chars().count() > 3).collect();
            self.trace(&format!("bag: after the item: {}", clip(&shown.join(", "), 120)));
            if let Some(msg) = texts.iter().find(|t| AFTER_ITEM.is_match(&t.text)).cloned() {
                if said.is_empty() {
                    said = msg.text.clone();
                }
                self.click(msg.pt())?;
                continue;
            }
            let party = texts.iter().any(|t| {
                let l = t.text.to_lowercase();
                l.starts_with("select a pokemon") || l.starts_with("pokemon party")
            });
            if let Some(cancel) = texts.iter().find(|t| t.is("cancel")).map(|t| t.pt()) {
                if party && !said.is_empty() && now() - cancelled > 1.0 {
                    self.click(cancel)?;
                    cancelled = now();
                    continue;
                }
            }
            if texts.iter().any(|t| t.is("close")) && !party {
                return Ok(said); // the bag again
            }
        }
        Ok(said)
    }

    /// The bag's Close (it goes back to the game, not the MENU). True once it's
    /// gone (an encounter coming in closes it too).
    fn close_bag_only(&mut self) -> R<bool> {
        let (end, mut clicked) = (now() + 6.0, 0.0);
        while now() < end {
            let f = self.grab();
            if self.find_mute(&f, None).is_some() {
                return Ok(true);
            }
            let texts = self.screen_texts(&f);
            let close = texts.iter().find(|t| t.is("close")).map(|t| t.pt());
            if close.is_none() && (clicked > 0.0 || find_menu_tab(&f, &texts).is_some()) {
                return Ok(true); // (gone after its click: MENU's tab comes back by itself)
            }
            let menu = texts.iter().any(|t| norm(&t.text) == "menu");
            self.trace(&format!("bag: closing: {}, {}", if close.is_some() { "Close up" } else { "Close gone" },
                                if menu { "MENU" } else { "no MENU" }));
            if let Some(c) = close.filter(|_| now() - clicked > 1.0) {
                self.click(c)?;
                clicked = now();
            }
            self.nap(self.traits.poll)?;
        }
        Ok(false)
    }

    /// Closes a leftover menu or chat panel between encounters.
    /// True if it found something to close.
    pub(crate) fn clear_leftovers(&mut self) -> R<bool> {
        let f = self.grab();
        let texts = self.screen_texts(&f);
        if menu_open(&texts) {
            if let Some(tab) = find_menu_tab(&f, &texts) {
                self.click(tab)?;
                return Ok(true);
            }
        }
        if self.close_chat(&f, &texts)? {
            return Ok(true);
        }
        Ok(false)
    }

    /// The window's Debug mode (a right click on play), on one encounter: a
    /// Quick Ball (and Yawn, when the lead is the Yawn user and it wasn't
    /// caught), then away; MENU > Bag > Recovers: the heal and PP items,
    /// favorites as you keep them (at the top of the tab), each used once
    /// outside battle (the Pokemon Party screen); then the bag is closed.
    /// Each step is reported to the debug channel, with a picture of any
    /// failure and a summary at the end. True if every step went right.
    pub(crate) fn debug_workflow(&mut self, enc: &Enc, f: &Frame, lay: &Layout) -> R<bool> {
        let cfg = self.c.cfg.clone();
        let mut steps: Vec<(String, bool, String)> = Vec::new();
        let step = |h: &mut Hunt, steps: &mut Vec<(String, bool, String)>, name: &str, ok: bool, detail: &str, picture: Option<&Frame>| {
            steps.push((name.into(), ok, detail.into()));
            h.print(&format!("  debug: {name}: {}{}", if ok { "ok" } else { "FAILED" },
                             if detail.is_empty() { String::new() } else { format!(" ({detail})") }));
            if let Some(t) = h.timing.as_mut() {
                let lap = t.lap();
                h.print(&format!("    [{lap}]"));
            }
            if !ok {
                let p = picture.cloned().unwrap_or_else(|| h.grab());
                h.c.out.debug.shot(&format!("debug run: {name} went wrong{}",
                                            if detail.is_empty() { String::new() } else { format!(" ({detail})") }), &p, true);
                let path = h.save_picture(&crate::picture::png(&p), "debug", "debug-run");
                h.print(&format!("    (the screen then: {path})"));
            }
        };
        let said = |msgs: &[String], outcome: &str| {
            let m = if msgs.is_empty() { String::new() } else { format!(": {}", msgs.join(" / ")) };
            clip(&format!("{outcome}{m}"), 250)
        };
        let lead = reader::should(&self.region_text(f, lay, layout::MSG)).unwrap_or_default();
        let target = if lead.is_empty() { cfg.str("sleep_with") } else { lead.clone() };
        let leading = if lead.is_empty() { "?" } else { &lead };
        self.print(&format!("  debug run on {}, {leading} leading", enc.title()));
        self.c.out.debug.shot(&format!("debug run: {}, {leading} leading", enc.describe()), &game_shot(f, lay), false);
        step(self, &mut steps, "reading the battle", !enc.species.is_empty(), &enc.describe(), Some(&game_shot(f, lay)));

        // the battle: a Quick Ball; Yawn too with the Yawn user, if it's still there; then away
        let ball = Some(cfg.str("first_ball")).filter(|b| !b.is_empty()).unwrap_or_else(|| "Quick Ball".into());
        let mut lay = lay.clone();
        let battle = (|| -> R<()> {
            let used = self.use_item(&lay, "Pokeballs", &ball, None, true)?;
            step(self, &mut steps, &format!("throwing a {ball}"), used.is_some(), "", None);
            let mut outcome = "turn";
            if used.is_some() {
                let (o, _, l, msgs) = self.wait_turn(&lay)?;
                (outcome, lay) = (o, l);
                step(self, &mut steps, &format!("the {ball}'s turn"), ["caught", "turn", "over"].contains(&outcome),
                     &said(&msgs, outcome), None);
            }
            if outcome == "turn" && same_word(&cfg.str("sleep_with"), &lead) {
                let mv = Some(cfg.str("sleep_move")).filter(|m| !m.is_empty()).unwrap_or_else(|| "Yawn".into());
                let r = self.use_move(&lay, &mv)?;
                step(self, &mut steps, &mv, r == "used", r, None);
                if r == "used" {
                    let (o, _, l, msgs) = self.wait_turn(&lay)?;
                    (outcome, lay) = (o, l);
                    step(self, &mut steps, &format!("{mv}'s turn"), ["caught", "turn", "over"].contains(&outcome),
                         &said(&msgs, outcome), None);
                }
            }
            if outcome == "turn" {
                let ran = self.run_away(&lay, false, 0.25)?; // (straight on to MENU, which checks again)
                step(self, &mut steps, "running", ran, "", None);
            }
            Ok(())
        })();
        match battle {
            Err(Halt::BattleOver) => step(self, &mut steps, "the battle", true, "it ended", None),
            r => r?,
        }
        // the MENU at once, as the battle goes (no encounter comes while it or the bag is up):
        // the bag's favorites, an item, its message, the other item; then close it
        let (heal, pp) = (cfg.str("heal_item"), cfg.str("pp_item"));
        let opened = self.open_bag("debug")?;
        step(self, &mut steps, "MENU > Bag", opened, "", None);
        if opened {
            for (name, fav) in self.favorites_in_bag(&lay, "Recovers", &[heal.clone(), pp.clone()])? {
                step(self, &mut steps, &format!("{name} is a favorite"), fav,
                     if fav { "" } else { "favorite it in the bag (right click)" }, None);
            }
            for item in [&heal, &pp] {
                let used = self.use_item(&lay, "Recovers", item, Some(&target), false)?;
                let said = if used.is_some() { self.back_to_bag(8.0)? } else { String::new() };
                let who = if target.is_empty() { "?" } else { &target };
                step(self, &mut steps, &format!("a {item} on {who}"), used.is_some(), &said, None);
            }
            let closed = self.close_bag_only()?;
            step(self, &mut steps, "the bag closed", closed, "", None);
        }

        // Keep the next input away from any menu controls.
        self.click(lay.pt(layout::AFK_SPOT))?; // leave focus in the game, outside menu controls

        let bad: Vec<&(String, bool, String)> = steps.iter().filter(|s| !s.1).collect();
        let summary = if bad.is_empty() {
            format!("debug run: all {} steps went right", steps.len())
        } else {
            format!("debug run: {} of {} steps went wrong: {}", bad.len(), steps.len(),
                    bad.iter().map(|(n, _, d)| if d.is_empty() { n.clone() } else { format!("{n} ({d})") }).collect::<Vec<_>>().join("; "))
        };
        self.print(&format!("  {summary}"));
        let lines: Vec<String> = steps.iter()
            .map(|(n, ok, d)| format!("{} {n}{}", if *ok { '✓' } else { '✗' }, if d.is_empty() { String::new() } else { format!(": {d}") }))
            .collect();
        let debug = self.c.out.debug.clone();
        debug.flush();
        debug.post(&clip(&format!("{summary}\n{}", lines.join("\n")), 1900), None);
        Ok(bad.is_empty())
    }
}
