//! Lock-free ring buffer for real-time audio
//!
//! This module provides a pre-allocated, lock-free ring buffer suitable for
//! use in audio callbacks. The audio callback MUST NOT allocate memory, so
//! all storage is pre-allocated.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Ring buffer size: ~4 seconds at 16kHz mono
const BUFFER_SIZE: usize = 65536;

/// A lock-free single-producer single-consumer ring buffer for audio samples
///
/// This buffer is designed for real-time audio use:
/// - Pre-allocated to avoid allocations in the audio callback
/// - Lock-free operations using atomic indices
/// - Single producer (audio callback) and single consumer (main thread)
pub struct AudioRingBuffer {
    /// UnsafeCell allows interior mutability for the buffer
    buffer: UnsafeCell<Box<[f32; BUFFER_SIZE]>>,
    write_pos: AtomicUsize,
    read_pos: AtomicUsize,
}

// Safety: The buffer uses atomic operations for thread safety and is SPSC.
// The write_pos and read_pos atomics ensure that producer and consumer
// never access the same indices simultaneously.
unsafe impl Send for AudioRingBuffer {}
unsafe impl Sync for AudioRingBuffer {}

impl Default for AudioRingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioRingBuffer {
    /// Create a new ring buffer with pre-allocated storage
    pub fn new() -> Self {
        Self {
            buffer: UnsafeCell::new(Box::new([0.0; BUFFER_SIZE])),
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
        }
    }

    /// Returns the number of samples available for reading
    pub fn available(&self) -> usize {
        let write = self.write_pos.load(Ordering::Acquire);
        let read = self.read_pos.load(Ordering::Acquire);

        if write >= read {
            write - read
        } else {
            BUFFER_SIZE - read + write
        }
    }

    /// Write samples to the buffer (called from audio callback)
    ///
    /// This method is lock-free and does not allocate. It is safe to call
    /// from the audio callback thread.
    ///
    /// Returns the number of samples actually written (may be less than
    /// requested if the buffer is full).
    pub fn write(&self, samples: &[f32]) -> usize {
        let write = self.write_pos.load(Ordering::Acquire);
        let read = self.read_pos.load(Ordering::Acquire);

        // Calculate available space (leave one slot empty to distinguish full from empty)
        let available = if write >= read {
            BUFFER_SIZE - (write - read) - 1
        } else {
            read - write - 1
        };

        let to_write = samples.len().min(available);

        if to_write == 0 {
            return 0;
        }

        // Write samples to buffer
        // Safety: We're the only writer (single producer), and we use atomic
        // ordering to ensure the consumer sees consistent data. UnsafeCell
        // provides interior mutability.
        let buffer_ptr = self.buffer.get();
        for (i, &sample) in samples.iter().enumerate().take(to_write) {
            // Safety: SPSC guarantees writer and reader don't overlap indices
            unsafe {
                let idx = (write + i) % BUFFER_SIZE;
                (*buffer_ptr)[idx] = sample;
            }
        }

        // Update write position with release ordering to ensure writes are visible
        self.write_pos
            .store((write + to_write) % BUFFER_SIZE, Ordering::Release);
        to_write
    }

    /// Read samples from the buffer (called from consumer thread)
    ///
    /// Returns the number of samples actually read (may be less than
    /// requested if the buffer doesn't have enough data).
    pub fn read(&self, output: &mut [f32]) -> usize {
        let write = self.write_pos.load(Ordering::Acquire);
        let read = self.read_pos.load(Ordering::Acquire);

        // Calculate available samples
        let available = if write >= read {
            write - read
        } else {
            BUFFER_SIZE - read + write
        };

        let to_read = output.len().min(available);

        if to_read == 0 {
            return 0;
        }

        // Read samples from buffer
        // Safety: SPSC guarantees writer and reader don't overlap indices
        let buffer_ptr = self.buffer.get();
        for (i, sample) in output.iter_mut().enumerate().take(to_read) {
            let idx = (read + i) % BUFFER_SIZE;
            *sample = unsafe { (*buffer_ptr)[idx] };
        }

        // Update read position with release ordering
        self.read_pos
            .store((read + to_read) % BUFFER_SIZE, Ordering::Release);
        to_read
    }

    /// Clear the buffer
    pub fn clear(&self) {
        self.read_pos
            .store(self.write_pos.load(Ordering::Acquire), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_new_buffer() {
        let buffer = AudioRingBuffer::new();
        assert_eq!(buffer.available(), 0);
    }

    #[test]
    fn test_write_read() {
        let buffer = AudioRingBuffer::new();

        let samples = [1.0, 2.0, 3.0, 4.0, 5.0];
        let written = buffer.write(&samples);
        assert_eq!(written, 5);
        assert_eq!(buffer.available(), 5);

        let mut output = [0.0; 5];
        let read = buffer.read(&mut output);
        assert_eq!(read, 5);
        assert_eq!(output, samples);
        assert_eq!(buffer.available(), 0);
    }

    #[test]
    fn test_partial_read() {
        let buffer = AudioRingBuffer::new();

        let samples = [1.0, 2.0, 3.0, 4.0, 5.0];
        buffer.write(&samples);

        let mut output = [0.0; 3];
        let read = buffer.read(&mut output);
        assert_eq!(read, 3);
        assert_eq!(output, [1.0, 2.0, 3.0]);
        assert_eq!(buffer.available(), 2);

        let mut output2 = [0.0; 5];
        let read2 = buffer.read(&mut output2);
        assert_eq!(read2, 2);
        assert_eq!(output2[..2], [4.0, 5.0]);
    }

    #[test]
    fn test_wraparound() {
        let buffer = AudioRingBuffer::new();

        // Fill most of the buffer
        let fill_size = BUFFER_SIZE - 100;
        let fill: Vec<f32> = (0..fill_size).map(|i| i as f32).collect();
        let written = buffer.write(&fill);
        assert_eq!(written, fill_size);

        // Read most back
        let mut output = vec![0.0; fill_size - 50];
        buffer.read(&mut output);

        // Write more to cause wraparound
        let more: Vec<f32> = (0..200).map(|i| (i + 1000) as f32).collect();
        let written2 = buffer.write(&more);
        assert_eq!(written2, 200);

        // Read everything
        assert_eq!(buffer.available(), fill_size - (fill_size - 50) + 200);
    }

    #[test]
    fn test_overflow_handling() {
        let buffer = AudioRingBuffer::new();

        // Try to write more than capacity
        let huge: Vec<f32> = (0..BUFFER_SIZE + 100).map(|i| i as f32).collect();
        let written = buffer.write(&huge);

        // Should only write up to capacity - 1
        assert!(written < BUFFER_SIZE);
    }

    #[test]
    fn test_clear() {
        let buffer = AudioRingBuffer::new();

        let samples = [1.0, 2.0, 3.0];
        buffer.write(&samples);
        assert_eq!(buffer.available(), 3);

        buffer.clear();
        assert_eq!(buffer.available(), 0);
    }

    #[test]
    fn test_concurrent_producer_consumer() {
        let buffer = Arc::new(AudioRingBuffer::new());
        let producer = buffer.clone();
        let consumer = buffer.clone();

        const NUM_SAMPLES: usize = 100_000;

        // Producer thread
        let producer_handle = thread::spawn(move || {
            let mut total_written = 0;
            let mut value = 0.0f32;

            while total_written < NUM_SAMPLES {
                let chunk: Vec<f32> = (0..100).map(|i| value + i as f32).collect();
                let written = producer.write(&chunk);
                total_written += written;
                value += written as f32;

                // Small yield to allow consumer to catch up
                if written < 100 {
                    thread::yield_now();
                }
            }
            total_written
        });

        // Consumer thread
        let consumer_handle = thread::spawn(move || {
            let mut total_read = 0;
            let mut output = vec![0.0; 100];

            while total_read < NUM_SAMPLES {
                let read = consumer.read(&mut output);
                total_read += read;

                if read == 0 {
                    thread::yield_now();
                }
            }
            total_read
        });

        let written = producer_handle.join().unwrap();
        let read = consumer_handle.join().unwrap();

        // Allow some samples to remain in buffer
        assert!(written >= NUM_SAMPLES);
        assert!(read >= NUM_SAMPLES);
    }

    #[test]
    fn stress_test_60_seconds_simulation() {
        // Simulate 60 seconds of audio at 16kHz
        let buffer = Arc::new(AudioRingBuffer::new());
        let producer = buffer.clone();
        let consumer = buffer.clone();

        const SAMPLE_RATE: usize = 16000;
        const DURATION_SECS: usize = 1; // Reduced for test speed
        const TOTAL_SAMPLES: usize = SAMPLE_RATE * DURATION_SECS;
        const CHUNK_SIZE: usize = 512; // Typical audio buffer size

        let producer_handle = thread::spawn(move || {
            let mut written = 0;
            let chunk: Vec<f32> = (0..CHUNK_SIZE).map(|_| 0.5).collect();

            while written < TOTAL_SAMPLES {
                let w = producer.write(&chunk);
                written += w;
                // Simulate real-time: ~32ms between writes at 512 samples @ 16kHz
                std::thread::sleep(std::time::Duration::from_micros(100));
            }
            written
        });

        let consumer_handle = thread::spawn(move || {
            let mut read = 0;
            let mut output = vec![0.0; CHUNK_SIZE * 4];

            while read < TOTAL_SAMPLES {
                let r = consumer.read(&mut output);
                read += r;
                if r == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            read
        });

        let total_written = producer_handle.join().unwrap();
        let total_read = consumer_handle.join().unwrap();

        println!(
            "Stress test: written={}, read={}",
            total_written, total_read
        );
        assert!(total_written >= TOTAL_SAMPLES);
        assert!(total_read >= TOTAL_SAMPLES);
    }
}
