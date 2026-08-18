//! The screen coordinate space: which display the user is working on, and the
//! units a window position is measured in.
//!
//! The overlay belongs on the screen the dictated text is about to land in,
//! which is the screen holding the **foreground window** — not the screen
//! holding the pointer, which is frequently left somewhere else entirely.
//!
//! Windows measures the whole virtual desktop in physical pixels, the same
//! units [`super::super::MonitorBounds`] carries, so nothing here is converted.
//! That is the difference from the macOS backend, which works in points. tao
//! calls `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` at startup, so
//! the numbers below are real pixels rather than ones Windows has virtualised.

use tauri::{PhysicalPosition, Position};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowRect,
};

use super::super::ScreenUnit;

/// Windows measures the virtual desktop in physical pixels throughout.
pub const SCREEN_UNIT: ScreenUnit = ScreenUnit::Physical;

/// The centre of the foreground window, in physical pixels.
///
/// A minimised window reports a rectangle far outside the desktop rather than
/// failing, so the answer is allowed to fall outside every monitor —
/// `monitor_containing` returns `None` and the pointer decides instead.
pub fn focused_window_centre() -> Option<(f64, f64)> {
    // SAFETY: takes no arguments and returns a window handle or null.
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };

    // SAFETY: `window` is a live handle and `rect` is a live, writable `RECT`.
    let ok = unsafe { GetWindowRect(window, std::ptr::addr_of_mut!(rect)) };
    if ok == 0 {
        tracing::debug!("the foreground window would not report its rectangle");
        return None;
    }

    // Measured in floating point: a window spanning the whole virtual desktop
    // would overflow the halfway point in `i32`.
    Some((
        f64::midpoint(f64::from(rect.left), f64::from(rect.right)),
        f64::midpoint(f64::from(rect.top), f64::from(rect.bottom)),
    ))
}

/// Where the pointer is, in physical pixels.
pub fn pointer_position() -> Option<(f64, f64)> {
    let mut point = POINT { x: 0, y: 0 };

    // SAFETY: `point` is a live, writable `POINT`.
    let ok = unsafe { GetCursorPos(std::ptr::addr_of_mut!(point)) };
    if ok == 0 {
        return None;
    }

    Some((f64::from(point.x), f64::from(point.y)))
}

/// Physical pixels, unchanged: `SetWindowPos` takes virtual-desktop pixels, and
/// tao passes a `Position::Physical` straight through to it.
///
/// The scale factor is part of the contract because macOS needs it; here the
/// position is already in the units the window API wants.
pub const fn window_position(physical: PhysicalPosition<i32>, _monitor_scale: f64) -> Position {
    Position::Physical(physical)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows wants the number `place` already produced. The scale factor is
    /// in the signature for macOS's sake and must not be applied here, or a
    /// window moving onto a 150% display would be positioned at two-thirds of
    /// where it belongs.
    #[test]
    fn a_position_is_passed_through_whatever_the_scale_factor() {
        let position = PhysicalPosition::new(2680, 1356);
        assert_eq!(window_position(position, 1.0), Position::Physical(position));
        assert_eq!(window_position(position, 1.5), Position::Physical(position));
        assert_eq!(window_position(position, 2.0), Position::Physical(position));
    }
}
