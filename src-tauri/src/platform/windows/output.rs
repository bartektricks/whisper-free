//! Silencing whatever the machine is playing, for the length of a recording.
//!
//! Reached only as `platform::backend::output`. Windows offers no supported way
//! to mute one *other* application, so this mutes the render endpoint
//! everything is playing through and puts it back afterwards. See
//! `docs/decisions/0009-muting-other-audio-while-recording.md`.

use std::cell::Cell;

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

thread_local! {
    /// COM is initialised once here and never uninitialised.
    ///
    /// Only the `mute` thread reaches this module, and that thread lives as
    /// long as the process, so there is nothing to tear down and no other
    /// thread whose apartment this could disturb.
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

/// What was silenced.
///
/// The endpoint is held rather than looked up again on the way out: plugging in
/// headphones mid-dictation changes which endpoint is default, and unmuting
/// *that* one would leave the muted one muted while touching a device nobody
/// asked about.
pub struct Mute {
    volume: IAudioEndpointVolume,
}

fn ensure_com() {
    COM_READY.with(|ready| {
        if ready.get() {
            return;
        }
        ready.set(true);
        // SAFETY: no reserved pointer, and a plain threading-model request.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            // Almost certainly `RPC_E_CHANGED_MODE`, meaning something else put
            // this thread in an apartment already. The calls below still work,
            // so this is worth a line rather than a refusal.
            tracing::debug!(event = "com_already_initialised", hresult = hr.0);
        }
    });
}

fn endpoint_volume() -> Option<IAudioEndpointVolume> {
    ensure_com();
    // SAFETY: COM is initialised on this thread, and every interface below is
    // used only on the thread that created it.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        device
            .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            .ok()
    }
}

/// Silence the default render endpoint, if there is one and it is not silent
/// already.
pub fn mute() -> Option<Mute> {
    let volume = endpoint_volume()?;

    // SAFETY: the interface was activated on this thread a moment ago.
    unsafe {
        match volume.GetMute() {
            // Somebody muted their own machine. Unmuting them afterwards would
            // be worse than never having muted at all.
            Ok(muted) if muted.as_bool() => {
                tracing::debug!(event = "output_already_muted");
                return None;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!(event = "output_mute_unavailable", error = %e);
                return None;
            }
        }

        if let Err(e) = volume.SetMute(true, std::ptr::null()) {
            tracing::debug!(event = "output_mute_unavailable", error = %e);
            return None;
        }
    }

    tracing::debug!(event = "output_muted", method = "switch");
    Some(Mute { volume })
}

/// Put back exactly what [`mute`] changed.
pub fn restore(mute: &Mute) {
    // SAFETY: as above; the endpoint is the one this thread activated.
    match unsafe { mute.volume.SetMute(false, std::ptr::null()) } {
        Ok(()) => tracing::debug!(event = "output_restored"),
        // Usually the device is gone: unplugged headphones take their mute
        // state with them, and the endpoint now playing was never touched.
        Err(e) => tracing::warn!(event = "output_restore_failed", error = %e),
    }
}
