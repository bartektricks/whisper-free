//! Which display the user is working on.
//!
//! The overlay belongs on the screen the dictated text is about to land in,
//! which is the screen holding the **foreground window** — not the screen
//! holding the pointer, which is frequently left somewhere else entirely.
//!
//! Windows measures the whole virtual desktop in physical pixels, the same
//! units [`MonitorBounds`] carries, so nothing here is converted. That is the
//! difference from the macOS backend, which works in points.

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowRect,
};

use super::super::{monitor_containing, MonitorBounds, ScreenUnit};

/// The display the user is working on: the foreground window's, else the
/// pointer's.
pub fn active_monitor(monitors: &[MonitorBounds]) -> Option<usize> {
    let foreground = foreground_window_centre()
        .and_then(|(x, y)| monitor_containing(monitors, x, y, ScreenUnit::Physical));

    if let Some(index) = foreground {
        tracing::debug!(event = "active_monitor", source = "foreground_window", index);
        return Some(index);
    }

    let pointer = pointer_position()
        .and_then(|(x, y)| monitor_containing(monitors, x, y, ScreenUnit::Physical));

    tracing::debug!(
        event = "active_monitor",
        source = "pointer",
        index = pointer,
        "no foreground window on a known display"
    );
    pointer
}

/// The centre of the foreground window, in physical pixels.
///
/// A minimised window reports a rectangle far outside the desktop rather than
/// failing, so the answer is allowed to fall outside every monitor —
/// [`monitor_containing`] returns `None` and the pointer decides instead.
fn foreground_window_centre() -> Option<(f64, f64)> {
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
fn pointer_position() -> Option<(f64, f64)> {
    let mut point = POINT { x: 0, y: 0 };

    // SAFETY: `point` is a live, writable `POINT`.
    let ok = unsafe { GetCursorPos(std::ptr::addr_of_mut!(point)) };
    if ok == 0 {
        return None;
    }

    Some((f64::from(point.x), f64::from(point.y)))
}
