//! For checking a new computer: the samples read (--diagnose), a screenshot
//! file read (--image), a test message (--test-discord).

use crate::calibrate::calibrate;
use crate::hunter::Hunt;
use crate::layout::{self, Layout};
use crate::picture::{self, crop};
use crate::rules::{same_word, wanted};
use crate::util::{now, sleep};
use crate::vision::{box_visible, chat_showing, find_mute, menu_ready};
use crate::px::Pt;
use std::path::{Path, PathBuf};

/// What the Mac reads in samples/ (species, shiny, tint, reskin, gradient); a new
/// computer should read the same. Levels are left out: the Mac misses a few.
pub const EXPECTED: [(&str, (&str, bool, bool, &str, &str)); 16] = [
    ("dewpider-mirage.png", ("Dewpider", false, false, "", "Mirage")),
    ("marill-inferno-tint.png", ("Marill", false, true, "INFERNO", "")),
    ("quaxly-shiny.png", ("Quaxly", true, false, "", "")),
    ("remoraid-shiny.png", ("Remoraid", true, false, "", "")),
    ("shellos-tint.png", ("Shellos", false, true, "", "")),
    ("phone-pikachu-shiny.png", ("Pikachu", true, false, "", "")),
    ("phone16-pichu-shiny.png", ("Pichu-Spiky-eared", true, false, "", "")),
    ("entei-galaxy.png", ("Entei", false, false, "GALAXY", "")),
    ("entei-plain.png", ("Entei", false, false, "", "")),
    ("entei-wave-tint.png", ("Entei", false, true, "WAVE", "")),
    ("entei-coal.png", ("Entei", false, false, "COAL", "")),
    ("mew-toxic.png", ("Mew", false, false, "TOXIC FLUID", "")),
    ("lotad-skittles.png", ("Lotad", false, false, "", "Skittles")),
    ("tyrogue-mummified.png", ("Tyrogue", false, false, "MUMMIFIED", "")),
    ("tatsugiri-plain-tint.png", ("Tatsugiri-Droopy", false, true, "", "")),
    ("runerigus-amberfall.png", ("Runerigus", false, false, "", "Amberfall")),
];

/// samples/: next to the program (as installed), in the folder it runs in, or
/// HUNT_SAMPLES.
pub fn samples_dir() -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = std::env::var_os("HUNT_SAMPLES").map(PathBuf::from).into_iter().collect();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        places.push(dir.join("samples"));
        // (a build in target/release: the repository's)
        places.extend(dir.ancestors().map(|a| a.join("samples")));
    }
    places.push(PathBuf::from("samples"));
    places.into_iter().find(|p| p.join("quaxly-shiny.png").exists())
}

impl Hunt {
    /// For a first run on a new computer: is the text read like on the Mac, how
    /// big is the screen, do mouse positions line up, is Roblox there? Moves the
    /// mouse a little, never clicks.
    pub fn diagnose(&mut self) {
        let dev = self.dev.clone();
        self.print(&format!("Hunt on {}", dev.system()));
        self.print("== reading the samples");
        let real = self.cal_file.clone();
        self.cal_file = self.c.store.path("debug").join("calibration-samples.json");
        let _ = std::fs::create_dir_all(self.c.store.path("debug"));
        let dir = samples_dir();
        let mut ok = 0;
        for (name, want) in EXPECTED {
            let Some(f) = dir.as_ref().and_then(|d| picture::open(&d.join(name)).ok()) else {
                self.print(&format!("  {name}: missing"));
                continue;
            };
            self.forget_screen();
            let (_, lay) = self.look_at(&f);
            let lay = lay.and_then(|l| {
                self.auto_calibrate(&f, l.mute, false);
                Some(Layout::new(l.mute, &self.cal))
            });
            let Some(lay) = lay.filter(|l| menu_ready(&f, l)) else {
                self.print(&format!("  {name}: FAILED, no battle found"));
                continue;
            };
            let start = now();
            let enc = self.read_enemy(&f, &lay);
            let same = same_word(want.0, &enc.species) && enc.shiny == want.1 && enc.tint == want.2
                && [(want.3, &enc.reskin), (want.4, &enc.gradient)].iter()
                    .all(|(w, g)| if w.is_empty() { g.is_empty() } else { same_word(w, g) });
            ok += same as u32;
            self.print(&format!("  {name}: {}  read {}  ({:.1} s){}", if same { "ok" } else { "DIFFERENT" }, enc.describe(),
                                now() - start, if same { String::new() } else { format!("  expected {want:?}") }));
            if !same {
                // what the text recognition saw in the enemy's box, to tell why
                let texts = self.ocr(&f, Some(lay.boxed(layout::BOX)), 1.0);
                self.print(&format!("    the box's text: {}", texts.iter()
                    .map(|t| format!("{:?} at ({:.2}, {:.2})", t.text, t.x, t.y)).collect::<Vec<_>>().join(", ")));
            }
        }
        self.print(&format!("  {ok} of {} read like on the Mac", EXPECTED.len()));
        self.cal_file = real;
        self.forget_screen();

        self.print("== the screen");
        let f = self.grab();
        self.print(&format!("  screenshot {}x{}, {} screenshot px per mouse px", f.width(), f.height(), dev.scale()));
        let info = dev.screen_info();
        if !info.is_empty() {
            self.print(&format!("  {}", info.replace('\n', "\n  ")));
        }
        self.c.out.debug.shot("what the screen looks like", &f, false);
        let (f, lay) = self.look(None);
        match lay {
            Some(l) => {
                self.print("  a battle is on screen: calibrating");
                if !self.auto_calibrate(&f, l.mute, true) {
                    self.print("  couldn't calibrate: the Fight / Switch / Bag / Run menu isn't up");
                }
                let lay = Layout::new(l.mute, &self.cal);
                if menu_ready(&f, &lay) && box_visible(&f, &lay) {
                    let enc = self.read_enemy(&f, &lay);
                    self.print(&format!("  reads: {}", enc.describe()));
                }
            }
            None => self.print("  no battle on screen (that's fine; start one and run this again to check calibration)"),
        }

        if let Some(home) = dev.pointer() {
            self.print("== the mouse (moves, no clicks)");
            for (fx, fy) in [(0.3, 0.3), (0.7, 0.3), (0.7, 0.7), (0.3, 0.7)] {
                let want = Pt::new(f.width() as f64 * fx, f.height() as f64 * fy);
                dev.move_pointer(want);
                sleep(0.15);
                let at = dev.pointer().unwrap_or_default();
                let off = (at.x - want.x).abs() + (at.y - want.y).abs();
                self.print(&format!("  to ({:.0}, {:.0}): mouse at ({:.0}, {:.0}), {}", want.x, want.y, at.x, at.y,
                                    if off <= 3.0 * dev.scale() { "ok".to_string() } else { format!("OFF BY {off:.0}") }));
            }
            dev.move_pointer(home);
        }
        self.print("== done");
        self.c.out.debug.flush();
    }

    /// Reads a screenshot file and says what it sees.
    pub fn read_image(&mut self, path: &Path, draw: bool) {
        let shown = path.display();
        let f = match picture::open(path) {
            Ok(f) => f,
            Err(e) => return self.print(&e),
        };
        let (_, lay) = self.look_at(&f);
        let Some(l) = lay else { return self.print(&format!("{shown}: no encounter (no Mute button)")) };
        if draw && !self.auto_calibrate(&f, l.mute, true) {
            self.print(&format!("{shown}: couldn't calibrate: no Fight/Switch/Bag/Run menu found"));
        }
        if !self.cal_saved {
            // a screen of another shape: fit it, without saving
            if let Some(cal) = calibrate(&f, l.mute) {
                self.cal = cal;
            }
        }
        let lay = Layout::new(l.mute, &self.cal);
        if chat_showing(&f, &lay) {
            return self.print(&format!("{shown}: encounter, but chat messages cover the enemy's box"));
        }
        if !box_visible(&f, &lay) {
            return self.print(&format!("{shown}: encounter, but the enemy's box is covered"));
        }
        let enc = self.read_enemy(&f, &lay);
        let rule = wanted(&enc, &self.c.cfg.targets);
        self.print(&format!("{shown}: {}  menu {}  -> {}", enc.describe(),
                            if menu_ready(&f, &lay) { "ready" } else { "not ready" },
                            rule.map(|r| format!("WANTED {}", serde_json::Value::Object(r.clone()))).unwrap_or_else(|| "run".into())));
        let _ = crop;
        let _ = find_mute;
    }

    /// A test message to the webhook; true once Discord has it.
    pub fn test_discord(&self) -> bool {
        self.c.discord.post_and_wait("pokehunt is connected. Wanted encounters will show up here.", None)
    }
}
