//! In battle: the bag (balls, healing), the Fight menu, waiting out a turn,
//! the catch plan, and battles fought from the website.

use crate::hunter::{Halt, Hunt, GONE_AFTER, R, TURN_TIMEOUT};
use crate::layout::{self, Layout};
use crate::picture::{game_shot, png};
use crate::px::{Frame, Pt};
use crate::reader::{self, bag_still, find_tab, is_count, search_box, TextHit};
use crate::rules::{field_ok, norm, same_mon, same_word, Enc};
use crate::util::{lower_first, now};
use crate::vision::{self, box_visible, chat_showing, find_use_button, menu_ready};
use serde_json::Value;

/// Scrolling the backpack's list (phones, when its search box can't be typed
/// into): first to the top, then down a page at a time until the page stops
/// changing. The list's height is taken from the tab buttons beside it.
#[derive(Default)]
pub(crate) struct Scroll {
    pub at: f64,
    at_top: bool,
    last: Option<Vec<String>>,
    pages: u32,
}

impl Hunt {
    /// Waits for the Fight / Bag / Run menu; a battle that's over is
    /// Halt::BattleOver (out of battle the Bag button's spot is somewhere else).
    pub(crate) fn ensure_battle(&mut self, lay: &Layout) -> R<()> {
        let end = now() + 4.0;
        while now() < end {
            let (f, cur) = self.look(Some(lay.mute));
            if cur.is_some_and(|cur| menu_ready(&f, &cur)) {
                return Ok(());
            }
            self.nap(self.traits.poll)?;
        }
        Err(Halt::BattleOver)
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

    fn close_bag(&mut self, texts: &[TextHit]) -> R<()> {
        if let Some(t) = texts.iter().find(|t| t.is("close")) {
            self.click(t.pt())?;
        }
        Ok(())
    }

    fn party_prompt(texts: &[TextHit]) -> bool {
        texts.iter().any(|t| {
            let l = t.text.to_lowercase();
            l.starts_with("choose a pokemon")
                || l.starts_with("select a pokemon")
                || l.starts_with("pokemon party")
        })
    }

    /// Wait for the selection panel to leave. A single OCR miss is not enough:
    /// require two consecutive reads without the party prompt.
    fn wait_party_closed(&mut self, seconds: f64, mut no_effect: bool) -> R<(bool, bool, Vec<TextHit>)> {
        let end = now() + seconds;
        let mut gone = 0;
        let mut last = Vec::new();
        while now() < end {
            self.nap(0.3)?;
            let f = self.grab();
            let texts = self.screen_texts(&f);
            let effect_message = texts.iter().any(|t| t.text.to_lowercase().contains("any effect"));
            if !no_effect && effect_message {
                no_effect = true;
                self.trace("bag: item reported no effect; dismissing the message");
                if let Some(c) = texts.iter().find(|t| t.is("close")) {
                    self.click(c.pt())?;
                }
                gone = 0;
                last = texts;
                continue;
            }
            // The effect message itself is an overlay even if OCR misses the party
            // prompt behind it. Do not count that as a closed panel.
            if Self::party_prompt(&texts) || effect_message {
                gone = 0;
            } else {
                gone += 1;
                if gone >= 2 {
                    return Ok((true, no_effect, texts));
                }
            }
            last = texts;
        }
        Ok((false, no_effect, last))
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
        let mut r = Ok(None);
        for attempt in 1..=(if in_battle { 3 } else { 1 }) {
            if attempt > 1 {
                self.trace(&format!("bag: trying again for the {item} ({attempt} of 3)"));
                self.nap(0.6)?; // (it's closing)
            }
            r = self.use_item_in(lay, tab, item, target, in_battle);
            if !matches!(r, Ok(None)) {
                break;
            }
        }
        self.dev.keyboard_hidden(false);
        let r = r?;
        if let Some(used) = &r {
            self.used_item = used.clone();
        }
        Ok(r)
    }

    fn use_item_in(&mut self, lay: &Layout, tab: &str, item: &str, target: Option<&str>, in_battle: bool) -> R<Option<String>> {
        let phone = self.traits.phone;
        let item = item.to_string();
        if in_battle {
            self.ensure_battle(lay)?;
            self.trace(&format!("bag: opening it for {item}"));
            self.click(lay.pt(layout::BAG))?;
        }
        self.move_mouse(lay.pt(layout::AFK_SPOT))?;
        let split = lay.xy(layout::BAG_SPLIT, 0.0).x;
        let v = lay.v(); // screen px per reference px, down
        let (mut step, mut row_y, mut tab_at, mut searched) = ("tab", 0.0, 0.0, false);
        let (mut picked_at, mut picks) = (0.0, 0);
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
            let bag_up = reader::bag_open(&texts, split);
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
                bag_still(&texts, &before, split)
            };
            if step != "target" && !still {
                continue;
            }
            if step == "tab" {
                // the bag opens on the tab it was left on: the item in sight there, no click
                if texts.iter().any(|t| same_word(&item, &t.text) && t.x < split) {
                    self.trace(&format!("bag: {item} is in sight already"));
                    (step, tab_at) = ("item", now()); // (and on with this read)
                } else {
                    if let Some(hit) = find_tab(&texts, tab, split, f.width()) {
                        let worked_out = !texts.iter().any(|t| t == &hit);
                        self.trace(&format!("bag: {tab} tab at ({:.0}, {:.0}){}", hit.x, hit.y,
                                            if worked_out { ", worked out from the tabs around it" } else { "" }));
                        self.click(hit.pt())?;
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
                // exact names: "Dive Ball" and "Dream Ball" are one misread apart
                let row = texts.iter().filter(|t| same_word(&item, &t.text) && t.x < split)
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
                    self.move_mouse(lay.pt(layout::AFK_SPOT))?;
                    (step, picked_at) = ("use", now());
                } else if !searched && now() - tab_at > 1.5 {
                    last_row_y = None;
                    // not a favorite: type it into the tab's search box
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
                } else if phone && searched && now() - tab_at > 1.5 && now() > scroll.at {
                    last_row_y = None;
                    // not in sight: scroll the list, top to bottom
                    if scroll.pages == 0 && !scroll.at_top {
                        if let Some(t) = texts.iter().find(|t| {
                            t.x > split && t.text == t.text.to_uppercase() && reader::two_letters(&t.text)
                                && !t.is("close") && !is_count(&t.text)
                        }) {
                            let shows = t.text.clone();
                            self.say(&format!("  (the bag's search box says \"{shows}\": clear it in Roblox, it hides the rest)"));
                        }
                    }
                    if !self.scroll_bag(&mut scroll, tab_hit.as_ref(), &texts, split)? {
                        self.trace(&format!("bag: looked through {tab}, no {item}"));
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
                if let Some(u) = use_at.filter(|_| named.iter().any(|(t, _)| same_word(&item, t))) {
                    self.trace(&format!("bag: {item} is selected: Use"));
                    self.click(u)?;
                    self.move_mouse(lay.pt(layout::AFK_SPOT))?;
                    if target.is_none() {
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
                // In battle, verify that selecting the target actually leaves the
                // party panel. Previously, timing out this wait still returned success,
                // after which wait_turn could sit behind the panel for 60 seconds.
                let party = Self::party_prompt(&texts);
                let target = target.unwrap_or("");
                if party {
                    if let Some(hit) = texts.iter().find(|t| reader::is_target(target, &t.text)).map(|t| t.pt()) {
                        self.trace(&format!("bag: {item} on {target}"));
                        self.click(hit)?;
                        if in_battle {
                            self.move_mouse(lay.pt(layout::AFK_SPOT))?;
                            let (mut closed, mut no_effect, mut last_texts) =
                                self.wait_party_closed(2.5, false)?;

                            // Retry the target click at most once, and only if a fresh
                            // OCR read still shows the same prompt and same card in place.
                            if !closed && !no_effect {
                                let f = self.grab();
                                let current = self.screen_texts(&f);
                                let target_hit = current.iter()
                                    .find(|t| reader::is_target(target, &t.text));
                                let same_card = target_hit.is_some_and(|t| {
                                    (t.x - hit.x).abs() + (t.y - hit.y).abs() <= 24.0 * v
                                });
                                let effect = current.iter()
                                    .any(|t| t.text.to_lowercase().contains("any effect"));
                                if Self::party_prompt(&current) && same_card && !effect {
                                    self.trace("bag: party prompt and target are unchanged; retrying that target once");
                                    if let Some(t) = target_hit {
                                        self.click(t.pt())?;
                                        self.move_mouse(lay.pt(layout::AFK_SPOT))?;
                                    }
                                    let result = self.wait_party_closed(2.5, no_effect)?;
                                    closed = result.0;
                                    no_effect |= result.1;
                                    last_texts = result.2;
                                } else {
                                    last_texts = current;
                                }
                            }

                            let mut closed_with_close = false;
                            if !closed {
                                // Close is clicked only if a fresh read confirms that
                                // this exact party panel remains open and Close is visible.
                                let f = self.grab();
                                let current = self.screen_texts(&f);
                                let effect_message = current.iter()
                                    .any(|t| t.text.to_lowercase().contains("any effect"));
                                if Self::party_prompt(&current) || effect_message {
                                    if let Some(c) = current.iter().find(|t| t.is("close")) {
                                        self.trace("bag: party/effect panel stayed open; closing its visible Close button");
                                        self.click(c.pt())?;
                                        closed_with_close = true;
                                        let result = self.wait_party_closed(1.5, no_effect)?;
                                        closed = result.0;
                                        no_effect |= result.1;
                                        last_texts = result.2;
                                    } else {
                                        last_texts = current;
                                    }
                                } else {
                                    let result = self.wait_party_closed(0.8, no_effect)?;
                                    closed = result.0;
                                    no_effect |= result.1;
                                    last_texts = result.2;
                                }
                            }

                            if closed {
                                if closed_with_close && !no_effect {
                                    self.trace("bag: target wasn't accepted; exited the party panel");
                                    return Ok(None);
                                }
                                self.trace(&format!("bag: confirmed the party panel closed after {item}"));
                                return Ok(Some(item));
                            }

                            let f = self.grab();
                            let shot = self.snapshot(&f, "party-stuck");
                            let read = last_texts.iter().map(|t| t.text.as_str())
                                .collect::<Vec<_>>().join(", ");
                            self.say(&format!("  (party panel stayed open after {item}: {shot}, text read: {})",
                                              crate::util::clip(&read, 300)));
                            return Ok(None);
                        }
                        // Outside battle, the debug workflow performs its own
                        // result read and returns to the bag.
                        return Ok(Some(item));
                    }
                }
            }
        }
        let read = texts.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(", ");
        let shot = img.as_ref().map(|f| self.snapshot(f, "bag")).unwrap_or_else(|| "-".into());
        self.say(&format!("  (couldn't use {item}: {shot}, text read: {})", crate::util::clip(&read, 300)));
        self.close_bag(&texts)?;
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

    /// Fight -> move. "used", "no pp" (it goes Back to the menu then), or "no menu".
    pub(crate) fn use_move(&mut self, lay: &Layout, mv: &str) -> R<&'static str> {
        self.ensure_battle(lay)?;
        self.click(lay.pt(layout::FIGHT))?;
        self.move_mouse(lay.pt(layout::IDLE_SPOT))?;
        let v = lay.v();
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
    /// is back), "over" (the battle ended without a catch) or "timeout"; the
    /// last screenshot, its layout and what the battle said.
    pub(crate) fn wait_turn(&mut self, lay: &Layout) -> R<(&'static str, Option<Frame>, Layout, Vec<String>)> {
        let (start, mut seen, mut gone) = (now(), false, None::<f64>);
        let mut msgs: Vec<String> = Vec::new();
        let mut lay = lay.clone();
        let mut last = None;
        while now() - start < TURN_TIMEOUT {
            self.nap(self.traits.poll)?;
            let (f, cur) = self.look(Some(lay.mute));
            let Some(cur) = cur else {
                // backpack or party screen still up, or the battle is over
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

    pub(crate) fn own_hp(&mut self, f: &Frame, lay: &Layout) -> Option<f64> {
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
        let lead = reader::should(&self.region_text(&f, &lay, layout::MSG)).unwrap_or_else(|| cfg.str("lead"));
        let legendary = field_ok(&Value::from("legendary"), &Value::from(enc.species.as_str())); // much harder to catch
        let max_balls = cfg.num(if legendary { "legendary_max_balls" } else { "max_balls" }) as usize;
        let max_turns = cfg.num(if legendary { "legendary_max_turns" } else { "max_turns" }) as u32;
        let (mut balls, mut turns, mut drowsy, mut heals): (Vec<String>, u32, bool, u32) = (Vec::new(), 0, false, 0);
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
            } else if !cur.status.is_empty() {
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
                self.trace(&format!("throwing a {b} (#{})", balls.len() + 1));
                let mut used = self.use_item(&lay, "Pokeballs", &b, None, true)?;
                if used.is_none() {
                    if let Some(p) = plain.take() {
                        self.trace(&format!("no {b} to throw: {p}s from now on"));
                        timer.clear();
                        b = p;
                        used = self.use_item(&lay, "Pokeballs", &b, None, true)?;
                    }
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
            let details = format!("{} ball(s), last {}, {turns} turns", balls.len(), balls.last().map(|s| s.as_str()).unwrap_or("-"));
            let Some(img) = img.filter(|_| outcome == "turn") else { return Ok((outcome.into(), details)) };
            f = img;
            if msgs.iter().any(|m| m.to_lowercase().contains("fell asleep")) {
                drowsy = false;
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

/// A row of the bag, by its count tag's y: for the debug run's favorites.
pub(crate) fn favorite_rows(f: &Frame, texts: &[TextHit], items: &[String], split: f64, lay: &Layout) -> Vec<(String, bool)> {
    items.iter().map(|item| {
        let mut rows: Vec<f64> = texts.iter().filter(|t| norm(&t.text) == norm(item) && t.x < split).map(|t| t.y).collect();
        rows.sort_by(f64::total_cmp);
        (item.clone(), rows.first().is_some_and(|y| vision::favorited(f, texts, *y, lay)))
    }).collect()
}
