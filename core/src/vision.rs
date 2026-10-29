//! Reading the screen by its colors: the Mute button (an encounter is on),
//! the battle menu, the enemy's box and icons, chat, the bag's favorites, MENU's tab.

use crate::layout::{self, Calibration, Layout};
use crate::px::{self, at, clip, hi, lo, mask, median, near, share, Frame, IBox, Pt, Rgb};
use crate::reader::{is_count, TextHit};
use crate::rules::norm;

#[inline]
pub fn red(c: Rgb) -> bool {
    c[0] > 150 && c[1] < 50 && c[2] < 50
}

#[inline]
fn gray_pill(c: Rgb) -> bool {
    let (h, l) = (hi(c), lo(c));
    h - l < 12 && (15..=60).contains(&h)
}

/// Red button with white text, 3.3x as wide as tall.
pub fn is_mute(f: &Frame, b: IBox) -> bool {
    let (w, h) = (b.w(), b.h());
    if h < 12 || !(2.6 < w as f64 / h as f64 && (w as f64 / h as f64) < 4.2) || px::count(f, b) == 0 {
        return false;
    }
    let white = share(f, b, |c| lo(c) > 200);
    share(f, b, red) > 0.6 && 0.05 < white && white < 0.4
}

/// The Mute button, or None. Checks the last known spot first, which is much
/// cheaper than searching the whole screen.
pub fn find_mute(f: &Frame, hint: Option<IBox>) -> Option<IBox> {
    if let Some(h) = hint.filter(|h| is_mute(f, *h)) {
        return Some(h);
    }
    mask(f, px::whole(f), red).blobs().into_iter().map(|b| b.b).find(|b| is_mute(f, *b))
}

/// Fight / Switch / Bag / Run are showing (one may be lit up by the mouse), in
/// their usual colors or the ones calibration saw (colors).
pub fn menu_ready_with(f: &Frame, lay: &Layout, colors: &Calibration) -> bool {
    layout::BUTTONS
        .iter()
        .filter(|(name, area, color)| {
            let Some(m) = median(f, lay.boxed(*area), |_| true) else { return false };
            [Some(*color), colors.colors.get(*name).copied()].into_iter().flatten().any(|c| {
                (0..3).all(|k| (m[k] - c[k] as f64).abs() < 30.0)
            })
        })
        .count()
        >= 3
}

pub fn menu_ready(f: &Frame, lay: &Layout) -> bool {
    menu_ready_with(f, lay, &lay.cal)
}

/// The enemy's gray box isn't covered (by the chat window, say): every
/// quarter of it is mostly its gray.
pub fn box_visible(f: &Frame, lay: &Layout) -> bool {
    let b = lay.boxed(IBox::new(layout::BOX.l, layout::BOX.t, 882, layout::BOX_END));
    let gray = lay.color("box").unwrap_or(layout::BOX_GRAY);
    let (x0, y0, x1, y1) = clip(f, b);
    let n = x1 - x0;
    if n == 0 || y1 == y0 {
        return false;
    }
    // numpy's array_split: the first n % 4 parts a column wider
    let mut x = x0;
    for k in 0..4 {
        let w = n / 4 + (k < n % 4) as usize;
        let part = IBox::new(x as i32, y0 as i32, (x + w) as i32, y1 as i32);
        x += w;
        if w == 0 || share(f, part, |c| near(c, gray, 18)) <= 0.5 {
            return false;
        }
    }
    true
}

/// Chat messages are on screen. They float over the enemy's box and get read
/// as its name, so nothing is read while they show. Every chat line starts in
/// the black margin left of the game.
pub fn chat_showing(f: &Frame, lay: &Layout) -> bool {
    let b = lay.boxed(layout::CHAT_MARGIN);
    let (x0, y0, x1, y1) = clip(f, b);
    if x1 == x0 || y1 == y0 {
        return false;
    }
    // On a wide screen (a phone) Roblox's own menu and chat buttons reach into
    // the top of this strip, dark gray pills: start below them.
    let rows = y1 - y0;
    let mut top = y0;
    if let Some(last) = (0..rows / 3).rev().find(|r| {
        share(f, IBox::new(x0 as i32, (y0 + r) as i32, x1 as i32, (y0 + r + 1) as i32), gray_pill) > 0.15
    }) {
        top = y0 + last + 3;
    }
    if top >= y1 {
        return false;
    }
    share(f, IBox::new(x0 as i32, top as i32, x1 as i32, y1 as i32), |c| hi(c) > 90) > 0.003
}

/// Roblox's chat button: the right end of the dark gray pill (menu, chat) in
/// the top-left corner, or None. Roblox draws it by its window, not by the
/// game, so it isn't where the layout would put it on every screen shape.
pub fn chat_button(f: &Frame) -> Option<Pt> {
    roblox_pill(f).map(|b| Pt::new(b.r as f64 - 0.6 * b.h() as f64, (b.t + b.b) as f64 / 2.0))
}

/// Roblox's own dark gray pill at the top left (its menu and chat buttons): the
/// phone's pill parks right after it.
pub fn roblox_pill(f: &Frame) -> Option<IBox> {
    let (w, h) = (f.width() as f64, f.height() as f64);
    let area = IBox::new(0, 0, (w * 0.3) as i32, (h * 0.15) as i32);
    mask(f, area, gray_pill)
        .blobs()
        .into_iter()
        .map(|b| b.b)
        .filter(|b| {
            let (bh, bw) = (b.h() as f64, b.w() as f64);
            h * 0.03 < bh && bh < h * 0.12 && (1.5..=4.0).contains(&(bw / bh)) && (b.l as f64) < w * 0.18
                && (b.r as f64) < w * 0.29
        })
        .fold(None::<IBox>, |best, b| if best.is_some_and(|x| x.t <= b.t) { best } else { Some(b) }) // the topmost
}

/// Roblox's chat window is open: its button's bubble is filled white (just an
/// outline while it's closed). None when the button isn't found.
pub fn chat_open(f: &Frame) -> Option<bool> {
    let p = chat_button(f)?;
    let r = 4.max((f.height() as f64 * 0.011) as i32);
    let (x, y) = (p.x as i32, p.y as i32);
    let b = IBox::new((x - r).max(0), (y - r).max(0), x + r, y + r);
    Some(px::count(f, b) > 0 && share(f, b, |c| lo(c) > 200) > 0.4)
}

/// The gradient's colored tab can be visible even when Windows OCR can't read its name.
/// Compare the two sides of the tab's lower part (the actual swatch, below its gray margin):
/// a plain box is gray on both sides; a gradient swatch has visibly different colors.
pub fn gradient_visible(f: &Frame, lay: &Layout) -> bool {
    let b = lay.boxed(layout::GRADIENT_TAB);
    let (x0, y0, x1, y1) = px::clip(f, b);
    let (w, h) = (x1 - x0, y1 - y0);
    if w < 12 || h < 6 {
        return false;
    }
    // The swatch sits in the lower part; exclude the outline and center divider too.
    let top = y0 + h * 2 / 5;
    let inset = (w / 10).max(1);
    let seam = (w / 24).max(1);
    let middle = x0 + w / 2;
    let left = IBox::new((x0 + inset) as i32, top as i32, (middle - seam) as i32, y1 as i32);
    let right = IBox::new((middle + seam) as i32, top as i32, (x1 - inset) as i32, y1 as i32);
    let (Some(a), Some(b)) = (median(f, left, |_| true), median(f, right, |_| true)) else {
        return false;
    };
    let difference: f64 = (0..3).map(|i| (a[i] - b[i]).abs()).sum();
    difference >= 45.0
}

/// (shiny, tint, owned): the dark blue star, the yellow sparkle, and the little
/// Poke Ball left of the HP bar (a species caught before).
pub fn icons(f: &Frame, lay: &Layout) -> (bool, bool, bool) {
    let b = lay.boxed(layout::ICONS);
    let shiny = share(f, b, |c| c[2] > 100 && c[0] < 60 && c[1] < 60) > 0.008;
    let tint = share(f, b, |c| c[0] > 220 && c[1] > 200 && c[2] < 160) > 0.008;
    let owned = share(f, lay.boxed(layout::OWNED_ICON), |c| c[0] > 170 && c[1] < 90 && c[2] < 90) > 0.01;
    (shiny, tint, owned)
}

/// How much of the screen isn't black, in %: 0 means the screenshot is blank.
pub fn brightness(f: &Frame) -> u32 {
    let p = f.as_raw();
    let n = p.len() / 3;
    let lit = (0..n).step_by(97).filter(|i| p[i * 3] as u32 + p[i * 3 + 1] as u32 + p[i * 3 + 2] as u32 > 30).count();
    (lit * 100 / (n / 97 + 1)) as u32
}

/// Center of the bright green Use button under the backpack's row at below.
pub fn find_use_button(f: &Frame, below: f64) -> Option<Pt> {
    let mut best: Option<(i32, Pt)> = None;
    for b in mask(f, px::whole(f), |c| c[1] > 200 && c[0] < 170 && c[2] < 170).blobs() {
        let (h, w) = (b.b.h(), b.b.w());
        let r = w as f64 / h as f64;
        if h > 8 && 3.0 < r && r < 12.0 && b.b.t as f64 > below && best.is_none_or(|x| w * h > x.0) {
            best = Some((w * h, b.b.center()));
        }
    }
    best.map(|b| b.1)
}

/// The bag's row at y is a favorite: its count tag is yellow; the others' are
/// light blue. Looked at left of the count ("x985") when that's read, and
/// where the layout puts the tag.
pub fn favorited(f: &Frame, texts: &[TextHit], y: f64, lay: &Layout) -> bool {
    let v = lay.v();
    let mut spots = vec![(lay.xy(layout::BAG_TAG.0, 0.0).x, lay.xy(layout::BAG_TAG.1, 0.0).x)];
    if let Some(t) = texts.iter().find(|t| is_count(&t.text) && (t.y - y).abs() < 30.0 * v) {
        spots.push((t.x - 110.0 * v, t.x - 70.0 * v));
    }
    spots.iter().any(|&(x0, x1)| {
        let b = IBox::new((x0 as i32).max(0), ((y - 10.0 * v) as i32).max(0), (x1 as i32).max(0),
                          ((y + 10.0 * v) as i32).max(0));
        px::count(f, b) > 0 && share(f, b, |c| c[0] > 200 && c[1] > 150 && c[2] < 150) > 0.4
    })
}

/// A row that isn't a favorite is in sight, and no search is narrowing the
/// list: favorites sit at the top, so then all of them are in sight.
pub fn plain_row_in_sight(f: &Frame, texts: &[TextHit], split: f64, lay: &Layout) -> bool {
    texts.iter().any(|t| norm(&t.text).starts_with("search") && t.x > split)
        && texts.iter().any(|t| is_count(&t.text) && t.x > split && !favorited(f, texts, t.y, lay))
}

const MENU_BLUE: [i32; 3] = [80, 150, 255]; // the MENU tab on the screen's left edge (STORE's green tab under it)

/// MENU's tab on the screen's left edge, the blue tab right above STORE's green
/// one (with the MENU open both sit right of its panel, which is that blue
/// too). Its word runs sideways, which Windows doesn't read (a Mac does): the
/// word only if the tabs aren't seen.
pub fn find_menu_tab(f: &Frame, texts: &[TextHit]) -> Option<Pt> {
    let h = f.height() as i32;
    let strip = IBox::new(0, 0, 8.max((f.width() as f64 * 0.15) as i32), h);
    let green = |c: Rgb| {
        let (r, g, b) = (c[0] as i32, c[1] as i32, c[2] as i32);
        g > 160 && g - r > 60 && g - b > 50
    };
    let store = mask(f, strip, green)
        .blobs()
        .into_iter()
        .map(|b| b.b)
        .filter(|b| b.h() as f64 > (1.5 * b.w() as f64).max(0.04 * h as f64))
        .fold(None::<IBox>, |best, b| if best.is_some_and(|x| x.h() >= b.h()) { best } else { Some(b) });
    if let Some(sl) = store {
        let (x0, x1, top) = (sl.l, sl.r, sl.t);
        // rows of MENU's blue in STORE's columns (its sideways letters leave its edges blue)
        let blue: Vec<bool> = (0..top)
            .map(|y| {
                share(f, IBox::new(x0, y, x1, y + 1), |c| {
                    (c[0] as i32 - MENU_BLUE[0]).abs() < 45 && (c[1] as i32 - MENU_BLUE[1]).abs() < 45
                        && (c[2] as i32 - MENU_BLUE[2]).abs() < 40
                }) > 0.15
            })
            .collect();
        let hf = h as f64;
        let mut y = top - 1;
        while y >= 0 && !blue[y as usize] && ((top - y) as f64) < 0.03 * hf {
            y -= 1; // the gap between the two tabs
        }
        let bottom = y;
        let mut last = y;
        while y >= 0 && (blue[y as usize] || ((last - y) as f64) < 0.01 * hf) {
            if blue[y as usize] {
                last = y;
            }
            y -= 1;
        }
        if bottom >= 0 && blue[bottom as usize] && (bottom - last) as f64 > 0.04 * hf {
            return Some(Pt::new((x0 + x1) as f64 / 2.0, (last + bottom) as f64 / 2.0));
        }
    }
    texts.iter().find(|t| norm(&t.text) == "menu").map(|t| Pt::new(t.x, t.y))
}

/// MENU's tab, as it looks: its patch's mean color (the game's back once it's this).
pub fn tab_look(f: &Frame, tab: Pt) -> [f64; 3] {
    let (x, y, r) = (tab.x as i32, tab.y as i32, 8);
    px::mean(f, IBox::new((x - r).max(0), (y - r).max(0), x + r, y + r))
        .unwrap_or([0.0; 3])
        .map(|c| (c * 10.0).round() / 10.0)
}

/// The wild Pokemon's 96 x 96 sprite as the game draws it (shiny, gradient,
/// reskin and all), for the website, which cuts it out with the species'
/// shape. It keeps its shape: scaled like the screen's height (phones squeeze
/// the battle).
pub fn sprite(f: &Frame, lay: &Layout) -> Frame {
    let o = lay.pt(layout::FOE_SPRITE);
    let k = layout::SPRITE_PIXEL * lay.v();
    let (w, h) = (f.width() as i64, f.height() as i64);
    Frame::from_fn(96, 96, |i, j| {
        let x = ((o.x + (i as f64 + 0.5) * k) as i64).clamp(0, w - 1);
        let y = ((o.y + (j as f64 + 0.5) * k) as i64).clamp(0, h - 1);
        image::Rgb(at(f, x as usize, y as usize))
    })
}

#[cfg(test)]
mod gradient_tests {
    use super::*;

    #[test]
    fn gradient_swatch_is_detected_without_reading_its_label() {
        let lay = Layout::flat(layout::MUTE);
        let tab = lay.boxed(layout::GRADIENT_TAB);
        let mut plain = Frame::from_pixel(2000, 1200, image::Rgb([143, 143, 143]));
        assert!(!gradient_visible(&plain, &lay));

        // A two-color swatch, like the gradient indicator, with no OCR text at all.
        let top = tab.t + tab.h() * 2 / 5;
        let middle = tab.l + tab.w() / 2;
        for y in top..tab.b {
            for x in tab.l..middle {
                plain.put_pixel(x as u32, y as u32, image::Rgb([100, 96, 117]));
            }
            for x in middle..tab.r {
                plain.put_pixel(x as u32, y as u32, image::Rgb([189, 184, 197]));
            }
        }
        assert!(gradient_visible(&plain, &lay));
    }
}
