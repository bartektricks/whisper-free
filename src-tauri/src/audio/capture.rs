//! Microphone capture, driven from a dedicated thread.
//!
//! `cpal::Stream` is not `Send` on macOS, so it cannot live in Tauri's managed
//! state. Instead one thread owns the stream for its whole life and the rest of
//! the app talks to it over a channel.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig, SupportedStreamConfig};

use super::resample::{resample_mono, to_mono};
use super::{AudioDevice, AudioError, TARGET_SAMPLE_RATE};
use crate::asr::AudioBuffer;

/// Guards against a stuck hotkey filling memory. 10 minutes of 16 kHz mono f32
/// is about 38 MB.
const MAX_RECORDING_SECS: usize = 600;

enum Command {
    ListDevices(Sender<Result<Vec<AudioDevice>, AudioError>>),
    Start {
        device: Option<String>,
        reply: Sender<Result<(), AudioError>>,
    },
    Stop {
        reply: Sender<Result<AudioBuffer, AudioError>>,
    },
    Cancel {
        reply: Sender<()>,
    },
    IsRecording {
        reply: Sender<bool>,
    },
}

/// Handle to the audio thread.
pub struct AudioEngine {
    tx: Sender<Command>,
}

impl AudioEngine {
    #[must_use]
    pub fn spawn() -> Self {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || audio_thread(&rx));

        // The receiver goes down with the failed spawn, so every later request
        // returns `EngineGone` — a microphone error the user can read, rather
        // than a panic at startup.
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not start the audio thread");
        }
        Self { tx }
    }

    fn request<T>(&self, make: impl FnOnce(Sender<T>) -> Command) -> Result<T, AudioError> {
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(make(reply_tx))
            .map_err(|_| AudioError::EngineGone)?;
        reply_rx.recv().map_err(|_| AudioError::EngineGone)
    }

    /// # Errors
    ///
    /// [`AudioError::NoDevice`] when the host reports no inputs, or
    /// [`AudioError::DeviceQuery`] when it cannot be asked.
    pub fn list_devices(&self) -> Result<Vec<AudioDevice>, AudioError> {
        self.request(Command::ListDevices)?
    }

    /// Begin capturing from `device`, or the system default when `None`.
    ///
    /// # Errors
    ///
    /// [`AudioError::AlreadyRecording`] when a take is in flight,
    /// [`AudioError::PermissionDenied`] when microphone access is refused, or
    /// [`AudioError::DeviceUnavailable`] when the chosen device has gone.
    pub fn start(&self, device: Option<String>) -> Result<(), AudioError> {
        self.request(|reply| Command::Start { device, reply })?
    }

    /// Stop capturing and return the audio as 16 kHz mono.
    ///
    /// # Errors
    ///
    /// [`AudioError::NotRecording`] when nothing was running,
    /// [`AudioError::Empty`] when the take contained no samples, or
    /// [`AudioError::Resample`] when the conversion fails.
    pub fn stop(&self) -> Result<AudioBuffer, AudioError> {
        self.request(|reply| Command::Stop { reply })?
    }

    /// Stop capturing and throw the audio away.
    ///
    /// # Errors
    ///
    /// [`AudioError::EngineGone`] when the audio thread is no longer running.
    pub fn cancel(&self) -> Result<(), AudioError> {
        self.request(|reply| Command::Cancel { reply })
    }

    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.request(|reply| Command::IsRecording { reply })
            .unwrap_or(false)
    }
}

/// State for one in-progress recording.
struct Active {
    /// Held only to keep the capture alive; dropping it stops the callbacks.
    stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    channels: u16,
    sample_rate: u32,
    device_name: String,
}

fn audio_thread(rx: &Receiver<Command>) {
    let mut active: Option<Active> = None;

    while let Ok(command) = rx.recv() {
        match command {
            Command::ListDevices(reply) => {
                let _ = reply.send(list_devices());
            }
            Command::IsRecording { reply } => {
                let _ = reply.send(active.is_some());
            }
            Command::Start { device, reply } => {
                if active.is_some() {
                    let _ = reply.send(Err(AudioError::AlreadyRecording));
                    continue;
                }
                match start_stream(device.as_deref()) {
                    Ok(started) => {
                        tracing::info!(
                            event = "recording_started",
                            device = %started.device_name,
                            sample_rate = started.sample_rate,
                            channels = started.channels
                        );
                        active = Some(started);
                        let _ = reply.send(Ok(()));
                    }
                    Err(e) => {
                        tracing::warn!(event = "recording_start_failed", error = %e);
                        let _ = reply.send(Err(e));
                    }
                }
            }
            Command::Stop { reply } => {
                let Some(current) = active.take() else {
                    let _ = reply.send(Err(AudioError::NotRecording));
                    continue;
                };
                let _ = reply.send(finish(current));
            }
            Command::Cancel { reply } => {
                if active.take().is_some() {
                    tracing::info!(event = "recording_cancelled");
                }
                let _ = reply.send(());
            }
        }
    }
}

/// Drop the stream, then convert what was captured into the ASR contract.
fn finish(active: Active) -> Result<AudioBuffer, AudioError> {
    let Active {
        stream,
        samples,
        channels,
        sample_rate,
        device_name,
    } = active;

    // Dropping the stream first guarantees no callback is still appending.
    drop(stream);

    let raw = match samples.lock() {
        Ok(mut guard) => std::mem::take(&mut *guard),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };

    if raw.is_empty() {
        tracing::warn!(event = "recording_stopped", device = %device_name, samples = 0);
        return Err(AudioError::Empty);
    }

    let mono = to_mono(&raw, channels);
    let resampled = resample_mono(&mono, sample_rate, TARGET_SAMPLE_RATE)?;
    let buffer = AudioBuffer::new(resampled, TARGET_SAMPLE_RATE);

    // Sample counts and durations only — never the audio itself (plan §18).
    tracing::info!(
        event = "recording_stopped",
        device = %device_name,
        duration_ms = crate::millis(buffer.duration())
    );

    if buffer.is_empty() {
        return Err(AudioError::Empty);
    }
    Ok(buffer)
}

fn host_devices() -> Result<Vec<Device>, AudioError> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|d| d.collect())
        .map_err(|e| AudioError::DeviceQuery(e.to_string()))
}

/// cpal identifies devices by a `DeviceId`; the `Display` name is for humans
/// only.
///
/// CoreAudio and WASAPI ids survive reboots and reconnection, which is what
/// makes them safe to persist in settings. ALSA ids are positional, so a Linux
/// backend would need to re-check this assumption — a device that moves is
/// reported as unavailable rather than silently swapped, so it fails loudly.
fn device_id(device: &Device) -> Option<String> {
    device.id().ok().map(|id| id.to_string())
}

fn list_devices() -> Result<Vec<AudioDevice>, AudioError> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| device_id(&d));

    let devices: Vec<AudioDevice> = host_devices()?
        .iter()
        .filter_map(|d| {
            let id = device_id(d)?;
            Some(AudioDevice {
                is_default: Some(&id) == default_id.as_ref(),
                name: d.to_string(),
                id,
            })
        })
        .collect();

    if devices.is_empty() {
        return Err(AudioError::NoDevice);
    }
    Ok(devices)
}

fn find_device(requested: Option<&str>) -> Result<Device, AudioError> {
    let host = cpal::default_host();
    match requested {
        Some(id) => host_devices()?
            .into_iter()
            .find(|d| device_id(d).as_deref() == Some(id))
            // A microphone that was unplugged since it was chosen: say so
            // rather than silently recording from something else.
            .ok_or_else(|| AudioError::DeviceUnavailable(id.to_string())),
        None => host.default_input_device().ok_or(AudioError::NoDevice),
    }
}

/// Choose a capture format, preferring one that needs no resampling.
///
/// CoreAudio will hand us 16 kHz directly on most built-in microphones, which
/// skips a conversion step and the quality loss that comes with it. WASAPI in
/// shared mode will not, so on Windows the resampler is the normal path rather
/// than the exception; the cost is negligible against inference.
fn pick_config(device: &Device) -> Result<SupportedStreamConfig, AudioError> {
    if let Ok(ranges) = device.supported_input_configs() {
        let native_16k = ranges
            .filter_map(|range| range.try_with_sample_rate(TARGET_SAMPLE_RATE))
            .min_by_key(|cfg| {
                // Fewest channels first, then prefer f32 to avoid a conversion.
                (
                    cfg.channels(),
                    u8::from(cfg.sample_format() != SampleFormat::F32),
                )
            });
        if let Some(cfg) = native_16k {
            return Ok(cfg);
        }
    }
    device
        .default_input_config()
        .map_err(|e| AudioError::StreamStart(e.to_string()))
}

fn start_stream(requested: Option<&str>) -> Result<Active, AudioError> {
    let device = find_device(requested)?;
    let device_name = device.to_string();
    let supported = pick_config(&device)?;

    let sample_format = supported.sample_format();
    let channels = supported.channels();
    let sample_rate = supported.sample_rate();
    let config: StreamConfig = supported.config();

    let per_second = usize::try_from(sample_rate)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(channels));
    // Eight seconds up front covers a typical dictation without reallocating.
    let samples = Arc::new(Mutex::new(Vec::<f32>::with_capacity(
        per_second.saturating_mul(8),
    )));
    let cap = per_second.saturating_mul(MAX_RECORDING_SECS);

    let error_name = device_name.clone();
    let on_error = move |e: cpal::Error| {
        // Typically an unplugged device. Log it; the empty/short buffer is what
        // the user ends up seeing.
        tracing::error!(event = "audio_stream_error", device = %error_name, error = %e);
    };

    // CoreAudio hands back f32 or i16, but WASAPI and ALSA routinely offer the
    // wider integer formats too. Each arm converts straight to f32 — decision
    // 0001 found that a detour through 16-bit changes what the model hears.
    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, config, samples.clone(), cap, on_error),
        SampleFormat::F64 => build_stream::<f64>(&device, config, samples.clone(), cap, on_error),
        SampleFormat::I8 => build_stream::<i8>(&device, config, samples.clone(), cap, on_error),
        SampleFormat::I16 => build_stream::<i16>(&device, config, samples.clone(), cap, on_error),
        SampleFormat::I32 => build_stream::<i32>(&device, config, samples.clone(), cap, on_error),
        SampleFormat::U8 => build_stream::<u8>(&device, config, samples.clone(), cap, on_error),
        SampleFormat::U16 => build_stream::<u16>(&device, config, samples.clone(), cap, on_error),
        SampleFormat::U32 => build_stream::<u32>(&device, config, samples.clone(), cap, on_error),
        other => {
            return Err(AudioError::StreamStart(format!(
                "unsupported sample format {other:?}"
            )))
        }
    }?;

    stream
        .play()
        .map_err(|e| AudioError::StreamStart(e.to_string()))?;

    Ok(Active {
        stream,
        samples,
        channels,
        sample_rate,
        device_name,
    })
}

fn build_stream<T>(
    device: &Device,
    config: StreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
    cap: usize,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, AudioError>
where
    T: cpal::SizedSample + cpal::FromSample<T> + 'static,
    f32: cpal::FromSample<T>,
{
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let Ok(mut guard) = samples.lock() else { return };
                if guard.len() >= cap {
                    return; // stuck hotkey — stop growing rather than exhaust memory
                }
                let room = cap.saturating_sub(guard.len());
                guard.extend(
                    data.iter()
                        .take(room)
                        .map(|s| -> f32 { cpal::Sample::from_sample(*s) }),
                );
            },
            on_error,
            Some(Duration::from_secs(2)),
        )
        .map_err(|e| map_build_error(&e, &device.to_string()))
}

/// `E_ACCESSDENIED`, as `std::io::Error` renders it.
///
/// WASAPI reports a microphone-privacy refusal with this HRESULT, and cpal
/// builds the message with `io::Error::from_raw_os_error`, whose `Display`
/// appends the raw code. The sentence in front of the code comes from
/// `FormatMessageW` and is localised; the code is not.
const WINDOWS_ACCESS_DENIED: &str = "(os error -2147024891)";

/// Turn a cpal build failure into something the app can act on.
///
/// cpal classifies every backend error into an [`cpal::ErrorKind`], so the kind
/// is the source of truth — never the message, which is whatever the backend's
/// own `Display` says (`"Unauthorized"` for a macOS TCC refusal, a localised
/// sentence on Windows). `ErrorKind` is `#[non_exhaustive]`, and anything not
/// named here stays a generic start failure.
///
/// The one gap is WASAPI: it has no arm for `E_ACCESSDENIED` and reports the
/// privacy refusal as `BackendError`, so that single case is recovered from the
/// raw code in the message.
///
/// Takes the device name rather than the `Device` so it can be tested; a
/// `cpal::Device` cannot be constructed without a host.
fn map_build_error(e: &cpal::Error, device_name: &str) -> AudioError {
    match e.kind() {
        cpal::ErrorKind::PermissionDenied => AudioError::PermissionDenied,
        cpal::ErrorKind::DeviceNotAvailable => {
            AudioError::DeviceUnavailable(device_name.to_owned())
        }
        cpal::ErrorKind::BackendError
            if e.message()
                .is_some_and(|m| m.contains(WINDOWS_ACCESS_DENIED)) =>
        {
            AudioError::PermissionDenied
        }
        // `DeviceBusy` lands here on purpose: `StreamStart`'s user message
        // already says to check whether another app holds the microphone.
        _ => AudioError::StreamStart(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::{Error, ErrorKind};

    /// What `coreaudio-rs` renders `kAudioUnitErr_Unauthorized` as. The word is
    /// one token, so no substring of it reads as "not authorized".
    const MACOS_TCC_REFUSAL: &str = "Unauthorized";

    #[test]
    fn a_macos_tcc_refusal_tells_the_user_to_grant_access() {
        let e = Error::with_message(ErrorKind::PermissionDenied, MACOS_TCC_REFUSAL);
        assert_eq!(
            map_build_error(&e, "Built-in"),
            AudioError::PermissionDenied
        );
        // The point of the mapping: the message names where to fix it.
        assert!(map_build_error(&e, "Built-in")
            .user_message()
            .contains(crate::platform::strings::MICROPHONE_SETTINGS));
    }

    #[test]
    fn a_windows_privacy_refusal_is_read_from_the_code_not_the_sentence() {
        // cpal has no arm for E_ACCESSDENIED, so it arrives as a backend error.
        let english = Error::with_message(
            ErrorKind::BackendError,
            format!("Access is denied. {WINDOWS_ACCESS_DENIED}"),
        );
        // FormatMessageW is localised; the appended code is not.
        let polish = Error::with_message(
            ErrorKind::BackendError,
            format!("Odmowa dostępu. {WINDOWS_ACCESS_DENIED}"),
        );
        assert_eq!(
            map_build_error(&english, "Yeti"),
            AudioError::PermissionDenied
        );
        assert_eq!(
            map_build_error(&polish, "Yeti"),
            AudioError::PermissionDenied
        );
    }

    #[test]
    fn an_unclassified_backend_error_is_not_mistaken_for_a_refusal() {
        let e = Error::with_message(ErrorKind::BackendError, "AUDCLNT_E_CPUUSAGE_EXCEEDED");
        assert!(matches!(
            map_build_error(&e, "Yeti"),
            AudioError::StreamStart(_)
        ));
    }

    #[test]
    fn an_unplugged_device_is_named_so_the_log_says_which_one() {
        // The backend text says nothing about availability; only the kind does.
        let e = Error::with_message(
            ErrorKind::DeviceNotAvailable,
            "No matching default audio unit found",
        );
        assert_eq!(
            map_build_error(&e, "Yeti"),
            AudioError::DeviceUnavailable("Yeti".into())
        );
    }

    #[test]
    fn a_busy_device_stays_a_generic_start_failure() {
        let e = Error::new(ErrorKind::DeviceBusy);
        let mapped = map_build_error(&e, "Yeti");
        assert!(matches!(mapped, AudioError::StreamStart(_)));
        assert!(mapped.user_message().contains("no other app"));
    }

    #[test]
    fn an_unsupported_config_does_not_blame_permissions() {
        let e = Error::with_message(ErrorKind::UnsupportedConfig, "Unsupported sample rate");
        assert!(matches!(
            map_build_error(&e, "Yeti"),
            AudioError::StreamStart(_)
        ));
    }
}
