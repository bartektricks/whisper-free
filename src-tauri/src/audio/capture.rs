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
    pub fn spawn() -> Self {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || audio_thread(rx))
            .expect("could not start the audio thread");
        Self { tx }
    }

    fn request<T>(&self, make: impl FnOnce(Sender<T>) -> Command) -> Result<T, AudioError> {
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(make(reply_tx))
            .map_err(|_| AudioError::EngineGone)?;
        reply_rx.recv().map_err(|_| AudioError::EngineGone)
    }

    pub fn list_devices(&self) -> Result<Vec<AudioDevice>, AudioError> {
        self.request(Command::ListDevices)?
    }

    /// Begin capturing from `device`, or the system default when `None`.
    pub fn start(&self, device: Option<String>) -> Result<(), AudioError> {
        self.request(|reply| Command::Start { device, reply })?
    }

    /// Stop capturing and return the audio as 16 kHz mono.
    pub fn stop(&self) -> Result<AudioBuffer, AudioError> {
        self.request(|reply| Command::Stop { reply })?
    }

    /// Stop capturing and throw the audio away.
    pub fn cancel(&self) -> Result<(), AudioError> {
        self.request(|reply| Command::Cancel { reply })
    }

    pub fn is_recording(&self) -> bool {
        self.request(|reply| Command::IsRecording { reply }).unwrap_or(false)
    }
}

/// State for one in-progress recording.
struct Active {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    channels: u16,
    sample_rate: u32,
    device_name: String,
}

fn audio_thread(rx: Receiver<Command>) {
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
        _stream,
        samples,
        channels,
        sample_rate,
        device_name,
    } = active;

    // Dropping the stream first guarantees no callback is still appending.
    drop(_stream);

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
        duration_ms = buffer.duration().as_millis() as u64
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

/// cpal identifies devices by a `DeviceId` that is stable across reboots; the
/// `Display` name is for humans only.
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
/// skips a conversion step and the quality loss that comes with it.
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

    let samples = Arc::new(Mutex::new(Vec::<f32>::with_capacity(
        sample_rate as usize * channels as usize * 8,
    )));
    let cap = MAX_RECORDING_SECS * sample_rate as usize * channels as usize;

    let error_name = device_name.clone();
    let on_error = move |e: cpal::Error| {
        // Typically an unplugged device. Log it; the empty/short buffer is what
        // the user ends up seeing.
        tracing::error!(event = "audio_stream_error", device = %error_name, error = %e);
    };

    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, config, samples.clone(), cap, on_error),
        SampleFormat::I16 => build_stream::<i16>(&device, config, samples.clone(), cap, on_error),
        SampleFormat::U16 => build_stream::<u16>(&device, config, samples.clone(), cap, on_error),
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
        _stream: stream,
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
                let room = cap - guard.len();
                guard.extend(
                    data.iter()
                        .take(room)
                        .map(|s| -> f32 { cpal::Sample::from_sample(*s) }),
                );
            },
            on_error,
            Some(Duration::from_secs(2)),
        )
        .map_err(|e| map_build_error(e, device))
}

/// Turn a cpal build failure into something the app can act on.
fn map_build_error(e: cpal::Error, device: &Device) -> AudioError {
    let text = e.to_string();
    // cpal surfaces a TCC refusal as a generic backend error, so match on the
    // message. When in doubt this stays a generic start failure.
    let lowered = text.to_lowercase();
    if lowered.contains("permission") || lowered.contains("not authorized") {
        return AudioError::PermissionDenied;
    }
    if lowered.contains("device not available") || lowered.contains("devicenotavailable") {
        return AudioError::DeviceUnavailable(device.to_string());
    }
    AudioError::StreamStart(text)
}
