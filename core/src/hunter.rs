//! The hunt: reads each encounter (the red Mute button is up) from the enemy's
//! box, runs from the ones not wanted, catches the ones worth a ball, and
//! pings Discord and holds the battle for you when a wanted one shows up.
//!
//! Control is what the window, the app, the hotkeys and the website reach
//! (pause, Debug mode, the website code); Hunt is the hunt's own thread.

use crate::calibrate::calibrate;
use crate::config::Config;
use crate::device::{Device, Sound, Traits};
use crate::discord::Webhook;
use crate::layout::{self, Calibration, Layout};
use crate::out::{DebugChannel, Out, Sink};
use crate::picture::{self, crop, game_shot, png, resize};
use crate::px::{self, Frame, IBox, Pt};
use crate::reader::{self, Lead, Move, TextHit};
use crate::rules::{self, norm, same_mon, settled, wanted, Enc, Rule};
use crate::store::Store;
use crate::util::{local, now, sleep};
use crate::vision::{self, box_visible, chat_open, chat_showing, find_mute, menu_ready};
use crate::web::Link;
use base64::Engine;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};

pub const SAVE_EVERY: f64 = 125.0; // s; the game allows one save every 2 minutes
pub const GONE_AFTER: f64 = 1.0; // s without the Mute button before an encounter counts as over
pub const TURN_TIMEOUT: f64 = 60.0; // s for one turn: a throw with its shakes, or a move
pub const AFK_EVERY: f64 = 15.0 * 60.0; // Roblox kicks after 20 min without input

/// Why the hunt's work stopped short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halt {
    /// The hunt is over (its window closed, the app's Stop).
    Stopped,
    /// Switched off, or taken over from the website, mid-battle: it's yours from there.
    Paused,
    /// The battle ended (it fled, it was caught, ...) while a catch was going.
    BattleOver,
    /// The mouse was flung into a screen corner.
    FailSafe,
}

pub type R<T> = Result<T, Halt>;

/// How a hunt is run.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Read and log encounters, never click.
    pub watch: bool,
    /// Start paused (the window's hunter: the website code is there at once).
    pub paused: bool,
    /// Catch every encounter (then save), a picture of every catch turn and click.
    pub debug: bool,
    /// Catch the next n encounters too, without the first-turn ball.
    pub practice: u32,
    /// Fit the layout at the next encounter and draw it.
    pub calibrate: bool,
    /// Run by a window (Hunt.exe, the app): a new install's first hunt is a debug run.
    pub window: bool,
    /// The build, for the website: "23 Oct 17:53".
    pub build: String,
    /// This device's secret for the website; empty: no website.
    pub key: String,
    /// Each run-from encounter's time, traced (HUNT_TIMES).
    pub times: bool,
    /// Encounters so far: a hunt carried on from before an update.
    pub count: u32,
    /// A hunt going on from before an update: a battle on screen when it starts is just
    /// its next encounter (the update takes a few seconds), not yours to play.
    pub carried: bool,
}

/// What the website shows of the hunt.
#[derive(Default)]
pub struct WebState {
    pub encounters: u32,
    pub phase: String,
    pub enc: Option<Enc>,
    pub wanted: bool,
    pub lead: Lead,
    pub moves: Vec<Move>,
    pub msg: String,
    pub acting: String,
    pub error: String,
    pub done: i64,
    pub known_moves: HashMap<String, Vec<Move>>,
}

/// The hunt, as the window, the app, the hotkeys and the website reach it.
pub struct Control {
    pub dev: Arc<dyn Device>,
    pub out: Arc<Out>,
    pub cfg: Config,
    pub store: Store,
    pub opts: Options,
    pub traits: Traits,
    running: AtomicBool,
    stopped: AtomicBool,
    remote: AtomicBool,
    busy: AtomicBool,
    between: AtomicBool,
    debug_run: AtomicBool,
    fit_next: AtomicBool,
    want_shot: AtomicBool,
    test_catch: AtomicU32,
    commands: Mutex<VecDeque<Value>>,
    wake: Condvar,
    pub web: Mutex<WebState>,
    link: Mutex<Option<Arc<Link>>>,
    /// Your webhook.
    pub discord: Webhook,
}

impl Control {
    pub fn running(&self) -> bool {
        self.running.load(Relaxed)
    }
    pub fn remote(&self) -> bool {
        self.remote.load(Relaxed)
    }
    pub fn stopped(&self) -> bool {
        self.stopped.load(Relaxed)
    }
    pub fn testing(&self) -> bool {
        self.debug_run.load(Relaxed) || self.test_catch.load(Relaxed) > 0
    }

    pub fn print(&self, msg: &str) {
        self.out.print(msg);
    }

    /// Pause / resume (Ctrl+Shift+H, the pill's play, the phone's ON / OFF).
    pub fn toggle(&self) {
        let key = self.traits.toggle_key;
        if self.running() {
            self.running.store(false, Relaxed);
            self.print(&format!("\n|| paused ({key} to resume)"));
            self.dev.sound(Sound::Paused);
        } else {
            self.print(&format!("\n>> hunting ({key} to pause)"));
            self.dev.sound(Sound::Resumed);
            self.running.store(true, Relaxed);
        }
        self.wake.notify_all();
        self.publish();
    }

    /// The hunt ends (its window closed, the app's Stop).
    pub fn stop(&self) {
        self.stopped.store(true, Relaxed);
        self.wake.notify_all();
        if let Some(link) = self.link.lock().unwrap().take() {
            link.stop();
        }
    }

    /// The hunt ends at its next quiet moment: no battle on screen, nothing owed
    /// (a save after a catch). For an update, to carry on in the new build.
    pub fn stop_between(&self) {
        self.between.store(true, Relaxed);
        self.wake.notify_all();
    }

    /// The window's Debug mode: a debug run on the next encounter (with the
    /// layout fitted again and drawn), or off.
    pub fn set_test(&self, on: bool) {
        self.debug_run.store(on, Relaxed);
        self.fit_next.store(on, Relaxed);
        self.update_loud();
        self.out.say(if on { "  (Debug mode: a debug run on the next encounter)" } else { "  (Debug mode off)" });
        self.publish();
    }

    /// The layout fitted again at the next encounter, and drawn (Pictures/Hunt on a phone).
    pub fn calibrate_next(&self) {
        self.fit_next.store(true, Relaxed);
    }

    fn update_loud(&self) {
        self.out.debug.loud.store(self.opts.debug || self.debug_run.load(Relaxed), Relaxed);
    }

    /// The website: a new code (true), or every browser logged out (false). In the background.
    pub fn website(self: &Arc<Self>, new_code: bool) {
        let Some(link) = self.link.lock().unwrap().clone() else { return };
        let me = self.clone();
        std::thread::spawn(move || {
            let r = if new_code { link.new_code() } else { link.log_out_browsers() };
            match r {
                Ok(()) if !new_code => me.print("  (website: every browser is logged out)"),
                Ok(()) => {}
                Err(e) => me.print(&format!("  (website: no connection: {e})")),
            }
        });
    }

    /// The website's login code, while there's one to type.
    pub fn code(&self) -> String {
        self.link.lock().unwrap().as_ref().map(|l| l.code()).unwrap_or_default()
    }

    /// A browser logged in with the last code.
    pub fn logged_in(&self) -> bool {
        self.link.lock().unwrap().as_ref().is_some_and(|l| l.logged_in())
    }

    /// The website, when this hunt is linked to it.
    pub fn link(&self) -> Option<Arc<Link>> {
        self.link.lock().unwrap().clone()
    }

    pub fn linked(&self) -> bool {
        self.link.lock().unwrap().is_some()
    }

    /// The window's view: {"running", "remote", "count", "phase", "wanted", "enc", "test"}.
    pub fn status(&self) -> Value {
        let web = self.web.lock().unwrap();
        json!({
            "running": self.running(), "remote": self.remote(), "count": web.encounters,
            "phase": if web.phase.is_empty() { "waiting" } else { web.phase.as_str() }, "wanted": web.wanted,
            "enc": web.enc.as_ref().map(|e| e.title()).unwrap_or_default(), "test": self.testing(),
        })
    }

    /// The website and the window see what the hunt's doing.
    pub fn publish(&self) {
        self.out.sink.state(&self.status());
        let Some(link) = self.link.lock().unwrap().clone() else { return };
        let web = self.web.lock().unwrap();
        let mut s = json!({
            "hunting": true, "running": self.running(), "remote": self.remote(),
            "phase": if web.phase.is_empty() { "waiting" } else { web.phase.as_str() },
            "count": web.encounters, "wanted": web.wanted, "msg": web.msg, "acting": web.acting, "error": web.error,
            "done": web.done, "balls": self.cfg.web_balls(), "heal_item": self.cfg.str("heal_item"),
            "pp_item": self.cfg.str("pp_item"), "moves": web.moves.iter().map(Move::to_json).collect::<Vec<_>>(),
            "log": self.out.recent(),
        });
        if let Some(e) = &web.enc {
            s["enc"] = json!({"species": e.species, "level": e.level, "reskin": e.reskin, "gradient": e.gradient,
                              "status": e.status, "shiny": e.shiny, "tint": e.tint});
        }
        if web.lead != Lead::default() {
            s["lead"] = web.lead.to_json();
        }
        drop(web);
        link.send_state(s);
    }

    /// A command from the website (the link's thread).
    fn from_website(&self, cmd: Value) {
        match cmd["cmd"].as_str().unwrap_or("") {
            "takeover" if !self.remote() => {
                self.commands.lock().unwrap().clear();
                self.remote.store(true, Relaxed);
                self.print("\n<< taken over from the website");
                self.dev.sound(Sound::Paused);
                self.wake.notify_all();
                self.publish();
            }
            "resume" if self.remote() => {
                self.remote.store(false, Relaxed);
                self.running.store(true, Relaxed);
                self.print("\n>> handed back from the website: hunting");
                self.dev.sound(Sound::Resumed);
                self.wake.notify_all();
                self.publish();
            }
            "shot" => self.want_shot.store(true, Relaxed),
            "login" => {
                self.print(if self.traits.phone {
                    "  (website: a browser logged in with the code, so it's spent. Wasn't you? Log out browsers in the app)"
                } else {
                    "  (website: a browser logged in with the code, so it's spent: Ctrl+Shift+N for a new one. \
                     Wasn't you? Ctrl+Shift+L logs every browser out)"
                });
                self.dev.sound(Sound::Login);
                self.dev.website_login();
            }
            "ball" | "move" | "heal" | "pp" | "run" => {
                self.commands.lock().unwrap().push_back(cmd);
                self.wake.notify_all();
            }
            _ => {}
        }
    }

    /// The page asked for a new picture (once).
    pub(crate) fn want_shot_taken(&self) -> bool {
        self.want_shot.swap(false, Relaxed)
    }

    /// The website's next battle command, waiting up to seconds for one.
    pub(crate) fn next_command(&self, seconds: f64) -> Option<Value> {
        let q = self.commands.lock().unwrap();
        let (mut q, _) = self.wake
            .wait_timeout_while(q, std::time::Duration::from_secs_f64(seconds), |q| q.is_empty() && !self.stopped())
            .unwrap();
        q.pop_front()
    }
}

/// A debug run's time, added up per step: the screen's grabs, text read, looks
/// for Mute, clicks, and the waits.
pub(crate) struct Timing {
    t0: f64,
    mark: f64,
    spent: BTreeMap<&'static str, (u32, f64)>,
}

impl Timing {
    fn new() -> Timing {
        Timing { t0: now(), mark: now(), spent: BTreeMap::new() }
    }
    pub(crate) fn at(&self) -> f64 {
        now() - self.t0
    }
    /// Since the last: "2.31 s: ocr 1.20 (2), sleep 0.60 (4), grab 0.41 (9), ...".
    pub(crate) fn lap(&mut self) -> String {
        let t = now();
        let total = t - self.mark;
        let mut spent: Vec<(&str, (u32, f64))> = std::mem::take(&mut self.spent).into_iter().collect();
        spent.sort_by(|a, b| b.1 .1.total_cmp(&a.1 .1));
        let rest = total - spent.iter().map(|s| s.1 .1).sum::<f64>();
        self.mark = t;
        let mut parts: Vec<String> = spent.iter().map(|(n, (c, s))| format!("{n} {s:.2} ({c})")).collect();
        parts.push(format!("the rest {rest:.2}"));
        format!("{total:.2} s: {}", parts.join(", "))
    }
}

/// A read encounter.
pub(crate) struct Read {
    pub enc: Enc,
    pub f: Frame,
    pub lay: Layout,
}

/// The hunt's own thread.
pub struct Hunt {
    pub(crate) c: Arc<Control>,
    pub(crate) dev: Arc<dyn Device>,
    pub(crate) traits: Traits,
    pub(crate) cal: Calibration,
    cal_key: String,
    pub(crate) cal_saved: bool,
    pub(crate) cal_file: std::path::PathBuf,
    pub(crate) last_click: f64,
    pub(crate) save_wait: u64,
    pub(crate) pp_for: String,
    /// What the text recognition saw in the enemy's box last (for the website's log).
    pub(crate) box_read: String,
    pub(crate) menu_spots: Option<Map<String, Value>>,
    /// The debug run is going: every bag and move position seen is saved over the old (menu.json).
    pub(crate) learning: bool,
    pub(crate) timing: Option<Timing>,
    timing_is_times: bool,
    pub(crate) chat_closed_at: f64,
    last_frame: Option<(Frame, f64)>,
    pub(crate) used_item: String,
    /// The item the bag was looked all through for, last, and didn't have.
    pub(crate) not_in_bag: String,
    /// A ball the next use_item takes instead of its item if it's a favorite in sight (Level Ball).
    pub(crate) prefer: String,
    /// Since when Roblox's window has been gone (closed, crashed), and when the hunter last
    /// said it brought Roblox to the front.
    roblox_gone: Option<f64>,
    front_said: f64,
    /// A calibration far from the saved one, kept till a second agrees (one off read of
    /// the Mute button fitted 1.44x and the menu went unseen for a minute).
    pending_cal: Option<Calibration>,
    pub(crate) move_pp: Option<i64>,
    pub(crate) calibrate_next: bool,
    pub(crate) notify: Vec<Rule>,
}

/// Starts a hunt: the website linked (unless watching), Control for whoever
/// runs it, and the hunt itself to run on a thread of its own.
pub fn start(dev: Arc<dyn Device>, sink: Arc<dyn Sink>, debug: Arc<DebugChannel>, cfg: Config, store: Store,
             opts: Options) -> (Arc<Control>, Hunt) {
    let traits = dev.traits();
    // the update log: the build this ran as last time here (build.txt), and a word to the
    // debug channel when it's another one (an update), or the first time (a new install)
    let last = store.path("build.txt");
    let before = std::fs::read_to_string(&last).unwrap_or_default().trim().to_string();
    if !opts.build.is_empty() && before != opts.build {
        let msg = if before.is_empty() { format!("installed: {}", opts.build) }
                  else { format!("updated: {before} → {}", opts.build) };
        let d = debug.clone();
        std::thread::spawn(move || d.post(&msg, None));
        let _ = std::fs::write(&last, &opts.build);
    }
    let out = Arc::new(Out::new(sink, debug));
    let o = out.clone();
    let discord = Webhook::new(&cfg.webhook, false, Arc::new(move |e: &str| o.print(e)));
    let c = Arc::new(Control {
        running: AtomicBool::new(!opts.paused),
        stopped: AtomicBool::new(false),
        remote: AtomicBool::new(false),
        busy: AtomicBool::new(false),
        between: AtomicBool::new(false),
        debug_run: AtomicBool::new(false),
        fit_next: AtomicBool::new(false),
        want_shot: AtomicBool::new(false),
        test_catch: AtomicU32::new(opts.practice),
        commands: Mutex::new(VecDeque::new()),
        wake: Condvar::new(),
        web: Mutex::new(WebState { phase: "waiting".into(), encounters: opts.count, ..Default::default() }),
        link: Mutex::new(None),
        discord,
        dev: dev.clone(),
        out,
        cfg,
        store,
        opts,
        traits: traits.clone(),
    });
    c.update_loud();
    if !c.opts.watch && !c.opts.key.is_empty() {
        // watching never clicks, so there's nothing to take over
        let (weak, sink) = (Arc::downgrade(&c), c.out.sink.clone());
        let link = Link::new(&c.opts.key, &c.opts.build, &dev.label(), traits.kind,
                             Box::new(move |cmd| if let Some(c) = weak.upgrade() { c.from_website(cmd) }),
                             Box::new(move |code| sink.code(code.as_deref())));
        link.start();
        *c.link.lock().unwrap() = Some(link);
    }
    let mut notify = c.cfg.notify.clone();
    if c.opts.debug {
        notify.push(json!({"species": "any"}).as_object().unwrap().clone());
    }
    let hunt = Hunt {
        dev,
        traits,
        cal: Calibration::default(),
        cal_key: String::new(),
        cal_saved: false,
        cal_file: c.store.calibration_file(),
        last_click: now(),
        save_wait: 0,
        pp_for: String::new(),
        box_read: String::new(),
        menu_spots: None,
        learning: false,
        timing: None,
        timing_is_times: false,
        chat_closed_at: 0.0,
        last_frame: None,
        used_item: String::new(),
        not_in_bag: String::new(),
        prefer: String::new(),
        roblox_gone: None,
        front_said: 0.0,
        pending_cal: None,
        move_pp: None,
        calibrate_next: c.opts.calibrate,
        notify,
        c: c.clone(),
    };
    (c, hunt)
}

impl Hunt {
    // ------------------------------------------------------------ saying

    pub(crate) fn print(&self, msg: &str) {
        self.c.out.print(msg);
    }

    pub(crate) fn say(&self, msg: &str) {
        self.c.out.say(msg);
    }

    /// A step of a catch, for following it in the log (in a debug run, with its time).
    pub(crate) fn trace(&self, msg: &str) {
        match &self.timing {
            Some(t) => self.print(&format!("    +{:6.2}  {msg}", t.at())),
            None => self.print(&format!("    {msg}")),
        }
    }

    fn spent<T>(&mut self, what: &'static str, f: impl FnOnce(&mut Self) -> T) -> T {
        if self.timing.is_none() {
            return f(self);
        }
        let t = now();
        let out = f(self);
        if let Some(timing) = self.timing.as_mut() {
            let e = timing.spent.entry(what).or_insert((0, 0.0));
            e.0 += 1;
            e.1 += now() - t;
        }
        out
    }

    // ------------------------------------------------------------ time

    fn check(&self) -> R<()> {
        if self.c.stopped() {
            return Err(Halt::Stopped);
        }
        if self.c.busy.load(Relaxed) && (!self.c.running() || self.c.remote()) {
            return Err(Halt::Paused);
        }
        Ok(())
    }

    /// A wait; switching off mid-battle (or the hunt ending) cuts it short.
    pub(crate) fn nap(&mut self, seconds: f64) -> R<()> {
        self.check()?;
        self.spent("sleep", |_| {});
        let t0 = now();
        let end = t0 + seconds;
        while now() < end {
            sleep((end - now()).min(0.05));
            self.check()?;
        }
        if let Some(t) = self.timing.as_mut() {
            let e = t.spent.entry("sleep").or_insert((0, 0.0));
            e.1 += now() - t0;
        }
        self.check()
    }

    /// Catching or running from a battle; switching off leaves the battle to you.
    pub(crate) fn acting<T>(&mut self, what: impl FnOnce(&mut Self) -> R<T>) -> R<Option<T>> {
        self.c.busy.store(true, Relaxed);
        let r = what(self);
        self.c.busy.store(false, Relaxed);
        match r {
            Ok(v) => Ok(Some(v)),
            Err(Halt::Paused) => {
                self.print(if self.c.remote() { "  taken over mid-battle" } else { "  switched off mid-battle: it's yours" });
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Paused: waits for the toggle, or the website taking over.
    pub(crate) fn wait_running(&mut self) -> R<()> {
        while !self.c.running() && !self.c.remote() {
            self.nap(0.2)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ the screen

    pub(crate) fn grab(&mut self) -> Frame {
        let f = self.spent("grab", |h| h.dev.grab());
        self.last_frame = Some((f.clone(), now()));
        f
    }

    fn keep_roblox_in_front(&mut self) {
        if self.dev.roblox_in_front() {
            return;
        }
        if !self.dev.roblox_open() {
            // closed or crashed: said once (not "bringing Roblox back" every second), you pinged
            if self.roblox_gone.is_none() {
                self.roblox_gone = Some(now());
                self.say("  (Roblox isn't open: closed or crashed? Hunting goes on once it's back)");
                self.c.discord.post("pokehunt: Roblox isn't open (closed or crashed?): hunting goes on once it's back.", None);
            }
            return;
        }
        if let Some(since) = self.roblox_gone.take() {
            self.say(&format!("  (Roblox is back, after {:.0} min)", (now() - since) / 60.0));
        }
        if now() - self.front_said > 30.0 {
            self.front_said = now();
            self.say("  (bringing Roblox back to the front)");
        }
        self.dev.focus_roblox();
    }

    pub(crate) fn click(&mut self, p: Pt) -> R<()> {
        self.click_with(p, false)
    }

    pub(crate) fn click_with(&mut self, p: Pt, right: bool) -> R<()> {
        if self.dev.fail_safe() {
            return Err(Halt::FailSafe);
        }
        self.keep_roblox_in_front();
        if self.roblox_gone.is_some() {
            return Ok(()); // (nothing to click: it'd land on whatever's on the desktop)
        }
        self.last_click = now();
        let word = if self.traits.phone { "tap" } else { "click" };
        self.c.out.debug.note(&format!("    {}{word} ({:.0}, {:.0})", if right { "right " } else { "" }, p.x, p.y));
        if self.c.out.debug.click_shot() {
            // see where the first clicks land
            let f = match &self.last_frame {
                Some((f, at)) if now() - at < 1.5 => f.clone(),
                _ => self.grab(),
            };
            let caption = format!("{word} at ({:.0}, {:.0})", p.x, p.y);
            self.c.out.debug.shot(&caption, &picture::click_closeup(&f, p, None), false);
        }
        self.spent("click_at", |h| h.dev.click(p, right));
        Ok(())
    }

    pub(crate) fn swipe(&mut self, from: Pt, to: Pt, seconds: f64) -> R<()> {
        self.check()?;
        self.last_click = now();
        self.c.out.debug.note(&format!("    swipe ({:.0}, {:.0}) to ({:.0}, {:.0})", from.x, from.y, to.x, to.y));
        self.dev.swipe(from, to, seconds);
        Ok(())
    }

    /// The mouse wheel over p: notches, up when positive (a PC's bag list).
    pub(crate) fn wheel(&mut self, p: Pt, notches: i32) -> R<()> {
        self.check()?;
        if self.dev.fail_safe() {
            return Err(Halt::FailSafe);
        }
        self.keep_roblox_in_front();
        if self.roblox_gone.is_some() {
            return Ok(());
        }
        self.last_click = now();
        self.c.out.debug.note(&format!("    wheel {notches:+} at ({:.0}, {:.0})", p.x, p.y));
        self.dev.wheel(p, notches);
        Ok(())
    }

    pub(crate) fn move_mouse(&mut self, p: Pt) -> R<()> {
        if self.dev.fail_safe() {
            return Err(Halt::FailSafe);
        }
        self.dev.move_pointer(p);
        Ok(())
    }

    /// Text read in area of f (all of it if None), its x, y as fractions of the
    /// area, enlarged first by zoom; what's under the ON / OFF button left out.
    pub(crate) fn ocr(&mut self, f: &Frame, area: Option<IBox>, zoom: f64) -> Vec<TextHit> {
        let b = area.unwrap_or_else(|| px::whole(f));
        let (x0, y0, x1, y1) = px::clip(f, b);
        if x1 - x0 < 2 || y1 - y0 < 2 {
            return Vec::new();
        }
        let mut img = crop(f, b);
        if zoom > 1.01 {
            img = resize(&img, (img.width() as f64 * zoom) as u32, (img.height() as f64 * zoom) as u32);
        }
        let texts = self.spent("ocr", |h| h.dev.ocr(&img));
        let Some(chip) = self.dev.overlay() else { return texts };
        let (w, h) = ((x1 - x0) as f64, (y1 - y0) as f64);
        texts.into_iter().filter(|t| {
            let (x, y) = ((x0 as f64 + t.x * w) as i32, (y0 as f64 + t.y * h) as i32);
            !((chip.l - 4..=chip.r + 4).contains(&x) && (chip.t - 4..=chip.b + 4).contains(&y))
        }).collect()
    }

    /// All the text on screen, at screen px.
    pub(crate) fn screen_texts(&mut self, f: &Frame) -> Vec<TextHit> {
        let (w, h) = (f.width() as f64, f.height() as f64);
        self.ocr(f, None, 1.0).into_iter().map(|t| TextHit::new(t.text.trim(), t.x * w, t.y * h)).collect()
    }

    pub(crate) fn region_text(&mut self, f: &Frame, lay: &Layout, b: IBox) -> String {
        self.ocr(f, Some(lay.boxed(b)), 1.0).iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ")
    }

    pub(crate) fn find_mute(&mut self, f: &Frame, hint: Option<IBox>) -> Option<IBox> {
        self.spent("find_mute", |_| find_mute(f, hint))
    }

    /// The saved calibration for this screen size, or the reference layout.
    fn use_calibration(&mut self, f: &Frame) {
        let key = format!("{}x{}", f.width(), f.height());
        if key == self.cal_key {
            return;
        }
        self.pending_cal = None; // (a fit of another screen size never agrees with this one's)
        let saved = self.c.store.calibration(&self.cal_file, &key);
        self.cal_saved = saved.is_some();
        self.cal = saved.unwrap_or_default();
        self.cal_key = key;
    }

    /// Another screen next (the samples, for --diagnose): its calibration looked up again.
    pub(crate) fn forget_screen(&mut self) {
        self.cal_key.clear();
        self.cal = Calibration::default();
        self.cal_saved = false;
    }

    /// The screenshot and, in an encounter, its layout.
    pub(crate) fn look(&mut self, hint: Option<IBox>) -> (Frame, Option<Layout>) {
        let f = self.grab();
        let lay = self.look_in(&f, hint);
        (f, lay)
    }

    /// A screenshot from a file: its layout, if it's an encounter.
    pub(crate) fn look_at(&mut self, f: &Frame) -> (Frame, Option<Layout>) {
        self.use_calibration(f);
        (f.clone(), self.find_mute(f, None).map(|m| Layout::new(m, &self.cal)))
    }

    fn look_in(&mut self, f: &Frame, hint: Option<IBox>) -> Option<Layout> {
        let f = f.clone();
        self.use_calibration(&f);
        let mute = self.find_mute(&f, hint);
        if self.traits.phone {
            // the pill parks right after Roblox's menu and chat buttons (top left: nothing read
            // there), once the battle screen has settled
            if mute.is_some_and(|m| menu_ready(&f, &Layout::new(m, &self.cal))) {
                if let Some(p) = vision::roblox_pill(&f) {
                    self.dev.place_overlay(p);
                }
            }
        }
        mute.map(|m| Layout::new(m, &self.cal))
    }

    /// Fits and saves the layout for this screen. True if it worked.
    pub(crate) fn auto_calibrate(&mut self, f: &Frame, mute: IBox, draw: bool) -> bool {
        let Some(new) = calibrate(f, mute) else { return false };
        // far from the saved fit (a Windows PC fitted 1.44x off one read, then 0.99x again):
        // only once a second fit agrees
        let far = |a: &Calibration, b: &Calibration| (0..2).any(|i| (a.fit[i] - b.fit[i]).abs() > 0.08 * b.fit[i].abs().max(0.1));
        if self.cal_saved && !draw && far(&new, &self.cal) {
            let agrees = self.pending_cal.as_ref().is_some_and(|p| !far(&new, p) && (0..2).all(|i| (new.fit[i] - p.fit[i]).abs() < 0.02));
            if !agrees {
                self.trace(&format!("a fit of {:.3} across, {:.3} down (the saved one's {:.3}, {:.3}): kept till another agrees, \
                                     the Mute button at ({}, {}) {}x{}", new.fit[0], new.fit[1], self.cal.fit[0], self.cal.fit[1],
                                    mute.l, mute.t, mute.w(), mute.h()));
                self.pending_cal = Some(new);
                return false;
            }
        }
        self.pending_cal = None;
        let changed = !self.cal_saved || (0..4).any(|i| (new.fit[i] - self.cal.fit[i]).abs() > 0.003);
        self.c.store.save_calibration(&self.cal_file, &self.cal_key, &new);
        self.cal = new.clone();
        self.cal_saved = true;
        let lay = Layout::new(mute, &new);
        if changed || draw {
            self.say(&format!("  calibrated for this {} screen: {:.3} across, {:.3} down, within {:.0} px",
                              self.cal_key, new.fit[0], new.fit[1], new.miss));
            let drawn = picture::draw_layout(f, &lay);
            self.c.out.debug.shot(&format!("calibrated for {}: the boxes should sit on the game's", self.cal_key),
                                  &drawn, false);
        }
        if draw {
            let drawn = png(&picture::draw_layout(f, &lay));
            let path = self.save_picture(&drawn, "debug", "calibration");
            self.say(&format!("  the layout is drawn on {path}: the boxes should sit on the game's"));
        }
        true
    }

    pub(crate) fn save_picture(&self, png: &[u8], folder: &str, name: &str) -> String {
        self.dev.save_picture(png, folder, name).unwrap_or_else(|| self.c.store.save_picture(png, folder, name))
    }

    /// Something unexpected: a screenshot kept (debug/) and sent to the debug channel.
    pub(crate) fn snapshot(&self, f: &Frame, reason: &str) -> String {
        let path = self.save_picture(&png(&picture::at_most(f.clone(), 2000)), "debug", reason);
        self.c.out.debug.shot(reason, f, true);
        path
    }

    /// Something needs a look: said, a screenshot kept, Discord told.
    pub(crate) fn trouble(&self, msg: &str, f: &Frame) {
        self.say(&format!("  ({msg}, screenshot: {})", self.snapshot(f, "trouble")));
        let shot = picture::thumbnail(f.clone(), 1200);
        self.c.discord.post(&format!("pokehunt: {msg}."), Some(png(&shot)));
    }

    // ------------------------------------------------------------ reading

    pub(crate) fn zoom(lay: &Layout) -> f64 {
        if lay.s < 1.0 { 1.0 / lay.s } else { 1.0 }
    }

    /// The enemy's species, level, shiny, tint, reskin and gradient.
    /// An encounter's reads, each, and what the box's text was on the last (the website's
    /// log only: why it was taken for what it was).
    fn log_reads(&self, reads: &[Enc]) {
        let each: Vec<String> = reads.iter().map(|e| e.describe()).collect();
        self.c.out.detail(&format!("    reads: {}; the box: {}", each.join(" / "), self.box_read));
    }

    pub(crate) fn read_enemy(&mut self, f: &Frame, lay: &Layout) -> Enc {
        let texts = self.ocr(f, Some(lay.boxed(layout::BOX)), Self::zoom(lay));
        let mut seen: Vec<String> = texts.iter().map(|t| format!("{:?}", t.text)).collect();
        let mut enc = reader::enemy(&texts);
        // the name, the first way it reads as a real Pokemon's (species.txt): the box made
        // bigger, as it is, then its row on its own (Windows: a light italic "Entei" gone from
        // the bigger box, "Mew" read "Me Ill" there and missed at all in the box)
        if !reader::known_species(&enc.species) {
            let plain = reader::enemy(&self.ocr(f, Some(lay.boxed(layout::BOX)), 1.0));
            if reader::known_species(&plain.species) || enc.species.is_empty() {
                enc.species = plain.species;
            }
            if enc.level.is_empty() {
                enc.level = plain.level;
            }
        }
        if !reader::known_species(&enc.species) {
            let row = reader::clean_name(&self.row_text(f, lay, layout::NAME_ROW, &["x2", "far"]));
            if reader::known_species(&row) || enc.species.is_empty() {
                enc.species = row;
            }
        }
        if self.traits.ocr_rows {
            // Windows' text recognition misses these two in the whole box: each is read on its own
            // (each kept only when it reads at least as much as the box did: "FLUID" alone
            // isn't "TOXIC FLUID")
            let r = self.row_text(f, lay, layout::RESKIN_ROW, &["x2", "far"]);
            seen.push(format!("reskin row {r:?}"));
            if !r.is_empty() && r.chars().count() >= enc.reskin.chars().count() {
                enc.reskin = r;
            }
            let g = self.row_text(f, lay, layout::GRADIENT_TAB, &["sat", "far"]);
            seen.push(format!("gradient row {g:?}"));
            if !g.is_empty() && g.chars().count() >= enc.gradient.chars().count() {
                enc.gradient = g;
            }
        }
        (enc.shiny, enc.tint, enc.owned) = vision::icons(f, lay);
        self.box_read = seen.join(", ");
        enc
    }

    /// One row of the enemy's box, cut out and prepared each way in turn (tried
    /// on the samples on Windows): the longest reading, since the colored
    /// letters come back partly ("Mira" for Mirage).
    ///   x2   twice the size
    ///   far  what isn't the row's background, black on white, 3x
    ///   sat  what's strongly colored, black on white, 3x
    fn row_text(&mut self, f: &Frame, lay: &Layout, r: IBox, ways: &[&str]) -> String {
        let mut c = crop(f, lay.boxed(r));
        if c.width() < 2 || c.height() < 2 {
            return String::new();
        }
        if lay.s < 1.0 {
            c = resize(&c, (c.width() as f64 / lay.s) as u32, (c.height() as f64 / lay.s) as u32);
        }
        let bg = px::median(&c, px::whole(&c), |_| true).unwrap_or([0.0; 3]);
        let mut best = String::new();
        for way in ways {
            let (prep, k, fill) = if *way == "x2" {
                (c.clone(), 2, image::Rgb(bg.map(|v| v as u8)))
            } else {
                let ink = Frame::from_fn(c.width(), c.height(), |x, y| {
                    let p = px::at(&c, x as usize, y as usize);
                    let on = if *way == "far" {
                        (0..3).map(|i| (p[i] as f64 - bg[i]).abs()).fold(0.0, f64::max) > 40.0
                    } else {
                        px::hi(p) - px::lo(p) > 50
                    };
                    let v = if on { 0 } else { 255 };
                    image::Rgb([v, v, v])
                });
                (ink, 3, image::Rgb([255, 255, 255]))
            };
            let prep = resize(&prep, prep.width() * k, prep.height() * k);
            let mut framed = Frame::from_pixel(prep.width() + 40, prep.height() + 40, fill);
            image::imageops::replace(&mut framed, &prep, 20, 20);
            let texts = self.spent("ocr", |h| h.dev.ocr(&framed));
            let text = texts.iter().map(|t| t.text.trim()).collect::<Vec<_>>().join(" ");
            let text = text.trim_matches(|ch| ch == ' ' || ch == '.').to_string();
            if reader::two_letters(&text) && text.chars().count() > best.chars().count() {
                best = text;
            }
        }
        best
    }

    /// Roblox opens its chat window when messages come in, and on a phone it
    /// covers the game (the bag's list, the enemy's box). Closes it with
    /// Roblox's chat button. True if it clicked.
    pub(crate) fn close_chat(&mut self, f: &Frame, texts: &[TextHit]) -> R<bool> {
        if now() - self.chat_closed_at < 3.0 {
            return Ok(false);
        }
        // its button says whether it's open (filled) or not (an outline); without it, its words
        let open = chat_open(f).unwrap_or_else(|| texts.iter().any(|t| t.text.to_lowercase().contains("tap here to chat")));
        if !open {
            return Ok(false);
        }
        self.chat_closed_at = now();
        let Some(button) = vision::chat_button(f) else {
            self.trace("the chat window is open over the game, and Roblox's chat button wasn't found");
            return Ok(false);
        };
        self.trace("the chat window is open over the game: closing it");
        self.click(button)?;
        Ok(true)
    }

    /// Waits for the Fight/Run menu and reads the enemy until the reads agree
    /// (settled). None once the encounter is over. With clicks it may click, to
    /// close the chat when that's in the way. The layout is fitted on the first
    /// encounter on this screen, when the battle can't be read for a few
    /// seconds, or (draw) to draw it for you to check.
    pub(crate) fn read_encounter(&mut self, hint: IBox, clicks: bool, mut draw: bool) -> R<Option<Read>> {
        let mut hint = hint;
        let (mut reads, mut gone, start, mut saved): (Vec<Enc>, Option<f64>, f64, bool) = (Vec::new(), None, now(), false);
        let mut nameless: Option<f64> = None; // since when the reads have had no name
        let (mut chat_click, mut chat_clicks, mut cal_at) = (0.0, 0, 0.0);
        loop {
            let (f, lay) = self.look(Some(hint));
            let Some(lay) = lay else {
                let g = *gone.get_or_insert(now());
                if now() - g > GONE_AFTER {
                    return Ok(None);
                }
                self.nap(0.1)?;
                continue;
            };
            gone = None;
            hint = lay.mute;
            if clicks {
                self.keep_roblox_in_front(); // a popup may be covering the enemy's box
            }
            let menu = menu_ready(&f, &lay);
            let is_open = chat_open(&f);
            // its lines over the game; not with its button saying it's closed (the mouse
            // pointer in the margin looks like a line)
            let chat = chat_showing(&f, &lay) && is_open != Some(false);
            // Its button says whether chat's open (filled) or not (an outline): closed till it
            // says so. Without the button only guessed at from chat lines, and two clicks at most
            // then (it toggles: one more could open it again).
            let t = now();
            let close = match is_open {
                Some(true) => t - chat_click > 2.5 && chat_clicks < 4,
                Some(false) => false,
                None => chat && t - chat_click > 5.0 && chat_clicks < 2,
            };
            if clicks && close {
                self.say(&format!("  closing the chat{}", if chat { ", it covers the enemy's box" } else { "" }));
                let at = vision::chat_button(&f).unwrap_or(lay.pt(layout::CHAT_BUTTON));
                self.click(at)?;
                self.move_mouse(lay.pt(layout::IDLE_SPOT))?;
                (chat_click, chat_clicks) = (now(), chat_clicks + 1);
            }
            if clicks && chat && now() - chat_click > 2.5 {
                self.move_mouse(lay.pt(layout::IDLE_SPOT))?; // (the pointer out of the margin, in case it's that)
            }
            let readable = menu && !chat && box_visible(&f, &lay);
            if self.timing.is_some() {
                self.trace(&format!("read: {}{}{}", if menu { "menu" } else { "no menu yet" }, if chat { ", chat" } else { "" },
                                    if readable || !menu { "" } else { ", box covered" }));
            }
            if now() - cal_at > 2.0 && (draw || !self.cal_saved || (!readable && now() - start > 3.0)) {
                cal_at = now();
                if self.auto_calibrate(&f, lay.mute, draw) {
                    draw = false;
                    continue; // read again with the new layout
                }
            }
            if readable {
                let enc = self.read_enemy(&f, &lay);
                reads.push(enc);
                if reads.len() > 8 {
                    reads.remove(0);
                }
                let got = settled(&reads);
                if self.timing.is_some() {
                    let d = reads.last().unwrap().describe();
                    self.trace(&format!("read: {d}{}", if got.is_some() { " (agrees: done)" } else { "" }));
                }
                if let Some(enc) = got {
                    self.log_reads(&reads);
                    return Ok(Some(Read { enc, f, lay }));
                }
                // a name that never reads (missed every way, every time) doesn't hold it up: after
                // a few seconds the rest decides (a reskin and a gradient is the bar for any
                // Pokemon; anything a rule might name is held for you)
                if reads.last().is_some_and(|r| r.species.is_empty()) {
                    let since = *nameless.get_or_insert(now());
                    if now() - since > 4.0 {
                        if let Some(enc) = rules::settled_nameless(&reads) {
                            self.say(&format!("  (the name won't read: going by the rest, {})", enc.describe()));
                            self.log_reads(&reads);
                            return Ok(Some(Read { enc, f, lay }));
                        }
                    }
                } else {
                    nameless = None;
                }
            }
            if now() - start > 15.0 && !saved {
                // never Run from something unread: it might be the one
                let why = if chat { "chat is covering the enemy's box" } else if menu { "can't read the enemy's box" } else { "no Fight/Run menu" };
                if menu && !chat {
                    // what the text recognition saw there, and how it was read: to tell why from
                    // the log (sent with the trouble), not by trying the picture on a build
                    let texts = self.ocr(&f, Some(lay.boxed(layout::BOX)), Self::zoom(&lay)); // (as read_enemy reads it)
                    let tried: Vec<String> = reads.iter().rev().take(3).map(|e| e.describe()).collect();
                    self.say(&format!("  (the box{}: {}; read as: {})",
                                      if readable { "" } else { ", not quite in sight" },
                                      texts.iter().map(|t| format!("{:?} at ({:.2}, {:.2})", t.text, t.x, t.y))
                                          .collect::<Vec<_>>().join(", "),
                                      if tried.is_empty() { "nothing yet".into() } else { tried.join(" / ") }));
                }
                if !menu {
                    // where it looked for the menu, and what it found there: to tell why from the log
                    let seen: Vec<String> = layout::BUTTONS.iter().map(|(name, area, _)| {
                        let m = px::median(&f, lay.boxed(*area), |_| true).unwrap_or([0.0; 3]);
                        format!("{name} {:.0},{:.0},{:.0}", m[0], m[1], m[2])
                    }).collect();
                    self.say(&format!("  (the Mute button at ({}, {}) {}x{}, fit {:.3} x {:.3}; the menu's colors: {})",
                                      lay.mute.l, lay.mute.t, lay.mute.w(), lay.mute.h(), lay.cal.fit[0], lay.cal.fit[1],
                                      seen.join("; ")));
                }
                self.trouble(&format!("{why} for 15 s, waiting"), &f);
                saved = true;
            }
            if clicks && now() - self.last_click > AFK_EVERY {
                // waiting for you: Roblox kicks at 20 min
                self.click(lay.pt(layout::AFK_SPOT))?;
                self.say("  (anti-AFK click)");
            }
            if !readable {
                self.nap(0.1)?; // (a read again at once: the next frame's, to agree with)
            }
        }
    }

    /// Clicks Run (again if the escape failed) until the encounter is over
    /// (gone_after s without the Mute button). False when the website takes
    /// over, unless it's the website's Run.
    pub(crate) fn run_away(&mut self, lay: &Layout, for_website: bool, gone_after: f64) -> R<bool> {
        let (mut lay, mut clicked, mut gone, start) = (lay.clone(), 0.0, None::<f64>, now());
        let mut last = None;
        while now() - start < 20.0 {
            self.wait_running()?;
            if self.c.remote() && !for_website {
                return Ok(false);
            }
            let (f, cur) = self.look(Some(lay.mute));
            match cur {
                None => {
                    let g = *gone.get_or_insert(now());
                    if now() - g > gone_after {
                        return Ok(true);
                    }
                }
                Some(cur) => {
                    gone = None;
                    lay = cur;
                    if menu_ready(&f, &lay) && now() - clicked > 2.5 {
                        if clicked > 0.0 {
                            self.say("  run failed, trying again");
                        }
                        self.click(lay.pt(layout::RUN))?;
                        self.move_mouse(lay.pt(layout::IDLE_SPOT))?; // don't light up Run
                        clicked = now();
                    }
                }
            }
            last = Some(f);
            self.nap(0.15)?;
        }
        if let Some(f) = last {
            self.trouble("still in the encounter 20 s after clicking Run", &f);
        }
        Ok(false)
    }

    /// The wild one fainted: its battle's last words (experience, a level, a move to
    /// learn: "No") let go by till the battle's gone. False, with a screenshot, if it's
    /// still there after 30 s (it sat a minute on Giratina's, nothing said what for).
    pub(crate) fn finish_battle(&mut self, lay: &Layout) -> R<bool> {
        let (end, mut gone, mut clicked) = (now() + 30.0, None::<f64>, 0.0);
        let mut last = None;
        while now() < end {
            let f = self.grab();
            if self.find_mute(&f, Some(lay.mute)).is_none() {
                let g = *gone.get_or_insert(now());
                if now() - g > GONE_AFTER {
                    return Ok(true);
                }
                self.nap(0.2)?;
                continue;
            }
            gone = None;
            if now() - clicked > 1.5 {
                let texts = self.screen_texts(&f);
                let no = texts.iter().find(|t| {
                    let l = norm(&t.text);
                    ["no", "cancel", "skip", "dontlearn", "keepoldmoves"].contains(&l.as_str())
                });
                let at = match no {
                    Some(t) => {
                        self.trace(&format!("after the battle: \"{}\"", t.text));
                        t.pt()
                    }
                    None => lay.xy(500.0, 952.0), // the message box's left: on with its words (a Yes sits right)
                };
                self.click(at)?;
                clicked = now();
            }
            last = Some(f);
            self.nap(0.3)?;
        }
        if let Some(f) = last {
            self.trouble("the battle didn't end after the wild one fainted", &f);
        }
        Ok(false)
    }

    pub(crate) fn wait_gone(&mut self, lay: &Layout) -> R<()> {
        let mut gone = None::<f64>;
        loop {
            let f = self.grab();
            if self.find_mute(&f, Some(lay.mute)).is_some() {
                gone = None;
            } else {
                let g = *gone.get_or_insert(now());
                if now() - g > GONE_AFTER {
                    return Ok(());
                }
            }
            self.nap(0.2)?;
        }
    }

    /// Clicks an empty spot every few seconds until you're back (the mouse
    /// moved, a key pressed, the screen touched) or the encounter is over.
    /// Then pauses the hunt.
    pub(crate) fn keep_alive(&mut self, lay: &Layout) -> R<()> {
        let every = self.c.cfg.number("keepalive_seconds", 5.0);
        let since = now();
        let doing = if self.traits.phone { "tapping" } else { "clicking" };
        'alive: while self.c.running() && !self.c.remote() {
            self.click(lay.pt(layout::IDLE_SPOT))?;
            let parked = self.dev.pointer();
            let until = now() + every;
            while now() < until {
                self.nap(0.1)?;
                if self.c.remote() || !self.c.running() {
                    break 'alive;
                }
                let moved = match (parked, self.dev.pointer()) {
                    (Some(a), Some(b)) => (a.x - b.x).abs() + (a.y - b.y).abs() > 4.0,
                    _ => false,
                };
                let touched = self.dev.idle().is_some_and(|idle| idle < now() - since);
                if moved || touched {
                    self.print(&format!("  you're back: stopped {doing}."));
                    break 'alive;
                }
            }
            let mut gone = 0;
            while gone < 3 {
                let f = self.grab();
                if self.find_mute(&f, Some(lay.mute)).is_some() {
                    break;
                }
                gone += 1;
                self.nap(0.4)?;
            }
            if gone < 3 {
                continue;
            }
            self.print("  the encounter ended.");
            break;
        }
        if self.c.running() && !self.c.remote() {
            self.c.running.store(false, Relaxed);
            self.dev.sound(Sound::Paused);
            self.print(&format!("|| paused. {} to hunt again.", self.traits.toggle_key));
            self.c.publish();
        }
        Ok(())
    }

    pub(crate) fn log_encounter(&self, enc: &Enc, action: &str) {
        self.c.store.log_encounter(&[enc.species.clone(), enc.level.clone(), (enc.shiny as u8).to_string(),
                                     (enc.tint as u8).to_string(), enc.reskin.clone(), enc.gradient.clone(), action.into()]);
    }

    /// A wanted one: kept in finds/, Discord pinged, a sound.
    pub(crate) fn found(&mut self, enc: &Enc, f: &Frame, lay: &Layout) -> R<()> {
        let shot = png(&game_shot(f, lay));
        self.save_picture(&shot, "finds", &norm(&enc.species));
        self.c.discord.post(format!("{} {}", self.c.cfg.ping, enc.title()).trim(), Some(shot));
        self.c.out.debug.shot(&format!("found: {}", enc.title()), &game_shot(f, lay), false);
        self.dev.sound(Sound::Found);
        Ok(())
    }

    // ------------------------------------------------------------ the website

    pub(crate) fn link(&self) -> Option<Arc<Link>> {
        self.c.link.lock().unwrap().clone()
    }

    /// The wild Pokemon's sprite as the game draws it (shiny, gradient, reskin
    /// and all), for the website: the sprite's 96 x 96 pixels, background and
    /// all; the page cuts it out with the species' shape.
    fn send_sprite(&self, f: &Frame, lay: &Layout, enc: &Enc) {
        let Some(link) = self.link() else { return };
        let count = self.c.web.lock().unwrap().encounters;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png(&vision::sprite(f, lay)));
        link.send_sprite(json!({"count": count, "species": enc.species, "png": b64}));
    }

    pub(crate) fn send_shot(&self, f: &Frame, lay: &Layout) {
        if let Some(link) = self.link() {
            let shot = picture::thumbnail(crop(f, lay.boxed(layout::PANEL)), 960);
            link.send_shot(picture::jpg(&shot, 70));
        }
    }

    // ------------------------------------------------------------ the hunt

    /// Runs until stopped; an unexpected error restarts the hunt, never ends it.
    pub fn run(mut self) {
        loop {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.hunt()));
            self.c.busy.store(false, Relaxed);
            let what = match r {
                Ok(Err(Halt::Stopped)) | Ok(Ok(())) => {
                    self.print("\nstopped.");
                    break;
                }
                Ok(Err(Halt::FailSafe)) => {
                    self.print("\nstopped (mouse flung into a corner).");
                    break;
                }
                Ok(Err(e)) => format!("{e:?} (unexpected here)"),
                Err(panic) => panic.downcast_ref::<&str>().map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_else(|| "a crash".into()),
            };
            if self.c.stopped() {
                self.print("\nstopped.");
                break;
            }
            self.c.store.error(&what);
            self.print(&format!("\n! {what} (restarting, details in debug/errors.log)"));
            self.c.discord.post(&format!("pokehunt hit an error and restarted: {what}"), None);
            if self.nap(5.0).is_err() {
                break;
            }
        }
        self.c.out.debug.flush();
        self.c.stop();
        self.c.publish();
    }

    fn hunt(&mut self) -> R<()> {
        let c = self.c.clone();
        let watch = c.opts.watch;
        let mut hint: Option<IBox> = None;
        let mut since: Option<f64> = None;
        let mut last_battle: Option<Enc> = None; // the battle on screen, once read
        let (mut last_enc, mut last_status, mut quiet_said) = (now(), now(), false);
        let (mut save_due, mut last_save, mut last_check) = (false, 0.0, 0.0);
        let (mut first_look, mut last_seen) = (true, now());
        let key = self.traits.toggle_key;
        // A new install (no debug run gone right, no encounter ever): the first hunt's first
        // encounter is a debug run, as on play's right click, to see everything works here
        // Also a hunt that hasn't learned the bag and fight menu yet (an install from before the
        // positions were cached): its debug run is what finds them (menu.json).
        let learned = Store::read_json(&c.store.path("menu.json")).values()
            .any(|v| v.as_object().is_some_and(|o| o.keys().any(|k| k.starts_with("fav:"))));
        let fresh = !c.store.path("debug-run.json").exists() && !c.store.path("encounters.csv").exists();
        if c.opts.window && (fresh || !learned) {
            c.debug_run.store(true, Relaxed);
            c.fit_next.store(true, Relaxed);
            c.update_loud();
            self.print(&format!("  ({}: the first encounter is a debug run, to {})",
                                if fresh { "first time here" } else { "the bag's and the moves' spots aren't known yet" },
                                if fresh { "check everything works" } else { "learn them" }));
        }
        if !c.running() {
            self.print(&format!("pokehunt paused: {key} to start."));
        } else {
            if !watch {
                self.dev.focus_roblox();
            }
            self.print(if watch { "pokehunt watching (no clicks)." } else { "pokehunt hunting." });
        }
        for rule in &c.cfg.targets {
            self.print(&format!("  wanted: {}", rules::show(rule)));
        }
        for rule in self.notify.clone() {
            self.print(&format!("  auto-catch: {}", rules::show(&rule)));
        }
        if c.opts.debug {
            self.print("  debug mode: catching every encounter, then saving; a screenshot every catch turn and click");
            c.out.debug.every_shot.store(true, Relaxed);
        }
        for rule in &c.cfg.post {
            self.print(&format!("  post only: {}", rules::show(rule)));
        }
        if !self.traits.phone {
            self.print(&format!("  {key}  pause / resume"));
            if c.linked() {
                self.print("  Ctrl+Shift+N  new website code    Ctrl+Shift+L  log every browser out");
            }
        }
        // started in the middle of a battle: that one's yours (the hunter would act on it at once).
        // A phone's first pictures can be black for a few seconds: wait for real ones, then 2 s.
        if c.running() && !watch && !c.opts.carried {
            let until = now() + if self.traits.phone { 10.0 } else { 0.0 };
            let mut lit: Option<f64> = None;
            loop {
                let (f, lay) = self.look(None);
                if lay.is_some() {
                    c.running.store(false, Relaxed);
                    self.print(&format!("  a battle is on screen, so pokehunt is paused: {key} when it should carry on \
                                         (or take over on the website)"));
                    self.dev.sound(Sound::Paused);
                    break;
                }
                if lit.is_none() && vision::brightness(&f) > 0 {
                    lit = Some(now());
                }
                if now() >= until || lit.is_some_and(|l| now() - l >= 2.0) {
                    break;
                }
                self.nap(0.3)?;
            }
        }
        c.publish(); // the website sees it at once, not at the first encounter
        // its first read is slow (~0.5 s): not at the first encounter
        let _ = self.dev.ocr(&Frame::from_pixel(160, 48, image::Rgb([255, 255, 255])));
        loop {
            if !c.running() && !c.remote() {
                self.wait_running()?;
                if !watch {
                    self.dev.focus_roblox();
                }
                since = None;
                (last_enc, last_status) = (now(), now());
            }
            let (f, lay) = self.look(hint);
            if first_look && self.traits.phone {
                // for checking what it sees
                first_look = false;
                let path = self.save_picture(&png(&picture::at_most(f.clone(), 1200)), "debug", "first-look");
                self.print(&format!("  sees a {}x{} screen, {}% bright: {path}", f.width(), f.height(), vision::brightness(&f)));
            }
            if lay.is_some() {
                last_seen = now();
            } else if self.traits.phone && now() - last_seen > 30.0 {
                last_seen = now();
                self.print(&format!("  ({}: no Mute button in sight on the {}x{} screen, {}% bright)", local("%H:%M:%S"),
                                    f.width(), f.height(), vision::brightness(&f)));
            }
            if now() - last_status > 60.0 * c.cfg.number("status_every_minutes", 60.0) {
                let n = c.web.lock().unwrap().encounters;
                c.discord.post(&format!("pokehunt: still hunting, {n} encounters so far."), None);
                last_status = now();
            }
            let Some(lay) = lay else {
                since = None;
                last_battle = None;
                if self.timing_is_times {
                    self.timing = None;
                    self.timing_is_times = false;
                }
                let phase_changed = {
                    let mut web = c.web.lock().unwrap();
                    let changed = web.phase != "waiting";
                    web.phase = "waiting".into();
                    changed
                };
                if phase_changed {
                    c.publish();
                }
                if c.between.load(Relaxed) && !c.remote() && !save_due {
                    return Err(Halt::Stopped);
                }
                if save_due && !watch && now() - last_save > SAVE_EVERY {
                    self.save_wait = 0;
                    if self.save_game(0.0)? {
                        self.say("  (game saved)");
                        save_due = false;
                        last_save = now();
                    } else {
                        // try again in 20 s, or when the game said it can
                        if self.save_wait > 0 {
                            self.say(&format!("  (the game says wait {} s to save)", self.save_wait));
                        }
                        last_save = now() - SAVE_EVERY + f64::max(20.0, self.save_wait as f64 + 5.0);
                    }
                    continue;
                }
                self.say("waiting for an encounter...");
                if !watch {
                    self.keep_roblox_in_front(); // a popup may be hiding the encounter
                }
                if !watch && now() - last_enc > 10.0 && now() - last_check > 10.0 {
                    last_check = now();
                    if self.clear_leftovers()? {
                        self.say("  (closed a save message / menu left open)");
                    }
                }
                if let (Some(h), false) = (hint, watch) {
                    if now() - self.last_click > AFK_EVERY {
                        let at = Layout::new(h, &self.cal).pt(layout::AFK_SPOT);
                        self.click(at)?;
                        self.say("  (anti-AFK click)");
                    }
                }
                let quiet = now() - last_enc;
                if quiet > 60.0 * c.cfg.number("quiet_alert_minutes", 3.0) && !quiet_said {
                    self.trouble(&format!("no encounter for {:.0} min", quiet / 60.0), &f);
                    quiet_said = true;
                }
                self.nap(if self.traits.phone { 0.2 } else { 0.1 })?; // (an encounter's seen within ~0.15 s)
                continue;
            };
            hint = Some(lay.mute);
            if since.is_none() && c.opts.times && self.timing.is_none() {
                self.timing = Some(Timing::new()); // this encounter's time, from its Mute button on
                self.timing_is_times = true;
                self.trace("Mute seen");
            }
            let seen = *since.get_or_insert(now());
            if now() - seen < self.traits.seen_after {
                self.nap(0.1)?;
                continue;
            }

            let draw = self.calibrate_next || c.fit_next.load(Relaxed);
            let got = self.read_encounter(lay.mute, !watch, draw)?;
            self.calibrate_next = false;
            c.fit_next.store(false, Relaxed);
            since = None;
            let read_lap = if self.timing_is_times { self.timing.as_mut().map(|t| t.lap()) } else { None };
            let Some(Read { enc, f, lay }) = got else {
                last_battle = None;
                continue;
            };
            // the same battle read again: handed back from the website, or resumed mid-battle
            let again = last_battle.as_ref().is_some_and(|b| same_mon(b, &enc));
            last_battle = Some(enc.clone());
            if !again {
                c.web.lock().unwrap().encounters += 1;
                (last_enc, quiet_said) = (now(), false);
            }
            let mut rule = wanted(&enc, &c.cfg.targets).cloned();
            // Automatic catches require both independently OCR-read labels. The visual chip
            // heuristic can false-positive on ordinary encounters, so it must not cause catches.
            let auto_rule = crate::config::auto_catch();
            let auto_catch = self.notify.contains(&auto_rule) && rules::has_reskin_gradient(&enc, false);
            // Keep other explicit notification rules (including debug mode) working as before.
            let explicit_catch = wanted(&enc, &self.notify).is_some_and(|matched| matched != &auto_rule);
            let catch_it = auto_catch || explicit_catch;
            let special = enc.shiny || !enc.reskin.is_empty() || !enc.gradient.is_empty();
            if enc.species.is_empty() && rule.is_none() && !catch_it && special
                && (rules::might_want(&enc, &c.cfg.targets) || rules::might_want(&enc, &self.notify))
            {
                // its name never read, and a rule naming a species might want it: held for you, not run from
                self.say("  (its name won't read, and a rule might want it: held for you)");
                rule = Some(serde_json::json!({"species": "?"}).as_object().unwrap().clone());
            }
            c.out.reset_say();
            {
                let mut web = c.web.lock().unwrap();
                web.phase = "battle".into();
                web.enc = Some(enc.clone());
                web.wanted = rule.is_some();
                web.msg.clear();
                web.error.clear();
            }
            if !again {
                self.send_sprite(&f, &lay, &enc);
            }
            let practice = c.test_catch.load(Relaxed) > 0;
            let action = if c.remote() { "website" } else if rule.is_some() { "WANTED!" } else if watch { "-" }
                         else if practice { "practice" } else if catch_it { "catch" } else { "run" };
            if !again {
                let n = c.web.lock().unwrap().encounters;
                self.print(&format!("{}  #{n:<4} {:<55} {action}", local("%H:%M:%S"), enc.describe()));
            }
            c.publish();
            if c.remote() {
                if !again {
                    self.log_encounter(&enc, if rule.is_some() { "found" } else { "website" });
                    if rule.is_some() {
                        self.found(&enc, &f, &lay)?; // you may not be looking at the page
                    }
                }
                if self.remote_battle(&enc, &f, &lay)? {
                    save_due = c.cfg.boolean("save_after_catch", true);
                }
            } else if rule.is_some() {
                if !again {
                    self.log_encounter(&enc, "found");
                    self.found(&enc, &f, &lay)?;
                }
                if !watch { self.keep_alive(&lay)? } else { self.wait_gone(&lay)? }
            } else if watch {
                if !again {
                    self.log_encounter(&enc, "seen");
                }
                self.wait_gone(&lay)?;
            } else if c.debug_run.load(Relaxed) {
                self.timing = Some(Timing::new()); // (HUNT_TIMES': the debug run has its own)
                self.timing_is_times = false;
                let ok = self.acting(|h| h.debug_workflow(&enc, &f, &lay))?.unwrap_or(false);
                let took = self.timing.take().map(|t| t.at()).unwrap_or(0.0);
                self.print(&format!("  (the debug run took {took:.1} s)"));
                c.debug_run.store(false, Relaxed);
                c.update_loud();
                if ok {
                    self.print("  (the debug run went right: hunting on)");
                    let _ = std::fs::write(c.store.path("debug-run.json"),
                                           json!({"ok": local("%Y-%m-%d %H:%M:%S")}).to_string());
                } else if c.running() {
                    self.print("  (the debug run had a step go wrong: paused, see above)");
                    c.toggle();
                }
                c.publish(); // the window's Debug mode turns off
                last_save = now(); // it saved
            } else if practice || catch_it {
                // worth catching, not stopping
                if practice {
                    c.test_catch.fetch_sub(1, Relaxed);
                }
                let Some((result, details)) = self.acting(|h| h.catch(&enc, &f, &lay, !practice))? else {
                    continue; // read again: the website battles it, or it's yours
                };
                if result == "taken over" {
                    continue;
                }
                if result == "fainted" {
                    // nothing to hold: it's gone (poisoned by the lead's ability, say)
                    self.print(&format!("  catch: fainted ({details})"));
                    self.log_encounter(&enc, "fainted");
                    if !practice {
                        c.discord.post(&format!("Lost: {} fainted before it was caught ({details}).", enc.title()),
                                       Some(png(&game_shot(&f, &lay))));
                    }
                    self.acting(|h| h.finish_battle(&lay))?;
                    continue;
                }
                self.print(&format!("  catch: {result} ({details})"));
                self.log_encounter(&enc, &result);
                if result == "caught" {
                    save_due = c.cfg.boolean("save_after_catch", true);
                    if !practice {
                        c.discord.post(&format!("Caught: {} ({details})", enc.title()), Some(png(&game_shot(&f, &lay))));
                    }
                    continue;
                }
                // couldn't do it alone (the bag, a turn that never ended, or out of balls with
                // on_fail "hold"): never run from it, ping, hold the battle for you
                if matches!(result.as_str(), "gave up" | "no bag" | "timeout") && !practice
                    && (result != "gave up" || c.cfg.str("on_fail") == "hold")
                {
                    self.found(&enc, &f, &lay)?;
                    self.keep_alive(&lay)?;
                    continue;
                }
                if !practice {
                    let said = if result == "over" {
                        // gone mid-catch with no word of a catch read: it may well be caught
                        format!("The battle ended mid-catch ({details}) with no catch message read: {} may have been \
                                 caught, check your box.", enc.title())
                    } else {
                        format!("Couldn't catch ({result}, {details}), ran from: {}", enc.title())
                    };
                    c.discord.post(&said, Some(png(&game_shot(&f, &lay))));
                }
                let now_f = self.grab();
                if self.find_mute(&now_f, Some(lay.mute)).is_some() {
                    self.acting(|h| h.run_away(&lay, false, 0.5))?;
                }
            } else {
                if !again && wanted(&enc, &c.cfg.post).is_some() {
                    // worth a look, not a ball
                    self.say("  (posted to Discord)");
                    c.discord.post(&format!("Seen: {} (ran)", enc.title()), Some(png(&game_shot(&f, &lay))));
                }
                if !again {
                    self.log_encounter(&enc, "ran");
                }
                if let (Some(lap), Some(t)) = (read_lap, self.timing.as_mut()) {
                    let rest = t.lap();
                    self.print(&format!("    [seen and read: {lap}]\n    [the rest, to Run: {rest}]"));
                }
                self.acting(|h| h.run_away(&lay, false, 0.5))?;
                if self.timing_is_times {
                    if let Some(t) = self.timing.as_mut() {
                        let (lap, at) = (t.lap(), t.at());
                        self.print(&format!("    [run, till it's gone: {lap}]\n    [{at:.2} s from its Mute button]"));
                    }
                }
            }
            if self.timing_is_times {
                self.timing = None;
                self.timing_is_times = false;
            }
        }
    }
}
