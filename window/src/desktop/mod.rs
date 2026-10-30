//! The hunter's eyes and hands on a Mac or a Windows PC: the main screen,
//! clicked with the mouse. Screenshots are in the screen's real pixels and so
//! are clicks, so the hunter never deals with Retina points or Windows display
//! scaling. Text recognition is the system's own: Vision on macOS,
//! Windows.Media.Ocr on Windows.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as os;
#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as os;

#[allow(unused_imports)]
pub use os::set_own_window;

use hunt_core::device::{Device, Sound, Traits};
use hunt_core::{Frame, Pt, TextHit};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The keys the hunter listens for: Ctrl+Shift+H, N, L.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey {
    /// Pause / resume.
    Toggle,
    /// A new website code.
    NewCode,
    /// Every browser logged out.
    LogOut,
}

/// What the keyboard has down, looked at often.
#[derive(Clone, Copy, Default)]
pub struct Keys {
    pub ctrl: bool,
    pub shift: bool,
    pub h: bool,
    pub n: bool,
    pub l: bool,
    /// Any key at all (you're back at the keyboard).
    pub any: bool,
}

pub struct Desktop {
    last_key: Arc<Mutex<Option<Instant>>>,
}

impl Desktop {
    /// The desktop, with the keyboard watched (hotkeys go to on_key).
    pub fn new(on_key: impl Fn(Hotkey) + Send + 'static) -> Arc<Desktop> {
        os::init();
        let last_key = Arc::new(Mutex::new(None));
        let seen = last_key.clone();
        std::thread::Builder::new()
            .name("keys".into())
            .spawn(move || {
                let mut was = Keys::default();
                loop {
                    std::thread::sleep(Duration::from_millis(40));
                    let k = os::keys();
                    if k.any && !was.any {
                        *seen.lock().unwrap() = Some(Instant::now());
                    }
                    if k.ctrl && k.shift {
                        for (down, before, key) in [(k.h, was.h, Hotkey::Toggle), (k.n, was.n, Hotkey::NewCode), (k.l, was.l, Hotkey::LogOut)] {
                            if down && !before {
                                on_key(key);
                            }
                        }
                    }
                    was = k;
                }
            })
            .expect("a thread for the keys");
        Arc::new(Desktop { last_key })
    }
}

impl Device for Desktop {
    fn traits(&self) -> Traits {
        Traits {
            phone: false,
            kind: if cfg!(windows) { "win32" } else { "darwin" },
            seen_after: 0.0,
            poll: 0.15,
            ocr_rows: cfg!(windows),
            toggle_key: "Ctrl+Shift+H",
        }
    }

    fn grab(&self) -> Frame {
        loop {
            if let Some(f) = os::grab() {
                return f;
            }
            std::thread::sleep(Duration::from_millis(200)); // (the screen locked, say)
        }
    }

    fn click(&self, at: Pt, right: bool) {
        os::click(at, right);
    }

    fn move_pointer(&self, at: Pt) {
        os::move_to(at);
    }

    fn wheel(&self, at: Pt, notches: i32) {
        os::wheel(at, notches);
    }

    fn pointer(&self) -> Option<Pt> {
        Some(os::pointer())
    }

    fn fail_safe(&self) -> bool {
        os::in_corner()
    }

    fn type_text(&self, text: &str) -> bool {
        os::type_text(text);
        true
    }

    fn roblox_in_front(&self) -> bool {
        os::roblox_in_front()
    }

    fn roblox_open(&self) -> bool {
        #[cfg(windows)]
        return os::roblox_open();
        #[cfg(not(windows))]
        return true;
    }

    fn focus_roblox(&self) {
        os::focus_roblox();
        std::thread::sleep(Duration::from_millis(800));
    }

    fn idle(&self) -> Option<f64> {
        self.last_key.lock().unwrap().map(|t| t.elapsed().as_secs_f64())
    }

    fn ocr(&self, img: &Frame) -> Vec<TextHit> {
        os::ocr(img)
    }

    fn sound(&self, sound: Sound) {
        std::thread::spawn(move || match sound {
            Sound::Found => {
                for _ in 0..3 {
                    os::sound(sound);
                    std::thread::sleep(Duration::from_millis(400));
                }
            }
            _ => os::sound(sound),
        });
    }

    fn label(&self) -> String {
        format!("{} ({})", os::hostname(), if cfg!(windows) { "Windows" } else { "Mac" })
    }

    fn system(&self) -> String {
        os::system()
    }

    fn screen_info(&self) -> String {
        os::screen_info()
    }

    fn scale(&self) -> f64 {
        os::scale()
    }
}

/// Words far apart on a line come as texts of their own ("Quick Ball      x12"),
/// the way a Mac's Vision reports them: (text, left, top, right, bottom) in px.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn split_line(words: &[(String, f64, f64, f64, f64)], w: f64, h: f64, out: &mut Vec<TextHit>) {
    let mut group: Vec<&(String, f64, f64, f64, f64)> = Vec::new();
    let flush = |group: &mut Vec<&(String, f64, f64, f64, f64)>, out: &mut Vec<TextHit>| {
        if group.is_empty() {
            return;
        }
        let text = group.iter().map(|g| g.0.as_str()).collect::<Vec<_>>().join(" ");
        let l = group.iter().map(|g| g.1).fold(f64::INFINITY, f64::min);
        let t = group.iter().map(|g| g.2).fold(f64::INFINITY, f64::min);
        let r = group.iter().map(|g| g.3).fold(f64::NEG_INFINITY, f64::max);
        let b = group.iter().map(|g| g.4).fold(f64::NEG_INFINITY, f64::max);
        out.push(TextHit::new(text, (l + r) / 2.0 / w, (t + b) / 2.0 / h));
        group.clear();
    };
    for word in words {
        if let Some(prev) = group.last() {
            let gap = word.1 - prev.3;
            if gap > 1.2 * (word.4 - word.2).max(prev.4 - prev.2) {
                flush(&mut group, out);
            }
        }
        group.push(word);
    }
    flush(&mut group, out);
}
