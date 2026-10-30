//! Where the battle screen's parts are. Measured on samples/*.png (a 2000 px
//! wide Roblox window). Everything is placed relative to the Mute button, so
//! other window sizes scale along; windows (and phones) that stretch the game
//! differently are calibrated (calibrate.rs).

use crate::px::{IBox, Pt};
use std::collections::BTreeMap;

pub const MUTE: IBox = IBox::new(1533, 153, 1749, 218);
pub const BOX: IBox = IBox::new(370, 317, 890, 535); // enemy box plus the gradient tab under it
pub const HP_BAR_END: i32 = 435; // below this, inside the box: the reskin
pub const BOX_END: i32 = 487; // below this: the gradient tab
pub const ICONS: IBox = IBox::new(395, 425, 525, 490); // shiny star / tint sparkle
pub const OWNED_ICON: IBox = IBox::new(495, 395, 565, 445); // Poke Ball left of the HP bar: species caught before
pub const PANEL: IBox = IBox::new(300, 140, 1750, 1220); // the whole game, for screenshots
pub const FOE_SPRITE: Pt = Pt::new(1170.0, 285.0); // top left of the wild Pokemon's 96 x 96 sprite (Gen 5),
pub const SPRITE_PIXEL: f64 = 4.165; // drawn this many reference pixels per sprite pixel
/// Fight / Switch / Bag / Run: area, usual color.
pub const BUTTONS: [(&str, IBox, [i32; 3]); 4] = [
    ("fight", IBox::new(305, 1025, 655, 1115), [210, 219, 189]),
    ("switch", IBox::new(668, 1025, 1020, 1115), [145, 170, 157]),
    ("bag", IBox::new(1030, 1025, 1383, 1115), [61, 92, 111]),
    ("run", IBox::new(1393, 1025, 1745, 1115), [26, 51, 66]),
];
pub const RUN: Pt = Pt::new(1568.0, 1070.0);
pub const BAG: Pt = Pt::new(1206.0, 1070.0);
pub const FIGHT: Pt = Pt::new(480.0, 1070.0);
pub const MSG: IBox = IBox::new(300, 885, 1750, 1020); // battle text ("What should Clodsire do?")
pub const PLAYER_BOX: IBox = IBox::new(1168, 685, 1680, 870); // your Pokemon's box, HP "350 / 376"
pub const HP_BAR: IBox = IBox::new(1353, 784, 1655, 806); // inside your Pokemon's HP bar (its outline left out)
pub const BAG_SPLIT: f64 = 1100.0; // bag rows' names are left of this, the search box right
pub const BAG_TAG: (f64, f64) = (1320.0, 1380.0); // across: inside a bag row's count tag, left of its "x985"
pub const AFK_SPOT: Pt = Pt::new(150.0, 800.0); // black margin left of the game, below the chat window
pub const CHAT_MARGIN: IBox = IBox::new(80, 140, 270, 560); // black margin where chat lines start
pub const CHAT_BUTTON: Pt = Pt::new(185.0, 82.0); // Roblox's chat bubble, top left, if chat_button can't see it
pub const IDLE_SPOT: Pt = Pt::new(1550.0, 960.0); // empty right end of the "What should ... do?" box
pub const NAME_ROW: IBox = IBox::new(BOX.l + 15, BOX.t + 15, BOX.l + 320, BOX.t + 90); // "Mew", left of the level
pub const RESKIN_ROW: IBox = IBox::new(BOX.l + 40, HP_BAR_END, BOX.r - 20, BOX_END); // INFERNO, under the HP bar
pub const GRADIENT_TAB: IBox = IBox::new(BOX.l + 60, BOX_END, BOX.r - 60, BOX.b); // Mirage, in many colors
pub const RESKIN_INK: IBox = IBox::new(480, 442, 862, 483); // the reskin row right of the icons: plain gray without one
pub const TAB_EDGES: (i32, i32, i32) = (429, 824, 530); // the gradient tab's dark outline: left, right, bottom

/// Found on screen to calibrate, where the samples have them.
pub const ANCHORS: [(&str, IBox); 5] = [
    ("fight", IBox::new(300, 1020, 661, 1120)),
    ("switch", IBox::new(663, 1020, 1024, 1119)),
    ("bag", IBox::new(1025, 1020, 1388, 1119)),
    ("run", IBox::new(1389, 1020, 1750, 1120)),
    ("box", IBox::new(373, 319, 880, 485)),
];
pub const BOX_GRAY: [i32; 3] = [143, 143, 143];
pub const FLAT: [f64; 4] = [1.0, 1.0, 0.0, 0.0]; // kx, ky, dx, dy: the reference layout, just scaled

pub fn button_color(name: &str) -> [i32; 3] {
    if name == "box" {
        return BOX_GRAY;
    }
    BUTTONS.iter().find(|b| b.0 == name).map(|b| b.2).unwrap_or(BOX_GRAY)
}

/// A screen's fit: kx, ky (how much more or less the game is stretched across /
/// down than the Mute button's width says), dx, dy (shifted by, reference px),
/// and the colors seen on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    pub fit: [f64; 4],
    pub colors: BTreeMap<String, [i32; 3]>,
    /// The worst miss of the fit, screen px (for the log).
    pub miss: f64,
}

impl Default for Calibration {
    fn default() -> Self {
        Calibration { fit: FLAT, colors: BTreeMap::new(), miss: 0.0 }
    }
}

impl Calibration {
    /// As calibration.json keeps it: {"fight": [r, g, b], ..., "fit": [kx, ky, dx, dy]}.
    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for (k, c) in &self.colors {
            m.insert(k.clone(), serde_json::json!(c));
        }
        m.insert("fit".into(), serde_json::json!(self.fit));
        serde_json::Value::Object(m)
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Calibration> {
        let o = v.as_object()?;
        let fit = o.get("fit")?.as_array()?;
        if fit.len() != 4 {
            return None;
        }
        let mut cal = Calibration::default();
        for (i, f) in fit.iter().enumerate() {
            cal.fit[i] = f.as_f64()?;
        }
        for (k, c) in o {
            if let Some(a) = c.as_array().filter(|a| a.len() == 3 && k != "fit") {
                let rgb = [a[0].as_i64()?, a[1].as_i64()?, a[2].as_i64()?].map(|v| v as i32);
                cal.colors.insert(k.clone(), rgb);
            }
        }
        Some(cal)
    }
}

/// Maps reference coordinates onto the current screenshot.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub mute: IBox,
    pub s: f64,
    pub cal: Calibration,
}

impl Layout {
    pub fn new(mute: IBox, cal: &Calibration) -> Layout {
        Layout { mute, s: mute.w() as f64 / MUTE.w() as f64, cal: cal.clone() }
    }

    pub fn flat(mute: IBox) -> Layout {
        Layout::new(mute, &Calibration::default())
    }

    pub fn pt(&self, p: Pt) -> Pt {
        let [kx, ky, dx, dy] = self.cal.fit;
        Pt::new(self.mute.l as f64 + ((p.x - MUTE.l as f64) * kx + dx) * self.s,
                self.mute.t as f64 + ((p.y - MUTE.t as f64) * ky + dy) * self.s)
    }

    pub fn xy(&self, x: f64, y: f64) -> Pt {
        self.pt(Pt::new(x, y))
    }

    /// A reference box on this screen (its top left kept on it).
    pub fn boxed(&self, b: IBox) -> IBox {
        self.boxf(b.l as f64, b.t as f64, b.r as f64, b.b as f64)
    }

    pub fn boxf(&self, l: f64, t: f64, r: f64, b: f64) -> IBox {
        let a = self.xy(l, t);
        let z = self.xy(r, b);
        IBox::new((a.x as i32).max(0), (a.y as i32).max(0), z.x as i32, z.y as i32)
    }

    /// Screen px per reference px, down.
    pub fn v(&self) -> f64 {
        self.s * self.cal.fit[1]
    }

    pub fn color(&self, name: &str) -> Option<[i32; 3]> {
        self.cal.colors.get(name).copied()
    }
}
