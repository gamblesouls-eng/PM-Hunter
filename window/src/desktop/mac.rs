//! A Mac: the main display through Quartz (as mss had it), the mouse and
//! keyboard as Quartz events, Vision's text recognition. Hunt runs from
//! Terminal, which holds the Screen Recording and Accessibility grants.

use super::Keys;
use hunt_core::device::Sound;
use hunt_core::{picture, Frame, Pt, TextHit};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

type Ref = *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGWindowListCreateImage(bounds: CGRect, option: u32, window: u32, image_option: u32) -> Ref;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> Ref;
    fn CGImageGetWidth(img: Ref) -> usize;
    fn CGImageGetHeight(img: Ref) -> usize;
    fn CGImageGetBytesPerRow(img: Ref) -> usize;
    fn CGImageGetBitsPerPixel(img: Ref) -> usize;
    fn CGImageGetBitmapInfo(img: Ref) -> u32;
    fn CGImageGetDataProvider(img: Ref) -> Ref;
    fn CGDataProviderCopyData(provider: Ref) -> Ref;
    fn CGImageRelease(img: Ref);
    fn CGEventCreate(source: Ref) -> Ref;
    fn CGEventGetLocation(event: Ref) -> CGPoint;
    fn CGEventCreateMouseEvent(source: Ref, kind: u32, at: CGPoint, button: u32) -> Ref;
    fn CGEventCreateKeyboardEvent(source: Ref, key: u16, down: bool) -> Ref;
    fn CGEventCreateScrollWheelEvent2(source: Ref, units: u32, count: u32, wheel1: i32, wheel2: i32, wheel3: i32) -> Ref;
    fn CGEventKeyboardSetUnicodeString(event: Ref, len: usize, chars: *const u16);
    fn CGEventSetFlags(event: Ref, flags: u64);
    fn CGEventPost(tap: u32, event: Ref);
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: Ref) -> bool;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
    fn IOHIDRequestAccess(request: u32) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: Ref);
    fn CFDataGetLength(data: Ref) -> isize;
    fn CFDataGetBytePtr(data: Ref) -> *const u8;
}

#[link(name = "Vision", kind = "framework")]
extern "C" {}

extern "C" {
    fn gethostname(name: *mut i8, len: usize) -> i32;
}

const ON_SCREEN_ONLY: u32 = 1;
const ON_SCREEN_BELOW_WINDOW: u32 = 4;
const EXCLUDE_DESKTOP: u32 = 16;
const HID_TAP: u32 = 0;
const COMBINED_STATE: i32 = 0;
const HID_STATE: i32 = 1; // the keyboard itself: not the hunter's own keys
const MOUSE_MOVED: u32 = 5;
const SHIFT: u64 = 0x0002_0000;
const COMMAND: u64 = 0x0010_0000;

/// Hunt's own window (the pill): screenshots are of what's under it.
static OWN_WINDOW: AtomicU32 = AtomicU32::new(0);

pub fn set_own_window(id: u32) {
    OWN_WINDOW.store(id, Ordering::Relaxed);
}

/// Hunt as an app (Hunt.app) asks for what it needs the first time: Screen
/// Recording, Accessibility (clicks), Input Monitoring (Ctrl+Shift+H). From
/// Terminal, Terminal's are used and nothing's asked.
pub fn init() {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    unsafe {
        if !CGPreflightScreenCaptureAccess() {
            CGRequestScreenCaptureAccess();
        }
        let prompt = CFDictionary::from_CFType_pairs(&[(CFString::new("AXTrustedCheckOptionPrompt"), CFBoolean::true_value())]);
        AXIsProcessTrustedWithOptions(prompt.as_concrete_TypeRef() as Ref);
        if IOHIDCheckAccess(1) != 0 {
            IOHIDRequestAccess(1); // listening to the keyboard
        }
    }
}

fn bounds() -> CGRect {
    unsafe { CGDisplayBounds(CGMainDisplayID()) }
}

/// The main display, in its real pixels.
pub fn grab() -> Option<Frame> {
    unsafe {
        let own = OWN_WINDOW.load(Ordering::Relaxed);
        let (option, below) = if own != 0 { (ON_SCREEN_BELOW_WINDOW, own) } else { (ON_SCREEN_ONLY, 0) };
        let img = CGWindowListCreateImage(bounds(), option, below, 0);
        if img.is_null() {
            return None;
        }
        let (w, h) = (CGImageGetWidth(img), CGImageGetHeight(img));
        let (row, bpp, info) = (CGImageGetBytesPerRow(img), CGImageGetBitsPerPixel(img), CGImageGetBitmapInfo(img));
        let data = CGDataProviderCopyData(CGImageGetDataProvider(img));
        CGImageRelease(img);
        if data.is_null() {
            return None;
        }
        let len = CFDataGetLength(data) as usize;
        let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data), len);
        let mut f = None;
        if bpp == 32 && len >= row * h && w > 0 {
            // 32 bits little-endian, alpha first or last (the screen's: BGRA)
            let little = info & 0x7000 == 0x2000;
            let alpha_first = matches!(info & 0x1f, 2 | 4 | 6); // premultiplied first, first, skip first
            let (r, g, b) = match (little, alpha_first) {
                (true, true) => (2, 1, 0),  // BGRA
                (true, false) => (3, 2, 1), // ABGR
                (false, true) => (1, 2, 3), // ARGB
                (false, false) => (0, 1, 2), // RGBA
            };
            let mut out = Vec::with_capacity(w * h * 3);
            for y in 0..h {
                let line = &bytes[y * row..y * row + w * 4];
                for p in line.chunks_exact(4) {
                    out.extend_from_slice(&[p[r], p[g], p[b]]);
                }
            }
            f = Frame::from_raw(w as u32, h as u32, out);
        }
        CFRelease(data);
        f
    }
}

/// Screenshot pixels per mouse point: 2 on Retina.
pub fn scale() -> f64 {
    let b = bounds();
    let w = unsafe {
        let img = CGWindowListCreateImage(b, ON_SCREEN_ONLY, 0, 0);
        if img.is_null() {
            return 2.0;
        }
        let w = CGImageGetWidth(img);
        CGImageRelease(img);
        w
    };
    if b.size.width > 0.0 { w as f64 / b.size.width } else { 1.0 }
}

fn points(at: Pt) -> CGPoint {
    static SCALE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    let s = *SCALE.get_or_init(scale);
    CGPoint { x: at.x / s, y: at.y / s }
}

fn post_mouse(kind: u32, at: CGPoint, button: u32) {
    unsafe {
        let e = CGEventCreateMouseEvent(std::ptr::null_mut(), kind, at, button);
        if !e.is_null() {
            CGEventPost(HID_TAP, e);
            CFRelease(e);
        }
    }
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Roblox reads input once a frame and only clicks a button it already sees the
/// mouse over, so: arrive, nudge, let it notice, press, hold, and stay a moment
/// after letting go.
pub fn click(at: Pt, right: bool) {
    let p = points(at);
    post_mouse(MOUSE_MOVED, CGPoint { x: p.x - 2.0, y: p.y }, 0);
    pause(30);
    post_mouse(MOUSE_MOVED, p, 0);
    pause(50);
    let (down, up, button) = if right { (3, 4, 1) } else { (1, 2, 0) };
    post_mouse(down, p, button);
    pause(40);
    post_mouse(up, p, button);
    pause(30);
}

pub fn move_to(at: Pt) {
    post_mouse(MOUSE_MOVED, points(at), 0);
}

/// The wheel over at, a line (a notch) at a time (Roblox scrolls a list under the mouse).
pub fn wheel(at: Pt, notches: i32) {
    move_to(at);
    pause(40);
    for _ in 0..notches.abs() {
        unsafe {
            let e = CGEventCreateScrollWheelEvent2(std::ptr::null_mut(), 1, 1, notches.signum(), 0, 0);
            if !e.is_null() {
                CGEventPost(HID_TAP, e);
                CFRelease(e);
            }
        }
        pause(25);
    }
}

pub fn pointer() -> Pt {
    let s = points(Pt::new(1.0, 1.0));
    unsafe {
        let e = CGEventCreate(std::ptr::null_mut());
        let p = CGEventGetLocation(e);
        CFRelease(e);
        Pt::new(p.x / s.x, p.y / s.y)
    }
}

/// The mouse in a corner of the main screen (pyautogui's fail-safe).
pub fn in_corner() -> bool {
    let b = bounds();
    let p = unsafe {
        let e = CGEventCreate(std::ptr::null_mut());
        let p = CGEventGetLocation(e);
        CFRelease(e);
        p
    };
    let (x, y) = (p.x - b.origin.x, p.y - b.origin.y);
    (x <= 0.0 || x >= b.size.width - 1.0) && (y <= 0.0 || y >= b.size.height - 1.0)
}

fn key(code: u16, down: bool, flags: u64, ch: Option<char>) {
    unsafe {
        let e = CGEventCreateKeyboardEvent(std::ptr::null_mut(), code, down);
        if e.is_null() {
            return;
        }
        if flags != 0 {
            CGEventSetFlags(e, flags);
        }
        if let Some(c) = ch {
            let mut buf = [0u16; 2];
            let s = c.encode_utf16(&mut buf);
            CGEventKeyboardSetUnicodeString(e, s.len(), s.as_ptr());
        }
        CGEventPost(HID_TAP, e);
        CFRelease(e);
    }
}

/// A US keyboard's key for a character, and whether it takes Shift.
fn keycode(c: char) -> Option<(u16, bool)> {
    const LOWER: &str = "asdfhgzxcv?bqweryt123465=97-80]ou[ip?lj'k;\\,/nm.";
    const UPPER: &str = "ASDFHGZXCV?BQWERYT!@#$^%+(&_*)}OU{IP?LJ\"K:|<?NM>";
    if c == ' ' {
        return Some((49, false));
    }
    if c == '`' {
        return Some((50, false));
    }
    if c == '~' {
        return Some((50, true));
    }
    if let Some(i) = LOWER.chars().position(|x| x == c && c != '?') {
        return Some((i as u16, false));
    }
    if c == '?' {
        return Some((44, true));
    }
    UPPER.chars().position(|x| x == c).map(|i| (i as u16, true))
}

/// Into the text box that has the focus: what's in it goes first (select
/// all, delete), wherever the click left the caret.
pub fn type_text(text: &str) {
    key(0, true, COMMAND, None); // Cmd+A
    pause(20);
    key(0, false, COMMAND, None);
    pause(50);
    key(51, true, 0, None); // delete
    key(51, false, 0, None);
    pause(50);
    for c in text.chars() {
        match keycode(c) {
            Some((code, shift)) => {
                if shift {
                    key(56, true, SHIFT, None);
                }
                let flags = if shift { SHIFT } else { 0 };
                key(code, true, flags, Some(c));
                key(code, false, flags, Some(c));
                if shift {
                    key(56, false, 0, None);
                }
            }
            None => {
                key(0, true, 0, Some(c));
                key(0, false, 0, Some(c));
            }
        }
        pause(40);
    }
}

pub fn keys() -> Keys {
    // the hotkeys as the session has them (the hunter never presses Ctrl+Shift itself);
    // "any" from the keyboard alone, so its own typing isn't you coming back
    let down = |k: u16| unsafe { CGEventSourceKeyState(COMBINED_STATE, k) || CGEventSourceKeyState(HID_STATE, k) };
    let hid = |k: u16| unsafe { CGEventSourceKeyState(HID_STATE, k) };
    Keys {
        ctrl: down(59) || down(62),
        shift: down(56) || down(60),
        h: down(4),
        n: down(45),
        l: down(37),
        any: (0..128).any(hid),
    }
}

/// The window list's windows, front to back: (layer, width, height, owner).
fn windows() -> Vec<(i64, f64, f64, String)> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    let raw = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0) };
    if raw.is_null() {
        return Vec::new();
    }
    let list: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(raw as _) };
    let num = |d: &CFDictionary<CFString, CFType>, k: &'static str| -> f64 {
        d.find(CFString::from_static_string(k)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_f64()).unwrap_or(0.0)
    };
    list.iter()
        .map(|d| {
            let (w, h) = d.find(CFString::from_static_string("kCGWindowBounds"))
                .and_then(|v| v.downcast::<CFDictionary>())
                .map(|b| {
                    let b: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(b.as_concrete_TypeRef()) };
                    (num(&b, "Width"), num(&b, "Height"))
                })
                .unwrap_or((0.0, 0.0));
            let owner = d.find(CFString::from_static_string("kCGWindowOwnerName"))
                .and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string()).unwrap_or_default();
            (num(&d, "kCGWindowLayer") as i64, w, h, owner)
        })
        .collect()
}

/// Roblox's window is the topmost app window (NSWorkspace's frontmost app goes
/// stale in a program that runs long; the window list doesn't).
pub fn roblox_in_front() -> bool {
    windows().into_iter()
        .find(|(layer, w, h, _)| *layer == 0 && *w > 200.0 && *h > 200.0)
        .is_some_and(|(_, _, _, owner)| owner.to_lowercase().contains("roblox"))
}

/// (open -a: no Automation permission to ask for, as osascript's "activate" needs from an app)
pub fn focus_roblox() {
    let _ = std::process::Command::new("open").args(["-a", "Roblox"]).output();
}

/// [(text, center x, center y)], x and y fractions of the picture, by Vision.
pub fn ocr(img: &Frame) -> Vec<TextHit> {
    use objc2::rc::{autoreleasepool, Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSRect, NSString};
    if img.width() < 2 || img.height() < 2 {
        return Vec::new();
    }
    let png = picture::png(img);
    autoreleasepool(|_| unsafe {
        let (Some(request), Some(handler)) = (AnyClass::get(c"VNRecognizeTextRequest"), AnyClass::get(c"VNImageRequestHandler"))
        else { return Vec::new() };
        let data = NSData::with_bytes(&png);
        let req: Retained<AnyObject> = msg_send![request, new];
        let _: () = msg_send![&*req, setRecognitionLevel: 0isize]; // accurate
        let _: () = msg_send![&*req, setUsesLanguageCorrection: false];
        let options = NSDictionary::<NSString, AnyObject>::new();
        let a: Allocated<AnyObject> = msg_send![handler, alloc];
        let handler: Retained<AnyObject> = msg_send![a, initWithData: &*data, options: &*options];
        let requests = NSArray::from_retained_slice(&[req.clone()]);
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let _: bool = msg_send![&*handler, performRequests: &*requests, error: &mut error];
        let results: Option<Retained<NSArray<AnyObject>>> = msg_send![&*req, results];
        let mut out = Vec::new();
        for r in results.iter().flat_map(|a| a.iter()) {
            let bb: NSRect = msg_send![&*r, boundingBox];
            let candidates: Retained<NSArray<AnyObject>> = msg_send![&*r, topCandidates: 1usize];
            let Some(best) = candidates.firstObject() else { continue };
            let text: Retained<NSString> = msg_send![&*best, string];
            out.push(TextHit::new(text.to_string(), bb.origin.x + bb.size.width / 2.0,
                                  1.0 - bb.origin.y - bb.size.height / 2.0));
        }
        out
    })
}

pub fn sound(sound: Sound) {
    let name = match sound {
        Sound::Found | Sound::Login => "Glass",
        Sound::Paused => "Bottle",
        Sound::Resumed => "Pop",
    };
    let _ = std::process::Command::new("afplay").arg(format!("/System/Library/Sounds/{name}.aiff")).status();
}

pub fn hostname() -> String {
    let mut buf = [0i8; 256];
    if unsafe { gethostname(buf.as_mut_ptr(), buf.len()) } != 0 {
        return "Mac".into();
    }
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn system() -> String {
    let version = std::process::Command::new("sw_vers").arg("-productVersion").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    format!("macOS {version} ({})", std::env::consts::ARCH)
}

/// The screens and Roblox's window, for --diagnose.
pub fn screen_info() -> String {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut n) };
    let screens: Vec<String> = ids[..n as usize].iter().map(|id| {
        let b = unsafe { CGDisplayBounds(*id) };
        format!("{}x{} at ({}, {})", b.size.width, b.size.height, b.origin.x, b.origin.y)
    }).collect();
    let robloxes: Vec<String> = windows().into_iter().filter(|w| w.3.to_lowercase().contains("roblox") && w.0 == 0)
        .map(|w| format!("{}x{}", w.1, w.2)).collect();
    format!("screens (points): {} (Hunt watches the first)\nRoblox windows: {}\nRoblox in front: {}", screens.join(", "),
            if robloxes.is_empty() { "none open".into() } else { robloxes.join("; ") }, roblox_in_front())
}
