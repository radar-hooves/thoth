//! Audio capture using cpal with a decoupled capture/encode pipeline.
//!
//! The cpal audio callback does the minimum possible work: while armed it copies
//! its sample block and hands it to an **unbounded** channel. A separate writer
//! thread drains that channel, resamples to 16kHz mono, and writes the WAV.
//!
//! This decoupling is deliberate. The recording is consumed *offline* by the
//! transcription engine, so the only hard requirement is that **no captured
//! sample is ever dropped**. An earlier design fed the callback into a fixed
//! ~0.7s ring buffer that was resampled inline on the consumer side; whenever
//! that resampling stalled (CPU/GPU contention from a previous transcription,
//! scheduler preemption) the bounded buffer overflowed and silently discarded
//! audio — truncating the tail of long recordings. An unbounded hand-off makes
//! overflow structurally impossible: a consumer stall delays the WAV, it can
//! never shorten it. Memory grows with recording length (~5.7 MB/min at 48kHz
//! mono f32) and is released when the recording finalises.
//!
//! The recorder keeps the cpal stream open ("warm") between recordings so that
//! pressing record is an instant flag flip rather than a ~150ms device open.

use crate::TELEMETRY_TARGET;

use super::format::AudioConverter;
use super::ring_buffer::AudioRingBuffer;
use super::speech_gate::{GateConfig, SpeechGate, speech_activity};
use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Target sample rate for transcription (whisper.cpp / Parakeet expect 16kHz mono).
const TARGET_SAMPLE_RATE: u32 = 16000;

/// Resampler input chunk size in frames. Matches the import path (decode.rs) so
/// live capture and file import resample through the same well-tested path.
const RESAMPLE_CHUNK_SIZE: usize = 1024;

/// Messages from the audio callback to the writer thread.
///
/// `Samples` carries one callback's worth of device-native interleaved f32.
/// `Stop` is the end-of-stream sentinel pushed by `disarm`; the writer drains
/// every queued `Samples` ahead of it before finalising, so no tail is lost.
enum RecordingMsg {
    Samples(Vec<f32>),
    Stop,
}

/// Audio recorder using cpal with warm-stream lifecycle.
///
/// Lifecycle:
/// 1. `warm_up` — opens the cpal device and starts the stream. The callback
///    runs continuously but only forwards samples to the writer while armed.
/// 2. `arm` — spawns the writer thread and flips the armed flag so samples flow
///    to the channel. Does NOT open the device (instant).
/// 3. `disarm` — flips the armed flag, sends the stop sentinel, and joins the
///    writer thread. Does NOT close the device. Returns the path to the WAV.
/// 4. `cool_down` — closes the device. Called after an idle timeout or when
///    the device changes.
pub struct AudioRecorder {
    /// The warm cpal stream (open from `warm_up` to `cool_down`).
    stream: Option<cpal::Stream>,
    /// Writer thread that resamples queued samples to a WAV file.
    writer_handle: Option<std::thread::JoinHandle<Result<()>>>,
    /// Sender end of the capture channel. Created in `warm_up`, held for the
    /// stream's lifetime, cloned into the callback. Sending only happens while
    /// armed; the channel persists across recordings.
    sender: Option<Sender<RecordingMsg>>,
    /// Receiver end. Cloned into the writer thread on each `arm`.
    receiver: Option<Receiver<RecordingMsg>>,
    /// Path where the current/last recording is being written.
    output_path: Option<PathBuf>,
    /// Optional ring buffer for real-time metering (recording indicator waveform).
    metering_buffer: Option<Arc<AudioRingBuffer>>,
    /// Source sample rate captured at warm_up time.
    source_rate: Option<u32>,
    /// Source channel count captured at warm_up time.
    source_channels: Option<usize>,
    /// Whether the callback should forward samples to the writer.
    armed: Arc<AtomicBool>,
}

impl Default for AudioRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioRecorder {
    /// Create a new audio recorder.
    pub fn new() -> Self {
        Self {
            stream: None,
            writer_handle: None,
            sender: None,
            receiver: None,
            output_path: None,
            metering_buffer: None,
            source_rate: None,
            source_channels: None,
            armed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set a dedicated metering ring buffer.
    ///
    /// The metering buffer receives samples continuously while warm (even before
    /// arming), so the recording indicator can show levels without delay. Must be
    /// called before `warm_up` — the callback captures the clone at warm-up time.
    /// A bounded ring buffer is correct here: metering only needs recent levels,
    /// so dropping the oldest samples under pressure is the desired behaviour.
    pub fn set_metering_buffer(&mut self, buffer: Arc<AudioRingBuffer>) {
        self.metering_buffer = Some(buffer);
    }

    /// Clear the metering buffer.
    pub fn clear_metering_buffer(&mut self) {
        self.metering_buffer = None;
    }

    /// Whether the recorder is currently armed (actively capturing to a WAV file).
    pub fn is_recording(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    /// Whether the cpal stream is open (warm).
    pub fn is_warm(&self) -> bool {
        self.stream.is_some()
    }

    /// Open the cpal input stream and start it playing ("warm").
    ///
    /// If a warm stream is already open, this is a no-op (the same device
    /// identity is assumed; callers must cool_down then re-warm on device change).
    /// The metering buffer set via `set_metering_buffer` must be in place before calling this.
    #[allow(deprecated)] // cpal 0.17 deprecates name() but description() is not yet stable
    pub fn warm_up(&mut self, device: &cpal::Device) -> Result<()> {
        if self.stream.is_some() {
            tracing::debug!("AudioRecorder::warm_up: stream already open, no-op");
            return Ok(());
        }

        let device_name = super::device::get_device_display_name(device);
        tracing::info!("AudioRecorder::warm_up: opening device '{}'", device_name);

        let supported_config = device.default_input_config()?;
        let source_rate = supported_config.sample_rate();
        let source_channels = supported_config.channels() as usize;

        tracing::info!(
            "Warm stream: device='{}', {}Hz, {} channels",
            device_name,
            source_rate,
            source_channels,
        );

        self.source_rate = Some(source_rate);
        self.source_channels = Some(source_channels);

        // The capture channel lives for the whole warm-stream lifetime so the
        // callback always holds a valid sender. Unbounded: the producer (audio
        // thread) never blocks and never drops.
        let (sender, receiver) = crossbeam_channel::unbounded::<RecordingMsg>();
        let callback_sender = sender.clone();
        self.sender = Some(sender);
        self.receiver = Some(receiver);

        let callback_metering = self.metering_buffer.clone();
        let callback_armed = self.armed.clone();

        let stream = device.build_input_stream(
            supported_config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Metering always runs while warm (regardless of armed state) so
                // the recording indicator can show levels before the user hits record.
                if let Some(ref m) = callback_metering {
                    m.write(data);
                }

                // While armed, hand this block to the writer thread. The copy +
                // unbounded send is the only work done here; it never blocks and
                // never drops, so no captured audio is ever lost. `disarm` pauses
                // the stream before disarming, so once it has done so this callback
                // cannot fire again and no block can be enqueued after the Stop
                // sentinel — the channel order faithfully reflects capture order.
                if callback_armed.load(Ordering::SeqCst) {
                    let _ = callback_sender.send(RecordingMsg::Samples(data.to_vec()));
                }
            },
            |err| {
                tracing::error!("Audio stream error: {}", err);
                telemetry::report_error_with_cause("audio_capture_failed", &err);
                tracing::error!(target: TELEMETRY_TARGET, error = %err, "audio_capture_failed");
            },
            None,
        )?;

        stream.play()?;
        self.stream = Some(stream);
        tracing::info!("AudioRecorder::warm_up: stream open and playing");
        Ok(())
    }

    /// Arm the recorder: prepare the output file and start the writer thread.
    ///
    /// Requires the stream to already be warm. This is intentionally instant —
    /// it sets the armed flag only AFTER the writer thread is spawned and ready.
    pub fn arm(&mut self, output_path: &Path) -> Result<()> {
        if self.stream.is_none() {
            return Err(anyhow!(
                "Cannot arm: stream is not warm. Call warm_up() first."
            ));
        }
        if self.armed.load(Ordering::Relaxed) {
            return Err(anyhow!("Already armed / recording in progress"));
        }

        let source_rate = self
            .source_rate
            .ok_or_else(|| anyhow!("source_rate not set — warm_up() must have failed"))?;
        let source_channels = self
            .source_channels
            .ok_or_else(|| anyhow!("source_channels not set — warm_up() must have failed"))?;
        let receiver = self
            .receiver
            .as_ref()
            .ok_or_else(|| anyhow!("capture channel missing — warm_up() must have failed"))?
            .clone();

        tracing::info!("AudioRecorder::arm: output={}", output_path.display());

        // Drain any samples queued before arming (e.g. a stray callback racing
        // the armed flag) so the recording starts clean.
        while receiver.try_recv().is_ok() {}

        self.output_path = Some(output_path.to_path_buf());

        let writer_path = output_path.to_path_buf();
        self.writer_handle = Some(std::thread::spawn(move || {
            write_audio_to_file(receiver, &writer_path, source_rate, source_channels)
        }));

        // Armed flag is set LAST so the callback doesn't send until the writer
        // thread is running.
        self.armed.store(true, Ordering::SeqCst);
        tracing::info!("AudioRecorder::arm: armed, recording started");
        Ok(())
    }

    /// Disarm the recorder: stop forwarding samples and finalise the WAV.
    ///
    /// The cpal stream stays open (warm). Returns the path to the finished WAV file.
    pub fn disarm(&mut self) -> Result<PathBuf> {
        if !self.armed.load(Ordering::Relaxed) {
            return Err(anyhow!("Not armed / no recording in progress"));
        }

        // Stop the callback forwarding new samples.
        self.armed.store(false, Ordering::SeqCst);

        // Pause the stream so the audio callback is quiesced before we send the
        // Stop sentinel. This narrows the only race that could enqueue a Samples
        // block AFTER Stop: a callback that had already passed the armed check
        // and is mid-`send` on another thread. With the callback quiesced, the
        // channel's FIFO order is exactly the capture order and Stop is last. We
        // re-play() below to keep the device warm for the next instant arm —
        // pause/play does not reopen the device, so it costs nothing close to a
        // cold open.
        //
        // Data correctness does NOT depend on pause() succeeding. The `armed`
        // flag was set false above with SeqCst ordering, so the callback stops
        // forwarding new samples regardless. Some ALSA devices do not support
        // pause(); there it is a no-op error (logged) and the worst case is one
        // extra in-flight block of *real* captured audio arriving just before
        // Stop — never corruption and never a dropped tail. Falling back to a
        // full device close here would only make the next recording slow for no
        // correctness gain, so we keep the stream warm.
        if let Some(stream) = self.stream.as_ref() {
            if let Err(e) = stream.pause() {
                tracing::debug!(
                    "AudioRecorder::disarm: stream.pause() not supported/failed ({}); the armed \
                     flag still guarantees no new samples are forwarded",
                    e
                );
            }
        }

        // Push the end-of-stream sentinel. The writer drains every queued sample
        // block before it sees this, so the full tail reaches the WAV.
        if let Some(sender) = self.sender.as_ref() {
            sender
                .send(RecordingMsg::Stop)
                .map_err(|_| anyhow!("Writer thread gone before stop sentinel"))?;
        }

        if let Some(handle) = self.writer_handle.take() {
            handle
                .join()
                .map_err(|_| anyhow!("Writer thread panicked"))??;
        }

        // Resume the stream so metering keeps flowing and the next arm is instant.
        // The device was only paused, never closed, so this is cheap.
        if let Some(stream) = self.stream.as_ref() {
            if let Err(e) = stream.play() {
                tracing::warn!(
                    "AudioRecorder::disarm: stream.play() (re-warm) failed: {}",
                    e
                );
            }
        }

        let path = self
            .output_path
            .take()
            .ok_or_else(|| anyhow!("output_path missing after disarm"))?;

        tracing::info!(
            "AudioRecorder::disarm: recording saved to {}",
            path.display()
        );
        Ok(path)
    }

    /// Close the cpal stream ("cool down").
    ///
    /// Called on idle timeout, device change, or sleep/wake. Refuses to act
    /// while a recording is armed — tearing the stream down mid-capture would
    /// silently lose the recording. Callers must stop the recording first (via
    /// the normal stop-and-process path) if they need to cool down during one.
    pub fn cool_down(&mut self) {
        if self.armed.load(Ordering::Relaxed) {
            tracing::warn!("AudioRecorder::cool_down skipped — a recording is in progress");
            return;
        }

        if let Some(stream) = self.stream.take() {
            drop(stream);
            tracing::info!("AudioRecorder::cool_down: stream closed");
        }

        // Drop the channel so a future warm_up starts a fresh one matched to the
        // (possibly different) device's rate and channel count.
        self.sender = None;
        self.receiver = None;
        self.source_rate = None;
        self.source_channels = None;
    }

    // -------------------------------------------------------------------------
    // Legacy single-call API (used by tests and VAD recorder path)
    // These wrap warm_up + arm or disarm + cool_down for callers that don't
    // need the split lifecycle.
    // -------------------------------------------------------------------------

    /// Start recording from the default input device (legacy single-call API).
    pub fn start_default(&mut self, output_path: &Path) -> Result<()> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("No default input device available"))?;
        self.start(&device, output_path)
    }

    /// Start recording from a specific device (legacy single-call API).
    pub fn start(&mut self, device: &cpal::Device, output_path: &Path) -> Result<()> {
        if self.stream.is_some() {
            return Err(anyhow!("Recording already in progress"));
        }
        let device_name = super::device::get_device_display_name(device);
        tracing::info!("AudioRecorder::start (legacy): device='{}'", device_name);

        self.warm_up(device)?;
        self.arm(output_path)?;
        Ok(())
    }

    /// Stop recording and return the path to the recorded file (legacy single-call API).
    pub fn stop(&mut self) -> Result<PathBuf> {
        let path = self.disarm()?;
        self.cool_down();
        Ok(path)
    }
}

/// Stops the live speech gate when the writer thread leaves, by any route.
///
/// The writer has several `?` paths — a full disk, a revoked permission, a
/// volume that went away mid-recording — and every one of them returns
/// without reaching the end of the function. A gate left `live` after its
/// writer has gone is worse than no gate: hands-free auto-stop (#88) reads a
/// silence count frozen at whatever it was when the thread died, so the
/// recording never ends by itself and the user has to notice.
struct ActiveGate;

impl Drop for ActiveGate {
    fn drop(&mut self) {
        speech_activity().stop();
    }
}

/// Feed one block of finished 16kHz mono PCM to the live speech gate.
fn feed_gate(gate: &mut SpeechGate, pcm: &[i16]) {
    let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
    gate.push(&samples);
}

/// Resample queued capture samples to a 16kHz mono WAV file.
///
/// Receives device-native interleaved f32 blocks from the capture channel,
/// accumulates them, and resamples in exact `RESAMPLE_CHUNK_SIZE` chunks through
/// the same anti-aliased rubato resampler the file-import path uses
/// (`AudioConverter`). Runs until the `Stop` sentinel, then finalises the
/// trailing partial chunk and drains the resampler's internal delay so the very
/// end of the recording reaches the file. The WAV header is stamped at 16kHz.
///
/// This thread also drives the live speech gate. It is the only place that
/// holds the resampled 16kHz mono stream, and it is off the cpal callback, so
/// gating here costs the real-time path nothing and sees exactly the audio the
/// transcriber will.
fn write_audio_to_file(
    receiver: Receiver<RecordingMsg>,
    path: &Path,
    source_rate: u32,
    source_channels: usize,
) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    tracing::info!(
        "Writer thread starting: source_rate={}, channels={}, target={}, output={}",
        source_rate,
        source_channels,
        TARGET_SAMPLE_RATE,
        path.display()
    );

    let mut converter = AudioConverter::new(
        source_rate,
        TARGET_SAMPLE_RATE,
        source_channels,
        RESAMPLE_CHUNK_SIZE,
    )
    .map_err(|e| anyhow!("Failed to create resampler: {}", e))?;

    // The resampler consumes fixed-size chunks of interleaved frames. Accumulate
    // the variable-sized callback blocks and drain in exact chunks.
    let frames_per_chunk = RESAMPLE_CHUNK_SIZE * source_channels;
    let mut accumulator: Vec<f32> = Vec::with_capacity(frames_per_chunk * 2);

    let mut writer = hound::WavWriter::create(path, spec)?;
    let mut total_samples = 0usize;

    // The gate is not Send (see `speech_gate::WebRtcDetector`), so it is built
    // here rather than handed in, and its state reaches other threads through
    // the shared snapshot.
    let mut gate = SpeechGate::new(GateConfig::default());
    let activity = speech_activity();
    activity.start();
    let _gate_guard = ActiveGate;

    let drain_full_chunks = |accumulator: &mut Vec<f32>,
                             converter: &mut AudioConverter,
                             writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
                             total: &mut usize,
                             gate: &mut SpeechGate|
     -> Result<()> {
        while accumulator.len() >= frames_per_chunk {
            let chunk: Vec<f32> = accumulator.drain(..frames_per_chunk).collect();
            let resampled = converter
                .process_to_i16(&chunk)
                .map_err(|e| anyhow!("Resampling error: {}", e))?;
            for sample in &resampled {
                writer.write_sample(*sample)?;
            }
            *total += resampled.len();
            feed_gate(gate, &resampled);
        }
        Ok(())
    };

    // Block on the channel; an unbounded receiver only returns Err once every
    // Sender is dropped, which we never do while warm, so we rely on the Stop
    // sentinel to end the loop.
    for msg in receiver.iter() {
        match msg {
            RecordingMsg::Samples(block) => {
                accumulator.extend_from_slice(&block);
                drain_full_chunks(
                    &mut accumulator,
                    &mut converter,
                    &mut writer,
                    &mut total_samples,
                    &mut gate,
                )?;
                activity.publish(&gate);
            }
            RecordingMsg::Stop => break,
        }
    }

    // Finalise: resample the trailing partial chunk AND drain the resampler's
    // internal delay line in one call. Without this the last few milliseconds —
    // the resampler's filter delay — are left buffered and never written.
    let leftover = std::mem::take(&mut accumulator);
    let tail = converter
        .finish_to_i16(&leftover)
        .map_err(|e| anyhow!("Resampling error during finalise: {}", e))?;
    for sample in &tail {
        writer.write_sample(*sample)?;
    }
    total_samples += tail.len();
    feed_gate(&mut gate, &tail);
    activity.publish(&gate);

    writer.finalize()?;
    let duration_secs = total_samples as f32 / TARGET_SAMPLE_RATE as f32;
    tracing::info!(
        "Audio writer finished: {} samples, {:.2}s at {}Hz -> {}",
        total_samples,
        duration_secs,
        TARGET_SAMPLE_RATE,
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::speech_gate::speech_activity;
    use std::fs;
    use tempfile::tempdir;

    /// Feed a number of interleaved frames through the writer thread and return
    /// the finished WAV's sample count. Exercises the real channel + writer path.
    fn run_writer(source_rate: u32, channels: usize, frames: usize, path: &Path) -> usize {
        let (tx, rx) = crossbeam_channel::unbounded::<RecordingMsg>();
        let writer_path = path.to_path_buf();
        let handle = std::thread::spawn(move || {
            write_audio_to_file(rx, &writer_path, source_rate, channels)
        });

        // Send the audio in small blocks, mimicking cpal callback cadence.
        let block_frames = 512usize;
        let mut sent = 0;
        while sent < frames {
            let n = block_frames.min(frames - sent);
            let mut block = Vec::with_capacity(n * channels);
            block.extend(std::iter::repeat_n(0.1f32, n * channels));
            tx.send(RecordingMsg::Samples(block)).unwrap();
            sent += n;
        }
        tx.send(RecordingMsg::Stop).unwrap();
        handle.join().unwrap().unwrap();

        let reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16000, "header must be stamped at 16kHz");
        assert_eq!(spec.channels, 1, "output must be mono");
        reader.into_samples::<i16>().count()
    }

    /// The live gate must actually run on the recording path. A gate that
    /// compiles but is never fed reports silence for every recording, which
    /// looks exactly like a user who never spoke — so this drives real speech
    /// through the real writer thread and asserts the published snapshot.
    #[test]
    #[serial_test::serial(writer_thread)]
    fn the_writer_thread_publishes_live_speech() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/speech_no_custom_terms.wav");
        let pcm: Vec<f32> = hound::WavReader::open(&fixture)
            .unwrap()
            .into_samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();

        let dir = tempdir().unwrap();
        let path = dir.path().join("gate_test.wav");
        let (tx, rx) = crossbeam_channel::unbounded::<RecordingMsg>();
        let writer_path = path.to_path_buf();
        // 16kHz mono in, 16kHz mono out: no resampling, so the gate sees the
        // fixture unchanged and the assertion is about the gate, not rubato.
        let handle = std::thread::spawn(move || write_audio_to_file(rx, &writer_path, 16_000, 1));

        for block in pcm.chunks(512) {
            tx.send(RecordingMsg::Samples(block.to_vec())).unwrap();
        }

        // Assert while the writer is still running: `Stop` ends the gate.
        // `is_live` comes first because `stop` deliberately leaves
        // `heard_speech` set for a reader that arrives after the recording —
        // so waiting on it alone would read the PREVIOUS recording's answer.
        let activity = speech_activity();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !activity.is_live() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(activity.is_live(), "the gate runs for the whole recording");
        while !activity.has_heard_speech() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            activity.has_heard_speech(),
            "a spoken fixture must register as speech on the live path"
        );

        tx.send(RecordingMsg::Stop).unwrap();
        handle.join().unwrap().unwrap();
        assert!(
            !activity.is_live(),
            "the gate must stop when the recording does"
        );
        fs::remove_file(path).ok();
    }

    /// Stand-in for the writer's several `?` paths: it starts the gate, then
    /// leaves through an error rather than through the `Stop` sentinel.
    fn writer_that_fails_after_starting_the_gate() -> Result<()> {
        let activity = speech_activity();
        activity.start();
        let _gate_guard = ActiveGate;
        assert!(activity.is_live(), "the gate is live while the writer runs");
        Err(anyhow!("write failed mid-recording"))
    }

    /// The gate must stop when the writer leaves by an ERROR path, not only by
    /// the `Stop` sentinel. A writer that dies on a full disk leaves the gate
    /// `live` with a frozen silence count, and hands-free auto-stop (#88) then
    /// waits on a number that will never move again.
    #[test]
    #[serial_test::serial(writer_thread)]
    fn the_gate_stops_when_the_writer_fails() {
        let activity = speech_activity();

        let outcome = writer_that_fails_after_starting_the_gate();

        assert!(outcome.is_err());
        assert!(
            !activity.is_live(),
            "an error on the way out must still stop the gate"
        );
    }

    #[test]
    fn test_recorder_new() {
        let recorder = AudioRecorder::new();
        assert!(!recorder.is_recording());
        assert!(!recorder.is_warm());
    }

    #[test]
    #[serial_test::serial(writer_thread)]
    fn test_writer_produces_correct_rate_wav() {
        // 0.25s of 48kHz stereo should resample to ~4000 samples at 16kHz,
        // proving the writer genuinely resamples rather than relabelling.
        let dir = tempdir().unwrap();
        let path = dir.path().join("rate_test.wav");
        let n = run_writer(48000, 2, 12000, &path); // 0.25s @ 48kHz
        assert!(
            n > 3000 && n < 5000,
            "expected ~4000 samples for 0.25s at 16kHz, got {}",
            n
        );
        fs::remove_file(path).ok();
    }

    #[test]
    #[serial_test::serial(writer_thread)]
    fn test_writer_preserves_tail_on_long_recording() {
        // The whole point of the decoupled pipeline: a long recording must not
        // lose its tail. 30s of 48kHz mono = 480000 frames -> ~480000 samples at
        // 16kHz. The count must be within resampler-delay tolerance of the full
        // duration; a truncated tail would fall well short.
        let dir = tempdir().unwrap();
        let path = dir.path().join("long_test.wav");
        let n = run_writer(48000, 1, 48000 * 30, &path);
        let expected = 16000 * 30;
        // Allow a small shortfall for the resampler's fixed delay only (~ a few
        // hundred samples), never a missing back-half.
        assert!(
            n >= expected - 2000 && n <= expected + 2000,
            "expected ~{} samples for 30s at 16kHz, got {} (tail loss?)",
            expected,
            n
        );
        fs::remove_file(path).ok();
    }

    #[test]
    #[serial_test::serial(writer_thread)]
    fn test_record_and_stop() {
        // Skip if no usable audio input. Probing default_input_device() is not
        // enough: a headless ALSA host (CI) still reports a "default" device
        // that then fails to open with "cannot find card '0'". So we treat a
        // failed start_default as "no usable audio" and skip, rather than
        // asserting — this test only exercises real behaviour where capture
        // hardware actually exists.
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("test_recording.wav");

        let mut recorder = AudioRecorder::new();

        assert!(!recorder.is_warm());
        if recorder.start_default(&output_path).is_err() {
            println!("No usable audio input device, skipping test");
            return;
        }
        assert!(recorder.is_recording());

        std::thread::sleep(std::time::Duration::from_millis(500));

        let result_path = recorder.stop().unwrap();
        assert!(!recorder.is_recording());
        assert!(result_path.exists());

        let reader = hound::WavReader::open(&result_path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16000);
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.bits_per_sample, 16);

        fs::remove_file(result_path).ok();
    }

    #[test]
    fn test_arm_requires_warm() {
        let dir = tempdir().unwrap();
        let output_path = dir.path().join("test.wav");
        let mut recorder = AudioRecorder::new();
        assert!(recorder.arm(&output_path).is_err());
    }

    #[test]
    fn test_disarm_without_arm_returns_error() {
        let mut recorder = AudioRecorder::new();
        assert!(recorder.disarm().is_err());
    }
}
