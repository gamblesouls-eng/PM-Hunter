//! In battle: the bag (balls, healing), the Fight menu, waiting out a turn,
//! the catch plan, and battles fought from the website.

use crate::hunter::{Halt, Hunt, GONE_AFTER, R, TURN_TIMEOUT};
use crate::layout::{self, Layout};
use crate::picture::{game_shot, png};
use crate::px::{Frame, Pt};
use crate::reader::{self, bag_still, find_tab, is_count, search_box, TextHit};
use crate::rules::{field_ok, norm, same_mon, same_word, Enc};
use crate::util::{lower_first, now};
use crate::vision::{self, box_visible, chat_showing, favorited, find_use_button, menu_ready};
use serde_json::{json, Value};

/// Scrolling the backpack's list (phones, when its search box can't be typed
/// into; a PC always, with the mouse wheel): first to the top, then down a page at
/// a time until the page stops changing. The list's height is taken from the tab
/// buttons beside it.
#[derive(Default)]
pub(crate) struct Scroll {
    pub at: f64,
    at_top: bool,
    /// (PC) wheel notches a page: more when a page barely moved, fewer when one skipped rows.
    notches: i32,
    /// (PC) wheel turns up so far, and whether the last turn was back up over skipped rows.
    ups: u32,
    backed: bool,
    last: Option<Vec<String>>,
    pages: u32,
}

impl Hunt {
    /// Waits for the Fight / Bag / Run menu; a battle that's over is
    /// Halt::BattleOver (out of battle the Bag button's spot is somewhere else).
    pub(crate) fn ensure_battle(&mut self, lay: &Layout) -> R<()> {
        let (start, mut end, mut looked) = (now(), now() + 4.0, 0.0);
        while now() < end {
            let (f, cur) = self.look(Some(lay.mute));
            if cur.as_ref().is_some_and(|cur| menu_ready(&f, cur)) {
                return Ok(());
            }
            // the backpack or the party screen still over it (a Full Restore's party screen
            // closed, its backpack left open) hides the battle: closed, not taken for its end
            if cur.is_none() && now() - looked > 0.8 {
                looked = now();
                if self.close_screens(5.0)? {
                    end = end.max(now() + 4.0).min(start + 20.0);
                    continue;
                }
            }
            self.nap(self.traits.poll)?;
        }
        Err(Halt::BattleOver)
    }

    /// Closes what's over the game: the Pokemon Party screen (its Close), then the backpack
    /// (its Close, by its red pill when the word isn't read); "Choose your next Pokemon."
    /// gets the next one sent out. True if anything was there.
    pub(crate) fn close_screens(&mut self, seconds: f64) -> R<bool> {
        let (end, mut any) = (now() + seconds, false);
        let mut title_before: Option<Pt> = None;
        while now() < end {
            let f = self.grab();
            let texts = self.screen_texts(&f);
            if reader::next_asked(&texts) {
                any = true;
                // a card clicked may ask what for: "Send out" (or "Switch", "Shift")
                let send = texts.iter().find(|t| {
                    let l = norm(&t.text);
                    l == "sendout" || l == "switch" || l == "shift"
                });
                if let Some(b) = send {
                    self.trace(&format!("sending it out: \"{}\"", b.text));
                    self.click(b.pt())?;
                } else if !self.send_next(&f)? {
                    break;
                }
            } else if reader::party_up(&texts) {
                any = true;
                // its Close sits right of the message, at the bottom; worked out from there only
                // while its cards are in sight (that spot is Run's once it's gone)
                let (w, h) = (f.width() as f64, f.height() as f64);
                let at = texts.iter().filter(|t| reader::close_word(t) && t.y > 0.6 * h).map(|t| t.pt()).next()
                    .or_else(|| (!vision::party_cards(&f).is_empty()).then(|| Pt::new(0.80 * w, 0.89 * h)));
                let Some(at) = at else { break };
                self.trace("closing the Pokemon Party screen");
                self.click(at)?;
            } else if let Some(title) = reader::bag_title(&texts).cloned() {
                any = true;
                // only once it holds still (a closing bag's title still reads as it slides away)
                let still = title_before.is_some_and(|b| (b.x - title.x).abs() + (b.y - title.y).abs() <= 4.0);
                title_before = Some(title.pt());
                if !still {
                    self.nap(0.25)?;
                    continue;
                }
                let row = (f.height() as f64) / 12.0;
                let at = texts.iter().filter(|t| reader::close_word(t) && t.x > title.x && (t.y - title.y).abs() < row)
                    .map(|t| t.pt()).next().or_else(|| vision::bag_close(&f, title.pt()));
                let Some(at) = at else {
                    self.trace("the backpack is open and its Close isn't found");
                    break;
                };
                self.trace("closing the backpack");
                self.click(at)?;
                title_before = None;
            } else {
                break;
            }
            self.move_mouse(Pt::new(f.width() as f64 * 0.06, f.height() as f64 * 0.7))?; // (off its buttons)
            self.nap(0.9)?;
        }
        Ok(any)
    }

    /// Yours fainted ("Choose your next Pokemon."): the first card that hasn't, sent out.
    /// False if there's none.
    pub(crate) fn send_next(&mut self, f: &Frame) -> R<bool> {
        let cards = vision::party_cards(f);
        let Some(next) = cards.iter().find(|c| !c.1).map(|c| c.0) else {
            self.say("  (every Pokemon has fainted)");
            return Ok(false);
        };
        self.say(&format!("  yours fainted: sending in the next one (card {} of {})",
                          cards.iter().position(|c| c.0 == next).unwrap_or(0) + 1, cards.len()));
        self.click(next.center())?;
        self.nap(1.0)?;
        Ok(true)
    }

    /// A click in a text box (the bag's search), a moment for Roblox to give it
    /// focus (typing at once went nowhere), what was in it replaced by text.
    fn type_in(&mut self, at: Pt, text: &str) -> R<bool> {
        self.click(at)?;
        self.nap(if self.traits.phone { 0.5 } else { 0.4 })?;
        let mut typed = self.dev.type_text(text);
        if !typed && self.traits.phone {
            self.nap(0.6)?;
            typed = self.dev.type_text(text);
        }
        self.nap(0.3)?;
        Ok(typed)
    }

    /// Bag -> tab -> item -> Use, and -> party member when the item asks for
    /// one. Favorited items sit at the top of their tab; anything else is
    /// searched for (scrolled to on a phone that can't type). The name of
    /// what was used; None on failure (the backpack is closed again). in_battle false: the bag is
    /// opening from the MENU already (restore_pp). In battle it's tried three times, the bag
    /// opened afresh each time: a bag that wouldn't read once never costs the catch.
    pub(crate) fn use_item(&mut self, lay: &Layout, tab: &str, item: &str, target: Option<&str>, in_battle: bool) -> R<Option<String>> {
        // the bag's search box would bring up the keyboard, which on a phone covers the list
        self.dev.keyboard_hidden(true);
        let prefer = std::mem::take(&mut self.prefer);
        // everything the debug run found cached: clicked at once, nothing read (no misreads)
        if !self.learning {
            if let Some(used) = self.cached_item(lay, tab, item, target, &prefer, in_battle)? {
                self.dev.keyboard_hidden(false);
                self.used_item = used.clone();
                return Ok(Some(used));
            }
        }
        let mut r = Ok(None);
        for attempt in 1..=(if in_battle { 3 } else { 1 }) {
            if attempt > 1 {
                self.trace(&format!("bag: trying again for the {item} ({attempt} of 3)"));
                self.nap(0.6)?; // (it's closing)
            }
            self.not_in_bag.clear();
            r = self.use_item_in(lay, tab, item, target, &prefer, in_battle);
            if !matches!(r, Ok(None)) || self.not_in_bag == item {
                break; // (one the bag hasn't got isn't looked for again)
            }
        }
        self.dev.keyboard_hidden(false);
        let r = r?;
        if let Some(used) = &r {
            self.used_item = used.clone();
        }
        Ok(r)
    }

    /// The bag from the cache alone (menu.json, from the debug run): its tab, the favorite's
    /// row, Use, and the party card for a heal, each clicked where it was found, with nothing read.
    /// None if any is unknown (or a preferred ball's favorite isn't): then it's read as before.
    fn cached_item(&mut self, lay: &Layout, tab: &str, item: &str, target: Option<&str>, prefer: &str,
                   in_battle: bool) -> R<Option<String>> {
        let f = self.grab();
        let mut item = item.to_string();
        if !prefer.is_empty() && norm(prefer) != norm(&item) {
            match self.spot_flag(&f, &format!("fav:{prefer}")) {
                Some(true) => item = prefer.to_string(),
                Some(false) => {}
                None => return Ok(None), // not learned yet
            }
        }
        let (Some(tab_at), Some(row)) = (self.spot(&f, &format!("bagtab:{tab}")), self.spot(&f, &format!("row:{tab}:{item}")))
        else { return Ok(None) };
        let Some(use_at) = self.spot(&f, &format!("use:{tab}:{item}")).or_else(|| self.spot(&f, &format!("use:{tab}")))
        else { return Ok(None) };
        let card = match target {
            Some(t) => match self.spot(&f, &format!("party:{in_battle}:{t}")) {
                Some(c) => Some(c),
                None => return Ok(None),
            },
            None => None,
        };
        if in_battle {
            self.ensure_battle(lay)?;
            self.trace(&format!("bag: opening it for {item} (the spots from the debug run)"));
            self.click(lay.pt(layout::BAG))?;
            self.nap(1.0)?; // it floats up as it opens
        }
        self.click(tab_at)?;
        self.nap(0.6)?;
        self.click(row)?;
        self.nap(0.6)?;
        self.click(use_at)?;
        self.move_mouse(lay.pt(layout::AFK_SPOT))?;
        if let Some(c) = card {
            self.nap(1.3)?; // the party screen
            self.click(c)?;
            self.nap(0.8)?;
        }
        Ok(Some(item))
    }

    fn keep_spots(&mut self, f: &Frame, learned: &[(String, Value)]) {
        for (k, v) in learned {
            self.put_spot(f, k, v.clone());
        }
    }

    fn use_item_in(&mut self, lay: &Layout, tab: &str, item: &str, target: Option<&str>, prefer: &str,
                   in_battle: bool) -> R<Option<String>> {
        let phone = self.traits.phone;
        let mut item = item.to_string();
        // prefer: looked for on the list first, taken if it's a favorite in sight; decided once
        let mut looking = if !prefer.is_empty() && norm(prefer) != norm(&item) { "look" } else { "done" };
        if in_battle {
            self.ensure_battle(lay)?;
            self.trace(&format!("bag: opening it for {item}"));
            self.click(lay.pt(layout::BAG))?;
        }
        self.move_mouse(lay.pt(layout::AFK_SPOT))?;
        let split = lay.xy(layout::BAG_SPLIT, 0.0).x;
        let v = lay.v(); // screen px per reference px, down
        let (mut step, mut row_y, mut tab_at, mut searched) = ("tab", 0.0, 0.0, false);
        let (mut picked_at, mut picks, mut cleared) = (0.0, 0, false);
        let mut texts: Vec<TextHit> = Vec::new();
        let mut img: Option<Frame> = None;
        let mut end = now() + 12.0;
        let opened_at = now();
        let (mut missing, mut reopened) = (0, false);
        let mut tab_hit: Option<TextHit> = None;
        let mut no_tab_since: Option<f64> = None; // the bag still, and no tab to be found
        let mut last_row_y: Option<f64> = None; // (phones) where the row was on the screenshot before
        let mut last_title: Option<TextHit> = None;
        let mut scroll = Scroll::default();
        let mut learned: Vec<(String, Value)> = Vec::new(); // the spots this goes through, kept once it worked
        while now() < end {
            if in_battle || img.is_some() {
                self.nap(self.traits.poll)?;
            }
            let f = self.grab();
            let before = std::mem::take(&mut texts);
            texts = self.screen_texts(&f);
            img = Some(f.clone());
            if self.close_chat(&f, &texts)? {
                end = end.max(now() + 3.0);
                continue;
            }
            // the bag can close under it (a chat window, a stray tap): notice, reopen it once,
            // rather than look through a bag that isn't there
            let bag_up = texts.iter().any(|t| t.is("backpack")) || find_tab(&texts, "Pokeballs", split, f.width()).is_some();
            missing = if bag_up { 0 } else { missing + 1 };
            if (step == "tab" || step == "item") && missing >= 3 && now() - opened_at > 2.0 {
                if reopened || !in_battle || !menu_ready(&f, lay) {
                    let chat = texts.iter().any(|t| t.text.to_lowercase().contains("tap here to chat"));
                    self.trace(&format!("bag: it isn't open anymore{}", if chat { " (the chat window is open)" } else { "" }));
                    break;
                }
                self.trace("bag: it closed, opening it again");
                self.click(lay.pt(layout::BAG))?;
                (reopened, missing, step) = (true, 0, "tab");
                continue;
            }
            // nothing's clicked till it stops (it floats up as it opens: a click lands where
            // the tab was a moment ago)
            let still = if phone {
                // a phone's title isn't always read: only its moving counts
                let title = texts.iter().find(|t| t.is("backpack")).cloned();
                let moved = matches!((&title, &last_title), (Some(a), Some(b)) if (a.x - b.x).abs() + (a.y - b.y).abs() > 4.0);
                last_title = title;
                !moved
            } else {
                bag_still(&texts, &before)
            };
            if step != "target" && !still {
                continue;
            }
            if step == "tab" {
                // the bag opens on the tab it was left on: the item in sight there, no click
                let names = [norm(&item), norm(prefer)];
                if texts.iter().any(|t| names.contains(&norm(&t.text)) && t.x < split) {
                    self.trace(&format!("bag: {item} is in sight already"));
                    (step, tab_at) = ("item", now()); // (and on with this read)
                } else {
                    if let Some(hit) = find_tab(&texts, tab, split, f.width()) {
                        let worked_out = !texts.iter().any(|t| t == &hit);
                        self.trace(&format!("bag: {tab} tab at ({:.0}, {:.0}){}", hit.x, hit.y,
                                            if worked_out { ", worked out from the tabs around it" } else { "" }));
                        self.click(hit.pt())?;
                        learned.push((format!("bagtab:{tab}"), json!([hit.x, hit.y])));
                        (step, tab_at) = ("item", now());
                        tab_hit = Some(hit);
                    } else if now() - *no_tab_since.get_or_insert(now()) > 5.0 {
                        self.trace(&format!("bag: no {tab} tab in 5 s"));
                        break; // (closed, and opened afresh)
                    }
                    continue;
                }
            }
            if step == "item" {
                if looking != "done" {
                    // favorites are at the top: prefer if it's one of those in sight, else item (never
                    // searched for: a search empties the list of the favorites that are there)
                    let mut rows: Vec<f64> = texts.iter().filter(|t| norm(&t.text) == norm(prefer) && t.x < split)
                        .map(|t| t.y).collect();
                    rows.sort_by(f64::total_cmp);
                    let listed = texts.iter().any(|t| is_count(&t.text));
                    if let Some(&y) = rows.first() {
                        if favorited(&f, &texts, y, lay) {
                            self.print(&format!("  {prefer} is a favorite: throwing that instead of the {item}"));
                            item = prefer.to_string();
                        }
                        looking = "done";
                    } else if listed || now() - tab_at > 3.0 {
                        looking = "done"; // not in sight (or no list read): item
                    } else {
                        continue; // the list isn't up yet
                    }
                }
                // exact names: "Dive Ball" and "Dream Ball" are one misread apart
                let row = texts.iter().filter(|t| norm(&t.text) == norm(&item) && t.x < split)
                    .min_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x))).cloned();
                // (phones) a list still gliding from a swipe moves rows under the finger, and a tap
                // only stops the glide: tapped once the row holds still
                let gliding = phone && scroll.pages + scroll.at_top as u32 > 0
                    && row.as_ref().is_some_and(|r| last_row_y.is_none_or(|y| (r.y - y).abs() > 6.0 * v));
                if let (Some(r), true) = (&row, gliding) {
                    last_row_y = Some(r.y);
                } else if let Some(r) = row {
                    row_y = r.y;
                    let count = texts.iter().find(|t| is_count(&t.text) && (t.y - row_y).abs() < 30.0 * v).cloned();
                    // (phones) a tap that didn't select it: another part of the row
                    let spot = match (&count, picks) {
                        (Some(c), 1) if phone => Pt::new(c.x, r.y),
                        (Some(c), n) if phone && n > 1 => Pt::new((r.x + c.x) / 2.0, r.y),
                        _ => r.pt(),
                    };
                    let left = count.map(|c| c.text).unwrap_or_else(|| "?".into());
                    self.trace(&format!("bag: clicking {item} ({left} in the bag) at ({:.0}, {:.0})", spot.x, spot.y));
                    self.click(spot)?;
                    // a favorite's row (at the top of the list) is the same every time; one
                    // scrolled to isn't
                    if picks == 0 && favorited(&f, &texts, r.y, lay) {
                        learned.push((format!("row:{tab}:{item}"), json!([spot.x, spot.y])));
                    }
                    self.move_mouse(lay.pt(layout::AFK_SPOT))?;
                    (step, picked_at) = ("use", now());
                } else if phone && tab == "Pokeballs" && scroll.pages + scroll.at_top as u32 == 0 && now() - tab_at > 1.5
                    && favorites_cover_list(&f, &texts, split, lay)
                {
                    // favorites sit at the top, a plain row under them is in sight: all the
                    // favorites are, and this ball isn't one. Not searched for, not scrolled
                    // to: the plan's other balls instead
                    self.trace(&format!("bag: {item} isn't among the favorites in sight"));
                    self.not_in_bag = item.clone();
                    break;
                } else if phone && !searched && now() - tab_at > 1.5 {
                    last_row_y = None;
                    // not a favorite: type it into the tab's search box (phones only: a PC never
                    // types a search, its list is scrolled with the wheel)
                    let b = search_box(&texts, split, v).or_else(|| self.search_box_below_close(&texts, split, v));
                    searched = true;
                    if let Some(b) = b {
                        let typed = self.type_in(b, &item)?;
                        self.trace(&if typed { format!("bag: searching for {item}") }
                                   else { "bag: couldn't type into the search box".to_string() });
                        if typed {
                            tab_at = now();
                        }
                    }
                } else if (searched || !phone) && now() - tab_at > 1.5 && now() > scroll.at {
                    last_row_y = None;
                    // not in sight: scroll the list, top to bottom
                    if scroll.pages == 0 && !scroll.at_top {
                        if phone {
                            if let Some(t) = texts.iter().find(|t| {
                                t.x > split && t.text == t.text.to_uppercase() && reader::two_letters(&t.text)
                                    && !t.is("close") && !is_count(&t.text)
                            }) {
                                let shows = t.text.clone();
                                self.say(&format!("  (the bag's search box says \"{shows}\": clear it in Roblox, it hides the rest)"));
                            }
                        } else if let Some(t) = reader::search_leftover(&texts, split, v).filter(|_| !cleared) {
                            // a search left in the box (an older Hunt typed "Dive Ball") hides the rest
                            // of the list: emptied, nothing typed
                            cleared = true;
                            self.trace(&format!("bag: the search box says \"{}\": emptying it", t.text));
                            self.type_in(t.pt(), "")?;
                            self.move_mouse(lay.pt(layout::AFK_SPOT))?;
                            tab_at = now();
                            continue;
                        }
                    }
                    if !self.scroll_bag(&mut scroll, tab_hit.as_ref(), &texts, split)? {
                        if scroll.at_top {
                            // (looked all through it: not when the list couldn't be scrolled at all)
                            self.trace(&format!("bag: looked through {tab}, no {item}"));
                            self.not_in_bag = item.clone();
                        } else {
                            self.trace(&format!("bag: {item} isn't in sight and the list couldn't be scrolled"));
                        }
                        break;
                    }
                    end = end.max(now() + 4.0); // scrolling takes a while
                }
            } else if step == "use" {
                let use_at = find_use_button(&f, row_y)
                    .or_else(|| texts.iter().find(|t| t.is("use") && t.y > row_y).map(|t| t.pt()));
                // the panel over Use names what's selected: never use anything else
                let named: Vec<(String, f64)> = match use_at {
                    Some(u) => texts.iter().filter(|t| (0.0..=140.0 * v).contains(&(u.y - t.y)) && t.x < u.x)
                        .map(|t| (t.text.clone(), t.y)).collect(),
                    None => Vec::new(),
                };
                if let Some(u) = use_at.filter(|_| named.iter().any(|(t, _)| norm(t) == norm(&item))) {
                    self.trace(&format!("bag: {item} is selected: Use"));
                    self.click(u)?;
                    learned.push((format!("use:{tab}:{item}"), json!([u.x, u.y])));
                    learned.push((format!("use:{tab}"), json!([u.x, u.y])));
                    self.move_mouse(lay.pt(layout::AFK_SPOT))?;
                    if target.is_none() {
                        self.keep_spots(&f, &learned);
                        return Ok(Some(item));
                    }
                    step = "target";
                    end = end.max(now() + 6.0); // the party screen takes a moment to come up
                } else if (!named.is_empty() && now() - picked_at > 0.8) || now() - picked_at > 2.5 {
                    let shown = named.iter().min_by(|a, b| a.1.total_cmp(&b.1)).map(|n| n.0.clone())
                        .unwrap_or_else(|| "nothing".into());
                    picks += 1;
                    if picks > 3 {
                        self.trace(&format!("bag: {shown} is selected, not {item}: giving up"));
                        break;
                    }
                    self.trace(&format!("bag: {shown} is selected, not {item}: picking it again"));
                    step = "item";
                    last_row_y = None;
                }
            } else if step == "target" {
                // in battle "Choose a Pokemon"; outside one the Pokemon Party screen, "Select a
                // pokemon to heal with a Full Restore", its cards named "Clodsire - Lv. 109"
                let party = texts.iter().any(|t| {
                    let l = t.text.to_lowercase();
                    l.starts_with("choose a pokemon") || l.starts_with("select a pokemon") || l.starts_with("pokemon party")
                });
                let target = target.unwrap_or("");
                if party {
                    // by its name; read as "sire" (the big white names on light blue), the first
                    // card that hasn't fainted: the one out in battle, the lead outside one
                    let card = || vision::party_cards(&f).into_iter().find(|c| !c.1).map(|c| c.0.center());
                    let named = texts.iter().find(|t| reader::is_target(target, &t.text)).map(|t| t.pt());
                    if let Some(hit) = named.or_else(card) {
                        self.trace(&format!("bag: {item} on {target}{}", if named.is_none() { " (its card, the first)" } else { "" }));
                        self.click(hit)?;
                        learned.push((format!("party:{in_battle}:{target}"), json!([hit.x, hit.y])));
                        self.keep_spots(&f, &learned);
                        if in_battle {
                            // on a Pokemon that doesn't need it (its HP misread): "It won't have any
                            // effect.", and the party screen stays: its Close, back to the battle
                            let end = now() + 2.5;
                            while now() < end {
                                self.nap(0.3)?;
                                let f = self.grab();
                                let texts = self.screen_texts(&f);
                                if texts.iter().any(|t| t.text.to_lowercase().contains("any effect")) {
                                    self.trace(&format!("bag: the {item} won't have any effect: Close"));
                                    if let Some(c) = texts.iter().find(|t| t.is("close")) {
                                        self.click(c.pt())?;
                                    }
                                    self.nap(0.8)?;
                                    break;
                                }
                                if !texts.iter().any(|t| t.text.to_lowercase().starts_with("pokemon party")
                                                     || t.text.to_lowercase().starts_with("choose a pokemon")) {
                                    break; // the party screen's gone: it was used
                                }
                            }
                        }
                        return Ok(Some(item));
                    }
                }
            }
        }
        let read = texts.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(", ");
        let shot = img.as_ref().map(|f| self.snapshot(f, "bag")).unwrap_or_else(|| "-".into());
        self.say(&format!("  (couldn't use {item}: {shot}, text read: {})", crate::util::clip(&read, 300)));
        if in_battle {
            // the party screen and the backpack under it, both (one Close left the backpack
            // open, the battle hidden, taken for over)
            self.close_screens(5.0)?;
        } else if let Some(t) = texts.iter().find(|t| t.is("close")) {
            self.click(t.pt())?; // (the MENU's bag stays for the next item)
        }
        Ok(None)
    }

    /// The search box as a phone shows it: right of the list's names, under
    /// Close, above the rows.
    fn search_box_below_close(&self, texts: &[TextHit], split: f64, v: f64) -> Option<Pt> {
        let close = texts.iter().find(|t| t.is("close"));
        let rows = texts.iter().filter(|t| is_count(&t.text)).map(|t| t.y).fold(None, |m: Option<f64>, y| Some(m.map_or(y, |m| m.min(y))));
        texts.iter()
            .filter(|t| {
                t.x > split && Some(*t) != close && !is_count(&t.text)
                    && close.is_none_or(|c| t.y > c.y + 20.0 * v) && rows.is_none_or(|r| t.y < r - 20.0 * v)
            })
            .min_by(|a, b| a.y.total_cmp(&b.y))
            .map(|t| t.pt())
    }

    fn scroll_bag(&mut self, s: &mut Scroll, tab: Option<&TextHit>, texts: &[TextHit], split: f64) -> R<bool> {
        let Some(tab) = tab else { return Ok(false) };
        // the list's height from the bag's own tab labels (not a chat window's lines)
        let mut column: Vec<f64> = texts.iter()
            .filter(|t| (t.x - tab.x).abs() < 0.15 * (split - tab.x)
                && reader::TABS.iter().any(|tb| norm(tb) == norm(&t.text) || same_word(tb, &t.text)))
            .map(|t| t.y)
            .collect();
        column.push(tab.y);
        let top = column.iter().cloned().fold(f64::INFINITY, f64::min);
        let bottom = column.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if bottom - top < 50.0 {
            return Ok(false);
        }
        let x = (tab.x + split) / 2.0;
        let (up, down) = (top + 0.3 * (bottom - top), top + 0.8 * (bottom - top));
        if !self.traits.phone {
            return self.wheel_bag(s, Pt::new(x, (top + bottom) / 2.0), texts, tab, split);
        }
        if !s.at_top {
            self.trace("bag: not in sight, scrolling to the top");
            for _ in 0..3 {
                self.swipe(Pt::new(x, up), Pt::new(x, down), 0.25)?; // fast: fling to the top
            }
            s.at_top = true;
        } else {
            // the list rows on this page (not the tabs); a page that's mostly the same as the
            // last one is the bottom
            let mut rows: Vec<String> = texts.iter()
                .filter(|t| t.x > tab.x + 0.15 * (split - tab.x) && t.x < split)
                .map(|t| norm(&t.text)).filter(|t| t.len() > 2).collect();
            rows.sort();
            rows.dedup();
            if let Some(before) = &s.last {
                let same = rows.iter().filter(|r| before.contains(r)).count();
                if !rows.is_empty() && same as f64 >= 0.7 * rows.len().min(before.len()) as f64 {
                    return Ok(false);
                }
            }
            s.pages += 1;
            if s.pages > 12 {
                return Ok(false);
            }
            s.last = Some(rows);
            self.trace("bag: next page");
            self.swipe(Pt::new(x, down), Pt::new(x, up), 0.7)?; // slow: about half a page, no fling
        }
        s.at = now() + 0.8; // look again once it has settled
        Ok(true)
    }

    /// scroll_bag on a PC: the wheel over the list, up till the page stops changing (the
    /// top), then down a page at a time: a few notches, more while a page barely moves,
    /// back up and fewer when one shares no row with the page before (rows skipped). The
    /// same page twice is the bottom.
    fn wheel_bag(&mut self, s: &mut Scroll, over: Pt, texts: &[TextHit], tab: &TextHit, split: f64) -> R<bool> {
        // the list's rows: names on a row with a count ("x985"), not the tab's heading above
        let counts: Vec<f64> = texts.iter().filter(|t| is_count(&t.text) && t.x > split).map(|t| t.y).collect();
        let gap = 0.5 * counts.windows(2).map(|w| (w[1] - w[0]).abs()).fold(f64::INFINITY, f64::min).min(60.0);
        let mut rows: Vec<String> = texts.iter()
            .filter(|t| t.x > tab.x + 0.15 * (split - tab.x) && t.x < split && counts.iter().any(|y| (y - t.y).abs() < gap))
            .map(|t| norm(&t.text)).filter(|t| t.len() > 2).collect();
        rows.sort();
        rows.dedup();
        if !s.at_top {
            if s.ups == 0 || s.last.as_ref() != Some(&rows) {
                if s.ups < 8 {
                    self.trace("bag: not in sight, scrolling the list to the top");
                    s.ups += 1;
                    s.last = Some(rows);
                    self.wheel(over, 40)?;
                    s.at = now() + 0.6;
                    return Ok(true);
                }
            }
            (s.at_top, s.notches, s.last) = (true, 3, None);
        }
        if let Some(before) = &s.last {
            if !s.backed && (rows.is_empty() || &rows == before) {
                return Ok(false); // the bottom (or it doesn't scroll)
            }
            let same = rows.iter().filter(|r| before.contains(r)).count();
            if same == 0 && !s.backed && s.notches > 1 {
                // a whole page went by unseen: back, and smaller steps
                self.wheel(over, s.notches)?;
                (s.notches, s.backed) = ((s.notches / 2).max(1), true);
                self.trace(&format!("bag: rows skipped, back up ({} notches a page now)", s.notches));
                s.at = now() + 0.6;
                return Ok(true);
            }
            if !s.backed && same as f64 >= 0.7 * rows.len().min(before.len()) as f64 {
                s.notches = (s.notches * 2).min(24); // barely moved: bigger steps
            }
        }
        if !s.backed {
            s.last = Some(rows);
        }
        s.backed = false;
        s.pages += 1;
        if s.pages > 30 {
            return Ok(false);
        }
        self.trace(&format!("bag: next page ({} notches)", s.notches));
        self.wheel(over, -s.notches)?;
        s.at = now() + 0.6; // look again once it has settled
        Ok(true)
    }

    /// Fight -> move. "used", "no pp" (it goes Back to the menu then), or "no menu".
    pub(crate) fn use_move(&mut self, lay: &Layout, mv: &str) -> R<&'static str> {
        self.ensure_battle(lay)?;
        self.click(lay.pt(layout::FIGHT))?;
        self.move_mouse(lay.pt(layout::IDLE_SPOT))?;
        let v = lay.v();
        // the move where the debug run found it: clicked without reading the list (its PP and names
        // misread); only whether the move list is still up after tells it didn't take
        let shot = self.grab();
        if let (false, Some(at)) = (self.learning, self.spot(&shot, &format!("move:{mv}"))) {
            self.nap(0.7)?;
            self.trace(&format!("fight: {mv} (the spot from the debug run)"));
            self.move_pp = None;
            self.click(at)?;
            self.move_mouse(lay.pt(layout::IDLE_SPOT))?;
            self.nap(1.2)?;
            let f = self.grab();
            let texts = self.screen_texts(&f);
            if texts.iter().any(|t| t.is("back")) && !texts.iter().any(|t| t.is("fight")) {
                self.trace(&format!("fight: {mv} didn't take (no PP left?), back"));
                self.back_to_menu(lay, 6.0)?;
                return Ok("no pp");
            }
            return Ok("used");
        }
        let end = now() + 6.0;
        while now() < end {
            self.nap(self.traits.poll)?;
            let f = self.grab();
            let texts = self.screen_texts(&f);
            if self.close_chat(&f, &texts)? {
                continue;
            }
            let Some(hit) = texts.iter().find(|t| t.text.to_lowercase() == mv.to_lowercase()).cloned() else { continue };
            // the moves on this row, left to right, and their "PP / max" under them
            let mut row: Vec<f64> = texts.iter().filter(|t| (t.y - hit.y).abs() < 15.0 * v && !reader::has_digit(&t.text))
                .map(|t| t.x).collect();
            row.sort_by(f64::total_cmp);
            let mut by_x: Vec<&TextHit> = texts.iter().collect();
            by_x.sort_by(|a, b| a.x.total_cmp(&b.x));
            let pps: Vec<i64> = by_x.iter().filter(|t| 20.0 * v < t.y - hit.y && t.y - hit.y < 120.0 * v)
                .flat_map(|t| reader::pp_numbers(&t.text)).collect();
            // one PP per move, or they can't be told apart (a slash misread, two read as one)
            let i = row.iter().position(|x| *x == hit.x).filter(|_| pps.len() == row.len());
            if let Some(i) = i.filter(|i| pps[*i] == 0) {
                let _ = i;
                self.trace(&format!("fight: {mv} has no PP left, back"));
                self.back_to_menu(lay, 6.0)?;
                return Ok("no pp");
            }
            for m in [mv.to_string(), self.c.cfg.str("sleep_move"), self.c.cfg.str("guard_move")] {
                if let Some(t) = texts.iter().find(|t| !m.is_empty() && t.text.to_lowercase() == m.to_lowercase()) {
                    self.put_spot(&f, &format!("move:{m}"), json!([t.x, t.y]));
                }
            }
            self.trace(&format!("fight: {mv}{}", i.map(|i| format!(" ({} PP)", pps[i])).unwrap_or_default()));
            self.move_pp = i.map(|i| pps[i]);
            self.click(hit.pt())?;
            self.move_mouse(lay.pt(layout::IDLE_SPOT))?;
            // the move list goes once a move's used; if it stays the click didn't take (no PP
            // left, its count misread): back, as for no PP (the battle stood in the list)
            let still = now() + 2.5;
            while now() < still {
                self.nap(0.3)?;
                let f = self.grab();
                let texts = self.screen_texts(&f);
                let listed = texts.iter().any(|t| t.text.to_lowercase() == mv.to_lowercase()
                                              && (t.x - hit.x).abs() < 30.0 * v && (t.y - hit.y).abs() < 15.0 * v);
                if !listed || !texts.iter().any(|t| t.is("back")) {
                    return Ok("used");
                }
            }
            self.trace(&format!("fight: {mv} didn't take (no PP left?), back"));
            self.back_to_menu(lay, 6.0)?;
            return Ok("no pp");
        }
        self.trace(&format!("fight: didn't find {mv} in the move menu"));
        Ok("no menu")
    }

    /// After an action, waits until the turn is over: "caught", "turn" (the menu
    /// is back), "fainted" (the wild one, its poison say), "over" (the battle ended
    /// without a catch) or "timeout"; the
    /// last screenshot, its layout and what the battle said.
    pub(crate) fn wait_turn(&mut self, lay: &Layout) -> R<(&'static str, Option<Frame>, Layout, Vec<String>)> {
        let (start, mut seen, mut gone) = (now(), false, None::<f64>);
        let mut msgs: Vec<String> = Vec::new();
        let mut lay = lay.clone();
        let mut last = None;
        let mut looked = 0.0;
        while now() - start < TURN_TIMEOUT {
            self.nap(self.traits.poll)?;
            let (f, cur) = self.look(Some(lay.mute));
            let Some(cur) = cur else {
                // yours fainted ("Choose your next Pokemon."), a backpack or party screen still
                // up: the battle's under it. Otherwise it's over.
                if now() - start > 1.5 && now() - looked > 0.9 {
                    looked = now();
                    let texts = self.screen_texts(&f);
                    if reader::next_asked(&texts) || reader::party_up(&texts) || reader::bag_title(&texts).is_some() {
                        if reader::next_asked(&texts) {
                            msgs.push("(yours fainted: the next one sent out)".into());
                        }
                        self.close_screens(6.0)?;
                        gone = None;
                        continue;
                    }
                }
                if seen {
                    let g = *gone.get_or_insert(now());
                    if now() - g > 4.0 {
                        let caught = msgs.iter().any(|m| reader::caught_said(m));
                        return Ok((if caught { "caught" } else { "over" }, Some(f), lay, msgs));
                    }
                }
                last = Some(f);
                continue;
            };
            (seen, gone, lay) = (true, None, cur.clone());
            let msg = self.region_text(&f, &cur, layout::MSG);
            if !msg.is_empty() && msgs.last() != Some(&msg) {
                msgs.push(msg.clone());
            }
            if reader::caught_said(&msg) {
                return Ok(("caught", Some(f), cur, msgs));
            }
            if reader::wild_fainted(&msg) {
                return Ok(("fainted", Some(f), cur, msgs));
            }
            if now() - start > 1.5 && menu_ready(&f, &cur) && msg.to_lowercase().contains("should") {
                return Ok(("turn", Some(f), cur, msgs));
            }
            last = Some(f);
        }
        Ok(("timeout", last, lay, msgs))
    }

    /// A move is out of PP: balls for the rest of this battle. No PP item while hunting
    /// (the game's glitched with it: a Max Elixir went on a lead with every move full);
    /// the next battle reads the moves' PP again.
    pub(crate) fn need_pp(&mut self, lead: &str, mv: &str) {
        if self.pp_for.is_empty() {
            self.pp_for = if lead.is_empty() { self.c.cfg.str("sleep_with") } else { lead.to_string() };
            self.print(&format!("  {mv} is out of PP: balls for the rest of this battle"));
        }
    }

    /// Your Pokemon's HP from its bar's colors; its "350 / 376" read only when the bar can't
    /// be seen (Windows read "412 / 415" as 31%, 9400% or nothing: a Full Restore every turn).
    pub(crate) fn own_hp(&mut self, f: &Frame, lay: &Layout) -> Option<f64> {
        if let Some(hp) = vision::hp_bar(f, lay) {
            return Some(hp);
        }
        let t = self.region_text(f, lay, layout::PLAYER_BOX);
        reader::hp(&t)
    }

    /// catching(), stopping when the battle ends under it.
    pub(crate) fn catch(&mut self, enc: &Enc, f: &Frame, lay: &Layout, first_ball: bool) -> R<(String, String)> {
        let mut progress = (0usize, 0u32, String::new());
        self.pp_for.clear(); // out of PP was for the last battle: this one reads the moves again
        match self.catching(enc, f, lay, first_ball, &mut progress) {
            Err(Halt::BattleOver) => {
                self.print("  the battle is over");
                Ok(("over".into(), format!("{} ball(s), {} turns", progress.0, progress.1)))
            }
            r => r,
        }
    }

    /// Catches with the catch plan (built in: config::catch_plan): a first-turn ball, then
    /// Yawn, Protect while it gets drowsy, and the sleep ball once it's asleep;
    /// Timer Balls instead once timer_after turns have gone by. Statused
    /// targets (Yawn can't work) get the other ball. Heals and restores PP
    /// along the way. (result, details): "caught", "gave up", "no bag"
    /// (couldn't use the bag), "over", "timeout" or "taken over".
    fn catching(&mut self, enc: &Enc, f: &Frame, lay: &Layout, first_ball: bool,
                progress: &mut (usize, u32, String)) -> R<(String, String)> {
        let cfg = self.c.cfg.clone();
        let (mut f, mut lay) = (f.clone(), lay.clone());
        let mut lead = reader::should(&self.region_text(&f, &lay, layout::MSG)).unwrap_or_else(|| cfg.str("lead"));
        let legendary = field_ok(&Value::from("legendary"), &Value::from(enc.species.as_str())); // much harder to catch
        let max_balls = cfg.num(if legendary { "legendary_max_balls" } else { "max_balls" }) as usize;
        let max_turns = cfg.num(if legendary { "legendary_max_turns" } else { "max_turns" }) as u32;
        let (mut balls, mut turns, mut drowsy, mut heals): (Vec<String>, u32, bool, u32) = (Vec::new(), 0, false, 0);
        // poisoned or burned, as the battle said (its badge isn't always read): balls every
        // turn, no Yawn or Protect (a Cyber Giratina was poisoned by Clodsire's ability and
        // fainted while turns went on those)
        let mut statused = false;
        let mut missing: Vec<String> = Vec::new(); // balls the bag hasn't got
        let mut cur = enc.clone();
        // A Repeat Ball only helps on species you've caught before (the little Poke Ball
        // icon); anything new gets new_ball instead.
        let new = if enc.owned { None } else { Some(cfg.str("new_ball")).filter(|b| !b.is_empty()) };
        let mut timer = cfg.str("timer_ball");
        let first = if first_ball { cfg.str("first_ball") } else { String::new() };
        let or_new = |b: String| new.clone().unwrap_or(b);
        self.print(&format!("  catching {} Lv{} with {}: up to {max_balls} balls, {max_turns} turns{}{}", enc.title(),
                            if enc.level.is_empty() { "?" } else { &enc.level }, if lead.is_empty() { "?" } else { &lead },
                            if legendary { " (legendary)" } else { "" },
                            if enc.owned { String::new() } else {
                                format!(", not caught before: {}", new.clone().unwrap_or_else(|| "same balls".into()))
                            }));
        loop {
            if self.c.remote() {
                self.print("  taken over mid-battle");
                return Ok(("taken over".into(), format!("{} ball(s), {turns} turns", balls.len())));
            }
            let hp = self.own_hp(&f, &lay);
            self.print(&format!("  turn {}: {} at {}, {} {}{}", turns + 1, if lead.is_empty() { "ours" } else { &lead },
                                hp.map(|h| format!("{:.0}%", h * 100.0)).unwrap_or_else(|| "? HP".into()), cur.species,
                                if cur.status.is_empty() { "no status" } else { &cur.status },
                                if drowsy { ", drowsy" } else { "" }));
            let mut ball: Option<String> = None;
            if turns == 0 && !first.is_empty() {
                ball = Some(first.clone());
            } else if hp.is_some_and(|h| h < cfg.catch.get("heal_below").and_then(Value::as_f64).unwrap_or(0.4)) && heals < 5
                // read twice: one misread of "367/367" shouldn't spend a Full Restore
                && { let again = self.grab(); self.own_hp(&again, &lay) }
                    .is_some_and(|h| h < cfg.catch.get("heal_below").and_then(Value::as_f64).unwrap_or(0.4))
            {
                let item = cfg.str("heal_item");
                self.trace(&format!("low HP: {item}"));
                if self.use_item(&lay, "Recovers", &item, Some(&lead), true)?.is_none() {
                    return Ok(("no bag".into(), format!("{item} failed")));
                }
                heals += 1;
            } else if cur.status == "SLP" {
                ball = Some(or_new(cfg.str("asleep_ball")));
            } else if !cur.status.is_empty() || statused {
                ball = Some(or_new(cfg.str("awake_ball"))); // poisoned etc.: Yawn can't put it to sleep
            } else if !same_word(&cfg.str("sleep_with"), &lead) || !self.pp_for.is_empty() {
                // only the Yawn user puts it to sleep, with PP left; others just throw
                ball = Some(or_new(cfg.str("awake_ball")));
            } else {
                let mv = cfg.str(if drowsy { "guard_move" } else { "sleep_move" });
                let used = self.use_move(&lay, &mv)?;
                if used == "used" {
                    drowsy = !drowsy;
                } else {
                    if used == "no pp" {
                        self.need_pp(&lead, &mv);
                    }
                    ball = Some(or_new(cfg.str("awake_ball")));
                }
            }
            let mut plain = None;
            if let Some(b) = ball.clone() {
                if !timer.is_empty() && turns as f64 >= cfg.catch.get("timer_after").and_then(Value::as_f64).unwrap_or(9.0) {
                    plain = Some(b);
                    ball = Some(timer.clone());
                }
            }
            if let Some(mut b) = ball {
                if balls.len() >= max_balls {
                    return Ok(("gave up".into(), format!("{} balls", balls.len())));
                }
                // a ball the bag hasn't got (no Dive Balls: a Skeledirge let go) isn't the end
                // of the catch: the plan's other balls
                let mut others: Vec<String> = [plain.clone(), Some(cfg.str("awake_ball")), Some(cfg.str("asleep_ball")),
                                               Some(cfg.str("timer_ball")), Some(cfg.str("first_ball"))]
                    .into_iter().flatten().filter(|x| !x.is_empty()).collect();
                others.dedup();
                let mut used = None;
                loop {
                    if missing.contains(&b) {
                        let Some(next) = others.iter().find(|x| !missing.contains(x)) else { break };
                        b = next.clone();
                    }
                    self.trace(&format!("throwing a {b} (#{})", balls.len() + 1));
                    self.prefer = cfg.str("level_ball");
                    used = self.use_item(&lay, "Pokeballs", &b, None, true)?;
                    if used.is_some() {
                        break;
                    }
                    if self.not_in_bag == b {
                        self.say(&format!("  (no {b} in the bag: the other balls from now on)"));
                        missing.push(b.clone());
                        if b == timer {
                            timer.clear();
                        }
                        continue;
                    }
                    if let Some(p) = plain.take() {
                        self.trace(&format!("no {b} to throw: {p}s from now on"));
                        timer.clear();
                        b = p;
                        continue;
                    }
                    break;
                }
                let Some(used) = used else { return Ok(("no bag".into(), format!("{b} failed"))) };
                balls.push(used.clone());
                *progress = (balls.len(), turns, used);
            }
            let (outcome, img, l, msgs) = self.wait_turn(&lay)?;
            lay = l;
            turns += 1;
            *progress = (balls.len(), turns, balls.last().cloned().unwrap_or_default());
            self.print(&format!("  turn {turns}: {outcome}{}",
                                if msgs.is_empty() { String::new() } else { format!(". The battle said: {}", msgs.join(" / ")) }));
            if self.c.opts.debug {
                if let Some(img) = &img {
                    self.c.out.debug.shot(&format!("catch turn {turns}: {outcome}"), &game_shot(img, &lay), false);
                }
            }
            if outcome == "timeout" {
                // what was on screen, to tell why from the log
                let shot = img.clone().unwrap_or_else(|| self.grab());
                self.trouble(&format!("turn {turns} never ended"), &shot);
            }
            let details = format!("{} ball(s), last {}, {turns} turns", balls.len(), balls.last().map(|s| s.as_str()).unwrap_or("-"));
            let Some(img) = img.filter(|_| outcome == "turn") else { return Ok((outcome.into(), details)) };
            f = img;
            if msgs.iter().any(|m| m.to_lowercase().contains("fell asleep")) {
                drowsy = false;
            }
            // yours fainted and the next went out: "What should Rotom do?" (only the Yawn user
            // sleeps it; another just throws)
            let ours_fainted = msgs.iter().any(|m| m.to_lowercase().contains("fainted") && !reader::wild_fainted(m));
            if let Some(now_out) = msgs.last().and_then(|m| reader::should(m)).filter(|l| ours_fainted && !same_word(l, &lead)) {
                self.trace(&format!("{now_out} is out now"));
                lead = now_out;
            }
            if !statused && msgs.iter().any(|m| reader::wild_statused(m)) {
                statused = true;
                self.trace("it has a status (the battle said so): balls from now on");
            }
            if turns >= max_turns {
                return Ok(("gave up".into(), details));
            }
            if chat_showing(&f, &lay) || !box_visible(&f, &lay) {
                self.trace("can't see its box this turn");
                self.nap(1.0)?; // read again next turn; keep going with what we know
                continue;
            }
            let again = self.read_enemy(&f, &lay);
            if !again.species.is_empty() && !same_mon(&again, enc) {
                // the next encounter already (caught, its words gone by), or this one misread
                // (a Windows read of "Mewtwo" was taken for a catch, and the battle went on):
                // the next only if another read says the same, and it's another Pokemon
                self.nap(0.4)?;
                let f2 = self.grab();
                let twice = self.read_enemy(&f2, &lay);
                if same_mon(&twice, &again) && !same_word(&again.species, &enc.species) {
                    return Ok(("caught".into(), details));
                }
                self.trace(&format!("read as {} this turn: the same battle", again.describe()));
                continue;
            }
            cur = again;
        }
    }

    // ------------------------------------------------------------ the website's battles

    /// Reads your Pokemon's box for the page; a box half covered (the level-up
    /// stats panel) leaves the last whole reading.
    fn read_lead(&mut self, f: &Frame, lay: &Layout) {
        let texts = self.ocr(f, Some(lay.boxed(layout::PLAYER_BOX)), Self::zoom(lay));
        let lead = reader::lead(&texts);
        if lead.complete() {
            self.c.web.lock().unwrap().lead = lead;
        }
    }

    /// Fight -> reads the moves and their PP -> Back to Fight / Bag / Run.
    fn read_moves(&mut self, lay: &Layout) -> R<Vec<reader::Move>> {
        self.ensure_battle(lay)?;
        self.click(lay.pt(layout::FIGHT))?;
        self.move_mouse(lay.pt(layout::IDLE_SPOT))?;
        let (mut moves, end) = (Vec::new(), now() + 5.0);
        let mut menu_text = Vec::new();
        while now() < end && moves.is_empty() {
            self.nap(0.4)?;
            let f = self.grab();
            if menu_ready(&f, lay) {
                continue; // the Fight menu isn't up yet
            }
            menu_text = self.ocr(&f, Some(lay.boxed(layout::MSG)), Self::zoom(lay));
            moves = reader::moves(&menu_text);
        }
        let shown = |p: Option<i64>| p.map(|p| p.to_string()).unwrap_or_else(|| "?".into());
        self.trace(&format!("fight: moves {}{}",
                            moves.iter().map(|m| format!("{} {}/{}", m.name, shown(m.pp), shown(m.max))).collect::<Vec<_>>().join(", "),
                            if moves.iter().any(|m| m.pp.is_none()) {
                                format!(" (read: {})", menu_text.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" | "))
                            } else { String::new() }));
        self.back_to_menu(lay, 6.0)?;
        Ok(moves)
    }

    /// From the move list back to Fight / Switch / Bag / Run: Back, again till that's up
    /// (a click on Back can go unseen, and the battle then stood in the move list). True
    /// once it's up.
    fn back_to_menu(&mut self, lay: &Layout, seconds: f64) -> R<bool> {
        let end = now() + seconds;
        while now() < end {
            let f = self.grab();
            if menu_ready(&f, lay) {
                return Ok(true);
            }
            let texts = self.screen_texts(&f);
            if self.close_chat(&f, &texts)? {
                continue;
            }
            if let Some(back) = texts.iter().find(|t| t.is("back")) {
                self.click(back.pt())?;
                self.move_mouse(lay.pt(layout::IDLE_SPOT))?;
            }
            self.nap(0.6)?;
        }
        self.trace("fight: still not back from the move list");
        Ok(false)
    }

    fn set_moves(&self, moves: Vec<reader::Move>) {
        let mut web = self.c.web.lock().unwrap();
        if !web.lead.name.is_empty() {
            let name = web.lead.name.clone();
            web.known_moves.insert(name, moves.clone());
        }
        web.moves = moves;
    }

    fn battle_over(&self, msg: &str) {
        self.print(&format!("  {msg}"));
        {
            let mut web = self.c.web.lock().unwrap();
            web.phase = "waiting".into();
            web.acting.clear();
            if !msg.is_empty() {
                web.msg = msg.into();
            }
        }
        self.c.publish();
    }

    fn web_set(&self, f: impl FnOnce(&mut crate::hunter::WebState)) {
        f(&mut self.c.web.lock().unwrap());
    }

    /// Taken over from the website: the battle waits for the page's buttons,
    /// and the page sees each turn (a picture, the battle's text, your lead).
    /// True if it caught it, once the battle is over or handed back.
    pub(crate) fn remote_battle(&mut self, enc: &Enc, f: &Frame, lay: &Layout) -> R<bool> {
        match self.remote_battling(enc, f, lay) {
            Err(Halt::BattleOver) => {
                self.battle_over("the battle is over");
                Ok(false)
            }
            r => r,
        }
    }

    fn remote_battling(&mut self, enc: &Enc, start_f: &Frame, start_lay: &Layout) -> R<bool> {
        let cfg = self.c.cfg.clone();
        let mut lay = start_lay.clone();
        self.read_lead(start_f, &lay);
        self.web_set(|w| w.moves = w.known_moves.get(&w.lead.name).cloned().unwrap_or_default());
        self.send_shot(start_f, &lay);
        self.c.publish();
        if self.c.web.lock().unwrap().moves.is_empty() {
            self.web_set(|w| w.acting = "Reading your moves".into());
            self.c.publish();
            let moves = self.read_moves(&lay)?;
            self.set_moves(moves);
            self.web_set(|w| {
                w.acting.clear();
                w.error = if w.moves.is_empty() { "Couldn't read your moves".into() } else { String::new() };
            });
            self.c.publish();
        }
        let (mut checked, mut gone) = (now(), None::<f64>);
        while self.c.remote() {
            let Some(cmd) = self.c.next_command(0.2) else {
                if self.c.stopped() {
                    return Err(Halt::Stopped);
                }
                if self.c.want_shot_taken() {
                    let f = self.grab();
                    self.send_shot(&f, &lay);
                }
                if now() - checked < 1.0 {
                    continue;
                }
                checked = now();
                let (_, cur) = self.look(Some(lay.mute));
                match cur {
                    None => {
                        let g = *gone.get_or_insert(now());
                        if now() - g > GONE_AFTER {
                            self.battle_over("the battle is over");
                            return Ok(false);
                        }
                    }
                    Some(cur) => (gone, lay) = (None, cur),
                }
                continue;
            };
            let what = cmd["cmd"].as_str().unwrap_or("").to_string();
            let name = cmd["name"].as_str().unwrap_or("").to_string();
            let seq = cmd["seq"].as_i64().unwrap_or(0);
            let acting = match what.as_str() {
                "ball" => format!("Throwing a {name}"),
                "move" => format!("Using {name}"),
                "heal" => format!("Using a {}", cfg.str("heal_item")),
                "pp" => "No PP item: out of PP, balls".into(),
                "run" => "Running".into(),
                _ => String::new(),
            };
            self.web_set(|w| {
                w.error.clear();
                w.acting = acting.clone();
            });
            if acting.is_empty() {
                continue;
            }
            self.print(&format!("  website: {}", lower_first(&acting)));
            self.c.publish();
            let mut error = String::new();
            let done = match what.as_str() {
                "ball" => cfg.web_balls().contains(&name) && self.use_item(&lay, "Pokeballs", &name, None, true)?.is_some(),
                "move" => match self.use_move(&lay, &name)? {
                    "used" => {
                        let seen = self.move_pp.take();
                        let moves: Vec<reader::Move> = self.c.web.lock().unwrap().moves.iter().map(|m| {
                            let mut m = m.clone();
                            if m.name == name {
                                m.pp = seen.or(m.pp).map(|p| (p - 1).max(0));
                            }
                            m
                        }).collect();
                        self.set_moves(moves);
                        true
                    }
                    "no pp" => {
                        let moves: Vec<reader::Move> = self.c.web.lock().unwrap().moves.iter().map(|m| {
                            let mut m = m.clone();
                            if m.name == name {
                                m.pp = Some(0);
                            }
                            m
                        }).collect();
                        self.set_moves(moves);
                        error = format!("{name} has no PP left");
                        false
                    }
                    _ => {
                        error = format!("Couldn't find {name} in the Fight menu");
                        false
                    }
                },
                "heal" => {
                    let lead = self.c.web.lock().unwrap().lead.name.clone();
                    self.use_item(&lay, "Recovers", &cfg.str("heal_item"), Some(&lead), true)?.is_some()
                }
                "pp" => {
                    // no PP item (the game's glitched with it): balls for the rest of this battle
                    let lead = self.c.web.lock().unwrap().lead.name.clone();
                    self.need_pp(&lead, "a move");
                    error = format!("No {} while hunting: balls for the rest of this battle", cfg.str("pp_item"));
                    false // no turn taken: the page shows that, and the battle goes on
                }
                _ => {
                    // run: clicks Run until the battle is over
                    self.web_set(|w| w.done = seq);
                    if self.run_away(&lay, true, 0.5)? {
                        self.battle_over("got away");
                        return Ok(false);
                    }
                    error = "Couldn't get away".into();
                    false
                }
            };
            self.web_set(|w| {
                w.done = seq;
                w.error = error.clone();
            });
            if !done {
                self.web_set(|w| {
                    if w.error.is_empty() {
                        w.error = format!("Couldn't: {} didn't work", lower_first(&w.acting));
                    }
                    w.acting.clear();
                });
                let (f, cur) = self.look(Some(lay.mute));
                if let Some(cur) = cur {
                    self.send_shot(&f, &cur);
                }
                self.c.publish();
                continue;
            }
            let (outcome, img, l, msgs) = self.wait_turn(&lay)?;
            lay = l;
            self.web_set(|w| {
                w.acting.clear();
                if !msgs.is_empty() {
                    w.msg = msgs[msgs.len().saturating_sub(3)..].join(" ");
                }
            });
            self.print(&format!("  turn: {outcome}{}",
                                if msgs.is_empty() { String::new() } else { format!(". The battle said: {}", msgs.join(" / ")) }));
            if let Some(img) = &img {
                if outcome == "turn" {
                    self.read_lead(img, &lay);
                    if !chat_showing(img, &lay) && box_visible(img, &lay) {
                        let again = self.read_enemy(img, &lay);
                        if !again.species.is_empty() && same_mon(&again, enc) {
                            self.web_set(|w| w.enc = Some(again));
                        }
                    }
                }
                if self.find_mute(img, Some(lay.mute)).is_some() {
                    self.send_shot(img, &lay);
                }
            }
            match outcome {
                "caught" => {
                    self.log_encounter(enc, "caught");
                    self.c.discord.post(&format!("Caught from the website: {}", enc.title()),
                                        Some(png(&game_shot(start_f, start_lay))));
                    self.battle_over("caught it!");
                    return Ok(true);
                }
                "over" => {
                    self.battle_over("the battle is over");
                    return Ok(false);
                }
                "timeout" => self.web_set(|w| w.error = "That turn took over a minute".into()),
                _ => {}
            }
            self.c.publish();
        }
        Ok(false)
    }
}

/// The bag's list shows favorites (yellow tag) and, under them, a row that isn't one: the
/// favorites all sit at the top, so every one of them is in sight.
fn favorites_cover_list(f: &Frame, texts: &[TextHit], split: f64, lay: &Layout) -> bool {
    let rows: Vec<bool> = {
        let mut r: Vec<&TextHit> = texts.iter().filter(|t| is_count(&t.text) && t.x > split * 0.5).collect();
        r.sort_by(|a, b| a.y.total_cmp(&b.y));
        r.iter().map(|t| vision::favorited(f, texts, t.y, lay)).collect()
    };
    // a favorite first, then no favorite after the first plain row
    rows.first() == Some(&true) && rows.iter().position(|&x| !x).is_some_and(|i| rows[i..].iter().all(|&x| !x))
}

/// A row of the bag, by its count tag's y: for the debug run's favorites.
pub(crate) fn favorite_rows(f: &Frame, texts: &[TextHit], items: &[String], split: f64, lay: &Layout) -> Vec<(String, bool)> {
    items.iter().map(|item| {
        let mut rows: Vec<f64> = texts.iter().filter(|t| norm(&t.text) == norm(item) && t.x < split).map(|t| t.y).collect();
        rows.sort_by(f64::total_cmp);
        (item.clone(), rows.first().is_some_and(|y| vision::favorited(f, texts, *y, lay)))
    }).collect()
}
