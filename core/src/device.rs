//! What the hunter needs from the computer or phone it runs on: its screen,
//! its mouse or touch, its text recognition, its sounds.

use crate::px::{Frame, IBox, Pt};
use crate::reader::TextHit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    /// A wanted one: holding the battle for you.
    Found,
    Paused,
    Resumed,
    /// A browser logged in with the website code.
    Login,
}

/// How the platform differs.
#[derive(Clone, Debug)]
pub struct Traits {
    /// A phone: taps and swipes, an ON / OFF button floating over the game, the
    /// bag scrolled when its search box can't be typed into.
    pub phone: bool,
    /// For the website: "darwin", "win32", "android".
    pub kind: &'static str,
    /// s with the Mute button before an encounter counts as started.
    pub seen_after: f64,
    /// s between looks while waiting on the game's screens.
    pub poll: f64,
    /// Windows' text recognition misses the reskin and gradient rows in the
    /// whole enemy's box: each is read on its own.
    pub ocr_rows: bool,
    /// How to pause and resume, for the hunter's messages: "Ctrl+Shift+H", "tap ON".
    pub toggle_key: &'static str,
}

pub trait Device: Send + Sync {
    fn traits(&self) -> Traits;

    /// A screenshot of the screen Roblox is on (waits for one if there's none yet).
    fn grab(&self) -> Frame;

    /// A click (a tap on a phone) at a screenshot's pixel. right: the right
    /// button (desktop).
    fn click(&self, at: Pt, right: bool);

    /// A finger dragged from one point to another over that long (phones).
    fn swipe(&self, _from: Pt, _to: Pt, _seconds: f64) {}

    /// The mouse wheel turned over a spot, notches (up when positive; desktop: the bag's
    /// list is scrolled with it, never searched).
    fn wheel(&self, _at: Pt, _notches: i32) {}

    /// The mouse to a spot, without clicking (desktop; out of the way of buttons).
    fn move_pointer(&self, _at: Pt) {}

    /// Where the mouse is, in screenshot px (to notice you moving it). None on a phone.
    fn pointer(&self) -> Option<Pt> {
        None
    }

    /// The mouse was flung into a screen corner: stop.
    fn fail_safe(&self) -> bool {
        false
    }

    /// Types into the text field that has the focus, what was in it replaced.
    /// True if it took.
    fn type_text(&self, text: &str) -> bool;

    /// Keeps the on-screen keyboard from showing (true), or lets it show again.
    fn keyboard_hidden(&self, _hidden: bool) {}

    fn roblox_in_front(&self) -> bool;
    fn focus_roblox(&self);

    /// Roblox has a window at all (false: closed, or crashed). True where that isn't known.
    fn roblox_open(&self) -> bool {
        true
    }

    /// What floats over the game (the phone's ON / OFF button), in screenshot
    /// px: text read under it isn't the game's.
    fn overlay(&self) -> Option<IBox> {
        None
    }

    /// The battle's Mute button is at mute: the ON / OFF button goes under it.
    fn place_overlay(&self, _after: IBox) {}

    /// s since you last pressed a key or touched the screen (yourself, not the
    /// hunter), if that's known.
    fn idle(&self) -> Option<f64> {
        None
    }

    /// The text in a picture: [(text, center x, center y)] with x, y as
    /// fractions of the picture, top-left origin. Words far apart on a row
    /// come as texts of their own.
    fn ocr(&self, img: &Frame) -> Vec<TextHit>;

    fn sound(&self, sound: Sound);

    /// Keeps a picture where you can see it; returns where. None: in Hunt's
    /// own folder (debug/, finds/), as the hunter does by itself.
    fn save_picture(&self, _png: &[u8], _folder: &str, _name: &str) -> Option<String> {
        None
    }

    /// For the website: "Mac mini (Mac)", "Google Pixel 7, Android 16".
    fn label(&self) -> String;

    /// For the debug channel: "Windows 11", "macOS 15.6".
    fn system(&self) -> String;

    /// A browser logged in with this device's website code.
    fn website_login(&self) {}

    /// The screens, Roblox's windows, display scaling: for --diagnose.
    fn screen_info(&self) -> String {
        String::new()
    }

    /// Screenshot px per mouse px (2 on a Retina Mac), for --diagnose.
    fn scale(&self) -> f64 {
        1.0
    }
}
