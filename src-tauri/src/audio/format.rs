//! Audio format conversion using rubato resampler
//!
//! Provides high-quality resampling from device sample rate (typically 48kHz)
//! to 16kHz for transcription, including stereo to mono conversion.

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
#[cfg(test)]
use rubato::{Fft, FixedSync};

/// Audio format converter for resampling and channel conversion
pub struct AudioConverter {
    resampler: Async<f32>,
    source_channels: usize,
}

impl AudioConverter {
    /// Create a new audio converter
    ///
    /// # Arguments
    /// * `source_rate` - Source sample rate (e.g., 48000)
    /// * `target_rate` - Target sample rate (typically 16000)
    /// * `source_channels` - Number of source channels (1 or 2)
    /// * `chunk_size` - Size of input chunks in frames (e.g., 1024)
    pub fn new(
        source_rate: u32,
        target_rate: u32,
        source_channels: usize,
        chunk_size: usize,
    ) -> Result<Self, rubato::ResamplerConstructionError> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: Some(0.95),
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };

        let resampler = Async::<f32>::new_sinc(
            target_rate as f64 / source_rate as f64,
            2.0, // max_resample_ratio_relative
            &params,
            chunk_size,
            1, // mono output (downmix happens before resampler)
            FixedAsync::Input,
        )?;

        Ok(Self {
            resampler,
            source_channels,
        })
    }

    /// Process audio samples
    ///
    /// Converts stereo to mono, resamples to target rate, and returns f32 samples.
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>, rubato::ResampleError> {
        let mono = self.to_mono(input);
        let frames = mono.len();

        let input_adapter = InterleavedSlice::new(&mono, 1, frames)
            .expect("input adapter: sizes are internally consistent");

        let max_out = self.resampler.output_frames_max();
        let mut out_data = vec![0.0f32; max_out];
        let mut output_adapter = InterleavedSlice::new_mut(&mut out_data, 1, max_out)
            .expect("output adapter: sizes are internally consistent");

        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            active_channels_mask: None,
            partial_len: None,
        };

        let (_in_frames, out_frames) = self.resampler.process_into_buffer(
            &input_adapter,
            &mut output_adapter,
            Some(&indexing),
        )?;

        out_data.truncate(out_frames);
        Ok(out_data)
    }

    /// Process and convert to i16
    ///
    /// This is a convenience method that calls `process` and converts the output
    /// to 16-bit signed integers suitable for WAV files.
    pub fn process_to_i16(&mut self, input: &[f32]) -> Result<Vec<i16>, rubato::ResampleError> {
        let resampled = self.process(input)?;
        Ok(f32_to_i16(&resampled))
    }

    /// Finalise the stream: resample a trailing partial chunk (fewer than
    /// `chunk_size` frames) and then drain the resampler's internal delay line.
    ///
    /// `Async<f32>` with `FixedAsync::Input` buffers `sinc_len/2` samples of
    /// lookahead state in its delay line, so the final output frames sit inside
    /// the resampler after the last full chunk is fed. Without this drain the
    /// tail of every recording — up to that delay — is silently lost. Call this
    /// exactly once at end of stream, after all full chunks have gone through
    /// [`process_to_i16`].
    ///
    /// In rubato the drain is expressed by calling `process_into_buffer` with
    /// `partial_len = Some(n)` where `n` is the number of leftover input frames
    /// (0 if the accumulator was empty). When `partial_len < chunk_size`, rubato
    /// zero-pads the input to fill one chunk and emits only the output frames
    /// that correspond to real input — including the samples stored in the delay
    /// line — so all trailing samples are recovered.
    ///
    /// `leftover` is the remaining interleaved source samples that did not fill a
    /// whole `chunk_size` chunk (may be empty).
    pub fn finish_to_i16(&mut self, leftover: &[f32]) -> Result<Vec<i16>, rubato::ResampleError> {
        let mono = self.to_mono(leftover);
        let leftover_frames = mono.len();

        // Pad the mono buffer to the resampler's expected input size so that
        // InterleavedSlice can wrap it. The extra zeros are excluded from the
        // resampled output because partial_len tells the resampler how many
        // real frames were present.
        let input_frames_next = self.resampler.input_frames_next();
        let padded_len = leftover_frames.max(input_frames_next);
        let mut padded = mono;
        padded.resize(padded_len, 0.0);

        let input_adapter = InterleavedSlice::new(&padded, 1, padded_len)
            .expect("input adapter: sizes are internally consistent");

        // output_delay() is the number of output frames the resampler will still
        // emit when draining. Allocate that plus whatever the partial input
        // produces; add a safety margin.
        let delay_frames = self.resampler.output_delay();
        let out_capacity = delay_frames + input_frames_next + 64;
        let mut out_data = vec![0.0f32; out_capacity];
        let mut output_adapter = InterleavedSlice::new_mut(&mut out_data, 1, out_capacity)
            .expect("output adapter: sizes are internally consistent");

        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            active_channels_mask: None,
            // partial_len = Some(n) triggers the flush: rubato processes exactly
            // n real frames (zero = drain only) then emits the delay line tail.
            partial_len: Some(leftover_frames),
        };

        let (_in_frames, out_frames) = self.resampler.process_into_buffer(
            &input_adapter,
            &mut output_adapter,
            Some(&indexing),
        )?;

        out_data.truncate(out_frames);
        Ok(f32_to_i16(&out_data))
    }

    /// Downmix interleaved source frames to mono.
    fn to_mono(&self, input: &[f32]) -> Vec<f32> {
        if self.source_channels == 2 {
            stereo_to_mono(input)
        } else {
            input.to_vec()
        }
    }
}

/// Convert f32 samples to i16 with proper scaling
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect()
}

/// Convert i16 samples to f32 with proper scaling
pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| s as f32 / 32768.0).collect()
}

/// Mix stereo to mono
pub fn stereo_to_mono(samples: &[f32]) -> Vec<f32> {
    samples
        .chunks(2)
        .map(|c| {
            if c.len() == 2 {
                (c[0] + c[1]) / 2.0
            } else {
                c[0]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A/B the shipped sinc resampler against rubato's FFT one on the fixed
    /// 48 kHz -> 16 kHz ratio live capture actually uses (#104).
    ///
    /// Ignored: it is a measurement, not an assertion, and its numbers move
    /// with whatever else the machine is doing. Run it deliberately, on an
    /// otherwise quiet machine:
    ///
    /// ```text
    /// cargo test --release --lib resampler_ab -- --ignored --nocapture
    /// ```
    ///
    /// Interleaved rather than one-after-the-other, so a machine that gets
    /// busier partway through spends that on both arms instead of only the
    /// second. `--release` matters: a debug build measures rustc's bounds
    /// checks, not the algorithms.
    ///
    /// Read the RATIO, not the milliseconds. Across five runs on one machine
    /// the absolute figures moved by 6x with load while the ratio stayed
    /// between 1.3x and 1.8x — which is the interleaving working, and the
    /// reason a single run of this proves nothing. Run it several times.
    #[test]
    #[ignore]
    fn resampler_ab_sinc_vs_fft() {
        use std::time::Instant;

        const SOURCE_RATE: u32 = 48_000;
        const TARGET_RATE: u32 = 16_000;
        const CHUNK: usize = 1024;
        // 60 s of mono audio per arm per round: long enough that one chunk's
        // noise does not decide the answer.
        const CHUNKS_PER_ROUND: usize = 60 * SOURCE_RATE as usize / CHUNK;
        const ROUNDS: usize = 5;

        // A 440 Hz tone, so both arms do real filtering work rather than
        // resampling silence.
        let chunk: Vec<f32> = (0..CHUNK)
            .map(|i| {
                (i as f32 / SOURCE_RATE as f32 * 440.0 * 2.0 * std::f32::consts::PI).sin() * 0.5
            })
            .collect();

        let mut sinc_total = std::time::Duration::ZERO;
        let mut fft_total = std::time::Duration::ZERO;
        let mut sinc_frames = 0usize;
        let mut fft_frames = 0usize;

        for _ in 0..ROUNDS {
            let mut sinc = AudioConverter::new(SOURCE_RATE, TARGET_RATE, 1, CHUNK).unwrap();
            let start = Instant::now();
            for _ in 0..CHUNKS_PER_ROUND {
                sinc_frames += sinc.process(&chunk).unwrap().len();
            }
            sinc_total += start.elapsed();

            let mut fft = Fft::<f32>::new(
                SOURCE_RATE as usize,
                TARGET_RATE as usize,
                CHUNK,
                1,
                FixedSync::Input,
            )
            .unwrap();
            let max_out = fft.output_frames_max();
            let mut out = vec![0.0f32; max_out];
            let start = Instant::now();
            for _ in 0..CHUNKS_PER_ROUND {
                let input = InterleavedSlice::new(&chunk, 1, CHUNK).unwrap();
                let mut output = InterleavedSlice::new_mut(&mut out, 1, max_out).unwrap();
                let (_, frames) = fft.process_into_buffer(&input, &mut output, None).unwrap();
                fft_frames += frames;
            }
            fft_total += start.elapsed();
        }

        let audio_secs = (ROUNDS * CHUNKS_PER_ROUND * CHUNK) as f64 / SOURCE_RATE as f64;
        println!(
            "sinc (shipped): {:>8.1} ms for {audio_secs:.0} s of audio  ({:.0}x real time, {sinc_frames} frames out)",
            sinc_total.as_secs_f64() * 1000.0,
            audio_secs / sinc_total.as_secs_f64()
        );
        println!(
            "fft            : {:>8.1} ms for {audio_secs:.0} s of audio  ({:.0}x real time, {fft_frames} frames out)",
            fft_total.as_secs_f64() * 1000.0,
            audio_secs / fft_total.as_secs_f64()
        );
        println!(
            "ratio          : sinc is {:.2}x the FFT cost",
            sinc_total.as_secs_f64() / fft_total.as_secs_f64()
        );
    }

    #[test]
    fn test_converter_new() {
        let converter = AudioConverter::new(48000, 16000, 2, 1024);
        assert!(converter.is_ok());
    }

    #[test]
    fn test_finish_to_i16_empty_leftover_does_not_error() {
        // A recording that stops on an exact chunk boundary leaves an empty
        // leftover. finish_to_i16 must drain the resampler delay via partial_len=Some(0).
        let mut converter = AudioConverter::new(48000, 16000, 1, 1024).unwrap();
        // Prime the resampler with a full chunk so it has delay-line state to drain.
        let chunk: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
        converter.process_to_i16(&chunk).unwrap();
        // The critical call: empty leftover must succeed, not error.
        let tail = converter
            .finish_to_i16(&[])
            .expect("empty-leftover finalise must not error");
        // It should emit the buffered delay tail (non-empty), proving the drain ran.
        assert!(
            !tail.is_empty(),
            "delay-line drain should emit the buffered tail"
        );
    }

    #[test]
    fn test_finish_to_i16_partial_leftover() {
        // A short trailing block (fewer than chunk_size frames) must resample and
        // emit the tail without error.
        let mut converter = AudioConverter::new(48000, 16000, 1, 1024).unwrap();
        let chunk: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
        converter.process_to_i16(&chunk).unwrap();
        let leftover: Vec<f32> = (0..300).map(|i| (i as f32 * 0.02).sin() * 0.3).collect();
        let tail = converter
            .finish_to_i16(&leftover)
            .expect("partial-leftover finalise must not error");
        assert!(!tail.is_empty());
    }

    /// Assert that resampling `input_len` 48kHz frames produces at least the
    /// expected number of 16kHz output samples (ratio 1/3) with no tail truncation.
    ///
    /// This test feeds N full chunks plus a partial leftover, then calls
    /// finish_to_i16. The total output (mid-stream + tail) must be >= the
    /// expected conversion result, ensuring no samples are silently dropped.
    #[test]
    fn test_no_tail_truncation_full_sample_count() {
        let source_rate = 48_000u32;
        let target_rate = 16_000u32;
        let chunk_size = 1024usize;
        let num_full_chunks = 5usize;
        let leftover_size = 300usize;

        let total_input_frames = num_full_chunks * chunk_size + leftover_size;
        // Expected output frames = input * (target/source), rounded down.
        let expected_min_output =
            (total_input_frames as f64 * target_rate as f64 / source_rate as f64) as usize;

        let mut converter = AudioConverter::new(source_rate, target_rate, 1, chunk_size).unwrap();

        let all_input: Vec<f32> = (0..total_input_frames)
            .map(|i| (i as f32 * 0.01).sin() * 0.3)
            .collect();

        let mut total_output_samples = 0usize;

        for chunk in all_input[..num_full_chunks * chunk_size].chunks(chunk_size) {
            let out = converter.process_to_i16(chunk).unwrap();
            total_output_samples += out.len();
        }

        let leftover = &all_input[num_full_chunks * chunk_size..];
        let tail = converter.finish_to_i16(leftover).unwrap();
        total_output_samples += tail.len();

        assert!(
            total_output_samples >= expected_min_output,
            "expected >= {} output samples (no truncation), got {}",
            expected_min_output,
            total_output_samples
        );
    }

    #[test]
    fn test_stereo_to_mono() {
        let stereo = vec![0.5, -0.5, 0.3, -0.3, 0.1, -0.1];
        let mono = stereo_to_mono(&stereo);
        assert_eq!(mono.len(), 3);
        assert!((mono[0] - 0.0).abs() < 0.0001); // (0.5 + -0.5) / 2
        assert!((mono[1] - 0.0).abs() < 0.0001); // (0.3 + -0.3) / 2
        assert!((mono[2] - 0.0).abs() < 0.0001); // (0.1 + -0.1) / 2
    }

    #[test]
    fn test_f32_to_i16() {
        let f32_samples = vec![1.0, 0.5, 0.0, -0.5, -1.0];
        let i16_samples = f32_to_i16(&f32_samples);

        assert_eq!(i16_samples[0], 32767); // 1.0 -> max positive
        assert_eq!(i16_samples[1], 16383); // 0.5 -> half max
        assert_eq!(i16_samples[2], 0); // 0.0 -> zero
        assert_eq!(i16_samples[3], -16383); // -0.5 -> half min
        assert_eq!(i16_samples[4], -32767); // -1.0 -> max negative
    }

    #[test]
    fn test_i16_to_f32() {
        let i16_samples = vec![32767i16, 0, -32768];
        let f32_samples = i16_to_f32(&i16_samples);

        assert!((f32_samples[0] - 1.0).abs() < 0.0001);
        assert!(f32_samples[1].abs() < 0.0001);
        assert!((f32_samples[2] - (-1.0)).abs() < 0.0001);
    }

    #[test]
    fn test_resampling_produces_output() {
        let mut converter = AudioConverter::new(48000, 16000, 1, 1024).unwrap();

        // Process multiple chunks to account for rubato's internal buffering
        let chunk: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.001).sin()).collect();
        let mut total_output = 0;

        for _ in 0..10 {
            let output = converter.process(&chunk).unwrap();
            total_output += output.len();
        }

        // After 10 chunks of 1024 samples at 48kHz, we should have produced
        // approximately 10240 / 3 ≈ 3413 samples at 16kHz
        // Allow wide tolerance due to rubato's internal buffering
        assert!(
            total_output > 2500,
            "Expected > 2500 samples, got {}",
            total_output
        );
    }

    #[test]
    fn test_process_to_i16() {
        let mut converter = AudioConverter::new(48000, 16000, 1, 1024).unwrap();

        // Generate a simple test signal
        let input: Vec<f32> = (0..3072).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();

        // The conversion must succeed; the `unwrap` is the assertion. rubato may
        // legitimately buffer and return an empty chunk, so output length is not
        // asserted.
        let _output = converter.process_to_i16(&input).unwrap();
    }

    #[test]
    fn test_stereo_input() {
        let mut converter = AudioConverter::new(48000, 16000, 2, 512).unwrap();

        // Stereo input: 1024 samples = 512 frames
        let input: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();

        let output = converter.process(&input).unwrap();

        // Should produce some output
        assert!(!output.is_empty());
    }
}
