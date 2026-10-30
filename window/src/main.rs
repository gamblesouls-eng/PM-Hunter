//! Hunt's window (Hunt.exe): one pill with no frame. Start / stop (a right
//! click: Debug mode), the website code, a box for the Discord webhook, and two
//! round buttons: Hunt's folder, quit. Updates install themselves while it isn't
//! hunting.
//!
//! The hunter (hunt-core, the same as Android's) runs in Hunt itself, on a
//! thread of its own: what it says goes to hunt.log, its state sets the pill,
//! and the pill's buttons reach it directly. What a PC keeps (config.json,
//! finds, the website key, hunt.log, window.json) is in %LOCALAPPDATA%\Hunt;
//! on a Mac, the folder Hunt runs in (HUNT_DATA).
//!
//! On a Mac (from Terminal, which holds the Screen Recording and Accessibility grants):
//!   HUNT_DATA=. cargo run --release            the pill
//!   cargo run --release -- --terminal          the hunter alone (--watch, --paused, --debug, --calibrate)
//!   cargo run --release -- --diagnose          the samples read, the screen, the mouse
//!   cargo run --release -- --image x.png       a screenshot read
#![cfg_attr(windows, windows_subsystem = "windows")]

mod desktop;
mod engine;

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CursorIcon, FontFamily, FontId, Id, Pos2, Rect, Sense, Stroke, Vec2,
    ViewportCommand,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{env, thread};

const W: f32 = 340.0; // the pill
const H: f32 = 40.0;
const CONTROL: f32 = 26.0; // every button and box is this tall
// room round the pill in its window (see-through): for the shadow it has while dragged.
// The pill's place (pos, window.json) is the pill's own; the window is M up and left of it.
const M: f32 = 16.0;
const MARGIN: Vec2 = Vec2::splat(M);
const WINDOW: Vec2 = Vec2::new(W + 2.0 * M, H + 2.0 * M);
const INSET: f32 = (H - CONTROL) / 2.0; // the same room all round the pill's ends
const TEXT: f32 = 13.0; // the words' size (a whole number draws crisper)

// the opening: a big Clodsire mid-screen, then it shrinks into the pill's end
const BIG: f32 = 160.0; // its size, at first
const INTRO_MOVE: f32 = 1.55; // s: when it sets off (it's come in and breathed by then)
const INTRO_LAND: f32 = 2.35; // s: when it's the pill's end, play in it
const INTRO_END: f32 = 2.65; // s: the landing's ripple gone, the window back over the pill
const HANDOVER: u64 = 90; // s: an update's new Hunt, as long as the old one waits for it
// the closing: the pill shrinks rightwards into a ball, and the ball goes
const SETTLE: f32 = 0.95; // s: an update's bar going back into play, the pill coming up
const FOLD: f32 = 0.3; // s, shrinking into the ball
const VANISH: f32 = 0.25; // s, the ball going

const INK: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);
const PAPER: Color32 = Color32::WHITE;

/// Where a newer build is looked for, the file it comes as, and this system's name for the website.
#[cfg(target_os = "macos")]
const UPDATES: (&str, &str, &str) = ("mac.json", "Hunt-mac.zip", "darwin");
#[cfg(not(target_os = "macos"))]
const UPDATES: (&str, &str, &str) = ("windows.json", "PoopihuntSetup.exe", "windows");

fn version() -> u64 {
    engine::build().0
}

fn name() -> &'static str {
    &engine::build().1
}

fn site() -> String {
    env::var("HUNT_SITE").unwrap_or_else(|_| "https://poopihunt.xyz".into())
}

/// The private test updater's token is kept locally, never in the repository.
/// On Windows: %LOCALAPPDATA%\Hunt\update-token.txt. HUNT_UPDATE_TOKEN can override it.
fn private_update_token() -> Option<String> {
    env::var("HUNT_UPDATE_TOKEN").ok()
        .or_else(|| fs::read_to_string(engine::data_dir().join("update-token.txt")).ok())
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

/// Find the newest unpublished personal test release. The caller provides a read-only
/// GitHub token; the payload includes the asset API URL and a mandatory SHA-256.
fn private_update_info(token: &str, current: u64) -> Option<Value> {
    let body = ureq::get("https://api.github.com/repos/gamblesouls-eng/PM-Hunter/releases?per_page=100")
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .set("User-Agent", "Poopihunt private test updater")
        .timeout(Duration::from_secs(15))
        .call().ok()?.into_string().ok()?;
    let releases: Vec<Value> = serde_json::from_str(&body).ok()?;
    let mut newest: Option<(u64, Value)> = None;
    for release in releases {
        if release.get("draft").and_then(Value::as_bool) != Some(true)
            || release.get("prerelease").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let Some(tag) = release.get("tag_name").and_then(Value::as_str)
            .and_then(|tag| tag.strip_prefix("rdp-private-test-")) else { continue };
        let Some(version) = tag.parse::<u64>().ok() else { continue };
        if version <= current { continue; }

        let Some(asset) = release.get("assets").and_then(Value::as_array)
            .and_then(|assets| assets.iter().find(|asset| {
                asset.get("name").and_then(Value::as_str) == Some("PoopihuntSetup.exe")
            })) else { continue };
        let Some(url) = asset.get("url").and_then(Value::as_str) else { continue };
        let checksum = asset.get("digest").and_then(Value::as_str)
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .map(str::to_string)
            .or_else(|| release.get("body").and_then(Value::as_str).and_then(|body| {
                body.lines().find_map(|line| line.trim().strip_prefix("sha256:")
                    .map(|value| value.trim().to_string()))
            }));
        let Some(sha256) = checksum.filter(|sha| sha.len() == 64
            && sha.chars().all(|c| c.is_ascii_hexdigit())) else { continue };
        let info = json!({
            "version": version,
            "file": "PoopihuntSetup.exe",
            "sha256": sha256,
            "download_url": url
        });
        if newest.as_ref().is_none_or(|(best, _)| version > *best) {
            newest = Some((version, info));
        }
    }
    newest.map(|(_, info)| info)
}

/// The pill's colors: black, or white while it shows a find.
struct Look {
    fill: Color32,
    ink: Color32,
    dim: Color32,
    edge: Color32,
    hover: Color32,
    boxed: Color32,
}

fn look(light: bool) -> Look {
    if light {
        Look { fill: PAPER, ink: INK, dim: gray(0x6a), edge: gray(0xcf), hover: gray(0xe8), boxed: gray(0xef) }
    } else {
        Look { fill: INK, ink: PAPER, dim: gray(0x84), edge: gray(0x2c), hover: gray(0x26), boxed: gray(0x1d) }
    }
}

/// The black look and the white one, k of the way (a find fades it white).
fn look_at(k: f32) -> Look {
    let (a, b) = (look(false), look(true));
    let mix = |x: Color32, y: Color32| {
        let m = |p: u8, q: u8| (p as f32 + (q as f32 - p as f32) * k).round() as u8;
        Color32::from_rgba_premultiplied(m(x.r(), y.r()), m(x.g(), y.g()), m(x.b(), y.b()), m(x.a(), y.a()))
    };
    Look { fill: mix(a.fill, b.fill), ink: mix(a.ink, b.ink), dim: mix(a.dim, b.dim), edge: mix(a.edge, b.edge),
           hover: mix(a.hover, b.hover), boxed: mix(a.boxed, b.boxed) }
}

/// A button under the mouse grows a little; pressed, it gives a little.
fn press(ctx: &egui::Context, resp: &egui::Response) -> f32 {
    let goal = if resp.is_pointer_button_down_on() { 0.9 } else if resp.hovered() { 1.07 } else { 1.0 };
    ctx.animate_value_with_time(resp.id.with("press"), goal, 0.12)
}

fn gray(v: u8) -> Color32 {
    Color32::from_rgb(v, v, v)
}

// ---------------------------------------------------------------- files

struct Paths {
    data: PathBuf,
    config: PathBuf,
    settings: PathBuf,
    log: PathBuf,
    updated: PathBuf, // left by a Hunt installing an update, there till the new one's up
}

impl Paths {
    /// An update's own start: the old Hunt's still there, handing over (or was, a moment ago).
    fn updating(&self) -> bool {
        fs::metadata(&self.updated).and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|e| e < Duration::from_secs(HANDOVER)))
    }

    fn new() -> Paths {
        let data = engine::data_dir();
        Paths {
            config: data.join("config.json"),
            settings: data.join("window.json"),
            log: data.join("hunt.log"),
            updated: data.join("updated"),
            data,
        }
    }
}

fn read_json(path: &Path) -> Option<Map<String, Value>> {
    match serde_json::from_str(&fs::read_to_string(path).ok()?).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn write_json(path: &Path, value: &Map<String, Value>) {
    let tmp = path.with_extension("json.new");
    let text = serde_json::to_string_pretty(value).unwrap_or_default() + "\n";
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn is_webhook(url: &str) -> bool {
    ["https://discord.com/api/webhooks/", "https://discordapp.com/api/webhooks/",
     "https://ptb.discord.com/api/webhooks/", "https://canary.discord.com/api/webhooks/"]
        .iter()
        .filter_map(|p| url.strip_prefix(p))
        .any(|rest| {
            let (id, token) = rest.split_once('/').unwrap_or(("", ""));
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !token.is_empty()
                && !token.chars().any(char::is_whitespace)
        })
}

fn open_path(path: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = std::ffi::OsStr::new(path).encode_wide().chain([0]).collect();
        let verb: Vec<u16> = "open\0".encode_utf16().collect();
        unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(), verb.as_ptr(), wide.as_ptr(), std::ptr::null(), std::ptr::null(), 1);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("open").arg(path).spawn();
    }
}

// ---------------------------------------------------------------- the hunter

enum Msg {
    Line(u64, String),
    Code(u64, Option<String>),
    State(u64, Map<String, Value>),
    Ended(u64),
    Update(Map<String, Value>),
    Progress(f32), // how much of the update's down, 0 to 1
    Downloaded(PathBuf),
    UpdateFailed(String),
}

/// The closing: the pill shrinking into a ball at its right end, the ball going.
struct Closing {
    started: Instant,
}

/// The new Hunt, after an update: it comes on as the old one is, Installing,
/// over it (the old one goes once it's seen the new one's up), then settles.
struct TakeOver {
    frames: u32,
    seen: Option<Instant>, // told the old one it's up
}

/// The opening's window: started once it's over the whole screen.
struct Intro {
    started: Option<Instant>,
    handover: Option<Instant>, // back to the pill's window: since when
    origin: Pos2, // where the window is: the screen's corner, give or take
    frames: u32,
}

/// The hunter that's on.
struct Proc {
    id: u64,
    control: Arc<hunt_core::Control>,
}

/// What the hunter says and does, to the window (from its thread).
struct ToWindow {
    id: u64,
    tx: Mutex<Sender<Msg>>,
    ctx: egui::Context,
}

impl ToWindow {
    fn send(&self, msg: Msg) {
        let _ = self.tx.lock().unwrap().send(msg);
        self.ctx.request_repaint();
    }
}

impl hunt_core::Sink for ToWindow {
    fn line(&self, text: &str) {
        self.send(Msg::Line(self.id, text.to_string()));
    }
    fn code(&self, code: Option<&str>) {
        self.send(Msg::Code(self.id, code.map(String::from)));
    }
    fn state(&self, state: &Value) {
        if let Value::Object(s) = state {
            self.send(Msg::State(self.id, s.clone()));
        }
    }
}

// ---------------------------------------------------------------- the window

struct Hunt {
    paths: Paths,
    settings: Map<String, Value>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    log: Option<File>,

    proc: Option<Proc>,
    next_id: u64,
    mode: String,
    state: Map<String, Value>,
    code: String,
    last_line: String,
    last_exit: Option<&'static str>,
    restart_to: Option<(String, bool)>, // the mode, paused
    test: bool,
    reports: bool,
    fake: bool, // --smoke: a hunter that isn't there

    hook: String,
    hook_edit: String,
    hook_open: bool,
    hook_note: Option<(&'static str, Instant)>,

    update: Option<Map<String, Value>>,
    note: String,
    installer: Option<PathBuf>,
    install_when_stopped: bool,
    downloading: bool, // an update's on its way down
    progress: Option<f32>, // the update, as the pill shows it (all of it a progress bar): 1 is installing
    was_hunting: bool, // the last state's: a stop is when it goes, and a look for a newer build

    pos: Option<Pos2>,
    pos_saved: Option<Pos2>,
    pos_changed: Option<Instant>,
    was_found: String,
    born: Option<Instant>, // the pill's own opening (stretching out from play): its first frame
    intro: Option<Intro>,   // the big Clodsire, while it's on
    closing: Option<Closing>, // ✕: the opening the other way
    handing_over: Option<Instant>, // an update's installer started: this one goes once the new one's up
    taking_over: Option<TakeOver>, // the new one, the other side of that
    overlay_on: bool, // the update's bar over the pill, last frame
    bar_done: f32, // how far along it the bar was
    settling: Option<(Instant, f32, f32)>, // the bar going back into play: since, how far it was, how much was up
    carried: Option<Value>, // the hunt as it was, stopped for an update: the new Hunt carries on with it
    count_from: u32, // the next hunter's first count (an update's, carried on)
    carrying: bool, // the next hunter hunts on from before an update
    checked_at: u32, // encounters at the last look for an update
    closed: bool,           // the closing's done: the window may go
    lifted: Option<Instant>, // being dragged, since
    moved_at: Option<Instant>, // its place last changed
    smoke: Option<Smoke>,
    start_now: bool, // the hunter starts with Hunt, paused (--start: hunting)
}

impl Hunt {
    fn new(cc: &eframe::CreationContext) -> Hunt {
        let paths = Paths::new();
        if !paths.config.exists() {
            // no rules: every reskin with a gradient is caught all the same
            let cfg = json!({"webhook": "", "ping": "@here", "targets": [], "notify": []});
            write_json(&paths.config, cfg.as_object().unwrap());
        }
        if fs::metadata(&paths.log).map(|m| m.len() > 5 << 20).unwrap_or(false) {
            let _ = fs::rename(&paths.log, paths.data.join("hunt.old.log"));
        }
        let log = OpenOptions::new().create(true).append(true).open(&paths.log).ok();
        let settings = read_json(&paths.settings).unwrap_or_default();
        let pos = settings.get("pos").and_then(Value::as_array).and_then(|a| {
            Some(pos2(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32))
        });
        hide_from_screenshots(cc);
        no_mac_shadow(cc);
        setup_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx); // the icons are SVG
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::TRANSPARENT;
        visuals.window_fill = Color32::TRANSPARENT;
        visuals.selection.bg_fill = gray(0x55);
        visuals.text_cursor.stroke = Stroke::new(1.5_f32, PAPER);
        cc.egui_ctx.set_visuals(visuals);

        let (tx, rx) = channel();
        // an update's own start: no opening, the pill just there as the old one is
        let updated = paths.updating();
        if !updated {
            let _ = fs::remove_file(&paths.updated);
        }
        let carried = updated.then(|| fs::read_to_string(&paths.updated).ok())
            .flatten().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        remove_old_exe();
        let mut hunt = Hunt {
            reports: settings.get("reports").and_then(Value::as_bool).unwrap_or(true),
            settings,
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            log,
            proc: None,
            next_id: 0,
            mode: String::new(),
            state: Map::new(),
            code: String::new(),
            last_line: String::new(),
            last_exit: None,
            restart_to: None,
            test: false,
            fake: false,
            hook: String::new(),
            hook_edit: String::new(),
            hook_open: false,
            hook_note: None,
            update: None,
            note: String::new(),
            installer: None,
            install_when_stopped: false,
            downloading: false,
            progress: updated.then_some(1.0),
            was_hunting: false,
            pos,
            pos_saved: pos,
            pos_changed: None,
            was_found: String::new(),
            born: updated.then(|| Instant::now().checked_sub(Duration::from_secs(5))).flatten(),
            closing: None,
            handing_over: None,
            taking_over: updated.then_some(TakeOver { frames: 0, seen: None }),
            overlay_on: false,
            bar_done: 0.0,
            settling: None,
            carried,
            count_from: 0,
            carrying: false,
            checked_at: 0,
            closed: false,
            lifted: None,
            moved_at: None,
            // (only where it's see-through: on Direct3D the window over the screen would be black)
            intro: ((!env::args().any(|a| a == "--smoke") || env::args().any(|a| a == "--intro"))
                    && !updated && env::var_os("HUNT_NO_INTRO").is_none() && env::var_os("HUNT_RENDERER").is_none())
                .then_some(Intro { started: None, handover: None, origin: Pos2::ZERO, frames: 0 }),
            smoke: env::args().any(|a| a == "--smoke").then(Smoke::new),
            start_now: !env::args().any(|a| a == "--smoke"),
            paths,
        };
        hunt.hook = hunt.hook_saved();
        hunt.check_update();
        if let (Some(smoke), true) = (hunt.smoke.as_mut(), hunt.intro.is_some()) {
            smoke.after = INTRO_END + 0.8;
        }
        hunt
    }

    fn running(&self) -> bool {
        self.proc.is_some() || self.fake
    }

    fn say(&mut self, line: &str) {
        if let Some(f) = self.log.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }

    // ------------------------------------------------------------ the hunter

    /// The hunter: paused (when Hunt opens, so the website's code is there at
    /// once) or hunting (play, after it ended on its own).
    fn start(&mut self, mode: &str, paused: bool) {
        if self.running() {
            return;
        }
        self.next_id += 1;
        let id = self.next_id;
        let sink = Arc::new(ToWindow { id, tx: Mutex::new(self.tx.clone()), ctx: self.ctx.clone() });
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let launch = engine::Launch {
            mode, paused, window: true, practice: 0, calibrate: false, reports: self.reports, config: &self.paths.config,
            count: std::mem::take(&mut self.count_from),
            carried: std::mem::take(&mut self.carrying),
        };
        let control = match engine::launch(launch, sink, move || {
            let _ = tx.send(Msg::Ended(id));
            ctx.request_repaint();
        }) {
            Ok(c) => c,
            Err(e) => {
                self.say(&format!("couldn't start the hunter: {e}"));
                self.last_exit = Some(if e.contains("config.json") { "Check config.json" } else { "No hunter" });
                return;
            }
        };
        if self.test {
            control.set_test(true); // Debug mode, from the start
        }
        self.proc = Some(Proc { id, control });
        self.mode = mode.into();
        self.state.clear();
        self.code.clear();
        self.last_exit = None;
        let stamp = chrono_now();
        self.say(&format!("\n--- {stamp} {mode} (build {}) ---", name()));
    }

    fn send(&mut self, cmd: &str) {
        if let Some(p) = &self.proc {
            match cmd {
                "toggle" => p.control.toggle(),
                "test on" => p.control.set_test(true),
                "test off" => p.control.set_test(false),
                _ => {}
            }
        }
    }

    /// It ends at its next look (a battle's step at most).
    fn stop(&mut self) {
        self.fake = false;
        if let Some(proc) = self.proc.as_ref() {
            proc.control.stop();
        }
    }

    /// New settings: a hunt that's on starts again with them, once the old one is gone.
    fn restart(&mut self) {
        if self.proc.is_some() && (self.mode == "hunt" || self.mode == "debug" || self.mode == "watch") {
            self.restart_to = Some((self.mode.clone(), !self.hunting()));
            self.stop();
        }
    }

    /// Hunting, not paused (by you, a find or Ctrl+Shift+H).
    fn hunting(&self) -> bool {
        let flag = |k: &str| self.state.get(k).and_then(Value::as_bool).unwrap_or(false);
        self.running() && (flag("running") || flag("remote"))
    }

    fn got_line(&mut self, line: String) {
        if !line.trim().is_empty() {
            self.last_line = line.trim().to_string();
            self.say(&line);
        }
    }

    fn got_state(&mut self, s: Map<String, Value>) {
        let flag = |k: &str| s.get(k).and_then(Value::as_bool).unwrap_or(false);
        if let Some(test) = s.get("test").and_then(Value::as_bool) {
            self.test = test; // off once its encounter is caught
        }
        let enc = s.get("enc").and_then(Value::as_str).unwrap_or("").to_string();
        let found = s.get("phase").and_then(Value::as_str) == Some("battle") && flag("wanted");
        if found && self.was_found != enc {
            // it's holding the battle: catch it in Roblox, then play to carry on
            self.say(&format!("Found: {enc}. Catch it in Roblox, then press play."));
            self.ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
            #[cfg(windows)]
            unsafe {
                windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(0);
            }
        }
        self.was_found = if found { enc } else { String::new() };
        let hunting = flag("running") || flag("remote");
        self.state = s;
        if std::mem::replace(&mut self.was_hunting, hunting) && !hunting {
            self.check_update(); // a hunt's stopped (you, a find, Ctrl+Shift+H): a newer build goes on now
        }
        // a look for a newer build every 5 encounters (mac.json / windows.json: a few bytes)
        let count = self.state.get("count").and_then(Value::as_u64).unwrap_or(0) as u32;
        if count >= self.checked_at + 5 {
            self.checked_at = count;
            self.check_update();
        }
        if self.install_when_stopped && hunting {
            self.carry(); // (as it is when it stops at its quiet moment)
        }
        self.install_when_quiet();
    }

    /// A downloaded update with a hunter on: paused (not with a find held on screen for
    /// you: Hunt stays, keeping Roblox awake), it stops now; hunting, it stops at its next
    /// quiet moment. ended() installs once it's gone, and the new Hunt carries on.
    fn install_when_quiet(&mut self) {
        if !self.install_when_stopped || self.installer.is_none() {
            return;
        }
        if self.hunting() {
            if let Some(p) = &self.proc {
                p.control.stop_between();
            }
        } else if self.was_found.is_empty() {
            self.carry();
            self.progress = Some(1.0);
            self.stop();
        }
    }

    fn ended(&mut self) {
        self.last_exit = if self.mode == "diagnose" {
            Some("Done") // what it found is in hunt.log
        } else if !self.last_line.is_empty() && !self.last_line.contains("stopped") {
            Some("Stopped") // why is in hunt.log
        } else {
            None
        };
        self.proc = None;
        self.state.clear();
        self.code.clear();
        self.test = false;
        self.was_found.clear();
        if self.install_when_stopped {
            self.progress = Some(1.0);
            self.install();
        } else if let Some((mode, paused)) = self.restart_to.take() {
            self.start(&mode, paused);
        }
    }

    fn found_now(&self) -> bool {
        let s = &self.state;
        self.running() && s.get("phase").and_then(Value::as_str) == Some("battle")
            && s.get("wanted").and_then(Value::as_bool).unwrap_or(false)
            && !s.get("remote").and_then(Value::as_bool).unwrap_or(false)
    }

    /// Play: not hunting, or paused (by a find, or Ctrl+Shift+H). Otherwise stop.
    fn shows_play(&self) -> bool {
        !self.hunting() && self.mode != "diagnose"
    }

    fn hunt_mode() -> &'static str {
        if env::var_os("HUNT_WATCH").is_some() { "watch" } else { "hunt" }
    }

    /// Play carries on, stop pauses: the hunter stays (and so does the website's
    /// code) until an update or quit. A hunter that ended on its own starts again.
    fn main_button(&mut self) {
        if env::var_os("HUNT_PREVIEW").is_some() {
            return; // a look at the window: its hunter stays paused
        }
        if !self.running() {
            self.start(Self::hunt_mode(), false);
        } else {
            self.send("toggle");
        }
    }

    fn code_text(&self) -> String {
        let web = self.running() && matches!(self.mode.as_str(), "hunt" | "debug" | "calibrate");
        if !self.note.is_empty() {
            self.note.clone()
        } else if web && !self.code.is_empty() {
            self.code.clone()
        } else if self.running() {
            String::new()
        } else {
            self.last_exit.unwrap_or("").into()
        }
    }

    // ------------------------------------------------------------ settings in config.json

    fn hook_saved(&self) -> String {
        read_json(&self.paths.config)
            .and_then(|c| c.get("webhook").and_then(Value::as_str).map(String::from))
            .unwrap_or_default()
    }

    fn hook_done(&mut self) {
        self.hook_open = false;
        let url = self.hook_edit.trim().to_string();
        if url == self.hook {
            return;
        }
        if !url.is_empty() && !is_webhook(&url) {
            self.hook_note = Some(("Not a webhook", Instant::now()));
            return;
        }
        let Some(mut cfg) = read_json(&self.paths.config) else {
            self.hook_note = Some(("config.json has a mistake", Instant::now()));
            return;
        };
        cfg.insert("webhook".into(), Value::String(url.clone()));
        write_json(&self.paths.config, &cfg);
        self.hook = url;
        self.restart();
    }

    /// Debug mode (a right click on play): it hunts at once, fits the layout
    /// again at the next encounter (drawn for the developer), catches that one
    /// with a screenshot every turn and saves, then turns itself off: it hunts
    /// on if every step went right, pauses if one didn't. A right click while
    /// it's on calls it off.
    fn debug_run(&mut self) {
        if env::var_os("HUNT_PREVIEW").is_some() {
            self.test = !self.test; // a look at the window: the ring, but its hunter stays paused
            return;
        }
        if self.test {
            self.test = false;
            self.send("test off");
            return;
        }
        self.test = true;
        if !self.running() {
            self.start(Self::hunt_mode(), false);
            return;
        }
        self.send("test on");
        if !self.hunting() {
            self.send("toggle");
        }
    }

    // ------------------------------------------------------------ updates

    /// When Hunt opens and whenever a hunt stops: a newer build on the website?
    /// It's downloaded, installed, and Hunt starts again (the installer starts it).
    fn check_update(&self) {
        if version() == 0 {
            return; // a development build
        }
        // RDP TEST builds only look at unpublished draft releases through GitHub's API.
        // Never fall back to poopihunt.xyz, where a public update could overwrite this build.
        let private = name() == "RDP TEST";
        if private && private_update_token().is_none() {
            return; // private channel has not been configured on this PC yet
        }
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let info = if private {
                private_update_token().and_then(|token| private_update_info(&token, version()))
            } else {
                ureq::get(&format!("{}/{}", site(), UPDATES.0))
                    .set("User-Agent", &format!("Hunt ({}, {})", UPDATES.2, name()))
                    .timeout(Duration::from_secs(15))
                    .call()
                    .ok()
                    .and_then(|r| r.into_string().ok())
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            };
            if let Some(Value::Object(info)) = info {
                if info.get("version").and_then(Value::as_u64).unwrap_or(0) > version() {
                    let _ = tx.send(Msg::Update(info));
                    ctx.request_repaint();
                }
            }
        });
    }

    /// A newer build, seen when Hunt opens or a hunt stops: downloaded and installed
    /// at once (the hunter's always there, paused from when Hunt opens, so not waiting
    /// for it to be gone). If a hunt's started meanwhile, it's installed when that stops,
    /// and till then it's quiet about it (the website code stays in the pill).
    fn download_update(&mut self) {
        let Some(info) = self.update.clone() else { return };
        if self.downloading || self.installer.is_some() {
            return; // on its way
        }
        self.downloading = true;
        if !self.hunting() {
            self.progress = Some(0.0);
        }
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let result = (|| -> Result<PathBuf, String> {
                let file = info.get("file").and_then(Value::as_str).unwrap_or(UPDATES.1);
                let number = info.get("version").and_then(Value::as_u64).unwrap_or(0);
                let private_url = info.get("download_url").and_then(Value::as_str).map(str::to_string);
                let url = private_url.clone().unwrap_or_else(|| format!("{}/{file}", site()));
                let path = env::temp_dir().join(format!("{number}-{file}"));
                let part = path.with_extension("part");
                let mut request = ureq::get(&url)
                    .set("User-Agent", &format!("Hunt ({}, {})", UPDATES.2, name()))
                    .timeout(Duration::from_secs(300));
                if private_url.is_some() {
                    let token = private_update_token()
                        .ok_or_else(|| "private update token is missing".to_string())?;
                    let authorization = format!("Bearer {token}");
                    request = request.set("Authorization", &authorization)
                        .set("Accept", "application/octet-stream");
                }
                let resp = request.call().map_err(|e| e.to_string())?;
                let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
                let mut body = resp.into_reader();
                let mut out = File::create(&part).map_err(|e| e.to_string())?;
                let (mut digest, mut got, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 16]);
                loop {
                    let n = body.read(&mut buf).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                    digest.update(&buf[..n]);
                    got += n as u64;
                    if total > 0 {
                        let _ = tx.send(Msg::Progress(got as f32 / total as f32));
                        ctx.request_repaint();
                    }
                }
                drop(out);
                let hex: String = digest.finalize().iter().map(|b| format!("{b:02x}")).collect();
                if info.get("sha256").and_then(Value::as_str).is_some_and(|want| want != hex) {
                    return Err("the download came out different from the website's".into());
                }
                fs::rename(&part, &path).map_err(|e| e.to_string())?;
                Ok(path)
            })();
            let _ = tx.send(match result {
                Ok(path) => Msg::Downloaded(path),
                Err(e) => Msg::UpdateFailed(e),
            });
            ctx.request_repaint();
        });
    }

    /// The installer replaces Hunt's folder and starts the new Hunt, which comes on over
    /// this one as it is (Installing); this one goes once it's there, so it never looks closed.
    fn install(&mut self) {
        self.hand_over();
        self.run_installer();
    }

    fn hand_over(&mut self) {
        let was = self.carried.clone().unwrap_or(Value::Null);
        let _ = fs::write(&self.paths.updated, was.to_string());
        self.handing_over = Some(Instant::now());
    }

    /// The hunt as it is, for the new Hunt to carry on with (taken just before it stops).
    fn carry(&mut self) {
        let count = self.state.get("count").and_then(Value::as_u64).unwrap_or(0);
        self.carried = Some(json!({"mode": self.mode, "hunting": self.hunting(), "count": count, "test": self.test}));
    }

    /// The old Hunt: gone once the new one's up (it takes the file away). One that never
    /// comes (the update didn't install): this one stays.
    fn handing_over(&mut self, ctx: &egui::Context) {
        let Some(since) = self.handing_over else { return };
        if !self.paths.updated.exists() {
            self.closed = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        } else if since.elapsed() > Duration::from_secs(HANDOVER) {
            let _ = fs::remove_file(&self.paths.updated);
            #[cfg(windows)]
            if let Ok(me) = env::current_exe() {
                if !me.with_file_name("Hunt.exe").exists() {
                    let _ = fs::rename(me.with_file_name("Hunt.old.exe"), me.with_file_name("Hunt.exe"));
                }
            }
            self.say("the update didn't install: the new Hunt never came");
            self.handing_over = None;
            self.installer = None;
            self.install_when_stopped = false;
            self.progress = None;
            self.start_now = true; // the hunt as it was (carried)
        } else {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// The new Hunt: up (a few frames drawn), it tells the old one, which goes; then
    /// Installing gives way to the pill.
    fn taking_over(&mut self, ctx: &egui::Context) {
        let Some(take) = self.taking_over.as_mut() else { return };
        take.frames += 1;
        if take.frames == 4 {
            let _ = fs::remove_file(&self.paths.updated);
            take.seen = Some(Instant::now());
        }
        if take.seen.is_some_and(|t| t.elapsed() > Duration::from_millis(500)) {
            self.taking_over = None;
            self.progress = None;
            remove_old_exe();
        }
        ctx.request_repaint();
    }

    fn run_installer(&mut self) {
        #[cfg(windows)]
        if let Some(setup) = &self.installer {
            use std::os::windows::process::CommandExt;
            // a running Hunt.exe can't be written over, but it can be moved: out of the installer's way
            if let Ok(me) = env::current_exe() {
                let _ = fs::remove_file(me.with_file_name("Hunt.old.exe"));
                let _ = fs::rename(&me, me.with_file_name("Hunt.old.exe"));
            }
            let _ = Command::new(setup)
                .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
                .creation_flags(0x0000_0008) // DETACHED_PROCESS
                .spawn();
        }
        // a Mac: the new Hunt.app (a zip) put in this one's place (this one runs on from what it's
        // read) and opened, as a Hunt of its own
        #[cfg(target_os = "macos")]
        if let (Some(zip), Some(app)) = (&self.installer, engine::app_bundle()) {
            let script = format!(
                "d=$(mktemp -d) && ditto -x -k '{zip}' \"$d\" && [ -d \"$d/Hunt.app\" ] && rm -rf '{app}' && \
                 mv \"$d/Hunt.app\" '{app}' && open -n '{app}'; rm -rf \"$d\" '{zip}'",
                zip = zip.display(), app = app.display());
            let _ = Command::new("/bin/sh").args(["-c", &script]).stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        }
    }

    // ------------------------------------------------------------ the window's place and size

    fn monitor(&self, ctx: &egui::Context) -> Vec2 {
        ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1920.0, 1080.0))
    }

    /// Where the pill is: followed as it's dragged, kept in window.json.
    fn track_place(&mut self, ctx: &egui::Context) {
        let Some(outer) = ctx.input(|i| i.viewport().outer_rect) else { return };
        let screen = self.monitor(ctx);
        if self.pos.is_some_and(|p| p.x < -W / 2.0 || p.x > screen.x - W / 2.0 || p.y < 0.0 || p.y > screen.y - H) {
            self.pos = None; // the screen it was on is gone
        }
        if self.pos.is_none() {
            // first time on this PC: the top middle
            let at = pos2(((screen.x - W) / 2.0).round(), 60.0);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(at - MARGIN));
            self.pos = Some(at);
            return;
        }
        let at = outer.min + MARGIN;
        if (at - self.pos.unwrap()).length() > 0.5 {
            self.pos = Some(at);
            self.pos_changed = Some(Instant::now());
            self.moved_at = self.pos_changed;
        }
        if self.pos_changed.is_some_and(|t| t.elapsed() > Duration::from_millis(800)) && self.pos != self.pos_saved {
            let p = self.pos.unwrap();
            self.settings.insert("pos".into(), json!([p.x.round(), p.y.round()]));
            write_json(&self.paths.settings, &self.settings);
            self.pos_saved = self.pos;
            self.pos_changed = None;
        }
    }

    /// The opening's window: the whole screen (see-through but for the big
    /// Clodsire), then back over the pill once it's got there.
    fn intro_window(&mut self, ctx: &egui::Context) {
        let screen = self.monitor(ctx);
        let outer = ctx.input(|i| i.viewport().outer_rect);
        let Some(intro) = self.intro.as_mut() else { return };
        intro.frames += 1;
        if intro.frames == 1 {
            let pos = self.pos.unwrap_or(pos2(((screen.x - W) / 2.0).round(), 60.0));
            if pos.x < 0.0 || pos.y < 0.0 || pos.x + W > screen.x || pos.y + H > screen.y {
                self.intro = None; // a pill on another screen: no opening
                return;
            }
            self.pos = Some(pos);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(Pos2::ZERO));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(screen));
            return;
        }
        if intro.started.is_none() {
            // it starts once the window's over the screen (or it's been a while)
            if outer.is_some_and(|o| o.width() >= screen.x - 1.0) || intro.frames > 30 {
                intro.started = Some(Instant::now());
                intro.origin = outer.map(|o| o.min).unwrap_or(Pos2::ZERO);
            }
            ctx.request_repaint();
            return;
        }
        if intro.started.is_some_and(|t| t.elapsed().as_secs_f32() >= INTRO_END) && intro.handover.is_none() {
            // back over the pill; nothing's drawn till the window's there (or it was
            // drawn in the screen's corner for a frame)
            intro.handover = Some(Instant::now());
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(WINDOW));
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(self.pos.unwrap_or(Pos2::ZERO) - MARGIN));
        }
        if let Some(since) = intro.handover {
            let there = outer.is_some_and(|o| {
                (o.min + MARGIN - self.pos.unwrap_or(Pos2::ZERO)).length() < 1.5 && (o.width() - WINDOW.x).abs() < 1.5
            });
            if there || since.elapsed() > Duration::from_millis(400) {
                self.intro = None;
                self.pos_saved = self.pos; // (not a move of yours)
                self.born = None; // the pill stretches out from here
            }
            ctx.request_repaint();
        }
    }

    /// The opening, in the window over the screen: the screen dims, Clodsire's
    /// circle springs up in the middle with a ripple of light, breathes, then
    /// sweeps along an arc down into the pill's end as the screen comes back,
    /// turning into play as it lands.
    fn draw_intro(&self, ui: &mut egui::Ui, t: f32, origin: Pos2) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint();
        // where the window is now (a Mac may still be moving it): the screen in its terms
        let origin = ctx.input(|i| i.viewport().outer_rect).map(|o| o.min).unwrap_or(origin);
        let painter = ui.painter().clone();
        let screen = self.monitor(&ctx);
        let clamp = |x: f32| x.clamp(0.0, 1.0);
        let out = |x: f32| 1.0 - (1.0 - clamp(x)).powi(3);
        let in_out = |x: f32| {
            let x = clamp(x);
            if x < 0.5 { 4.0 * x * x * x } else { 1.0 - (-2.0 * x + 2.0).powi(3) / 2.0 }
        };
        let spring = |x: f32| {
            // past it a little, and back (ease out back)
            let (x, c) = (clamp(x) - 1.0, 1.70158);
            1.0 + (c + 1.0) * x * x * x + c * x * x
        };
        let travel = in_out((t - INTRO_MOVE) / (INTRO_LAND - INTRO_MOVE));
        let landed = clamp((t - INTRO_LAND) / (INTRO_END - INTRO_LAND));

        // the veil: the screen dims while Clodsire's there, and comes back slowly
        // (soft at both ends) as it flies down, gone by the time it lands
        let soft = |x: f32| {
            let x = clamp(x);
            x * x * x * (x * (6.0 * x - 15.0) + 10.0)
        };
        let lifting = (t - (INTRO_MOVE - 0.3)) / (INTRO_LAND - INTRO_MOVE + 0.3);
        let veil = soft(t / 0.6) * (1.0 - soft(lifting));
        let whole = Rect::from_min_size(Pos2::ZERO - origin.to_vec2(), screen);
        painter.rect_filled(whole, 0.0, Color32::from_black_alpha((90.0 * veil).round() as u8));

        // the path: from the middle, along an arc, to the pill's end
        let start = pos2(screen.x / 2.0, screen.y / 2.0) - origin.to_vec2();
        let end = self.pos.unwrap_or(Pos2::ZERO) - origin.to_vec2() + vec2(H / 2.0, H / 2.0);
        let bend = start + (end - start) * 0.5 + vec2(0.0, -0.25 * (end - start).length().max(120.0));
        let center = start + (bend - start) * (2.0 * (1.0 - travel) * travel)
            + (end - start) * (travel * travel);
        let breath = 1.0 + 0.025 * (((t - 0.7) * 3.2).sin()) * clamp((t - 0.7) / 0.3) * (1.0 - travel);
        let arrive = spring((t - 0.1) / 0.75);
        let d = (BIG * (0.6 + 0.4 * arrive) * breath) * (1.0 - travel) + H * travel;
        let fade = out((t - 0.05) / 0.35);

        // its ripple of light, once it's up
        let ring = clamp((t - 0.45) / 0.8);
        if ring > 0.0 && ring < 1.0 {
            let r = BIG / 2.0 * (1.0 + 0.9 * out(ring));
            painter.circle_stroke(start, r, Stroke::new(2.0 * (1.0 - ring) + 0.5, PAPER.gamma_multiply(0.45 * (1.0 - ring))));
        }
        // its shadow falls away as it flies down: none left when it lands (the pill has none,
        // and one going all at once reads as the screen brightening); Clodsire gives way to play
        let morph = in_out((travel - 0.6) / 0.4);
        self.draw_flying(ui, &painter, center, d, fade, morph, 1.0 - in_out(travel / 0.85));
        // and a small ripple where it touches down
        if landed > 0.0 && landed < 1.0 {
            painter.circle_stroke(end, H / 2.0 + 14.0 * out(landed),
                                  Stroke::new(1.5_f32, PAPER.gamma_multiply(0.5 * (1.0 - landed))));
        }
    }

    /// The flying circle: Clodsire on it, or play (morph 1), and its shadow (lift 1: high).
    #[allow(clippy::too_many_arguments)]
    fn draw_flying(&self, ui: &mut egui::Ui, painter: &egui::Painter, center: Pos2, d: f32, fade: f32, morph: f32,
                   lift: f32) {
        let rect = Rect::from_center_size(center, Vec2::splat(d));
        let shadow = egui::Shadow { offset: [0, (14.0 * lift) as i8], blur: (16.0 + 40.0 * lift) as u8, spread: 0,
                                    color: Color32::from_black_alpha((150.0 * fade * lift).round() as u8) };
        painter.add(shadow.as_shape(rect, d / 2.0));
        painter.circle_filled(center, d / 2.0, gray(0x3a).gamma_multiply(fade)); // its hairline
        painter.circle_filled(center, d / 2.0 - 1.0, INK.gamma_multiply(fade));
        if morph < 1.0 {
            egui::Image::new(egui::include_image!("../../windows/hunt-icon.png"))
                .tint(Color32::WHITE.gamma_multiply(fade * (1.0 - morph)))
                .paint_at(ui, rect);
        }
        if morph > 0.0 {
            let r = d / 2.0 * (CONTROL / H);
            painter.circle_filled(center, r, PAPER.gamma_multiply(morph * fade));
            let s = r / (CONTROL / 2.0);
            let pts = [vec2(-3.0, -5.0), vec2(-3.0, 5.0), vec2(5.5, 0.0)].map(|v| center + v * s).to_vec();
            painter.add(egui::Shape::convex_polygon(pts, INK.gamma_multiply(morph * fade), Stroke::NONE));
        }
    }

    /// While the window goes back over the pill: the landed circle, play in it,
    /// kept where it is on the screen wherever the window is meanwhile.
    fn draw_landed(&self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(origin) = ctx.input(|i| i.viewport().outer_rect).map(|o| o.min) else { return };
        let center = self.pos.unwrap_or(Pos2::ZERO) - origin.to_vec2() + vec2(H / 2.0, H / 2.0);
        let painter = ui.painter();
        painter.circle_filled(center, H / 2.0, gray(0x3a));
        painter.circle_filled(center, H / 2.0 - 1.0, INK);
        painter.circle_filled(center, CONTROL / 2.0, PAPER);
        let pts = [vec2(-3.0, -5.0), vec2(-3.0, 5.0), vec2(5.5, 0.0)].map(|v| center + v).to_vec();
        painter.add(egui::Shape::convex_polygon(pts, INK, Stroke::NONE));
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let r = H / 2.0;
        let ease = |x: f32| 1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3);
        let ease_in = |x: f32| x.clamp(0.0, 1.0).powi(3);

        // lifted while it's dragged: a touch bigger, a shadow under it
        if self.lifted.is_some_and(|since| {
            let still = self.moved_at.is_none_or(|m| m.elapsed() > Duration::from_millis(250));
            !ctx.input(|i| i.pointer.primary_down()) || (since.elapsed() > Duration::from_millis(400) && still)
        }) {
            self.lifted = None;
        }
        let lift = ease(ctx.animate_bool_with_time(Id::new("lift"), self.lifted.is_some(), 0.18));
        let s = 1.0 + 0.03 * lift;
        let middle = vec2(W / 2.0, H / 2.0);
        ctx.set_transform_layer(ui.layer_id(),
                                egui::emath::TSTransform::new(MARGIN + middle * (1.0 - s), s)); // M in, and the lift

        // a find: the pill fades to white
        let white = ctx.animate_bool_with_time(Id::new("found"), self.found_now(), 0.45);
        let c = look_at(white);
        let light = white > 0.5;

        // opening (after the big Clodsire): it stretches out from play, the rest fades in;
        // closing: the other way, folding back into play
        let t = self.born.get_or_insert_with(Instant::now).elapsed().as_secs_f32();
        let spring = |x: f32| {
            let (x, c) = (x.clamp(0.0, 1.0) - 1.0, 1.2);
            1.0 + (c + 1.0) * x * x * x + c * x * x
        };
        let (grow, mut reveal) = (spring(t / 0.55).min(1.0), ease((t - 0.3) / 0.45));
        let closing = self.closing.as_ref().map(|c| c.started.elapsed().as_secs_f32());
        let shut = closing.map_or(0.0, |c| ease_in(c / FOLD)); // its left end sliding over to the right one
        if let Some(since) = closing {
            reveal = reveal.min(1.0 - ease(since / 0.15));
        }
        if reveal < 1.0 || lift > 0.0 {
            ctx.request_repaint();
        }
        // (out into the margin too: the lift's shadow)
        let mut painter = ui.painter().clone();
        painter.set_clip_rect(Rect::from_min_size(Pos2::ZERO - MARGIN, WINDOW));

        let shape = Rect::from_min_max(pos2((W - H) * shut, 0.0), pos2(H + (W - H) * grow, H));
        if let Some(since) = closing.filter(|since| *since >= FOLD) {
            // just the ball at the right end now, going
            let gone = ease_in((since - FOLD) / VANISH);
            let (ball, radius) = (pos2(W - r, r), r * (1.0 - gone));
            painter.circle_filled(ball, radius, c.edge.gamma_multiply(1.0 - gone));
            painter.circle_filled(ball, (radius - 1.0).max(0.0), c.fill.gamma_multiply(1.0 - gone));
            if self.progress.is_some() { // (the bar's white end, as after "Installing")
                painter.circle_filled(ball, (radius - INSET).max(0.0), c.ink.gamma_multiply(1.0 - gone));
            }
            return;
        }
        if lift > 0.0 {
            let shadow = egui::Shadow { offset: [0, (6.0 * lift) as i8], blur: 14, spread: 0,
                                        color: Color32::from_black_alpha((90.0 * lift) as u8) };
            painter.add(shadow.as_shape(shape, r));
        }
        // the outline; dragged by anything that isn't a button
        painter.rect_filled(shape, r, c.edge); // a hairline around it, for dark games
        painter.rect_filled(shape.shrink(1.0), r - 1.0, c.fill);
        let bg = ui.interact(shape, Id::new("bg"), Sense::click_and_drag());
        if bg.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            self.lifted = Some(Instant::now());
        }
        if bg.clicked() {
            ctx.memory_mut(|m| m.surrender_focus(Id::new("hook")));
        }
        if env::var_os("HUNT_PREVIEW_UPDATE").is_some() {
            // a look at the update: the pill as it is for 3 s, down in 4 s, installing, a new Hunt
            if self.handing_over.is_none() {
                self.progress = (t > 3.0).then(|| ((t - 3.5) / 4.0).clamp(0.0, 1.0));
            }
            ctx.request_repaint(); // (a real one's redrawn as it comes down)
            if t > 9.5 && self.handing_over.is_none() {
                self.carry();
                self.hand_over();
                let me = env::current_exe().unwrap_or_default();
                let _ = Command::new(me).args(env::args().skip(1)).env_remove("HUNT_PREVIEW_UPDATE").spawn();
            }
        }

        // an update on its way: the pill goes darker as what's in it fades, and play's white
        // circle stretches rightwards along it as the update comes down, "Installing" once
        // it's all the way; then Hunt closes, and the installer opens the new one. The new
        // one (or a download that didn't come) the other way: the words go, the bar slides
        // back into play's circle, and the pill comes up through the black, play's triangle with it
        let on = (self.progress.is_some() && !self.hunting()) || self.taking_over.is_some();
        let updating = ctx.animate_bool_with_time(Id::new("updating"), on, 0.7);
        let done_now = ctx.animate_value_with_time(Id::new("progress"), self.progress.unwrap_or(1.0), 0.3);
        if std::mem::replace(&mut self.overlay_on, on) && !on {
            self.settling = Some((Instant::now(), self.bar_done, updating));
        }
        if on {
            self.settling = None;
            self.bar_done = done_now;
        }
        let settle = self.settling.map(|(t0, d0, u0)| (t0.elapsed().as_secs_f32(), d0, u0)).filter(|s| s.0 < SETTLE);
        if settle.is_none() {
            self.settling = None;
        }
        if (on && updating > 0.0) || settle.is_some() {
            let in_out = |x: f32| {
                let x = x.clamp(0.0, 1.0);
                if x < 0.5 { 4.0 * x * x * x } else { 1.0 - (-2.0 * x + 2.0).powi(3) / 2.0 }
            };
            let (dark, words, done, said) = match settle {
                Some((t, d0, u0)) => {
                    let up = 1.0 - ease((t - 0.55) / 0.4); // the black (and the bar, play's circle by then) lifting
                    (ease(u0 / 0.55) * up, ease((u0 - 0.4) / 0.6) * (1.0 - ease(t / 0.15)),
                     d0 * (1.0 - in_out((t - 0.08) / 0.45)), d0)
                }
                None => (ease(updating / 0.55), ease((updating - 0.4) / 0.6), done_now, done_now),
            };
            ui.disable(); // (what's under it can't be clicked while it comes)
            let layer = egui::LayerId::new(egui::Order::Foreground, Id::new("update"));
            ctx.set_transform_layer(layer, egui::emath::TSTransform::new(MARGIN + middle * (1.0 - s), s));
            let p = ctx.layer_painter(layer).with_clip_rect(shape.shrink(1.0));
            p.rect_filled(shape.shrink(1.0), r - 1.0, Color32::BLACK.gamma_multiply(dark));
            // play's circle, where it is, stretched by how much is down
            let track = Rect::from_min_max(pos2(shape.min.x + INSET, H / 2.0 - CONTROL / 2.0),
                                           pos2(shape.max.x - INSET, H / 2.0 + CONTROL / 2.0));
            let bar = track.with_max_x(track.min.x + CONTROL + (track.width() - CONTROL).max(0.0) * done);
            p.rect_filled(track, CONTROL / 2.0, c.boxed.gamma_multiply(0.5 * words)); // the way it goes
            p.rect_filled(bar, CONTROL / 2.0, c.ink.gamma_multiply(dark));
            let label = if said < 0.995 { format!("Updating  {:.0}%", said * 100.0) } else { "Installing".into() };
            let font = FontId::proportional(TEXT);
            let at = track.center();
            // the words white on the black, black on the white
            p.with_clip_rect(track.with_min_x(bar.max.x)).text(at, Align2::CENTER_CENTER, &label, font.clone(),
                                                               c.ink.gamma_multiply(words));
            p.with_clip_rect(bar).text(at, Align2::CENTER_CENTER, &label, font, Color32::BLACK.gamma_multiply(words));
            ctx.request_repaint();
            if dark >= 1.0 {
                return; // all under it now
            }
        }
        let cy = H / 2.0;
        if closing.is_some() {
            ui.set_opacity(reveal); // play goes with the rest, and nothing's outside the shrinking pill
            painter = ui.painter().clone();
            painter.set_clip_rect(shape.shrink(1.0));
        }

        // start / stop; a right click starts Debug mode, and the button turns into a bug while it's on
        let main = pos2(INSET + CONTROL / 2.0, cy);
        let resp = ui.interact(Rect::from_center_size(main, Vec2::splat(CONTROL)), Id::new("main"), Sense::click());
        let size = press(&ctx, &resp);
        painter.circle_filled(main, CONTROL / 2.0 * size, if resp.hovered() { c.dim } else { c.ink });
        if resp.secondary_clicked() {
            self.debug_run();
        }
        let bug = ease(ctx.animate_bool_with_time(Id::new("bug"), self.test, 0.3));
        let (k, face) = ((1.0 - bug) * size, c.fill.gamma_multiply(1.0 - bug)); // play / stop, shrinking away
        // play and stop melt into each other: a triangle's corners slide out to a square's
        let stop = ease(ctx.animate_bool_with_time(Id::new("stop"), !self.shows_play(), 0.25));
        let play = [vec2(-3.0, -5.0), vec2(5.5, 0.0), vec2(5.5, 0.0), vec2(-3.0, 5.0)];
        let square = [vec2(-4.0, -4.0), vec2(4.0, -4.0), vec2(4.0, 4.0), vec2(-4.0, 4.0)];
        let pts: Vec<Pos2> = (0..4).map(|i| main + (play[i] + (square[i] - play[i]) * stop) * k).collect();
        painter.add(egui::Shape::convex_polygon(pts, face, Stroke::NONE));
        if bug > 0.0 {
            egui::Image::new(egui::include_image!("../icons/bug.svg"))
                .tint(c.fill.gamma_multiply(bug))
                .paint_at(ui, Rect::from_center_size(main, Vec2::splat(15.0 * (0.5 + 0.5 * bug) * size)));
        }
        if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            self.main_button();
        }

        if reveal <= 0.0 {
            return;
        }
        ui.set_opacity(reveal);
        let mut painter = ui.painter().clone();
        let slide = (1.0 - reveal) * 10.0; // what's in it slides in from the left as it comes
        if slide > 0.0 {
            painter.set_clip_rect(shape.shrink(1.0));
        }

        // the website code (or a word: Done, Stopped, an update's progress), which opens the website
        let left = main.x + CONTROL / 2.0 + 12.0 - slide;
        let words = self.code_text();
        let shown = words.clone(); // nothing, for the moment it takes to connect
        let is_code = !self.code.is_empty() && shown == self.code;
        let code_font = FontId::new(13.0, FontFamily::Name("code".into()));
        let area = painter.text(pos2(left, cy), Align2::LEFT_CENTER, &shown, code_font.clone(), Color32::TRANSPARENT);
        let resp = ui.interact(area.expand(4.0), Id::new("code"), Sense::click());
        let color = if is_code && (light || resp.hovered()) {
            c.ink
        } else if is_code {
            gray(0xd8)
        } else if words.is_empty() {
            c.edge
        } else {
            c.dim
        };
        painter.text(pos2(left, cy), Align2::LEFT_CENTER, &shown, code_font, color);
        let web = matches!(self.mode.as_str(), "hunt" | "debug" | "calibrate");
        if shown.is_empty() && self.running() && web && !self.fake {
            for i in 0..3 {  // the website's code on its way: three dots, breathing in turn
                let a = 0.5 + 0.5 * (t * 4.0 - i as f32 * 0.8).sin();
                painter.circle_filled(pos2(left + 3.0 + i as f32 * 9.0, cy), 2.2, c.dim.gamma_multiply(0.35 + 0.65 * a));
            }
            ctx.request_repaint();
        }
        if is_code && resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            open_path(&format!("{}/app", site()));
        }

        // from the right end: quit, Hunt's folder
        let gap = 6.0;
        let quit = pos2(W - INSET - CONTROL / 2.0, cy);
        let folder = quit - vec2(CONTROL + gap, 0.0);
        for (id, at) in [("folder", folder), ("quit", quit)] {
            let resp = ui.interact(Rect::from_center_size(at, Vec2::splat(CONTROL)), Id::new(id), Sense::click());
            let hovered = resp.hovered();
            let grown = press(&ctx, &resp);
            painter.circle_filled(at, CONTROL / 2.0 * grown, if hovered { c.hover } else { c.boxed });
            let tint = if hovered { c.ink } else if light { gray(0x55) } else { gray(0xb4) };
            let icon = match id {
                "folder" => egui::include_image!("../icons/folder.svg"),
                _ => egui::include_image!("../icons/x.svg"),
            };
            let size = if id == "quit" { 13.0 } else { 15.0 };
            egui::Image::new(icon).tint(tint).paint_at(ui, Rect::from_center_size(at, Vec2::splat(size * grown)));
            if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                match id {
                    "folder" => open_path(&self.paths.data.to_string_lossy()),
                    _ => self.quit(&ctx),
                }
            }
        }

        // the webhook box, between
        let hook_box = Rect::from_min_max(pos2(left + 70.0, cy - CONTROL / 2.0),
                                          pos2(folder.x - CONTROL / 2.0 - 8.0, cy + CONTROL / 2.0));
        painter.rect_filled(hook_box, CONTROL / 2.0, c.boxed);
        let inner = hook_box.shrink2(vec2(11.0, 2.0));
        let text_font = FontId::proportional(TEXT);
        if self.hook_open {
            let edit = egui::TextEdit::singleline(&mut self.hook_edit)
                .id(Id::new("hook"))
                .frame(false)
                .font(text_font)
                .text_color(c.ink)
                .margin(egui::Margin::ZERO)
                .vertical_align(egui::Align::Center);
            let resp = ui.put(inner, edit);
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.hook_edit = self.hook.clone();
            }
            if resp.lost_focus() {
                self.hook_done();
            }
        } else {
            let note = self.hook_note.filter(|(_, t)| t.elapsed() < Duration::from_millis(2500));
            if self.hook_note.is_some() && note.is_none() {
                self.hook_note = None;
            }
            if note.is_some() {
                ctx.request_repaint_after(Duration::from_millis(300));
            }
            let words = note.map(|(n, _)| n).unwrap_or(if self.hook.is_empty() { "Discord webhook" } else { "Webhook on" });
            painter.text(pos2(inner.min.x, cy), Align2::LEFT_CENTER, words, text_font, c.dim);
            let resp = ui.interact(hook_box, Id::new("hookbox"), Sense::click());
            if resp.on_hover_cursor(CursorIcon::Text).clicked() {
                self.hook_open = true;
                self.hook_edit = self.hook.clone();
                ctx.memory_mut(|m| m.request_focus(Id::new("hook")));
            }
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.stop();
        self.close_with(ctx);
    }

    /// The closing, then the window goes. Straight away where there's no see-through window to fly in.
    fn close_with(&mut self, ctx: &egui::Context) {
        if self.closing.is_some() || self.closed {
            return;
        }
        let animate = self.intro.is_none() && (self.smoke.is_none() || env::args().any(|a| a == "--intro"));
        if animate {
            self.closing = Some(Closing { started: Instant::now() });
            ctx.request_repaint();
        } else {
            self.finish_close(ctx);
        }
    }

    fn finish_close(&mut self, ctx: &egui::Context) {
        self.closed = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// The closing's end: the window goes.
    fn closing_window(&mut self, ctx: &egui::Context) {
        let Some(closing) = self.closing.as_ref() else { return };
        ctx.request_repaint();
        if closing.started.elapsed().as_secs_f32() >= FOLD + VANISH {
            self.finish_close(ctx);
        }
    }
}

impl eframe::App for Hunt {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Line(id, line) if self.proc.as_ref().map(|p| p.id) == Some(id) => self.got_line(line),
                Msg::Code(id, code) if self.proc.as_ref().map(|p| p.id) == Some(id) => self.code = code.unwrap_or_default(),
                Msg::State(id, s) if self.proc.as_ref().map(|p| p.id) == Some(id) => self.got_state(s),
                Msg::Ended(id) if self.proc.as_ref().map(|p| p.id) == Some(id) => self.ended(),
                Msg::Line(..) | Msg::Code(..) | Msg::State(..) | Msg::Ended(..) => {}
                Msg::Update(info) => {
                    self.update = Some(info);
                    self.download_update();
                }
                Msg::Progress(p) => {
                    if !self.hunting() {
                        self.progress = Some(p.min(0.99));
                    }
                }
                Msg::UpdateFailed(e) => {
                    self.say(&format!("the update didn't download: {e}"));
                    self.progress = None;
                    self.downloading = false; // (the next look tries again)
                }
                Msg::Downloaded(path) => {
                    self.installer = Some(path);
                    self.downloading = false;
                    if self.proc.is_some() {
                        self.install_when_stopped = true;
                        self.install_when_quiet();
                    } else {
                        self.progress = Some(1.0);
                        self.install();
                    }
                }
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.closed {
            // (Alt+F4, the taskbar's Close): the closing first, then it goes
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.stop();
            self.close_with(ctx);
        }
        // (the pill's drawing moves its layer M in; the opening draws in the window's own terms)
        ctx.set_transform_layer(egui::LayerId::background(), egui::emath::TSTransform::IDENTITY);
        self.handing_over(ctx);
        self.taking_over(ctx);
        if self.intro.is_some() {
            self.intro_window(ctx);
        } else if self.closing.is_some() {
            self.closing_window(ctx);
        } else {
            self.track_place(ctx);
        }
        if std::mem::take(&mut self.start_now) {
            match self.carried.take() {
                // after an update: the hunt as the old Hunt left it
                Some(was) => {
                    let mode = was["mode"].as_str().filter(|m| ["hunt", "watch", "debug"].contains(m))
                        .unwrap_or(Self::hunt_mode()).to_string();
                    self.test = was["test"].as_bool().unwrap_or(false);
                    self.count_from = was["count"].as_u64().unwrap_or(0) as u32;
                    self.carrying = was["hunting"].as_bool().unwrap_or(false);
                    self.checked_at = self.count_from;
                    self.start(&mode, !was["hunting"].as_bool().unwrap_or(false));
                }
                None => self.start(Self::hunt_mode(), !env::args().any(|a| a == "--start")),
            }
        }
        if let Some(mut smoke) = self.smoke.take() {
            smoke.step(self, ctx);
            self.smoke = Some(smoke);
        }
        egui::CentralPanel::default().frame(egui::Frame::default()).show(ctx, |ui| {
            match self.intro.as_ref().filter(|i| i.handover.is_none()).and_then(|i| Some((i.started?, i.origin))) {
                Some((t0, origin)) => self.draw_intro(ui, t0.elapsed().as_secs_f32(), origin),
                None if self.intro.as_ref().is_some_and(|i| i.handover.is_some()) => self.draw_landed(ui),
                None if self.intro.is_some() => {} // the window's getting to its size
                None => self.draw(ui),
            }
        });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0] // see-through, but for the pill
    }
}

/// The local time, for hunt.log's start lines.
fn chrono_now() -> String {
    #[cfg(windows)]
    unsafe {
        let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        format!("{}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
    #[cfg(not(windows))]
    {
        Command::new("date").arg("+%Y-%m-%d %H:%M:%S").output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
    }
}

/// A Mac gives a window without a frame a shadow, traced from what's on it
/// once: the opening's big circle left one behind in the middle of the screen.
fn no_mac_shadow(cc: &eframe::CreationContext) {
    #[cfg(target_os = "macos")]
    {
        use objc2::runtime::AnyObject;
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = cc.window_handle() {
            if let RawWindowHandle::AppKit(h) = handle.as_raw() {
                unsafe {
                    let view: *mut AnyObject = h.ns_view.as_ptr().cast();
                    let window: *mut AnyObject = objc2::msg_send![view, window];
                    if !window.is_null() {
                        let _: () = objc2::msg_send![window, setHasShadow: false];
                        // the hunter's screenshots are of what's under the pill
                        let number: isize = objc2::msg_send![window, windowNumber];
                        desktop::set_own_window(number as u32);
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = cc;
}

/// It's on top of Roblox, so the hunter's screenshots (and anyone's) look
/// through it, at the game under it (Windows 10 2004 and later).
fn hide_from_screenshots(cc: &eframe::CreationContext) {
    #[cfg(windows)]
    if env::var_os("HUNT_CAPTURABLE").is_none() { // (the Windows build's pictures want to see it)
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = cc.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetWindowDisplayAffinity(
                        h.hwnd.get() as _, 0x11); // WDA_EXCLUDEFROMCAPTURE
                }
            }
        }
    }
    #[cfg(not(windows))]
    let _ = cc;
}

// ---------------------------------------------------------------- fonts

fn setup_fonts(ctx: &egui::Context) {
    // Inter (fonts/, SIL Open Font License: fonts/Inter-LICENSE.txt), the same on every PC:
    // Regular for the words, Medium for the website code
    let mut fonts = egui::FontDefinitions::default();
    let inter = |bytes: &'static [u8]| Arc::new(egui::FontData::from_static(bytes));
    fonts.font_data.insert("inter".into(), inter(include_bytes!("../fonts/Inter-Regular.ttf")));
    fonts.font_data.insert("inter-medium".into(), inter(include_bytes!("../fonts/Inter-Medium.ttf")));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    let mut code = vec!["inter-medium".to_string()];
    code.extend(fonts.families[&FontFamily::Proportional].clone());
    fonts.families.insert(FontFamily::Name("code".into()), code);
    ctx.set_fonts(fonts);
}

// ---------------------------------------------------------------- --smoke

/// The Windows build's check (--smoke): the pill comes up and goes through its
/// states, the webhook box and Debug mode, then closes.
/// smoke.txt says how it went; anything wrong ends Hunt.exe with 1.
struct Smoke {
    t0: Instant,
    stage: usize,
    errors: Vec<String>,
    after: f32, // s: with the opening (--intro), the states come after it, and ✕ closes it at the end
}

impl Smoke {
    fn new() -> Smoke {
        Smoke { t0: Instant::now(), stage: 0, errors: Vec::new(), after: 0.0 }
    }

    fn step(&mut self, h: &mut Hunt, ctx: &egui::Context) {
        ctx.request_repaint();
        let at = self.t0.elapsed().as_secs_f32() - self.after;
        let state = |v: Value| v.as_object().cloned().unwrap_or_default();
        let stages: &[f32] = &[0.6, 0.8, 1.0, 1.2, 1.4, 1.6, 2.4, 3.4, 4.0];
        if self.stage >= stages.len() || at < stages[self.stage] {
            return;
        }
        match self.stage {
            0 => { h.fake = true; h.mode = "hunt".into(); h.got_state(state(json!({}))); }
            1 => { h.code = "AB12C".into(); h.got_state(state(json!({"running": true, "phase": "waiting"}))); }
            2 => h.got_state(state(json!({"running": false, "phase": "battle", "wanted": true, "enc": "Shiny Tympole"}))),
            3 => h.got_state(state(json!({"running": false, "phase": "waiting"}))),
            4 => {
                h.fake = false;
                h.state.clear();
                for url in ["not a webhook", "https://discord.com/api/webhooks/1/smoke", ""] {
                    h.hook_edit = url.into();
                    h.hook_done();
                }
                if h.hook_saved() != "" {
                    self.errors.push("the webhook didn't clear".into());
                }
            }
            5 => {
                h.fake = true;
                h.debug_run();
                if !h.test {
                    self.errors.push("Debug mode didn't come on".into());
                }
                h.debug_run();
                h.fake = false;
            }
            6 => {} // for the build's picture
            7 => {
                let outer = ctx.input(|i| i.viewport().outer_rect).unwrap_or(Rect::NOTHING);
                let words = if self.errors.is_empty() {
                    format!("ok: the pill is {} x {} at {}, {}\n", outer.width(), outer.height(), outer.min.x, outer.min.y)
                } else {
                    self.errors.join("\n") + "\n"
                };
                let _ = fs::write(h.paths.data.join("smoke.txt"), words);
            }
            _ if self.after > 0.0 && self.errors.is_empty() => h.quit(ctx), // the closing, too
            _ => std::process::exit(if self.errors.is_empty() { 0 } else { 1 }),
        }
        self.stage += 1;
    }
}

// ---------------------------------------------------------------- main

/// One Hunt at a time: two would fight over the mouse.
fn single_instance() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name: Vec<u16> = "Local\\HuntWindow\0".encode_utf16().collect();
        CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION};
            let text: Vec<u16> = "Hunt is already open: look for the Poke Ball in the taskbar.\0".encode_utf16().collect();
            let title: Vec<u16> = "Hunt\0".encode_utf16().collect();
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_ICONINFORMATION);
            return false;
        }
        let id: Vec<u16> = "poopihunt.Hunt\0".encode_utf16().collect();
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(id.as_ptr()); // its own taskbar icon
    }
    true
}

fn main() -> eframe::Result {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| ["--terminal", "--diagnose", "--image", "--test-discord", "--help"].contains(&a.as_str())) {
        if args.iter().any(|a| a == "--help") {
            println!("Hunt: the pill. Hunt --terminal [--watch] [--paused] [--debug] [--practice N] [--calibrate]: \
                      the hunter alone; --diagnose; --image x.png [--calibrate]; --test-discord; --config path");
            return Ok(());
        }
        std::process::exit(engine::terminal(&args));
    }
    std::panic::set_hook(Box::new(|info| note(&format!("Hunt's window crashed: {info}"))));
    // (an update's new Hunt comes on while the old one's still there)
    if env::var_os("HUNT_RENDERER").is_none() && !Paths::new().updating() && !single_instance() {
        return Ok(()); // (the Direct3D start is this same Hunt, already counted)
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Poopihunt")
        .with_inner_size(WINDOW)
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top() // over Roblox too (the hunter's screenshots see through it on Windows)
        .with_taskbar(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../icons/clodsire-192.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let settings = read_json(&Paths::new().settings).unwrap_or_default();
    if let Some([x, y]) = settings.get("pos").and_then(Value::as_array).map(|a| {
        [a.first().and_then(Value::as_f64).unwrap_or(0.0) as f32, a.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32]
    }) {
        viewport = viewport.with_position(pos2(x, y) - MARGIN);
    }
    // OpenGL draws it see-through. A PC without it (no graphics driver) gets
    // Direct3D instead, square: Hunt starts again that way (one window per run).
    let direct3d = env::var_os("HUNT_RENDERER").is_some_and(|r| r == "wgpu");
    let renderer = if direct3d { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow };
    let options = eframe::NativeOptions { viewport, renderer, wgpu_options: direct3d_anywhere(), ..Default::default() };
    let result = eframe::run_native("Poopihunt", options, Box::new(|cc| Ok(Box::new(Hunt::new(cc)))));
    if let Err(e) = &result {
        note(&format!("Hunt's window didn't open ({}): {e}", if direct3d { "Direct3D" } else { "OpenGL" }));
        if !direct3d {
            let me = env::current_exe().unwrap_or_default();
            if Command::new(me).args(env::args().skip(1)).env("HUNT_RENDERER", "wgpu").spawn().is_ok() {
                return Ok(());
            }
        }
    }
    result
}

/// Direct3D on a graphics card, or else on Windows' own software one (WARP),
/// which wgpu passes over by itself: a PC with no graphics driver still gets a window.
fn direct3d_anywhere() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    use eframe::wgpu::DeviceType;
    let pick: eframe::egui_wgpu::NativeAdapterSelectorMethod = Arc::new(|adapters, surface| {
        adapters
            .iter()
            .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
            .min_by_key(|a| match a.get_info().device_type {
                DeviceType::DiscreteGpu => 0,
                DeviceType::IntegratedGpu => 1,
                DeviceType::VirtualGpu => 2,
                DeviceType::Other => 3,
                DeviceType::Cpu => 4,
            })
            .cloned()
            .ok_or_else(|| format!("none of the {} graphics adapters can draw it", adapters.len()))
    });
    WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(WgpuSetupCreateNew { native_adapter_selector: Some(pick), ..Default::default() }),
        ..Default::default()
    }
}

/// The Hunt.exe an update moved out of the way, once it's done with.
fn remove_old_exe() {
    #[cfg(windows)]
    if let Ok(me) = env::current_exe() {
        let _ = fs::remove_file(me.with_file_name("Hunt.old.exe"));
    }
}

/// A line in hunt.log from before (or after) the window: what went wrong.
fn note(line: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(Paths::new().log) {
        let _ = writeln!(f, "{line}");
    }
}
