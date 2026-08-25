//! What Windows will say about the permissions dictation needs, which is
//! nothing it will say in advance.
//!
//! Synthetic input is not gated at all here (see `INPUT_PERMISSION_REQUIRED`).
//! The microphone is, by the per-user privacy switches, but a desktop app is
//! covered by one global "let desktop apps access your microphone" toggle
//! rather than by an entry of its own, and there is no supported API that
//! answers for the running process. Rather than read the consent store out of
//! the registry and present a guess as a fact, this reports
//! [`PermissionState::Unknown`] and the UI falls back to the one thing that is
//! authoritative on any platform: opening the microphone and seeing whether
//! anything arrives.

use crate::platform::PermissionState;

/// Windows will not answer for the process, so the microphone test is the
/// only truth available.
pub const fn microphone() -> PermissionState {
    PermissionState::Unknown
}

/// There is no prompt to raise: the switch lives in Settings, and the caller
/// opens that page instead.
pub const fn prompt_for_microphone() -> bool {
    false
}
