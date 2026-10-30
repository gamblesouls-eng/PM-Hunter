//! The samples in samples/, and the same squeezed into phone shapes; what the
//! Android app's tests checked, now for the hunter every platform shares.

use hunt_core::calibrate::calibrate;
use hunt_core::config::Config;
use hunt_core::layout::{self, Calibration, Layout};
use hunt_core::picture;
use hunt_core::px::{self, near, Frame};
use hunt_core::reader::{self, Lead, Move, TextHit};
use hunt_core::rules::{ratio, same_mon, wanted, Enc};
use hunt_core::vision::{self, box_visible, chat_showing, find_mute, menu_ready};
use std::path::PathBuf;

fn load(name: &str) -> Frame {
    picture::open(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../samples").join(name)).unwrap()
}

const BATTLES: [&str; 5] =
    ["quaxly-shiny.png", "shellos-tint.png", "remoraid-shiny.png", "dewpider-mirage.png", "marill-inferno-tint.png"];

/// The sample on a phone screen of w x h: the game scaled sx across and sy down
/// (Roblox fits its UI to the screen), centered on black, smoothed like a real rescale.
fn phone(src: &Frame, w: u32, h: u32, sx: f64, sy: f64) -> Frame {
    let (dw, dh) = ((src.width() as f64 * sx) as u32, (src.height() as f64 * sy) as u32);
    let scaled = image::imageops::resize(src, dw, dh, image::imageops::FilterType::Triangle);
    let mut out = Frame::from_pixel(w, h, image::Rgb([0, 0, 0]));
    image::imageops::overlay(&mut out, &scaled, ((w as i64 - dw as i64) / 2) as i64, ((h as i64 - dh as i64) / 2) as i64);
    out
}

#[test]
fn reads_the_samples() {
    for name in BATTLES {
        let f = load(name);
        let mute = find_mute(&f, None);
        assert_eq!(mute, Some(layout::MUTE), "{name}");
        let lay = Layout::flat(mute.unwrap());
        assert!(menu_ready(&f, &lay), "{name}");
        assert!(box_visible(&f, &lay), "{name}");
        assert!(!chat_showing(&f, &lay), "{name}");
    }
    let flat = Layout::flat(layout::MUTE);
    assert_eq!(vision::icons(&load("quaxly-shiny.png"), &flat).0, true);
    assert_eq!(vision::icons(&load("shellos-tint.png"), &flat).1, true);
    assert_eq!(find_mute(&load("bag-open.png"), None), None);
    assert!(!menu_ready(&load("fight-menu.png"), &flat));
}

#[test]
fn calibrating_the_samples_changes_nothing() {
    for name in BATTLES {
        let f = load(name);
        let cal = calibrate(&f, find_mute(&f, None).unwrap()).expect(name);
        assert!((cal.fit[0] - 1.0).abs() < 0.01 && (cal.fit[1] - 1.0).abs() < 0.01, "{name}: {:?}", cal.fit);
        assert!(cal.miss < 4.0, "{name}: miss {}", cal.miss);
    }
    assert!(calibrate(&load("fight-menu.png"), layout::MUTE).is_none()); // no menu up
}

#[test]
fn calibrates_a_real_phone() {
    // the game on the phone is squeezed down more than across: unreadable until calibrated
    let f = load("phone-pikachu-shiny.png");
    let mute = find_mute(&f, None).unwrap();
    assert!(!menu_ready(&f, &Layout::flat(mute)));
    let cal = calibrate(&f, mute).unwrap();
    assert!((cal.fit[1] / cal.fit[0] - 0.93).abs() < 0.02);
    let lay = Layout::new(mute, &cal);
    assert!(menu_ready(&f, &lay) && box_visible(&f, &lay));
    let (shiny, tint, _) = vision::icons(&f, &lay);
    assert!(shiny && !tint);
}

#[test]
fn samples_the_wild_pokemons_sprite() {
    // pixels deep inside the sprite
    let check = |name: &str, want: &[(u32, u32, [i32; 3])]| {
        let f = load(name);
        let mute = find_mute(&f, None).unwrap();
        let s = vision::sprite(&f, &Layout::new(mute, &calibrate(&f, mute).unwrap()));
        assert_eq!(s.width(), 96);
        for (x, y, c) in want {
            assert!(near(px::at(&s, *x as usize, *y as usize), *c, 20), "{name} {x},{y}");
        }
    };
    check("quaxly-shiny.png", &[(44, 30, [135, 240, 229]), (52, 47, [253, 253, 251])]);
    check("phone-pikachu-shiny.png", &[(55, 33, [85, 85, 85]), (60, 44, [242, 200, 80]), (39, 47, [242, 169, 59])]);
}

#[test]
fn robloxs_buttons_arent_chat() {
    // Android 16 phone: Roblox's menu and chat buttons sit where chat lines start
    let f = load("phone16-pichu-shiny.png");
    let mute = find_mute(&f, None).unwrap();
    let lay = Layout::new(mute, &calibrate(&f, mute).unwrap());
    assert!(!chat_showing(&f, &lay));
    assert!(box_visible(&f, &lay));
    assert!(vision::icons(&f, &lay).0);
}

#[test]
fn finds_robloxs_chat_button() {
    // the right end of the dark pill in the top-left corner (menu, chat)
    let a = vision::chat_button(&load("phone16-pichu-shiny.png")).unwrap();
    assert!((a.x - 249.0).abs() < 12.0 && (a.y - 44.0).abs() < 8.0, "{a:?}");
    let b = vision::chat_button(&load("phone-pikachu-shiny.png")).unwrap();
    assert!((b.x - 205.0).abs() < 12.0 && (b.y - 125.0).abs() < 8.0, "{b:?}");
}

#[test]
fn calibrates_phone_shaped_screens() {
    // (screen, how the game is squeezed across and down)
    let phones = [((2400, 1080), 0.9, 0.86), ((2340, 1080), 0.75, 0.86), ((1600, 720), 0.55, 0.57), ((2800, 1260), 1.0, 1.0)];
    for ((w, h), sx, sy) in phones {
        for name in BATTLES {
            let f = phone(&load(name), w, h, sx, sy);
            let mute = find_mute(&f, None).unwrap_or_else(|| panic!("{name} on {w}x{h}"));
            let cal = calibrate(&f, mute).unwrap_or_else(|| panic!("{name} on {w}x{h} squeezed {sx} x {sy}"));
            assert!((cal.fit[1] / cal.fit[0] - sy / sx).abs() < 0.03, "{name} on {w}x{h}: {:?}", cal.fit);
            let lay = Layout::new(mute, &cal);
            assert!(menu_ready(&f, &lay) && box_visible(&f, &lay), "{name} on {w}x{h}");
            // the Run button lands on Run
            let run = lay.pt(layout::RUN);
            assert!(near(px::at(&f, run.x as usize, run.y as usize), layout::button_color("run"), 30), "{name} on {w}x{h}");
        }
    }
}

#[test]
fn calibration_keeps_its_colors() {
    let f = load("quaxly-shiny.png");
    let cal = calibrate(&f, layout::MUTE).unwrap();
    let again = Calibration::from_json(&cal.to_json()).unwrap();
    assert_eq!(again.fit, cal.fit);
    assert_eq!(again.colors, cal.colors);
}

// ---------------------------------------------------------------- the rules and readers

fn cfg() -> Config {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/rules-config.json");
    Config::with_rules(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn enc(species: &str) -> Enc {
    Enc { species: species.into(), ..Default::default() }
}

fn hit(t: &str, x: f64, y: f64) -> TextHit {
    TextHit::new(t, x, y)
}

#[test]
fn ratio_matches_difflib() {
    for (a, b, r) in [("lapras", "lapros", 0.8333), ("suicune", "suicun", 0.9231), ("glacier", "glaciar", 0.8571),
                      ("pinstripe", "pinstripes", 0.9474), ("moonbane", "moonbone", 0.875), ("azurine", "zurine", 0.9231)] {
        assert!((ratio(a, b) - r).abs() < 1e-4, "{a} {b}");
    }
}

#[test]
fn matches_the_config() {
    let c = cfg();
    let e = |s: &str, reskin: &str, gradient: &str, shiny: bool, tint: bool| Enc {
        species: s.into(), reskin: reskin.into(), gradient: gradient.into(), shiny, tint, ..Default::default()
    };
    assert!(wanted(&e("Lapras", "DOODLE", "Mirage", false, false), &c.targets).is_some());
    assert!(wanted(&e("Suicune", "Doodle", "", false, false), &c.targets).is_some());
    assert!(wanted(&e("Lapras", "Doodle", "", false, false), &c.targets).is_none());
    assert!(wanted(&e("Marill", "INFERNO", "", false, true), &c.targets).is_none());
    // auto-catch: any reskin + gradient (a legendary too: not a reskin alone), shiny Doodle but not Frillish
    assert!(wanted(&e("Galarian Articuno", "Night", "", false, false), &c.notify).is_none());
    assert!(wanted(&e("Galarian Articuno", "Night", "Moonbane", false, false), &c.notify).is_some());
    assert!(wanted(&e("Marill", "INFERNO", "Mirage", false, false), &c.notify).is_some());
    assert!(wanted(&e("Marill", "Doodle", "", true, false), &c.notify).is_some());
    assert!(wanted(&e("Frillish", "Doodle", "", true, false), &c.notify).is_none());
    assert!(wanted(&e("Marill", "INFERNO", "", false, false), &c.notify).is_none());
}

#[test]
fn reads_the_enemy_box() {
    // as the text recognition reports marill-inferno-tint.png's box
    let e = reader::enemy(&[hit("Marill", 0.17, 0.2), hit("Lv. 52", 0.8, 0.2), hit("INFERNO", 0.67, 0.65)]);
    assert_eq!((e.species.as_str(), e.level.as_str(), e.reskin.as_str()), ("Marill", "52", "INFERNO"));
    assert_eq!(Enc { tint: true, ..e }.describe(), "Marill Lv52  (reskin INFERNO, tint)");
    assert_eq!(reader::clean_name("Dewpider o"), "Dewpider");
    assert_eq!(reader::enemy(&[hit("SLP", 0.1, 0.45)]).status, "SLP");
    assert_eq!(reader::enemy(&[hit("Mirage", 0.3, 0.9)]).gradient, "Mirage");
    // a phone reads the gender sign as a text of its own
    assert_eq!(reader::enemy(&[hit("Sentret", 0.1, 0.2), hit("O", 0.4, 0.2), hit("Lv. 3", 0.8, 0.2)]).species, "Sentret");
}

#[test]
fn finds_bag_tabs_from_the_ones_read() {
    // what the text recognition read on the Android 16 emulator's backpack:
    // "Recovers" and "Evolution" missed, "TMs" and "Held Items" garbled
    let texts = [hit("Backpack", 445.0, 204.0), hit("Pokeballs", 430.0, 322.0), hit("Key Items", 429.0, 366.0),
                 hit("Competitive", 429.0, 451.0), hit("TMB", 429.0, 493.0), hit("Heid Iterme", 429.0, 537.0),
                 hit("Z-Crystals", 429.0, 580.0), hit("Recover", 553.0, 263.0), hit("Antidote", 616.0, 324.0),
                 hit("Elixir", 602.0, 477.0)];
    let split = 1001.0;
    let recovers = reader::find_tab(&texts, "Recovers", split, 1676).unwrap();
    assert!((recovers.x - 429.0).abs() < 2.0);
    assert!((recovers.y - 280.0).abs() < 6.0); // the Recovers tab, not the "Recover" heading
    assert!((reader::find_tab(&texts, "Pokeballs", split, 1676).unwrap().y - 322.0).abs() < 0.1);
    assert!((reader::find_tab(&texts, "Evolution", split, 1676).unwrap().y - 409.0).abs() < 6.0);
}

#[test]
fn same_mon_forgives_misreads() {
    let lv = |s: &str, l: &str| Enc { level: l.into(), ..enc(s) };
    assert!(same_mon(&lv("Type: Null", "60"), &lv("Type: Nulll", "60")));
    assert!(!same_mon(&lv("Lapras", "60"), &lv("Lapras", "61")));
}

#[test]
fn reads_the_lead() {
    let lead = |n: &str, l, h, m| Lead { name: n.into(), level: Some(l), hp: Some(h), max: Some(m) };
    let b = [hit("Clodsire ♀ Lv. 109", 0.5, 0.27), hit("376 / 376", 0.8, 0.81), hit("Exp", 0.12, 0.9)];
    assert_eq!(reader::lead(&b), lead("Clodsire", 109, 376, 376));
    // in pieces, the level first
    let pieces = [hit("Lv. 109", 0.86, 0.27), hit("Clodsire", 0.25, 0.27), hit("350", 0.7, 0.81), hit("/ 376", 0.85, 0.81)];
    assert_eq!(reader::lead(&pieces), lead("Clodsire", 109, 350, 376));
    assert_eq!(reader::lead(&[]), Lead::default());
    assert!(reader::lead(&b).complete());
    // ML Kit reads the dot as a comma now and then
    assert_eq!(reader::lead(&[hit("Sobble", 0.2, 0.27), hit("Exp", 0.1, 0.9), hit("Lv, 11", 0.85, 0.3), hit("29 / 29", 0.8, 0.8)]),
               lead("Sobble", 11, 29, 29));
    // the level-up stats panel over the box: not a reading to check an answer against
    let panel = [hit("Sobble Sp.A:", 0.3, 0.27), hit("Exp", 0.12, 0.9), hit("Sp.D:", 0.3, 0.5), hit("Speed:", 0.3, 0.7),
                 hit("16 ta", 0.8, 0.3)];
    assert!(!reader::lead(&panel).complete());
}

#[test]
fn reads_the_moves() {
    let m = |n: &str, pp, max| Move { name: n.into(), pp: Some(pp), max: Some(max) };
    let menu = [hit("Yawn", 0.125, 0.26), hit("Protect", 0.375, 0.26), hit("10 / 10", 0.047, 0.84), hit("Normal", 0.125, 0.86),
                hit("Status", 0.2, 0.84), hit("7 / 10 Normal Status", 0.32, 0.84)];
    assert_eq!(reader::moves(&menu), vec![m("Yawn", 10, 10), m("Protect", 7, 10)]);
    // four moves, two buttons' PP read as one line
    let four = [hit("Pound", 0.125, 0.26), hit("Growl", 0.375, 0.26), hit("Water Gun", 0.625, 0.26), hit("Bind", 0.875, 0.26),
                hit("30 / 35 Normal Physical 40 / 40 Normal", 0.26, 0.84), hit("25 / 25", 0.55, 0.84),
                hit("Water Special", 0.65, 0.84), hit("20 / 20", 0.8, 0.84)];
    assert_eq!(reader::moves(&four), vec![m("Pound", 30, 35), m("Growl", 40, 40), m("Water Gun", 25, 25), m("Bind", 20, 20)]);
    // read on MuMu: one slash lost, one read as |
    let mumu = [hit("Pound", 0.125, 0.26), hit("Growl", 0.375, 0.26), hit("Water Gun", 0.625, 0.26), hit("Bind", 0.875, 0.26),
                hit("30/35 Normal Physical 40 40 Normal Status", 0.25, 0.84),
                hit("19|25 Water Special 20/ 20 Normal Physical", 0.75, 0.84)];
    assert_eq!(reader::moves(&mumu), vec![m("Pound", 30, 35), m("Growl", 40, 40), m("Water Gun", 19, 25), m("Bind", 20, 20)]);
    // the battle's text before the menu is up is no move
    assert!(reader::moves(&[hit("What should Clodsire do?", 0.2, 0.2), hit("Clodsire used Yawn!", 0.2, 0.2)]).is_empty());
}

/// Your Pokemon's HP from its bar: where the text says 282 / 367 (Entei samples) and full
/// elsewhere, on PCs and phone shapes; off the bar (an uncalibrated phone layout), unknown, never 0%.
#[test]
fn reads_hp_from_the_bar() {
    for name in BATTLES {
        let f = load(name);
        let m = find_mute(&f, None).unwrap();
        let hp = vision::hp_bar(&f, &Layout::new(m, &calibrate(&f, m).unwrap())).unwrap();
        assert!(hp > 0.98, "{name}: {hp}");
        let p = phone(&f, 2340, 1080, 0.75, 0.86);
        let m = find_mute(&p, None).unwrap();
        assert!(vision::hp_bar(&p, &Layout::flat(m)).is_none(), "{name} squeezed, uncalibrated");
        let hp = vision::hp_bar(&p, &Layout::new(m, &calibrate(&p, m).unwrap())).unwrap();
        assert!(hp > 0.97, "{name} squeezed: {hp}");
    }
    for name in ["entei-plain.png", "entei-galaxy.png"] {
        let f = load(name);
        let m = find_mute(&f, None).unwrap();
        let hp = vision::hp_bar(&f, &Layout::new(m, &calibrate(&f, m).unwrap())).unwrap();
        assert!((hp - 282.0 / 367.0).abs() < 0.02, "{name}: {hp}");
    }
    // a bar run down: its right part dark, as the game draws it
    let mut f = load("marill-inferno-tint.png");
    let m = find_mute(&f, None).unwrap();
    let lay = Layout::new(m, &calibrate(&f, m).unwrap());
    let bar = lay.boxed(layout::HP_BAR);
    let cut = bar.l + (bar.r - bar.l) * 3 / 10;
    for y in bar.t..bar.b {
        for x in cut..bar.r {
            f.put_pixel(x as u32, y as u32, image::Rgb([33, 33, 33]));
        }
    }
    let hp = vision::hp_bar(&f, &lay).unwrap();
    assert!((hp - 0.3).abs() < 0.03, "{hp}");
}

/// The Pokemon Party screen's cards by their colors, in party order: the lead first (a
/// Full Restore's target when its name reads as "sire"); a fainted one told apart.
#[test]
fn finds_the_party_cards() {
    let cards = vision::party_cards(&load("party-full-restore.png"));
    assert_eq!(cards.len(), 6);
    assert!(cards.iter().all(|c| !c.1));
    let lead = cards[0].0.center();
    assert!((lead.x - 383.0).abs() < 20.0 && (lead.y - 145.0).abs() < 20.0, "{lead:?}");
    let cards = vision::party_cards(&load("party-fainted.png"));
    assert_eq!(cards.len(), 6);
    assert!(cards[0].1, "Gallade fainted");
    assert!(cards[1..].iter().all(|c| !c.1));
    assert!(vision::party_cards(&load("bag-left-open.png")).is_empty());
    assert!(vision::party_cards(&load("quaxly-shiny.png")).is_empty());
}

/// The backpack's Close by its red pill, the word read in pieces ("C", "e", "lose").
#[test]
fn finds_the_bags_close() {
    let f = load("bag-left-open.png");
    let at = vision::bag_close(&f, hunt_core::Pt::new(333.0, 147.0)).unwrap();
    assert!((at.x - 937.0).abs() < 25.0 && (at.y - 147.0).abs() < 12.0, "{at:?}");
}

#[test]
fn reads_the_battle_and_party_words() {
    assert_eq!(reader::hp("Clodsire Lv. 129 412 / 415"), Some(412.0 / 415.0));
    assert_eq!(reader::hp("351 / 36"), None); // more than its max: a misread, not 975%
    assert!(reader::wild_fainted("The wild Giratina fainted!"));
    assert!(!reader::wild_fainted("Gallade fainted!"));
    assert!(reader::wild_statused("The wild Giratina was poisoned!"));
    assert!(reader::wild_statused("The wild Giratina was hurt by poison!"));
    assert!(!reader::wild_statused("The wild Misdreavus fell asleep!"));
    let t = |s: &str, x: f64, y: f64| TextHit::new(s, x, y);
    let party = [t("o", 0.1, 0.1), t("sire", 0.3, 0.15), t("Pokemon Party", 0.7, 0.11),
                 t("Choose a Pokemon to use Full Restore on.", 0.27, 0.81), t("Close", 0.8, 0.89)];
    assert!(reader::party_up(&party) && !reader::next_asked(&party));
    assert!(reader::next_asked(&[t("Choose your next Pokemon.", 0.2, 0.77)]));
    assert!(reader::close_word(&t("lose", 0.0, 0.0)) && !reader::close_word(&t("Clodsire", 0.0, 0.0)));
    // a search left in the box hides the list; the placeholder doesn't count
    let bag = [t("Backpack", 333.0, 147.0), t("Pokeball", 436.0, 210.0), t("Dive Ball", 950.0, 210.0), t("Close", 937.0, 147.0)];
    assert_eq!(reader::search_leftover(&bag, 700.0, 0.64).map(|h| h.text), Some("Dive Ball".to_string()));
    let bag = [t("Backpack", 333.0, 147.0), t("Search...", 950.0, 210.0), t("Close", 937.0, 147.0), t("x18,899", 940.0, 266.0)];
    assert!(reader::search_leftover(&bag, 700.0, 0.64).is_none());
}

#[test]
fn sees_reskins_and_gradient_tabs_without_reading() {
    // (reskin, gradient tab): every battle sample, on its own screen size
    let want = [
        ("quaxly-shiny.png", false, false),
        ("shellos-tint.png", false, false),
        ("remoraid-shiny.png", false, false),
        ("dewpider-mirage.png", false, true),
        ("marill-inferno-tint.png", true, false),
        ("lotad-skittles.png", false, true),
        ("tyrogue-mummified.png", true, false),
        ("tatsugiri-plain-tint.png", false, false),
        ("runerigus-amberfall.png", false, true), // its tab right under the box: no reskin
        ("entei-plain.png", false, false),
        ("entei-coal.png", true, false),
        ("entei-galaxy.png", true, false),
        ("entei-wave-tint.png", true, false),
        ("mew-toxic.png", true, false),
        ("phone-pikachu-shiny.png", false, false),
        ("phone16-pichu-shiny.png", false, false),
    ];
    for (name, reskin, tab) in want {
        let f = load(name);
        let mute = find_mute(&f, None).expect(name);
        let lay = Layout::new(mute, &calibrate(&f, mute).unwrap_or_default());
        assert_eq!(vision::reskin_shown(&f, &lay), reskin, "{name}: reskin");
        assert_eq!(vision::gradient_tab(&f, &lay).unwrap_or(false), tab, "{name}: gradient tab");
        // a window scaled a little off: the game 8 px from where the layout puts it
        for (dx, dy) in [(0, -8), (0, 8), (-8, 0), (8, 0)] {
            let mut g = px::Frame::from_pixel(f.width(), f.height(), image::Rgb([0, 0, 0]));
            image::imageops::overlay(&mut g, &f, dx, dy);
            assert_eq!(vision::reskin_shown(&g, &lay), reskin, "{name} moved {dx}, {dy}: reskin");
            assert_eq!(vision::gradient_tab(&g, &lay).unwrap_or(false), tab, "{name} moved {dx}, {dy}: gradient tab");
        }
    }
}

#[test]
fn sees_the_games_white_dialog_box() {
    // a save's messages, whatever they say ("Error. Your save did not save correctly." on a
    // Windows screen with its taskbar), and the red arrow asking for a click
    for name in ["save-done.png", "save-error.png"] {
        let f = load(name);
        let at = vision::dialog_box(&f).expect(name);
        assert!((at.y / f.height() as f64 - 0.87).abs() < 0.05, "{name}: {at:?}");
        assert!(vision::dialog_waiting(&f).is_some(), "{name}: arrow");
    }
    for name in ["menu-open.png", "bag-open.png", "quaxly-shiny.png", "fight-menu.png", "dewpider-mirage.png", "party-fainted.png"] {
        assert!(vision::dialog_box(&load(name)).is_none(), "{name}");
    }
}
