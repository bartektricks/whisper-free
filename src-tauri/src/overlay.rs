//! The floating dictation indicator (plan §14).
//!
//! A menu-bar app is invisible while it works: the tray status line needs the
//! menu open, and the status badge needs the settings window open. This module
//! puts a small pill on screen for as long as dictation is actually running.
//!
//! The split of responsibility is deliberate. Rust decides *whether* the window
//! is on screen and *where* it goes; the webview decides what it looks like,
//! driven by the same `state_changed` broadcast the settings window listens to.
//! Nothing here knows about bars, colours or labels.
//!
//! **The overlay is never focused.** It exists as an ordinary Tauri window
//! rather than an `NSPanel` because `focusable: false` makes the underlying tao
//! window refuse to become key on macOS and adds `WS_EX_NOACTIVATE` on Windows.
//! Taking focus would move the paste target away from the app the user is
//! typing in, which breaks dictation outright — see
//! `docs/decisions/0004-dictation-overlay.md`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, PhysicalRect, PhysicalSize};

use crate::state::{AppState, StateSnapshot};

/// The window label declared in `tauri.conf.json`.
pub const WINDOW_LABEL: &str = "overlay";

/// Gap between the pill and the edges of the screen's work area.
const INSET: u32 = 24;

/// Size while dictating: enough for a short status word beside the bars.
///
/// Both sizes include the 8 px the webview leaves around the pill so its drop
/// shadow is not clipped by the window edge.
const BUSY_SIZE: LogicalSize<f64> = LogicalSize::new(224.0, 60.0);

/// Size while showing a failure. Error text is a whole sentence that names
/// where to fix the problem, so it needs room to wrap.
const ERROR_SIZE: LogicalSize<f64> = LogicalSize::new(380.0, 92.0);

/// How long a failure stays on screen before it fades out on its own.
const ERROR_DISMISS: Duration = Duration::from_secs(4);

/// Serial number for the most recent [`apply`] call.
///
/// An error schedules its own dismissal on a spare thread. By the time that
/// thread wakes the user may have started dictating again, and hiding the
/// window then would be worse than leaving the error up. The timer therefore
/// only acts while its ticket is still the current one.
static TICKET: AtomicU64 = AtomicU64::new(0);

/// Where on the active screen the indicator sits.
///
/// A named anchor rather than a stored pixel position: coordinates from a
/// monitor that has since been unplugged would strand the overlay off-screen,
/// and an anchor is meaningful on whichever display the user is working on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OverlayAnchor {
    TopLeft,
    TopCentre,
    TopRight,
    CentreLeft,
    Centre,
    CentreRight,
    BottomLeft,
    /// Out of the way of the text cursor in most applications.
    #[default]
    BottomCentre,
    BottomRight,
}

impl OverlayAnchor {
    /// Position along one axis, as a fraction of the free space.
    const fn fractions(self) -> (Fraction, Fraction) {
        use Fraction::{End, Middle, Start};
        match self {
            Self::TopLeft => (Start, Start),
            Self::TopCentre => (Middle, Start),
            Self::TopRight => (End, Start),
            Self::CentreLeft => (Start, Middle),
            Self::Centre => (Middle, Middle),
            Self::CentreRight => (End, Middle),
            Self::BottomLeft => (Start, End),
            Self::BottomCentre => (Middle, End),
            Self::BottomRight => (End, End),
        }
    }
}

/// Which end of the free space along an axis the window is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fraction {
    Start,
    Middle,
    End,
}

impl Fraction {
    /// Offset from the start of the work area, given how much room is left over
    /// once the window has been placed.
    ///
    /// `free` is already saturated at zero by the caller, so a window wider than
    /// the screen pins to the top-left corner rather than hanging off it.
    const fn offset(self, free: u32, inset: u32) -> u32 {
        match self {
            Self::Start => if inset < free { inset } else { free },
            Self::Middle => free / 2,
            Self::End => free.saturating_sub(inset),
        }
    }
}

/// Should the indicator be on screen in this state?
///
/// Pure, so the rule is testable without a window server — the same split as
/// [`crate::state::is_valid`] and [`crate::hotkey::decide`].
const fn wants_overlay(state: AppState, enabled: bool) -> bool {
    enabled
        && matches!(
            state,
            AppState::Recording | AppState::Transcribing | AppState::Inserting | AppState::Error
        )
}

/// Top-left corner for a window of `size` anchored inside `work_area`.
///
/// `work_area` already excludes the menu bar, the Dock and the taskbar, so the
/// inset is measured from usable screen rather than from the physical edge.
fn place(
    anchor: OverlayAnchor,
    work_area: PhysicalRect<i32, u32>,
    size: PhysicalSize<u32>,
    inset: u32,
) -> PhysicalPosition<i32> {
    let (horizontal, vertical) = anchor.fractions();

    let free_x = work_area.size.width.saturating_sub(size.width);
    let free_y = work_area.size.height.saturating_sub(size.height);

    let offset_x = i32::try_from(horizontal.offset(free_x, inset)).unwrap_or(i32::MAX);
    let offset_y = i32::try_from(vertical.offset(free_y, inset)).unwrap_or(i32::MAX);

    PhysicalPosition::new(
        work_area.position.x.saturating_add(offset_x),
        work_area.position.y.saturating_add(offset_y),
    )
}

/// Bring the overlay in line with `snapshot`.
///
/// Called from `publish_state` for every transition, and from `update_settings`
/// when the toggle or the anchor changes.
///
/// Takes `ctx` rather than reaching for it through `app` because the caller
/// already holds it, and because `ctx.settings` is locked here — a caller
/// holding that same guard would deadlock.
///
/// The window work happens on the main thread and this returns immediately.
/// `publish_state` runs on whatever thread caused the transition — the
/// `dictation` worker, an audio error callback, or the hotkey handler, which on
/// macOS *is* the main thread. Posting to the event loop rather than blocking on
/// it is the same precaution as never registering a shortcut from the hotkey
/// handler.
pub fn apply(app: &AppHandle, ctx: &crate::AppContext, snapshot: &StateSnapshot) {
    // A poisoned lock must not leave the user with no feedback at all, so the
    // overlay defaults to on with the default anchor.
    let (enabled, anchor) = ctx.settings.lock().map_or_else(
        |_| (true, OverlayAnchor::default()),
        |s| (s.show_overlay, s.overlay_anchor),
    );

    // Claimed before anything is scheduled, so a pending error timer is stale
    // the moment any newer state arrives.
    let ticket = TICKET.fetch_add(1, Ordering::SeqCst).wrapping_add(1);

    let show = wants_overlay(snapshot.state, enabled);
    let is_error = snapshot.state == AppState::Error;

    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window(WINDOW_LABEL) else {
            tracing::error!("overlay window is missing from the app config");
            return;
        };

        if !show {
            if let Err(e) = window.hide() {
                tracing::warn!(error = %e, "could not hide the overlay");
            }
            return;
        }

        let size = if is_error { ERROR_SIZE } else { BUSY_SIZE };
        if let Err(e) = window.set_size(size) {
            tracing::warn!(error = %e, "could not size the overlay");
        }

        // The screen holding the focused window is the screen the user is
        // working on, and the screen the text is about to be pasted into.
        // `platform::active_monitor` falls back to the pointer itself; the
        // primary display is what is left when the desktop has focus and the
        // pointer is somewhere the OS does not recognise.
        let monitors = handle.available_monitors().unwrap_or_default();
        let bounds: Vec<crate::platform::MonitorBounds> =
            monitors.iter().map(Into::into).collect();

        let monitor = crate::platform::active_monitor(&bounds)
            .and_then(|index| monitors.get(index).cloned())
            .or_else(|| handle.primary_monitor().ok().flatten());

        if let Some(monitor) = monitor {
            let scale = monitor.scale_factor();
            let physical: PhysicalSize<u32> = size.to_physical(scale);
            let top_left = place(anchor, *monitor.work_area(), physical, INSET);
            tracing::debug!(
                event = "overlay_placed",
                x = top_left.x,
                y = top_left.y,
                scale
            );

            // `place` works in the target monitor's physical pixels, which is
            // not necessarily what the window API measures against — on macOS
            // it converts using the scale factor of the display the window is
            // still on, which is the wrong one whenever the overlay is about to
            // move between displays of different densities.
            let position = crate::platform::window_position(top_left, scale);
            if let Err(e) = window.set_position(position) {
                tracing::warn!(error = %e, "could not position the overlay");
            }
        } else {
            tracing::warn!("no monitor available; leaving the overlay where it is");
        }

        // Deliberately not `set_focus`: see the module docs.
        if let Err(e) = window.show() {
            tracing::warn!(error = %e, "could not show the overlay");
            return;
        }

        if is_error {
            dismiss_later(&handle, ticket);
        }
    }) {
        tracing::warn!(error = %e, "could not reach the main thread to update the overlay");
    }
}

/// Hide the overlay once [`ERROR_DISMISS`] has passed, unless something newer
/// has happened in the meantime.
fn dismiss_later(app: &AppHandle, ticket: u64) {
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("overlay-dismiss".into())
        .spawn(move || {
            std::thread::sleep(ERROR_DISMISS);
            if TICKET.load(Ordering::SeqCst) != ticket {
                return; // superseded; whoever superseded us owns the window now
            }
            let inner = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                // Checked again on the main thread: the state may have moved on
                // between the load above and this closure running.
                if TICKET.load(Ordering::SeqCst) != ticket {
                    return;
                }
                if let Some(window) = inner.get_webview_window(WINDOW_LABEL) {
                    let _ = window.hide();
                }
            });
        });

    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not schedule the overlay dismissal");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1920x1080 screen at the origin, as the primary display reports itself.
    fn primary() -> PhysicalRect<i32, u32> {
        PhysicalRect {
            position: PhysicalPosition::new(0, 0),
            size: PhysicalSize::new(1920, 1080),
        }
    }

    fn pill() -> PhysicalSize<u32> {
        PhysicalSize::new(400, 88)
    }

    #[test]
    fn bottom_centre_is_horizontally_centred_and_inset_from_the_bottom() {
        let position = place(OverlayAnchor::BottomCentre, primary(), pill(), INSET);
        assert_eq!(position.x, (1920 - 400) / 2);
        assert_eq!(position.y, 1080 - 88 - 24);
    }

    #[test]
    fn top_left_sits_one_inset_in_from_both_edges() {
        let position = place(OverlayAnchor::TopLeft, primary(), pill(), INSET);
        assert_eq!(position, PhysicalPosition::new(24, 24));
    }

    #[test]
    fn bottom_right_is_inset_from_the_far_edges() {
        let position = place(OverlayAnchor::BottomRight, primary(), pill(), INSET);
        assert_eq!(position.x, 1920 - 400 - 24);
        assert_eq!(position.y, 1080 - 88 - 24);
    }

    #[test]
    fn centre_ignores_the_inset_entirely() {
        let position = place(OverlayAnchor::Centre, primary(), pill(), INSET);
        assert_eq!(position, PhysicalPosition::new((1920 - 400) / 2, (1080 - 88) / 2));
    }

    #[test]
    fn every_anchor_stays_inside_the_work_area() {
        let anchors = [
            OverlayAnchor::TopLeft,
            OverlayAnchor::TopCentre,
            OverlayAnchor::TopRight,
            OverlayAnchor::CentreLeft,
            OverlayAnchor::Centre,
            OverlayAnchor::CentreRight,
            OverlayAnchor::BottomLeft,
            OverlayAnchor::BottomCentre,
            OverlayAnchor::BottomRight,
        ];
        for anchor in anchors {
            let position = place(anchor, primary(), pill(), INSET);
            assert!(position.x >= 0, "{anchor:?} ran off the left edge");
            assert!(position.y >= 0, "{anchor:?} ran off the top edge");
            assert!(
                position.x + 400 <= 1920,
                "{anchor:?} ran off the right edge"
            );
            assert!(
                position.y + 88 <= 1080,
                "{anchor:?} ran off the bottom edge"
            );
        }
    }

    /// A second display sits at a non-zero origin in the virtual desktop, and
    /// the work area excludes the menu bar, so neither may be assumed to be 0.
    #[test]
    fn placement_is_relative_to_the_monitors_own_origin() {
        let secondary = PhysicalRect {
            position: PhysicalPosition::new(-2560, 100),
            size: PhysicalSize::new(2560, 1340),
        };
        let position = place(OverlayAnchor::BottomCentre, secondary, pill(), INSET);
        assert_eq!(position.x, -2560 + (2560 - 400) / 2);
        assert_eq!(position.y, 100 + 1340 - 88 - 24);
    }

    /// A pill wider than the screen must pin to the corner rather than hang off
    /// it, which is what the saturating arithmetic buys.
    #[test]
    fn a_window_larger_than_the_screen_pins_to_the_origin() {
        let tiny = PhysicalRect {
            position: PhysicalPosition::new(0, 0),
            size: PhysicalSize::new(200, 50),
        };
        for anchor in [
            OverlayAnchor::TopLeft,
            OverlayAnchor::BottomCentre,
            OverlayAnchor::BottomRight,
        ] {
            let position = place(anchor, tiny, pill(), INSET);
            assert_eq!(position, PhysicalPosition::new(0, 0), "{anchor:?}");
        }
    }

    /// A screen with less free space than the inset must not push the window
    /// past the opposite edge.
    #[test]
    fn an_inset_larger_than_the_free_space_is_clamped() {
        let snug = PhysicalRect {
            position: PhysicalPosition::new(0, 0),
            size: PhysicalSize::new(410, 98),
        };
        let position = place(OverlayAnchor::TopLeft, snug, pill(), INSET);
        assert_eq!(position, PhysicalPosition::new(10, 10));
    }

    #[test]
    fn the_overlay_is_shown_only_while_dictation_is_running() {
        assert!(wants_overlay(AppState::Recording, true));
        assert!(wants_overlay(AppState::Transcribing, true));
        assert!(wants_overlay(AppState::Inserting, true));
        assert!(wants_overlay(AppState::Error, true));

        assert!(!wants_overlay(AppState::Ready, true));
        assert!(!wants_overlay(AppState::Uninitialized, true));
    }

    #[test]
    fn opting_out_hides_the_overlay_in_every_state() {
        for state in [
            AppState::Uninitialized,
            AppState::Ready,
            AppState::Recording,
            AppState::Transcribing,
            AppState::Inserting,
            AppState::Error,
        ] {
            assert!(!wants_overlay(state, false), "{state} showed the overlay");
        }
    }

    #[test]
    fn the_default_anchor_is_out_of_the_way_of_the_cursor() {
        assert_eq!(OverlayAnchor::default(), OverlayAnchor::BottomCentre);
    }

    #[test]
    fn anchors_round_trip_through_json_as_snake_case() {
        let json = serde_json::to_string(&OverlayAnchor::BottomCentre).unwrap();
        assert_eq!(json, "\"bottom_centre\"");
        let back: OverlayAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(back, OverlayAnchor::BottomCentre);
    }
}
