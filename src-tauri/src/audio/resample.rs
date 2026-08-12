//! Turning whatever the microphone gives us into what the model wants.
//!
//! Decision 0001 fixes the ASR contract at 16 kHz mono f32. Devices rarely hand
//! that over directly, so capture is downmixed to mono and resampled here.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use super::AudioError;

/// Average interleaved channels down to mono.
///
/// Averaging rather than picking channel 0: on a stereo mic, one channel may
/// carry most of the voice, and dropping it would throw away signal.
pub fn to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Resample mono audio from `from_rate` to `to_rate`.
///
/// Returns the input untouched when the rates already match.
pub fn resample_mono(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>, AudioError> {
    if from_rate == to_rate {
        return Ok(samples.to_vec());
    }
    if from_rate == 0 || to_rate == 0 {
        return Err(AudioError::Resample("sample rate of zero".into()));
    }
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    const CHANNELS: usize = 1;
    const CHUNK: usize = 1024;

    let mut resampler = Fft::<f32>::new(
        from_rate as usize,
        to_rate as usize,
        CHUNK,
        CHANNELS,
        FixedSync::Input,
    )
    .map_err(|e| AudioError::Resample(e.to_string()))?;

    let input_frames = samples.len();
    let expected_out =
        (input_frames as f64 * to_rate as f64 / from_rate as f64).ceil() as usize;
    let delay = resampler.output_delay();

    // Room for the delay at the head and the zero-padded tail of the final
    // partial chunk, both of which get trimmed below.
    let mut out = vec![0.0f32; expected_out + delay + CHUNK * 2];

    let input = InterleavedSlice::new(samples, CHANNELS, input_frames)
        .map_err(|e| AudioError::Resample(e.to_string()))?;
    let out_capacity = out.len();
    let mut output = InterleavedSlice::new_mut(&mut out, CHANNELS, out_capacity)
        .map_err(|e| AudioError::Resample(e.to_string()))?;

    let mut indexing = Indexing::new();
    let mut frames_left = input_frames;
    let mut next = resampler.input_frames_next();

    while frames_left >= next {
        let (used, produced) = resampler
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        indexing.input_offset += used;
        indexing.output_offset += produced;
        frames_left -= used;
        next = resampler.input_frames_next();
    }

    // Flush the remainder, which is shorter than a full chunk.
    indexing.partial_len = Some(frames_left);
    resampler
        .process_into_buffer(&input, &mut output, Some(&indexing))
        .map_err(|e| AudioError::Resample(e.to_string()))?;

    // Drop the resampler's start-up delay, then keep only as much as the input
    // duration justifies — the rest is padding from the final partial chunk.
    let end = (delay + expected_out).min(out.len());
    Ok(out[delay.min(end)..end].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_input_passes_through_untouched() {
        let samples = vec![0.1, -0.2, 0.3];
        assert_eq!(to_mono(&samples, 1), samples);
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        // L/R pairs: (1,0) -> 0.5, (0.5,0.5) -> 0.5
        let interleaved = vec![1.0, 0.0, 0.5, 0.5];
        assert_eq!(to_mono(&interleaved, 2), vec![0.5, 0.5]);
    }

    #[test]
    fn quiet_channel_still_contributes() {
        // Picking channel 0 alone would return silence and lose the voice.
        let interleaved = vec![0.0, 0.8, 0.0, 0.6];
        let mono = to_mono(&interleaved, 2);
        assert!(mono.iter().all(|s| *s > 0.0), "got {mono:?}");
    }

    #[test]
    fn trailing_partial_frame_is_dropped() {
        // Three samples cannot form two stereo frames; the stray one is ignored.
        let interleaved = vec![1.0, 1.0, 0.5];
        assert_eq!(to_mono(&interleaved, 2), vec![1.0]);
    }

    #[test]
    fn zero_channels_is_treated_as_mono_rather_than_dividing_by_zero() {
        let samples = vec![0.25, 0.5];
        assert_eq!(to_mono(&samples, 0), samples);
    }

    #[test]
    fn matching_rates_skip_resampling() {
        let samples = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_mono(&samples, 16_000, 16_000).unwrap(), samples);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert!(resample_mono(&[], 48_000, 16_000).unwrap().is_empty());
    }

    #[test]
    fn downsampling_produces_the_expected_length() {
        // One second at 48 kHz becomes one second at 16 kHz.
        let samples = vec![0.0f32; 48_000];
        let out = resample_mono(&samples, 48_000, 16_000).unwrap();
        let drift = (out.len() as i64 - 16_000).abs();
        assert!(drift <= 64, "expected ~16000 samples, got {}", out.len());
    }

    #[test]
    fn non_integer_ratio_produces_the_expected_length() {
        let samples = vec![0.0f32; 44_100];
        let out = resample_mono(&samples, 44_100, 16_000).unwrap();
        let drift = (out.len() as i64 - 16_000).abs();
        assert!(drift <= 64, "expected ~16000 samples, got {}", out.len());
    }

    #[test]
    fn a_sine_survives_resampling_with_its_amplitude_intact() {
        // 440 Hz is well under the 8 kHz Nyquist limit of the target rate, so
        // it should come through cleanly rather than aliased or attenuated.
        let from = 48_000u32;
        let to = 16_000u32;
        let freq = 440.0f32;
        let samples: Vec<f32> = (0..from as usize)
            .map(|i| {
                (2.0 * std::f32::consts::PI * freq * i as f32 / from as f32).sin() * 0.5
            })
            .collect();

        let out = resample_mono(&samples, from, to).unwrap();

        // Ignore the edges, where filter transients live.
        let core = &out[1000..out.len() - 1000];
        let peak = core.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            (peak - 0.5).abs() < 0.05,
            "amplitude should survive, peak was {peak}"
        );

        // Count zero crossings to confirm the frequency is preserved:
        // 440 Hz over ~0.875 s of core audio is ~770 crossings.
        let crossings = core.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let seconds = core.len() as f32 / to as f32;
        let measured = crossings as f32 / seconds;
        assert!(
            (measured - freq).abs() < 5.0,
            "expected ~{freq} Hz, measured {measured} Hz"
        );
    }

    #[test]
    fn silence_stays_silent() {
        let out = resample_mono(&vec![0.0f32; 24_000], 48_000, 16_000).unwrap();
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }
}
