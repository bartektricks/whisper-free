//! Window levels and Spaces behaviour, for the dictation overlay.
//!
//! Tauri's `always_on_top` maps to `NSFloatingWindowLevel`, which loses to any
//! other floating window on screen — another app's picture-in-picture, or its
//! own HUD. This raises the overlay to the tier the system's always-visible
//! chrome uses, and lets it follow the user between Spaces.

use objc2_app_kit::{NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior};
use tauri::WebviewWindow;

/// Raise `window` above other floating windows, and let it follow the user
/// between Spaces.
///
/// `NSStatusWindowLevel` rather than something higher on purpose. Levels above
/// it were tried against a full-screen application — `NSPopUpMenuWindowLevel`
/// (101) and `NSScreenSaverWindowLevel` (1000) — and neither made the overlay
/// appear there, because the obstacle is Space membership rather than z-order.
/// Since the extra height buys nothing, sitting in the screen-saver band would
/// be rude for no gain.
///
/// `FullScreenAuxiliary` is asked for even though it is **not sufficient on its
/// own**: a plain `NSWindow` is not admitted to another application's
/// full-screen Space whatever its level or collection behaviour. Making that
/// work needs the window to be an `NSPanel` with
/// `NSWindowStyleMask::NonactivatingPanel`, which tao does not create and which
/// is why apps like superwhisper carry `tauri-nspanel`. The flag stays because
/// it is half of the answer, and the half that costs nothing.
///
/// None of this affects focus: window level and key-window eligibility are
/// unrelated, so the `focusable: false` guarantee the paste depends on is
/// untouched.
///
/// # Panics
///
/// Never. A window without a backing `NSWindow` is logged and left alone.
pub fn float_above_other_windows(window: &WebviewWindow) {
    let ptr = match window.ns_window() {
        Ok(ptr) => ptr,
        Err(e) => {
            tracing::error!(error = %e, "no NSWindow behind the overlay; it stays at the default level");
            return;
        }
    };

    if ptr.is_null() {
        tracing::error!("overlay NSWindow pointer was null");
        return;
    }

    // SAFETY: `ns_window` returns the `NSWindow` Tauri created for this label,
    // which outlives the borrow; it is only ever called from the main thread,
    // which is where AppKit requires these two setters to run.
    let ns_window: &NSWindow = unsafe { &*ptr.cast::<NSWindow>() };

    ns_window.setLevel(NSStatusWindowLevel);
    // Assigned, not OR-ed into what tao left behind: the default includes
    // `Managed`, and at most one of `Managed`/`Transient`/`Stationary` may be
    // set, so leaving it in place cancels `CanJoinAllSpaces` out.
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );

    tracing::debug!(
        event = "overlay_window_level_raised",
        level = NSStatusWindowLevel
    );
}
