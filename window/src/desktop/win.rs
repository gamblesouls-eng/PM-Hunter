//! A Windows PC: the main screen through GDI (as mss had it, so Hunt's own
//! window, hidden from screenshots, isn't in them), the mouse and keyboard
//! through SendInput (the way a real mouse arrives: Roblox only clicks a button
//! it has seen the mouse move over), and Windows' own text recognition.

use super::Keys;
use hunt_core::device::Sound;
use hunt_core::{Frame, Pt, TextHit};
use std::time::Duration;
use windows::core::{HSTRING, PCWSTR};
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, SM_CXSCREEN, SM_CYSCREEN, SW_RESTORE,
};

#[allow(dead_code)]
pub fn set_own_window(_id: u32) {}

/// Screen coordinates in real pixels, the same as the screenshots' (the
/// window sets this itself; the Terminal's hunter needs it too).
pub fn init() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn screen() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// The main screen.
pub fn grab() -> Option<Frame> {
    let (w, h) = screen();
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen_dc = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen_dc));
        let bmp = CreateCompatibleBitmap(screen_dc, w, h);
        let old = SelectObject(dc, bmp.into());
        let ok = BitBlt(dc, 0, 0, w, h, Some(screen_dc), 0, 0, SRCCOPY | CAPTUREBLT).is_ok();
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(dc, bmp, 0, h as u32, Some(bgra.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen_dc);
        if !ok || lines == 0 {
            return None;
        }
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for p in bgra.chunks_exact(4) {
            rgb.extend_from_slice(&[p[2], p[1], p[0]]);
        }
        Frame::from_raw(w as u32, h as u32, rgb)
    }
}

pub fn scale() -> f64 {
    1.0
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, dwFlags: flags, ..Default::default() } } }
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

pub fn move_to(at: Pt) {
    let (w, h) = screen();
    let x = (at.x * 65535.0 / (w - 1).max(1) as f64).round() as i32;
    let y = (at.y * 65535.0 / (h - 1).max(1) as f64).round() as i32;
    send(&[mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, x, y)]);
}

fn nudge(dx: i32, dy: i32) {
    send(&[mouse(MOUSEEVENTF_MOVE, dx, dy)]);
}

/// Arrive, nudge (relative moves: Roblox sees these as the mouse moving), let
/// it notice, press, hold, and stay a moment after letting go.
pub fn click(at: Pt, right: bool) {
    move_to(Pt::new(at.x - 3.0, at.y));
    pause(30);
    nudge(2, 0);
    pause(20);
    move_to(at);
    nudge(1, 0);
    nudge(-1, 0);
    pause(50);
    send(&[mouse(if right { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_LEFTDOWN }, 0, 0)]);
    pause(40);
    send(&[mouse(if right { MOUSEEVENTF_RIGHTUP } else { MOUSEEVENTF_LEFTUP }, 0, 0)]);
    pause(30);
}

/// The wheel over at, a notch at a time (Roblox scrolls a list under the mouse).
pub fn wheel(at: Pt, notches: i32) {
    move_to(at);
    nudge(1, 0);
    nudge(-1, 0);
    pause(40);
    for _ in 0..notches.abs() {
        let delta = if notches > 0 { 120 } else { -120 };
        let mut i = mouse(MOUSEEVENTF_WHEEL, 0, 0);
        i.Anonymous.mi.mouseData = delta as u32;
        send(&[i]);
        pause(25);
    }
}

pub fn pointer() -> Pt {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    Pt::new(p.x as f64, p.y as f64)
}

/// Like pyautogui's fail-safe: the mouse in a screen corner stops the hunt.
pub fn in_corner() -> bool {
    let (w, h) = screen();
    let p = pointer();
    (p.x <= 0.0 || p.x >= (w - 1) as f64) && (p.y <= 0.0 || p.y >= (h - 1) as f64)
}

fn key_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, ..Default::default() } },
    }
}

fn scan(vk: u16) -> u16 {
    unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) as u16 }
}

/// A key, by its scan code too (Roblox goes by those). extended: End, Delete,
/// the arrows (the ones beside the letters, not the number pad's).
fn tap_key(vk: u16, extended: bool) {
    let ext = if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
    send(&[key_input(vk, scan(vk), ext), key_input(vk, scan(vk), ext | KEYEVENTF_KEYUP)]);
}

/// Ctrl+A, with scan codes, and Ctrl held a few frames round the A (Roblox
/// looks at the keys once a frame: sent all at once it saw no Ctrl).
fn select_all() {
    let (ctrl, a) = (0x11u16, 0x41u16);
    send(&[key_input(ctrl, scan(ctrl), KEYBD_EVENT_FLAGS(0))]);
    pause(50);
    send(&[key_input(a, scan(a), KEYBD_EVENT_FLAGS(0))]);
    pause(50);
    send(&[key_input(a, scan(a), KEYEVENTF_KEYUP)]);
    pause(30);
    send(&[key_input(ctrl, scan(ctrl), KEYEVENTF_KEYUP)]);
}

/// Into the text box that has focus, what's in it gone first (select all,
/// delete; and whatever select all didn't take, from the end back: a search
/// box holds a name or two, "Quick Ball", "Max Elixir").
pub fn type_text(text: &str) {
    select_all();
    pause(50);
    tap_key(0x08, false); // backspace
    pause(50);
    tap_key(0x23, true); // End
    for _ in 0..24 {
        pause(12);
        tap_key(0x08, false);
    }
    pause(50);
    for unit in text.encode_utf16() {
        send(&[key_input(0, unit, KEYEVENTF_UNICODE), key_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)]);
        pause(40);
    }
}

pub fn keys() -> Keys {
    let down = |vk: i32| unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 };
    Keys {
        ctrl: down(0x11),
        shift: down(0x10),
        h: down(0x48),
        n: down(0x4E),
        l: down(0x4C),
        any: (0x08..=0xFE).any(down),
    }
}

fn title(hwnd: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hwnd);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

/// Every visible top-level window.
fn windows() -> Vec<HWND> {
    unsafe extern "system" fn each(hwnd: HWND, all: LPARAM) -> windows::core::BOOL {
        let all = &mut *(all.0 as *mut Vec<HWND>);
        if IsWindowVisible(hwnd).as_bool() {
            all.push(hwnd);
        }
        true.into()
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut all as *mut Vec<HWND> as isize));
    }
    all
}

/// Roblox's player window (titled just "Roblox"; a browser on roblox.com has more in its title) is in front.
pub fn roblox_in_front() -> bool {
    title(unsafe { GetForegroundWindow() }) == "Roblox"
}

/// Roblox's player window is there (minimized or not): false once it's closed or crashed.
pub fn roblox_open() -> bool {
    windows().into_iter().any(|h| title(h) == "Roblox")
}

pub fn focus_roblox() {
    let Some(hwnd) = windows().into_iter().find(|h| title(*h) == "Roblox") else { return };
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        // Windows only lets the app in front hand over the focus; a tap of Alt counts as that.
        send(&[key_input(0x12, 0, KEYBD_EVENT_FLAGS(0)), key_input(0x12, 0, KEYEVENTF_KEYUP)]);
        let _ = SetForegroundWindow(hwnd);
    }
}

thread_local! {
    static ENGINE: std::cell::OnceCell<Option<OcrEngine>> = const { std::cell::OnceCell::new() };
}

fn engine() -> Option<OcrEngine> {
    ENGINE.with(|e| {
        e.get_or_init(|| unsafe {
            let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
            let english = windows::Globalization::Language::CreateLanguage(&HSTRING::from("en-US")).ok();
            english.and_then(|l| OcrEngine::TryCreateFromLanguage(&l).ok())
                .or_else(|| OcrEngine::TryCreateFromUserProfileLanguages().ok())
        })
        .clone()
    })
}

/// Windows puts a whole row of text in one line ("Quick Ball      x12"); it's
/// split where the words are far apart, the way a Mac's Vision reports them.
pub fn ocr(img: &Frame) -> Vec<TextHit> {
    let Some(engine) = engine() else { return Vec::new() };
    let most = OcrEngine::MaxImageDimension().unwrap_or(10000) as f64;
    let biggest = img.width().max(img.height()) as f64;
    let img = if biggest > most {
        let k = most / biggest;
        hunt_core::picture::resize(img, (img.width() as f64 * k) as u32, (img.height() as f64 * k) as u32)
    } else {
        img.clone()
    };
    let (w, h) = (img.width(), img.height());
    if w < 2 || h < 2 {
        return Vec::new();
    }
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for p in img.as_raw().chunks_exact(3) {
        bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    let run = || -> windows::core::Result<Vec<TextHit>> {
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        let bmp = SoftwareBitmap::CreateCopyFromBuffer(&writer.DetachBuffer()?, BitmapPixelFormat::Bgra8, w as i32, h as i32)?;
        let result = engine.RecognizeAsync(&bmp)?.join()?;
        let mut out = Vec::new();
        for line in result.Lines()? {
            let mut words = Vec::new();
            for word in line.Words()? {
                let r = word.BoundingRect()?;
                words.push((word.Text()?.to_string(), r.X as f64, r.Y as f64, (r.X + r.Width) as f64, (r.Y + r.Height) as f64));
            }
            super::split_line(&words, w as f64, h as f64, &mut out);
        }
        Ok(out)
    };
    run().unwrap_or_default()
}

pub fn sound(sound: Sound) {
    let alias = match sound {
        Sound::Found | Sound::Login => "SystemExclamation",
        Sound::Resumed => "SystemAsterisk",
        Sound::Paused => "SystemDefault",
    };
    let name = HSTRING::from(alias);
    unsafe {
        let _ = PlaySoundW(PCWSTR(name.as_ptr()), None, SND_ALIAS | SND_ASYNC);
    }
    std::thread::sleep(Duration::from_millis(300));
}

pub fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

pub fn system() -> String {
    // the real version (GetVersionEx says 8 to programs without a manifest)
    #[repr(C)]
    struct OsVersion {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersion) -> i32;
    }
    let mut v = OsVersion { size: std::mem::size_of::<OsVersion>() as u32, major: 0, minor: 0, build: 0, platform: 0, csd: [0; 128] };
    unsafe { RtlGetVersion(&mut v) };
    let name = if v.major == 10 && v.build >= 22000 { "11".to_string() } else { v.major.to_string() };
    format!("Windows {name} (build {})", v.build)
}

/// The screens, display scaling and Roblox's windows, for --diagnose.
pub fn screen_info() -> String {
    let (w, h) = screen();
    let mut lines = vec![format!("main screen: {w}x{h} (Hunt watches it)")];
    lines.push(format!("display scaling: {}%", unsafe { GetDpiForSystem() } * 100 / 96));
    let robloxes: Vec<String> = windows().into_iter().filter(|hw| title(*hw).to_lowercase().contains("roblox")).map(|hw| {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(hw, &mut r);
        }
        let min = if unsafe { IsIconic(hw).as_bool() } { " minimized" } else { "" };
        format!("\"{}\" {}x{} at ({}, {}){min}", title(hw), r.right - r.left, r.bottom - r.top, r.left, r.top)
    }).collect();
    lines.push(format!("Roblox windows: {}", if robloxes.is_empty() { "none open".into() } else { robloxes.join("; ") }));
    lines.push(format!("in front: \"{}\"", title(unsafe { GetForegroundWindow() })));
    lines.push(format!("Roblox in front: {}", roblox_in_front()));
    lines.join("\n")
}
