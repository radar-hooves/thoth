//! Audio metering for real-time level visualisation
//!
//! Provides RMS and peak level calculation for UI feedback.

use serde::Serialize;

/// Audio level data emitted to frontend
#[derive(Debug, Clone, Serialize)]
pub struct AudioLevel {
    /// RMS (root mean square) level, normalised 0.0-1.0
    pub rms: f32,
    /// Peak level with decay, normalised 0.0-1.0
    pub peak: f32,
    /// dB level (for display), typically -60 to 0
    pub db: f32,
}

/// Real-time audio meter
pub struct AudioMeter {
    peak: f32,
    decay_rate: f32,
    min_db: f32,
}

impl Default for AudioMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioMeter {
    /// Create a new audio meter
    ///
    /// Default decay rate gives ~300ms peak hold at 30Hz updates
    pub fn new() -> Self {
        Self {
            peak: 0.0,
            decay_rate: 0.95,
            min_db: -60.0,
        }
    }

    /// Process audio samples and return levels
    pub fn process(&mut self, samples: &[f32]) -> AudioLevel {
        if samples.is_empty() {
            return AudioLevel {
                rms: 0.0,
                peak: self.peak * self.decay_rate,
                db: self.min_db,
            };
        }

        // Calculate RMS
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        let rms = (sum_sq / samples.len() as f32).sqrt();

        // Find peak in this buffer
        let sample_peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        // Update peak with decay
        self.peak = if sample_peak > self.peak {
            sample_peak
        } else {
            self.peak * self.decay_rate
        };

        // Calculate dB (with floor)
        let db = if rms > 0.0 {
            (20.0 * rms.log10()).max(self.min_db)
        } else {
            self.min_db
        };

        AudioLevel {
            rms: rms.min(1.0),
            peak: self.peak.min(1.0),
            db,
        }
    }

    /// Reset the meter
    pub fn reset(&mut self) {
        self.peak = 0.0;
    }
}

/// Calculate RMS level for a buffer of samples
pub fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_meter_new() {
        let meter = AudioMeter::new();
        assert_eq!(meter.peak, 0.0);
    }

    #[test]
    fn test_process_silence() {
        let mut meter = AudioMeter::new();
        let samples = vec![0.0f32; 1024];
        let level = meter.process(&samples);

        assert_eq!(level.rms, 0.0);
        assert_eq!(level.peak, 0.0);
        assert_eq!(level.db, -60.0);
    }

    #[test]
    fn test_process_full_scale() {
        let mut meter = AudioMeter::new();
        let samples = vec![1.0f32; 1024];
        let level = meter.process(&samples);

        assert!((level.rms - 1.0).abs() < 0.001);
        assert!((level.peak - 1.0).abs() < 0.001);
        assert!((level.db - 0.0).abs() < 0.1);
    }

    #[test]
    fn test_process_sine_wave() {
        let mut meter = AudioMeter::new();
        // Generate a 1kHz sine wave at unit amplitude
        let samples: Vec<f32> = (0..1024)
            .map(|i| (2.0 * std::f32::consts::PI * i as f32 / 1024.0 * 10.0).sin())
            .collect();

        let level = meter.process(&samples);

        // RMS of a sine wave is amplitude / sqrt(2) ≈ 0.707
        assert!((level.rms - 0.707).abs() < 0.1, "RMS should be ~0.707");
        assert!((level.peak - 1.0).abs() < 0.1, "Peak should be ~1.0");
    }

    #[test]
    fn test_calculate_rms() {
        let samples = vec![0.5f32; 100];
        let rms = calculate_rms(&samples);
        assert!((rms - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_reset() {
        let mut meter = AudioMeter::new();

        let loud = vec![0.9f32; 512];
        meter.process(&loud);
        assert!(meter.peak > 0.8);

        meter.reset();
        assert_eq!(meter.peak, 0.0);
    }

    /// Verifies that writing non-zero samples into an AudioRingBuffer and then
    /// reading them through AudioMeter produces rms > 0. This proves the
    /// shared-buffer metering path delivers real levels to the meter.
    #[test]
    fn test_shared_buffer_metering_produces_nonzero_rms() {
        use crate::audio::ring_buffer::AudioRingBuffer;

        let buf = AudioRingBuffer::new();
        let samples = vec![0.5f32; 1024];
        buf.write(&samples);

        let mut scratch = vec![0.0f32; 4096];
        let n = buf.read(&mut scratch);
        assert!(n > 0, "ring buffer should have data after write");

        let mut meter = AudioMeter::new();
        let level = meter.process(&scratch[..n]);
        assert!(
            level.rms > 0.0,
            "rms should be > 0 for non-zero samples; got {}",
            level.rms
        );
    }
}
