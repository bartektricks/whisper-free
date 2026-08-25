//! What macOS has decided about the permissions dictation needs.
//!
//! Accessibility is answered by `AXIsProcessTrusted` in `text.rs`, beside the
//! code that needs it. This file covers the microphone, which is the one
//! permission macOS will raise a prompt for on request rather than only
//! reporting after the fact.

use block2::RcBlock;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Bool};

use crate::platform::PermissionState;

// `AVCaptureDevice` and the media-type constant live in AVFoundation. cpal
// already links AVFAudio out of the same umbrella, so this is a framework the
// process loads either way.
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    /// `AVMediaTypeAudio`, an `NSString *`.
    #[allow(non_upper_case_globals)]
    static AVMediaTypeAudio: &'static AnyObject;
}

/// `AVAuthorizationStatus`, from `AVCaptureDevice.h`.
mod status {
    pub const NOT_DETERMINED: isize = 0;
    pub const RESTRICTED: isize = 1;
    pub const DENIED: isize = 2;
    pub const AUTHORIZED: isize = 3;
}

/// The `AVCaptureDevice` class, looked up by name.
///
/// `None` would mean `AVFoundation` is not in the process, which the `#[link]`
/// above makes impossible. A missing class would be a `nil` receiver and a
/// silently wrong answer, so it is checked rather than assumed.
fn capture_device() -> Option<&'static AnyClass> {
    AnyClass::get(c"AVCaptureDevice")
}

/// Whether macOS will let the app open a microphone.
pub fn microphone() -> PermissionState {
    let Some(class) = capture_device() else {
        tracing::warn!("AVCaptureDevice is unavailable; microphone status unknown");
        return PermissionState::Unknown;
    };

    // Safety: a class method taking one object and returning an `NSInteger`,
    // called with the framework's own media-type constant.
    let raw: isize = unsafe { msg_send![class, authorizationStatusForMediaType: AVMediaTypeAudio] };

    match raw {
        status::AUTHORIZED => PermissionState::Granted,
        // Restricted is a policy the user cannot change themselves. From where
        // they are standing it is indistinguishable from a refusal, and the
        // message that names the settings pane is the right one either way.
        status::DENIED | status::RESTRICTED => PermissionState::Denied,
        status::NOT_DETERMINED => PermissionState::Unasked,
        other => {
            tracing::warn!(status = other, "unrecognised microphone authorization status");
            PermissionState::Unknown
        }
    }
}

/// Raise the system microphone prompt, and say whether one is on its way.
///
/// macOS shows that prompt exactly once per app: after the user has answered,
/// `requestAccessForMediaType:` calls the handler straight back without
/// putting anything on screen, which would leave someone staring at a button
/// that did nothing. So anything but [`PermissionState::Unasked`] reports "no
/// prompt", and the caller sends them to the settings pane instead.
pub fn prompt_for_microphone() -> bool {
    if microphone() != PermissionState::Unasked {
        return false;
    }
    let Some(class) = capture_device() else {
        return false;
    };

    // The answer is read back through `microphone()`, which the UI polls; the
    // handler exists because the API requires one, and records what happened.
    let handler = RcBlock::new(|granted: Bool| {
        tracing::info!(event = "microphone_permission_answered", granted = granted.as_bool());
    });

    // Safety: the declared signature of the class method, called with the
    // framework's own constant. AVFoundation copies the handler before it
    // returns, so dropping our reference on the way out is fine.
    unsafe {
        let _: () = msg_send![
            class,
            requestAccessForMediaType: AVMediaTypeAudio,
            completionHandler: &*handler,
        ];
    }

    tracing::info!(event = "microphone_permission_requested");
    true
}
