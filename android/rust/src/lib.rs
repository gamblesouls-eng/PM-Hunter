//! The hunter for the Android app (libhunt.so): hunt-core, the same as Mac and
//! Windows, with the phone's eyes and hands (an accessibility service's
//! screenshots, taps and swipes, ML Kit's text recognition) given by Kotlin
//! as a Phone. The Kotlin for it is made by uniffi-bindgen (com.pokehunt.core).

use hunt_core::device::{Device, Sound, Traits};
use hunt_core::{Config, Control, DebugChannel, Frame, IBox, Options, Pt, Sink, Store, TextHit};
use std::sync::{Arc, Mutex};

uniffi::setup_scaffolding!();

/// A screenshot (or a part of one): RGBA bytes, row by row, as an Android
/// Bitmap's ARGB_8888 pixels lie in memory.
#[derive(uniffi::Record)]
pub struct Shot {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

/// A box on the screenshot, in its pixels.
#[derive(uniffi::Record, Clone, Copy)]
pub struct Rect {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

/// Text read: its center as fractions of the picture, top-left origin.
#[derive(uniffi::Record)]
pub struct Text {
    pub text: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, uniffi::Error)]
pub enum HuntError {
    /// config.json has a mistake.
    Config { why: String },
}

impl std::fmt::Display for HuntError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HuntError::Config { why } => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for HuntError {}

/// What the hunter needs from the phone (HuntService does it).
#[uniffi::export(with_foreign)]
pub trait Phone: Send + Sync {
    /// A screenshot, upright, at most 2000 px wide.
    fn grab(&self) -> Shot;
    fn tap(&self, x: f64, y: f64);
    /// A finger dragged from (x0, y0) to (x1, y1) over that many seconds.
    fn swipe(&self, x0: f64, y0: f64, x1: f64, y1: f64, seconds: f64);
    /// Puts text into the text field that has the focus (Roblox opens a real one over a tapped
    /// TextBox, like the bag's search box). True if it took.
    fn type_text(&self, text: String) -> bool;
    /// Keeps the on-screen keyboard from showing (true), or lets it show again.
    fn keyboard_hidden(&self, hidden: bool);
    fn roblox_in_front(&self) -> bool;
    fn focus_roblox(&self);
    /// Where the ON / OFF button is on the screenshots, to not read it as the game's.
    fn button_box(&self) -> Option<Rect>;
    /// Roblox's menu and chat buttons are at after (screenshot px): the pill goes right after them.
    fn place_button(&self, after: Rect);
    /// Seconds since you last touched the screen yourself, if you have.
    fn idle(&self) -> Option<f64>;
    /// The text in a picture: words far apart on a row as texts of their own.
    fn ocr(&self, shot: Shot) -> Vec<Text>;
    /// "found", "paused", "resumed", "login"
    fn sound(&self, kind: String);
    /// Saves a picture where you can see it (Pictures/Hunt); returns where.
    fn save_picture(&self, png: Vec<u8>, name: String) -> String;
    /// Which phone this is: "Google Pixel 7, Android 16".
    fn label(&self) -> String;
    /// A browser logged in with the website code.
    fn website_login(&self);
    /// A line the hunter says: on the app's screen, in hunt.log.
    fn say(&self, line: String);
    /// The website's login code (None once it's spent).
    fn code(&self, code: Option<String>);
}

fn shot(f: &Frame) -> Shot {
    let mut rgba = Vec::with_capacity(f.as_raw().len() / 3 * 4);
    for p in f.as_raw().chunks_exact(3) {
        rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
    }
    Shot { width: f.width() as i32, height: f.height() as i32, rgba }
}

fn frame(s: Shot) -> Option<Frame> {
    let (w, h) = (s.width.max(0) as u32, s.height.max(0) as u32);
    if (w as usize) * (h as usize) * 4 > s.rgba.len() || w == 0 || h == 0 {
        return None;
    }
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for p in s.rgba.chunks_exact(4).take((w * h) as usize) {
        rgb.extend_from_slice(&p[..3]);
    }
    Frame::from_raw(w, h, rgb)
}

/// The phone as the hunter's Device.
struct OnPhone(Arc<dyn Phone>);

impl Device for OnPhone {
    fn traits(&self) -> Traits {
        Traits { phone: true, kind: "android", seen_after: 0.4, poll: 0.4, ocr_rows: false, toggle_key: "tap ON" }
    }

    fn grab(&self) -> Frame {
        loop {
            if let Some(f) = frame(self.0.grab()) {
                return f;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn click(&self, at: Pt, _right: bool) {
        self.0.tap(at.x, at.y);
    }

    fn swipe(&self, from: Pt, to: Pt, seconds: f64) {
        self.0.swipe(from.x, from.y, to.x, to.y, seconds);
    }

    fn type_text(&self, text: &str) -> bool {
        self.0.type_text(text.to_string())
    }

    fn keyboard_hidden(&self, hidden: bool) {
        self.0.keyboard_hidden(hidden);
    }

    fn roblox_in_front(&self) -> bool {
        self.0.roblox_in_front()
    }

    fn focus_roblox(&self) {
        self.0.focus_roblox();
    }

    fn overlay(&self) -> Option<IBox> {
        self.0.button_box().map(|r| IBox::new(r.l, r.t, r.r, r.b))
    }

    fn place_overlay(&self, after: IBox) {
        self.0.place_button(Rect { l: after.l, t: after.t, r: after.r, b: after.b });
    }

    fn idle(&self) -> Option<f64> {
        self.0.idle()
    }

    fn ocr(&self, img: &Frame) -> Vec<TextHit> {
        if img.width() < 8 || img.height() < 8 {
            return Vec::new();
        }
        self.0.ocr(shot(img)).into_iter().map(|t| TextHit::new(t.text, t.x, t.y)).collect()
    }

    fn sound(&self, sound: Sound) {
        let kind = match sound {
            Sound::Found => "found",
            Sound::Paused => "paused",
            Sound::Resumed => "resumed",
            Sound::Login => "login",
        };
        self.0.sound(kind.into());
    }

    fn save_picture(&self, png: &[u8], _folder: &str, name: &str) -> Option<String> {
        Some(self.0.save_picture(png.to_vec(), name.to_string()))
    }

    fn label(&self) -> String {
        self.0.label()
    }

    fn system(&self) -> String {
        self.0.label()
    }

    fn website_login(&self) {
        self.0.website_login();
    }
}

/// The hunter's words to the app.
struct ToApp(Arc<dyn Phone>);

impl Sink for ToApp {
    fn line(&self, text: &str) {
        self.0.say(text.to_string());
    }
    fn code(&self, code: Option<&str>) {
        self.0.code(code.map(String::from));
    }
}

/// A hunt: made by HuntService, run on a thread of its own (run() returns when it's stopped).
#[derive(uniffi::Object)]
pub struct Hunter {
    control: Arc<Control>,
    hunt: Mutex<Option<hunt_core::Hunt>>,
}

#[uniffi::export]
impl Hunter {
    /// config: config.json's text; data_dir: the app's files (calibration.json,
    /// encounters.csv, menu.json); key: the phone's secret for the website
    /// ("" for none: watching only), its log on the website too.
    #[uniffi::constructor]
    pub fn new(phone: Arc<dyn Phone>, config: String, data_dir: String, watch: bool, debug: bool, key: String,
               build: String) -> Result<Arc<Hunter>, HuntError> {
        let cfg = Config::parse(&config).map_err(|why| HuntError::Config { why })?;
        let label = phone.label();
        let channel = DebugChannel::new(if watch { "" } else { &key }, &format!("{label} · {build}"));
        channel.set_header(&format!("Hunt {build} on {label}"));
        let opts = Options { watch, debug, window: true, build, key: if watch { String::new() } else { key },
                             ..Default::default() };
        channel.loud.store(debug, std::sync::atomic::Ordering::Relaxed);
        let (control, hunt) = hunt_core::start(Arc::new(OnPhone(phone.clone())), Arc::new(ToApp(phone)), channel, cfg,
                                               Store::new(data_dir), opts);
        Ok(Arc::new(Hunter { control, hunt: Mutex::new(Some(hunt)) }))
    }

    /// The hunt, until stopped (on the caller's thread). Once only.
    pub fn run(&self) {
        let hunt = self.hunt.lock().unwrap().take();
        if let Some(h) = hunt {
            h.run();
        }
    }

    pub fn stop(&self) {
        self.control.stop();
    }

    /// ON / OFF.
    pub fn toggle(&self) {
        self.control.toggle();
    }

    pub fn running(&self) -> bool {
        self.control.running()
    }

    /// Taken over from the website.
    pub fn remote(&self) -> bool {
        self.control.remote()
    }

    /// The website's code, while there's one to type.
    pub fn code(&self) -> String {
        self.control.code()
    }

    /// A browser logged in with the last code.
    pub fn logged_in(&self) -> bool {
        self.control.logged_in()
    }

    /// Linked to the website (not while watching).
    pub fn linked(&self) -> bool {
        self.control.linked()
    }

    /// A new website code (waits on the network: not on the main thread).
    pub fn new_code(&self) -> bool {
        self.control.link().is_some_and(|l| l.new_code().is_ok())
    }

    /// Every browser logged out (waits on the network).
    pub fn log_out_browsers(&self) -> bool {
        self.control.link().is_some_and(|l| l.log_out_browsers().is_ok())
    }

    /// Debug mode: a debug run on the next encounter.
    pub fn set_test(&self, on: bool) {
        self.control.set_test(on);
    }

    /// A debug run on its way or going (play shows the bug till it's done).
    pub fn testing(&self) -> bool {
        self.control.testing()
    }

    /// A wanted one on screen, held for you (the pill turns white).
    pub fn found(&self) -> bool {
        let w = self.control.web.lock().unwrap();
        w.phase == "battle" && w.wanted
    }

    /// The layout fitted again at the next encounter, and drawn (Pictures/Hunt).
    pub fn calibrate_next(&self) {
        self.control.calibrate_next();
    }
}

/// Every screen's fit forgotten: fitted again at the next encounter.
#[uniffi::export]
pub fn forget_calibration(data_dir: String) {
    Store::new(data_dir).forget_calibrations();
}

/// How much of a screenshot isn't black, in %: 0 means it's blank.
#[uniffi::export]
pub fn brightness(shot: Shot) -> u32 {
    frame(shot).map(|f| hunt_core::vision::brightness(&f)).unwrap_or(0)
}

/// This phone's new secret for the website.
#[uniffi::export]
pub fn new_key() -> String {
    hunt_core::web::new_key()
}
