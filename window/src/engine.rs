//! The hunter, run in Hunt itself (it was hunt-engine.exe, a process of its
//! own): for the pill, and for the Terminal (--terminal, --diagnose, --image,
//! --test-discord).

use crate::desktop::{Desktop, Hotkey};
use hunt_core::{Config, Control, DebugChannel, Options, Sink, Store};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::{env, fs};

/// The build's number (higher is newer) and name, from build.json next to Hunt.exe
/// (the Windows build writes it, so Hunt.exe itself is the same from build to build
/// and is only built again when its code changes); without it, a build of one's own (0, "dev").
pub fn build() -> &'static (u64, String) {
    static BUILD: OnceLock<(u64, String)> = OnceLock::new();
    BUILD.get_or_init(|| {
        // next to Hunt.exe; in Hunt.app, its Resources
        let info = env::current_exe().ok()
            .and_then(|exe| fs::read_to_string(exe.with_file_name("build.json"))
                .or_else(|_| fs::read_to_string(exe.with_file_name("../Resources/build.json"))).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let get = |key| info.as_ref().and_then(|i| i.get(key).cloned());
        (get("version").and_then(|v| v.as_u64()).unwrap_or(0),
         get("name").and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "dev".into()))
    })
}

/// Hunt.app, when Hunt runs from one: the bundle.
pub fn app_bundle() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let bundle = exe.parent()?.parent()?.parent()?;
    (bundle.extension().is_some_and(|e| e == "app")).then(|| bundle.to_path_buf())
}

/// What a computer keeps: HUNT_DATA, else %LOCALAPPDATA%\Hunt on Windows. On a
/// Mac, Hunt.app keeps it in ~/Library/Application Support/Hunt, or the folder
/// named in its data-dir file there (make-mac-app.sh points it at the
/// repository's folder); from Terminal, the folder Hunt runs in.
pub fn data_dir() -> PathBuf {
    let data = env::var_os("HUNT_DATA").map(PathBuf::from).unwrap_or_else(|| {
        if cfg!(windows) {
            PathBuf::from(env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Hunt")
        } else if app_bundle().is_some() {
            let support = PathBuf::from(env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/Hunt");
            fs::read_to_string(support.join("data-dir")).ok()
                .map(|p| PathBuf::from(p.trim()))
                .filter(|p| p.is_dir())
                .unwrap_or(support)
        } else {
            env::current_dir().unwrap_or_default()
        }
    });
    let _ = fs::create_dir_all(&data);
    data
}

/// The hunt the keys reach.
static CURRENT: Mutex<Option<Arc<Control>>> = Mutex::new(None);

/// This computer, one for Hunt's life: Ctrl+Shift+H / N / L go to the hunt that's on.
pub fn desktop() -> Arc<Desktop> {
    static DESKTOP: OnceLock<Arc<Desktop>> = OnceLock::new();
    DESKTOP.get_or_init(|| {
        Desktop::new(|key| {
            let Some(c) = CURRENT.lock().unwrap().clone() else { return };
            match key {
                Hotkey::Toggle => c.toggle(),
                Hotkey::NewCode => c.website(true),
                Hotkey::LogOut => c.website(false),
            }
        })
    }).clone()
}

/// The debug channel: the hunt's log on the website, with the device's key ("" for
/// none); none with HUNT_DEBUG=0 (the pill's "problem reports" off).
pub fn debug_channel(key: &str, reports: bool) -> Arc<DebugChannel> {
    let off = env::var("HUNT_DEBUG").as_deref() == Ok("0") || !reports;
    let who = format!("{} · {}", desktop_system(), build().1);
    let d = DebugChannel::new(if off { "" } else { key }, &who);
    d.set_header(&format!("Hunt {} on {}", build().1, desktop_system()));
    d
}

fn desktop_system() -> String {
    use hunt_core::Device;
    static SYSTEM: OnceLock<String> = OnceLock::new();
    SYSTEM.get_or_init(|| desktop().system()).clone()
}

/// The hunt modes: "hunt", "watch" (never clicks), "debug" (catches every encounter).
pub struct Launch<'a> {
    pub mode: &'a str,
    pub paused: bool,
    pub window: bool,
    pub practice: u32,
    pub calibrate: bool,
    pub reports: bool,
    pub config: &'a Path,
    pub count: u32, // encounters so far (carried on from before an update)
    pub carried: bool, // hunting on from before an update: a battle on screen is its next encounter
}

/// A hunt on a thread of its own; done is called once it's over.
pub fn launch(l: Launch, sink: Arc<dyn Sink>, done: impl FnOnce() + Send + 'static) -> Result<Arc<Control>, String> {
    let cfg = Config::load(l.config)?;
    let store = Store::new(data_dir());
    let watch = l.mode == "watch";
    let key = if watch { String::new() } else { hunt_core::web::key(&store.path("website.key")) };
    let debug = debug_channel(&key, l.reports);
    let opts = Options {
        watch,
        paused: l.paused,
        debug: l.mode == "debug",
        practice: l.practice,
        calibrate: l.calibrate,
        window: l.window,
        build: build().1.clone(),
        key,
        times: env::var_os("HUNT_TIMES").is_some(),
        count: l.count,
        carried: l.carried,
    };
    debug.loud.store(opts.debug, std::sync::atomic::Ordering::Relaxed);
    let (control, hunt) = hunt_core::start(desktop(), sink, debug, cfg, store, opts);
    *CURRENT.lock().unwrap() = Some(control.clone());
    let c = control.clone();
    std::thread::Builder::new()
        .name("hunter".into())
        .spawn(move || {
            hunt.run();
            let mut current = CURRENT.lock().unwrap();
            if current.as_ref().is_some_and(|x| Arc::ptr_eq(x, &c)) {
                *current = None;
            }
            drop(current);
            done();
        })
        .map_err(|e| e.to_string())?;
    Ok(control)
}

/// The hunter on its own, in a Terminal: Hunt --terminal (--watch, --paused,
/// --debug, --practice N, --calibrate), --diagnose, --image x.png, --test-discord.
pub fn terminal(args: &[String]) -> i32 {
    attach_console();
    let has = |a: &str| args.iter().any(|x| x == a);
    let value = |a: &str| args.iter().position(|x| x == a).and_then(|i| args.get(i + 1)).cloned();
    let data = data_dir();
    let config = value("--config").map(PathBuf::from).unwrap_or_else(|| data.join("config.json"));
    let console: Arc<dyn Sink> = Arc::new(hunt_core::Console);
    let load = || Config::load(&config).or_else(|e| {
        // --diagnose and --image need no rules of yours
        if has("--diagnose") || has("--image") {
            Config::parse(r#"{"targets": [], "notify": []}"#)
        } else {
            Err(e)
        }
    });
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if has("--diagnose") || has("--image") || has("--test-discord") {
        // (to the log only from a device the website knows: one that has hunted)
        let key = std::fs::read_to_string(Store::new(&data).path("website.key")).unwrap_or_default();
        let debug = debug_channel(&key, true);
        debug.loud.store(has("--diagnose"), std::sync::atomic::Ordering::Relaxed);
        let opts = Options { build: build().1.clone(), ..Default::default() };
        let (_c, mut hunt) = hunt_core::start(desktop(), console, debug.clone(), cfg, Store::new(&data), opts);
        if has("--diagnose") {
            hunt.diagnose();
        } else if has("--test-discord") {
            let ok = hunt.test_discord();
            println!("{}", if ok { "sent." } else { "couldn't send it (no webhook in config.json?)" });
            return if ok { 0 } else { 1 };
        } else {
            let images: Vec<&String> = args.iter().skip_while(|a| *a != "--image").skip(1).take_while(|a| !a.starts_with("--")).collect();
            for path in images {
                hunt.read_image(Path::new(path), has("--calibrate"));
            }
        }
        debug.flush();
        return 0;
    }
    let mode = if has("--watch") { "watch" } else if has("--debug") { "debug" } else { "hunt" };
    let practice = value("--practice").and_then(|v| v.parse().ok()).unwrap_or(0);
    let (tx, rx) = std::sync::mpsc::channel();
    let l = Launch { mode, paused: has("--paused"), window: false, practice, calibrate: has("--calibrate"), reports: true,
                     config: &config, count: 0, carried: false };
    match launch(l, console, move || {
        let _ = tx.send(());
    }) {
        Ok(_) => {
            let _ = rx.recv();
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// On Windows Hunt.exe is a window's program: its words reach the console it
/// was started from (or the pipe it writes to) only this way.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}
