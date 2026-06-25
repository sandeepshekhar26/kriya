//! [`MacDesktopBackend`] — the macOS implementation of [`DesktopBackend`], over CoreGraphics
//! ([`core_graphics::event`]) for synthetic mouse/keyboard/scroll input, `screencapture` for the
//! screenshot, and `osascript` for the foreground-app list. This is the one place real OS input
//! lives; everything above it (the tool catalog, [`super::executor::ComputerUseExecutor`], the
//! [`super::ComputerUseServer`]) is platform-agnostic and tested against a fake backend, so this
//! file is the only part that needs a real screen + the Accessibility / Screen-Recording permissions.
//!
//! ## Why CGEvent for input
//! `CGEvent::new_mouse_event` / `new_scroll_event` / `new_keyboard_event` synthesize HID-level events
//! that any frontmost app receives — the same mechanism Front 2's typed-input path uses. We reuse
//! that approach here (duplicating a small keycode/type helper so Front 3 is self-contained and does
//! not depend on the reach-in module). Injecting synthetic input requires the **Accessibility**
//! permission; [`ensure_trusted`] checks it up front and returns an actionable error rather than
//! letting the events be silently dropped.
//!
//! ## Why shell out for screenshot + app list
//! `screencapture -x -t png <tmp>` is the dependency-free way to capture the screen as PNG; it needs
//! the **Screen Recording** permission and, when denied, writes nothing — we detect the empty/missing
//! file and return a clear `Err` (never a panic). `osascript` over System Events lists the foreground
//! apps, consistent with how the rest of the crate already talks to macOS (`approval.rs`, reach-in).

use core_foundation::base::CFRelease;
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use accessibility_sys::AXIsProcessTrusted;

use super::DesktopBackend;

// core-graphics 0.25 exposes no scroll-wheel constructor, so bind CoreGraphics directly. The C
// function is variadic over the wheel deltas; declared here with fixed trailing int args, which the
// macOS ABI passes as ordinary integers — the standard way Rust macOS-automation code calls this.
#[allow(non_snake_case)]
extern "C" {
    fn CGEventCreateScrollWheelEvent(
        source: *const std::ffi::c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
    ) -> *const std::ffi::c_void;
    fn CGEventPost(tap: u32, event: *const std::ffi::c_void);
}

/// The macOS desktop backend — stateless; each call synthesizes its own CGEvent / shells out. No
/// handle to keep alive, unlike the AX backend (there is no per-app element ref — Front 3 drives the
/// whole screen).
pub struct MacDesktopBackend;

impl MacDesktopBackend {
    /// Construct the backend. Does **not** check permissions here (so a server can start and report a
    /// readable failure per-call); each input op calls [`ensure_trusted`] before injecting.
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacDesktopBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopBackend for MacDesktopBackend {
    fn screenshot(&self) -> Result<Vec<u8>, String> {
        use std::process::Command;
        // A unique temp path so concurrent gateways don't clobber each other.
        let tmp =
            std::env::temp_dir().join(format!("kriya-computeruse-{}.png", uuid::Uuid::new_v4()));
        // `-x` = no capture sound; `-t png` = PNG; path = destination. Full-screen capture.
        let status = Command::new("screencapture")
            .arg("-x")
            .arg("-t")
            .arg("png")
            .arg(&tmp)
            .status()
            .map_err(|e| format!("failed to run screencapture: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "screencapture exited with {status}. Grant Screen Recording in System Settings → \
                 Privacy & Security → Screen Recording for the app running kriya-gateway, then retry."
            ));
        }
        // Even on a "success" exit, a denied Screen-Recording permission yields an empty/absent file.
        // Treat that as a readable failure rather than returning zero bytes.
        let bytes = match std::fs::read(&tmp) {
            Ok(b) => b,
            Err(e) => {
                return Err(format!(
                "could not read screenshot file (Screen Recording permission may be missing): {e}"
            ))
            }
        };
        let _ = std::fs::remove_file(&tmp);
        if bytes.is_empty() {
            return Err(
                "screenshot was empty — grant Screen Recording in System Settings → Privacy & \
                 Security → Screen Recording for the app running kriya-gateway, then retry."
                    .to_string(),
            );
        }
        Ok(bytes)
    }

    fn click(&self, x: f64, y: f64, button: &str) -> Result<(), String> {
        ensure_trusted()?;
        let point = CGPoint::new(x, y);
        let (down, up, cg_button) = if button.eq_ignore_ascii_case("right") {
            (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
            )
        } else {
            (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
            )
        };
        // A click is a down then an up at the same point.
        for ev_type in [down, up] {
            let source = new_source("mouse click")?;
            let event = CGEvent::new_mouse_event(source, ev_type, point, cg_button)
                .map_err(|()| "failed to create mouse CGEvent".to_string())?;
            event.post(CGEventTapLocation::HID);
            std::thread::sleep(std::time::Duration::from_millis(6));
        }
        std::thread::sleep(std::time::Duration::from_millis(12));
        Ok(())
    }

    fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
        ensure_trusted()?;
        let source = new_source("mouse move")?;
        // A move carries no button; the button arg is ignored for a MouseMoved event.
        let event = CGEvent::new_mouse_event(
            source,
            CGEventType::MouseMoved,
            CGPoint::new(x, y),
            CGMouseButton::Left,
        )
        .map_err(|()| "failed to create mouse-move CGEvent".to_string())?;
        event.post(CGEventTapLocation::HID);
        std::thread::sleep(std::time::Duration::from_millis(12));
        Ok(())
    }

    fn scroll(&self, dx: i32, dy: i32) -> Result<(), String> {
        ensure_trusted()?;
        // wheel1 = vertical, wheel2 = horizontal; units = line (1). A null source is valid for
        // CGEventCreateScrollWheelEvent, so we skip constructing a CGEventSource here.
        // SAFETY: the returned event (+1) is released after posting to the HID tap (0 = kCGHIDEventTap);
        // a null return means the OS refused to create it (e.g. no permission).
        unsafe {
            let event = CGEventCreateScrollWheelEvent(std::ptr::null(), 1, 2, dy, dx);
            if event.is_null() {
                return Err("failed to create scroll CGEvent".to_string());
            }
            CGEventPost(0, event);
            CFRelease(event);
        }
        std::thread::sleep(std::time::Duration::from_millis(12));
        Ok(())
    }

    fn type_text(&self, text: &str) -> Result<(), String> {
        ensure_trusted()?;
        // Same approach as reach-in's typed-input: a keycode-0 keyboard event carrying the unicode
        // string types `text` into whatever holds focus, bypassing the physical-key→char mapping so
        // multi-byte / layout / IME input is not garbled.
        for keydown in [true, false] {
            let source = new_source("type")?;
            let event = CGEvent::new_keyboard_event(source, 0, keydown)
                .map_err(|()| "failed to create keyboard CGEvent for typing".to_string())?;
            event.set_string(text);
            event.post(CGEventTapLocation::HID);
            std::thread::sleep(std::time::Duration::from_millis(6));
        }
        std::thread::sleep(std::time::Duration::from_millis(12));
        Ok(())
    }

    fn key(&self, key: &str) -> Result<(), String> {
        ensure_trusted()?;
        let keycode = keycode_for(key).ok_or_else(|| format!("unknown key '{key}'"))?;
        // A named key is a real virtual keycode → post a down then an up for a full keystroke.
        for keydown in [true, false] {
            let source = new_source("key press")?;
            let event = CGEvent::new_keyboard_event(source, keycode, keydown)
                .map_err(|()| format!("failed to create keyboard CGEvent for key '{key}'"))?;
            event.post(CGEventTapLocation::HID);
            std::thread::sleep(std::time::Duration::from_millis(6));
        }
        std::thread::sleep(std::time::Duration::from_millis(12));
        Ok(())
    }

    fn list_apps(&self) -> Result<Vec<String>, String> {
        use std::process::Command;
        // Foreground (non-background) processes = the apps with a visible UI a user could switch to.
        let script = "tell application \"System Events\" to get name of (processes where background only is false)";
        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| format!("failed to run osascript for list_apps: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "osascript list_apps failed (Automation permission for System Events may be \
                 missing): {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        // osascript returns a comma-space-separated AppleScript list on one line.
        let line = String::from_utf8_lossy(&output.stdout);
        let apps = line
            .trim()
            .split(", ")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Ok(apps)
    }
}

/// Build a `CGEventSource` for a synthetic event, mapping the unit failure to a readable error.
fn new_source(what: &str) -> Result<CGEventSource, String> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| format!("failed to create CGEventSource for {what}"))
}

/// Map a [`SUPPORTED_KEYS`] name to its macOS virtual keycode (standard ANSI keycodes from
/// `<HIToolbox/Events.h>`). `None` for any name outside the set, so an unknown key is an `Err`
/// upstream (never a silent no-op). Duplicated from reach-in's `keycode_for` so Front 3 is
/// self-contained; kept in lockstep with [`SUPPORTED_KEYS`] (guarded by a test).
fn keycode_for(key: &str) -> Option<CGKeyCode> {
    let code: CGKeyCode = match key {
        "return" | "enter" => 0x24,
        "tab" => 0x30,
        "space" => 0x31,
        "delete" | "backspace" => 0x33,
        "escape" => 0x35,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        _ => return None,
    };
    Some(code)
}

/// Check the process holds the **Accessibility** permission, required to inject synthetic input.
/// Without it CGEvent posts are silently dropped; we surface an actionable message instead.
fn ensure_trusted() -> Result<(), String> {
    // SAFETY: `AXIsProcessTrusted` takes no arguments and is always safe to call.
    if unsafe { AXIsProcessTrusted() } {
        Ok(())
    } else {
        Err(
            "Accessibility permission not granted (required to synthesize mouse/keyboard input). \
             Grant it in System Settings → Privacy & Security → Accessibility for the app running \
             kriya-gateway (e.g. your terminal), then retry."
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::SUPPORTED_KEYS;
    use super::*;

    /// Every key the `computer_key` tool advertises MUST have a real virtual keycode here, or it
    /// would be advertised but un-pressable. Guards the two from drifting (mirrors reach-in's test).
    #[test]
    fn every_supported_key_has_a_keycode() {
        for key in SUPPORTED_KEYS {
            assert!(
                keycode_for(key).is_some(),
                "advertised key '{key}' has no virtual keycode"
            );
        }
        // A name outside the set has no keycode (so it errs upstream).
        assert!(keycode_for("f13").is_none());
    }

    /// Real-input integration smoke test. Skipped cleanly unless the process holds the Accessibility
    /// permission, so CI/headless never fails. To run it for real, grant your terminal (or cargo)
    /// Accessibility permission, then:
    ///   cargo test --no-default-features --features computer-use -- --ignored real_move
    #[test]
    #[ignore = "needs a granted Accessibility permission; moves the real cursor"]
    fn real_move_when_trusted() {
        // SAFETY: argument-free, always safe.
        if !unsafe { AXIsProcessTrusted() } {
            eprintln!("skipping: Accessibility permission not granted");
            return;
        }
        let backend = MacDesktopBackend::new();
        backend.move_to(100.0, 100.0).expect("move cursor");
        eprintln!("moved cursor to (100,100)");
    }
}
