//! The screen coordinate space: which display the user is working on, and the
//! units a window position is measured in.
//!
//! The overlay belongs on the screen the dictated text is about to land in,
//! which is the screen holding the **focused window** — not the screen holding
//! the pointer, which is frequently left somewhere else entirely.
//!
//! macOS will only say which window that is through the Accessibility API. The
//! app already holds that permission, because synthesising the paste needs it,
//! and asking is read-only: a position and a size, never a title, a value or an
//! application name.
//!
//! Every coordinate here is in points with a top-left origin, which is the
//! space `AXPosition` and `CGEvent::location` both use — and the space
//! [`SCREEN_UNIT`] names.

use std::ffi::c_void;
use std::ptr;

use core_foundation::base::{CFGetTypeID, CFRelease, CFTypeID, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGSize};
use tauri::{LogicalPosition, PhysicalPosition, Position};

use super::super::ScreenUnit;

/// macOS measures every global coordinate in points.
pub const SCREEN_UNIT: ScreenUnit = ScreenUnit::Logical;

/// An opaque `AXUIElementRef` / `AXValueRef`; both are CoreFoundation types.
type AXUIElementRef = *const c_void;
type AXError = i32;

const AX_SUCCESS: AXError = 0;

/// `kAXValueCGPointType` and `kAXValueCGSizeType` from `AXValue.h`.
const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;

/// How long to wait for an application to answer, in seconds.
///
/// Accessibility calls are synchronous inter-process messages and default to
/// six seconds. This runs on the main thread while dictation is starting, so a
/// hung application must cost a barely visible pause and not a frozen app —
/// after which the pointer fallback still puts the overlay somewhere sensible.
const MESSAGING_TIMEOUT: f32 = 0.25;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> AXError;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXValueGetValue(value: AXUIElementRef, value_type: u32, out: *mut c_void) -> u8;
}

/// A CoreFoundation reference this module owns and has to release.
///
/// Every `Copy`-named Accessibility call hands back a +1 reference; wrapping it
/// means the early returns below cannot leak one.
struct Owned(CFTypeRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a `Copy`/`Create` call that
            // returned success, and is released exactly once.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// The centre of the window with keyboard focus, in points.
pub fn focused_window_centre() -> Option<(f64, f64)> {
    // SAFETY: takes no arguments and returns a +1 reference or null.
    let system = Owned(unsafe { AXUIElementCreateSystemWide() });
    if system.0.is_null() {
        return None;
    }

    // Set on the system-wide element, which is where the default for elements
    // this process creates comes from, and again on the application below —
    // the application is the one that can be hung.
    set_timeout(system.0);

    let app = element_attribute(system.0, "AXFocusedApplication")?;

    // Never ask our own process. `AXUIElement.h` warns that an application
    // talking to itself over the Accessibility API can deadlock, and this runs
    // on the main thread, which is also the thread that would have to answer.
    // The messaging timeout below would bound it to a stall rather than a
    // freeze, but a stall on every state transition is still a freeze to the
    // user — and when our own settings window has focus, the pointer is the
    // better signal anyway.
    if element_pid(app.0) == Some(std::process::id()) {
        tracing::trace!("the focused application is this one; deferring to the pointer");
        return None;
    }

    set_timeout(app.0);

    let window = element_attribute(app.0, "AXFocusedWindow")?;

    let position = point_attribute(window.0, "AXPosition")?;
    let size = size_attribute(window.0, "AXSize")?;

    Some((
        position.x + size.width / 2.0,
        position.y + size.height / 2.0,
    ))
}

/// Where the pointer is, in points with a top-left origin.
///
/// Read from a synthetic event rather than `NSEvent::mouseLocation`, whose
/// bottom-left origin would have to be flipped against the main display's
/// height — one more thing to get wrong on a display arrangement no test can
/// reach.
pub fn pointer_position() -> Option<(f64, f64)> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    let location = CGEvent::new(source).ok()?.location();
    Some((location.x, location.y))
}

/// Points, because that is what `setFrameTopLeftPoint:` takes.
///
/// tao's `set_outer_position` divides a *physical* position by the scale factor
/// of the display the window is currently on, not the one it is being sent to.
/// A `Position::Logical` skips that conversion (`Position::to_logical` is the
/// identity on one), so the point space macOS actually uses survives the trip
/// and the overlay lands on the display it was placed against — even when the
/// two displays have different densities.
pub fn window_position(physical: PhysicalPosition<i32>, monitor_scale: f64) -> Position {
    // A monitor reporting a nonsensical scale factor is measured as it is
    // rather than dividing by zero, matching `monitor_containing`.
    let scale = if monitor_scale > 0.0 { monitor_scale } else { 1.0 };

    Position::Logical(LogicalPosition::new(
        f64::from(physical.x) / scale,
        f64::from(physical.y) / scale,
    ))
}

/// The process an element belongs to, or `None` if it will not say.
fn element_pid(element: AXUIElementRef) -> Option<u32> {
    let mut pid: i32 = 0;

    // SAFETY: `element` is a live `AXUIElement` and `pid` is a live, writable
    // `pid_t`. Unlike the attribute calls this is answered locally, so it
    // cannot block on the other application.
    let status = unsafe { AXUIElementGetPid(element, ptr::addr_of_mut!(pid)) };
    if status != AX_SUCCESS {
        return None;
    }

    u32::try_from(pid).ok()
}

fn set_timeout(element: AXUIElementRef) {
    // SAFETY: `element` is a live `AXUIElement`; the call only stores a number.
    let status = unsafe { AXUIElementSetMessagingTimeout(element, MESSAGING_TIMEOUT) };
    if status != AX_SUCCESS {
        tracing::debug!(status, "could not shorten the accessibility timeout");
    }
}

/// Copy an attribute that should itself be an element, e.g. the focused window.
fn element_attribute(element: AXUIElementRef, name: &str) -> Option<Owned> {
    let value = copy_attribute(element, name)?;
    // SAFETY: `value` is a live CoreFoundation object.
    let is_element = unsafe { CFGetTypeID(value.0) == AXUIElementGetTypeID() };
    is_element.then_some(value)
}

fn point_attribute(element: AXUIElementRef, name: &str) -> Option<CGPoint> {
    let mut point = CGPoint::new(0.0, 0.0);
    let out = ptr::addr_of_mut!(point).cast::<c_void>();
    read_value(element, name, AX_VALUE_CG_POINT, out).then_some(point)
}

fn size_attribute(element: AXUIElementRef, name: &str) -> Option<CGSize> {
    let mut size = CGSize::new(0.0, 0.0);
    let out = ptr::addr_of_mut!(size).cast::<c_void>();
    read_value(element, name, AX_VALUE_CG_SIZE, out).then_some(size)
}

/// Unwrap an `AXValue` attribute into `out`, which must point at the type
/// `value_type` names.
fn read_value(element: AXUIElementRef, name: &str, value_type: u32, out: *mut c_void) -> bool {
    let Some(value) = copy_attribute(element, name) else {
        return false;
    };

    // SAFETY: `value.0` is a live CoreFoundation object.
    if unsafe { CFGetTypeID(value.0) != AXValueGetTypeID() } {
        return false;
    }

    // SAFETY: the object is an `AXValue`, and `out` points at a `CGPoint` or a
    // `CGSize` matching `value_type` — the two callers above are the only ones,
    // and each passes the constant for the type it allocated. `AXValueGetValue`
    // refuses a mismatch rather than writing, and leaves `out` untouched when
    // it does.
    unsafe { AXValueGetValue(value.0, value_type, out) != 0 }
}

/// Copy one attribute, or `None` if the element has no such attribute, the
/// application did not answer in time, or the permission is not granted.
fn copy_attribute(element: AXUIElementRef, name: &str) -> Option<Owned> {
    if element.is_null() {
        return None;
    }

    let attribute = CFString::new(name);
    let mut value: CFTypeRef = ptr::null();

    // SAFETY: `element` is a live `AXUIElement`, `attribute` outlives the call,
    // and `value` is only read when the call reported success.
    let status = unsafe {
        AXUIElementCopyAttributeValue(
            element,
            attribute.as_concrete_TypeRef(),
            ptr::addr_of_mut!(value),
        )
    };

    if status != AX_SUCCESS || value.is_null() {
        // Logged without the attribute's contents: names of attributes are
        // fine, what an application holds in them is not ours to record.
        tracing::trace!(attribute = name, status, "no accessibility answer");
        return None;
    }

    Some(Owned(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression: the overlay was placed against the target monitor but
    /// positioned against the one it was still on. With a 2× built-in and a 1×
    /// external to its right, a pill placed on the external at physical x=2680
    /// was handed to tao as a physical position, halved by the built-in's scale
    /// factor, and appeared at 1340 points — in the middle of the built-in.
    #[test]
    fn a_position_on_a_one_x_display_survives_a_two_x_window() {
        let placed = window_position(PhysicalPosition::new(2680, 1356), 1.0);
        assert_eq!(placed, Position::Logical(LogicalPosition::new(2680.0, 1356.0)));
    }

    /// The same conversion on a Retina target: its physical pixels are twice
    /// its points, and points are what macOS positions windows in.
    #[test]
    fn a_position_on_a_retina_display_is_halved_into_points() {
        let placed = window_position(PhysicalPosition::new(1288, 1820), 2.0);
        assert_eq!(placed, Position::Logical(LogicalPosition::new(644.0, 910.0)));
    }

    #[test]
    fn a_monitor_reporting_no_scale_factor_is_measured_as_it_is() {
        let placed = window_position(PhysicalPosition::new(100, 200), 0.0);
        assert_eq!(placed, Position::Logical(LogicalPosition::new(100.0, 200.0)));
    }
}
