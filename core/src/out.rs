//! What the hunter says: to the console, the window's hunt.log or the app's
//! screen (a Sink), and to the debug channel: the hunt's log on the website.

use crate::picture;
use crate::px::Frame;
use crate::util::{clip, random_hex, sleep};
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

/// Where the hunter's words go.
pub trait Sink: Send + Sync {
    /// A line the hunter says (it may be blank).
    fn line(&self, text: &str);
    /// The website's login code (None once it's spent). Never logged: it would
    /// reach the debug channel.
    fn code(&self, _code: Option<&str>) {}
    /// What the hunter's doing, for a window: {"running", "remote", "count",
    /// "phase", "wanted", "enc", "test"}.
    fn state(&self, _state: &Value) {}
}

/// To stdout (a Terminal's hunter, --diagnose, --image).
pub struct Console;

impl Sink for Console {
    fn line(&self, text: &str) {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
    fn code(&self, code: Option<&str>) {
        if let Some(code) = code {
            self.line(&format!("\n  Website code: {code}   (type it on {}/app)\n", crate::web::site()));
        }
    }
}

pub struct Out {
    pub sink: Arc<dyn Sink>,
    pub debug: Arc<DebugChannel>,
    said: Mutex<Option<String>>,
    lines: Mutex<VecDeque<String>>,
}

impl Out {
    pub fn new(sink: Arc<dyn Sink>, debug: Arc<DebugChannel>) -> Out {
        Out { sink, debug, said: Mutex::new(None), lines: Mutex::new(VecDeque::new()) }
    }

    pub fn print(&self, msg: &str) {
        for line in msg.split('\n') {
            self.sink.line(line);
            if line.trim().is_empty() {
                continue;
            }
            self.debug.note(line);
            if !["  wanted:", "  auto-catch:", "  post only:"].iter().any(|p| line.starts_with(p)) {
                let mut lines = self.lines.lock().unwrap();
                lines.push_back(line.to_string());
                while lines.len() > 8 {
                    lines.pop_front();
                }
            }
        }
    }

    /// Print, but don't repeat the same waiting message every loop.
    pub fn say(&self, msg: &str) {
        let mut said = self.said.lock().unwrap();
        if said.as_deref() != Some(msg) {
            *said = Some(msg.to_string());
            drop(said);
            self.print(msg);
        }
    }

    /// A line for the website's log only (how an encounter was read).
    pub fn detail(&self, msg: &str) {
        self.debug.detail(msg);
    }

    pub fn reset_say(&self) {
        *self.said.lock().unwrap() = None;
    }

    /// The last lines said, for the website.
    pub fn recent(&self) -> Vec<String> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }
}

// ---------------------------------------------------------------- the debug channel

/// a line saying something went wrong (the hunter's "trouble" always comes with a screenshot)
static FAILURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)FAILED|went wrong|couldn't|could not|can't|didn't|failed|stuck|gave up|giving up|error|exception|traceback|screenshot:|crash").unwrap()
});

/// a Discord webhook's address (an error can say it): never in the log
static WEBHOOK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)https?://[\w.]*discord(app)?\.com/api/webhooks/[\w/-]*").unwrap());

const SHOTS: usize = 25; // screenshots per run at most (besides crashes)
const CLICK_SHOTS: usize = 10; // a debug run's first clicks get a close-up each
const KEPT: usize = 5000; // lines held while the website can't be reached

/// The hunt's log, to the website (poopihunt.xyz/app/logs/), for following a run on
/// someone else's PC or phone: every line, each encounter's reads, what went wrong
/// with its picture, crashes. Sent every few seconds with the device's key (the
/// website code is never a line: Sink::code). Pictures of what went wrong, and in a
/// debug run (and --debug, --diagnose) of every step.
pub struct DebugChannel {
    key: Mutex<String>,
    run: String,
    who: String,
    site: String,
    lines: Mutex<VecDeque<Value>>,
    /// A debug run: a picture of every step.
    pub loud: AtomicBool,
    /// --debug: a picture of every click, not just the first few.
    pub every_shot: AtomicBool,
    shots: AtomicUsize,
    clicks: AtomicUsize,
    started: AtomicBool,
}

fn stamp() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl DebugChannel {
    /// key: the device's website key ("" for none: off); who: the system and build
    /// ("Windows 11 · 23 Oct 17:53").
    pub fn new(key: &str, who: &str) -> Arc<DebugChannel> {
        let d = Arc::new(DebugChannel {
            key: Mutex::new(key.trim().to_string()),
            run: random_hex(2),
            who: who.to_string(),
            site: crate::web::site(),
            lines: Mutex::new(VecDeque::new()),
            loud: AtomicBool::new(false),
            every_shot: AtomicBool::new(false),
            shots: AtomicUsize::new(0),
            clicks: AtomicUsize::new(0),
            started: AtomicBool::new(false),
        });
        d.start();
        d
    }

    pub fn off() -> Arc<DebugChannel> {
        DebugChannel::new("", "")
    }

    pub fn is_on(&self) -> bool {
        !self.key.lock().unwrap().is_empty()
    }

    /// The computer's details, the run's first line.
    pub fn set_header(&self, header: &str) {
        self.add("note", header);
    }

    fn loud(&self) -> bool {
        self.loud.load(Ordering::Relaxed)
    }

    fn start(self: &Arc<Self>) {
        if !self.is_on() || self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("debug channel".into()).spawn(move || loop {
            std::thread::sleep(Duration::from_secs(4));
            match me.upgrade() {
                Some(d) => d.flush(),
                None => break,
            }
        });
        // anything that ends the run
        let me = Arc::downgrade(self);
        let old = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(d) = me.upgrade() {
                let at = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
                let what = info.payload().downcast_ref::<&str>().map(|s| s.to_string())
                    .or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
                d.post(&format!("crashed: {}{at}", clip(&what, 1700)), None);
            }
            old(info);
        }));
    }

    fn add(&self, kind: &str, text: &str) {
        if !self.is_on() {
            return;
        }
        let mut lines = self.lines.lock().unwrap();
        lines.push_back(json!([stamp(), kind, clip(&WEBHOOK.replace_all(text, "[webhook]"), 2000)]));
        while lines.len() > KEPT {
            lines.pop_front();
        }
    }

    /// A line the hunter said ("problem" when it says something went wrong).
    pub fn note(&self, text: &str) {
        // (what the battle said is the game's: "But it failed!" is no problem of the hunter's)
        let own = text.split(". The battle said:").next().unwrap_or(text);
        self.add(if FAILURE.is_match(own) { "problem" } else { "line" }, text);
    }

    /// A line for the log only (not the hunter's own): how an encounter was read.
    pub fn detail(&self, text: &str) {
        self.add("detail", text);
    }

    /// What's gathered, to the website now (the caller's thread waits); kept for
    /// next time if it can't be reached.
    pub fn flush(&self) {
        let batch: Vec<Value> = {
            let mut lines = self.lines.lock().unwrap();
            let n = lines.len().min(500);
            lines.drain(..n).collect()
        };
        if batch.is_empty() {
            return;
        }
        let body = json!({"run": self.run, "who": self.who, "lines": batch});
        if !self.send("/api/device/log", &body) {
            let mut lines = self.lines.lock().unwrap();
            for l in batch.into_iter().rev() {
                lines.push_front(l);
            }
            while lines.len() > KEPT {
                lines.pop_back();
            }
        }
    }

    /// A screenshot with a caption, up to 25 a run: of a failure, or anything in a debug run.
    pub fn shot(self: &Arc<Self>, caption: &str, img: &Frame, failure: bool) {
        if !self.is_on() || !(failure || self.loud()) {
            return;
        }
        if self.shots.fetch_add(1, Ordering::SeqCst) >= SHOTS && !self.every_shot.load(Ordering::Relaxed) {
            return;
        }
        let small = picture::at_most(img.clone(), 1280);
        let (me, caption, at) = (self.clone(), caption.to_string(), stamp());
        std::thread::spawn(move || me.send_shot(at, &caption, &picture::jpg(&small, 80)));
    }

    fn send_shot(&self, at: u64, caption: &str, jpg: &[u8]) {
        let b64 = base64::engine::general_purpose::STANDARD.encode(jpg);
        let caption = clip(&WEBHOOK.replace_all(caption, "[webhook]"), 300);
        let body = json!({"run": self.run, "who": self.who, "at": at, "caption": caption, "jpg": b64});
        for _ in 0..3 {
            if self.send("/api/device/log/shot", &body) {
                return;
            }
            sleep(5.0);
        }
    }

    /// Whether this click gets a close-up (a debug run's first 10).
    pub fn click_shot(&self) -> bool {
        self.is_on() && self.loud()
            && (self.every_shot.load(Ordering::Relaxed) || self.clicks.fetch_add(1, Ordering::SeqCst) < CLICK_SHOTS)
    }

    /// Something to see at once (a debug run's summary, an update, a crash), with its
    /// picture if there is one; sent now (the caller's thread waits).
    pub fn post(&self, text: &str, png: Option<Vec<u8>>) {
        self.add("note", text);
        self.flush();
        if let Some(img) = png.as_deref().and_then(picture::decode) {
            self.send_shot(stamp(), text, &picture::jpg(&img, 80));
        }
    }

    fn send(&self, path: &str, body: &Value) -> bool {
        let key = self.key.lock().unwrap().clone();
        if key.is_empty() {
            return true;
        }
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
        match agent.post(&format!("{}{path}", self.site)).set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json").send_string(&body.to_string())
        {
            Ok(_) => true,
            // refused for good (too big, not a device of the site's): not tried again
            Err(ureq::Error::Status(c, _)) => c != 401 && c != 429 && c < 500,
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhooks_never_logged() {
        let said = "couldn't post: https://discord.com/api/webhooks/123456/abc-DEF_ghi: status code 404";
        assert_eq!(WEBHOOK.replace_all(said, "[webhook]"), "couldn't post: [webhook]: status code 404");
        assert!(FAILURE.is_match(said));
    }
}
