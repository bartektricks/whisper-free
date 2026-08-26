//! Silencing whatever the machine is playing, for the length of a recording.
//!
//! Reached only as `platform::backend::output`. macOS offers no way to mute one
//! *other* application, so this mutes the device everything is playing through
//! and puts that device back exactly as it was. See
//! `docs/decisions/0009-muting-other-audio-while-recording.md`.

use core::ffi::c_void;
use core::ptr::NonNull;

use objc2_core_audio::{
    kAudioDevicePropertyMute, kAudioDevicePropertyVolumeScalar,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, AudioObjectGetPropertyData,
    AudioObjectHasProperty, AudioObjectID, AudioObjectIsPropertySettable,
    AudioObjectPropertyAddress, AudioObjectPropertyScope, AudioObjectPropertySelector,
    AudioObjectSetPropertyData,
};

/// `kAudioObjectSystemObject`. The bindings give it as a `c_int` while every
/// call that takes it wants an `AudioObjectID`, and this crate denies `as`
/// casts, so it is restated at the width it is actually used at.
const SYSTEM_OBJECT: AudioObjectID = 1;

/// `kAudioHardwareNoError`.
const OK: i32 = 0;

/// What was silenced, and what it was before.
///
/// The device id is remembered rather than looked up again on the way out.
/// Plugging in headphones mid-dictation changes which device is default, and
/// restoring *that* one would leave the muted device muted while moving a
/// setting nobody touched.
pub enum Mute {
    /// The device had a mute switch, and it was off before we came along.
    Switch { device: AudioObjectID },
    /// It had no mute switch on the master element, so the volume was taken to
    /// zero instead and this is where it was.
    Volume {
        device: AudioObjectID,
        previous: f32,
    },
}

const fn address(
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Read one fixed-size property into `out`, reporting whether it arrived.
fn get<T>(device: AudioObjectID, address: AudioObjectPropertyAddress, out: &mut T) -> bool {
    let Ok(mut size) = u32::try_from(core::mem::size_of::<T>()) else {
        return false;
    };
    let mut address = address;
    // SAFETY: every pointer is derived from a live local, and `size` says how
    // many bytes `out` has room for, which is what the call contracts on.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&mut address),
            0,
            core::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(out).cast::<c_void>(),
        )
    };
    status == OK
}

/// Write one fixed-size property, reporting whether it took.
fn set<T>(device: AudioObjectID, address: AudioObjectPropertyAddress, value: &T) -> bool {
    let Ok(size) = u32::try_from(core::mem::size_of::<T>()) else {
        return false;
    };
    let mut address = address;
    // SAFETY: as above, and the callee only reads `size` bytes from `value`.
    let status = unsafe {
        AudioObjectSetPropertyData(
            device,
            NonNull::from(&mut address),
            0,
            core::ptr::null(),
            size,
            NonNull::from(value).cast::<c_void>(),
        )
    };
    status == OK
}

/// Whether the device both has this property and will let it be written.
///
/// Both halves matter: plenty of devices report a mute property on the master
/// element and refuse to change it.
fn settable(device: AudioObjectID, address: AudioObjectPropertyAddress) -> bool {
    let mut address = address;
    // SAFETY: the address is a live local for the length of both calls.
    unsafe {
        if !AudioObjectHasProperty(device, NonNull::from(&mut address)) {
            return false;
        }
        let mut yes: u8 = 0;
        let status = AudioObjectIsPropertySettable(
            device,
            NonNull::from(&mut address),
            NonNull::from(&mut yes),
        );
        status == OK && yes != 0
    }
}

fn default_output_device() -> Option<AudioObjectID> {
    let mut device: AudioObjectID = 0;
    let found = get(
        SYSTEM_OBJECT,
        address(
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal,
        ),
        &mut device,
    );
    // Zero is `kAudioObjectUnknown`: the call can succeed and still hand back
    // nothing, on a machine with no output at all.
    if found && device != 0 {
        Some(device)
    } else {
        None
    }
}

/// Silence the default output device, if there is one and it is not silent
/// already.
pub fn mute() -> Option<Mute> {
    let device = default_output_device()?;

    let switch = address(kAudioDevicePropertyMute, kAudioObjectPropertyScopeOutput);
    if settable(device, switch) {
        let mut current: u32 = 0;
        // A device whose mute state cannot be read is left alone rather than
        // muted: without knowing where it started, unmuting it later could be
        // unmuting somebody who had muted their own machine.
        if get(device, switch, &mut current) {
            if current != 0 {
                tracing::debug!(event = "output_already_muted");
                return None;
            }
            let on: u32 = 1;
            if set(device, switch, &on) {
                tracing::debug!(event = "output_muted", method = "switch");
                return Some(Mute::Switch { device });
            }
        }
    }

    // No usable mute switch on the master element. Taking the volume to zero
    // is the same silence, and the user's volume keys still bring it back.
    let volume = address(
        kAudioDevicePropertyVolumeScalar,
        kAudioObjectPropertyScopeOutput,
    );
    if settable(device, volume) {
        let mut previous: f32 = 0.0;
        if get(device, volume, &mut previous) {
            if previous <= 0.0 {
                tracing::debug!(event = "output_already_muted");
                return None;
            }
            let silent: f32 = 0.0;
            if set(device, volume, &silent) {
                tracing::debug!(event = "output_muted", method = "volume");
                return Some(Mute::Volume { device, previous });
            }
        }
    }

    // Some devices expose neither on the master element, only per channel.
    // Silencing is a courtesy, so that is a log line and nothing more.
    tracing::debug!(event = "output_mute_unavailable");
    None
}

/// Put back exactly what [`mute`] changed.
pub fn restore(mute: &Mute) {
    let restored = match *mute {
        Mute::Switch { device } => {
            let off: u32 = 0;
            set(
                device,
                address(kAudioDevicePropertyMute, kAudioObjectPropertyScopeOutput),
                &off,
            )
        }
        Mute::Volume { device, previous } => set(
            device,
            address(
                kAudioDevicePropertyVolumeScalar,
                kAudioObjectPropertyScopeOutput,
            ),
            &previous,
        ),
    };

    if restored {
        tracing::debug!(event = "output_restored");
    } else {
        // The device is usually gone: unplugged headphones take their mute
        // state with them, and the one now playing was never touched.
        tracing::warn!(event = "output_restore_failed");
    }
}
