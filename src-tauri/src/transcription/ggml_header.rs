//! What a Whisper model file says about itself.
//!
//! The manifest hand-types each model's capabilities — its languages, its
//! type — and nothing checks those against the file that actually gets loaded
//! (#107). A wrong entry there is invisible: the download works, the model
//! loads, and the Models pane confidently states something untrue.
//!
//! This reads the answer out of the file's own header instead. It is 48 bytes
//! at the front of every `ggml-*.bin`, so probing costs one short read.
//!
//! **These are GGML models, not GGUF.** #107 proposes a GGUF parser, and a
//! GGUF parser would find nothing here: `ggml-small.en.bin` opens with the
//! magic `0x67676d6c` ("ggml"), the older whisper.cpp format, and the crates
//! suggested for the job all expect the newer container. The layout below is
//! taken from whisper.cpp's own loader (`whisper_model_load`, the eleven
//! `read_safe(loader, hparams.*)` calls), not inferred.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// `"ggml"` little-endian, the first four bytes of every whisper GGML model.
const GGML_MAGIC: u32 = 0x6767_6d6c;

/// whisper.cpp packs a quantisation-format version into `ftype` by multiplying
/// it up by this, so the format itself is the remainder. `ggml.h`:
/// `#define GGML_QNT_VERSION_FACTOR 1000 // do not change this`.
const QNT_VERSION_FACTOR: i32 = 1000;

/// A Whisper model's own account of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhisperHeader {
    /// Vocabulary size. The multilingual flag is derived from it, and nothing
    /// else in the header says so.
    pub n_vocab: i32,
    /// Encoder layers. This is what whisper.cpp identifies the size from.
    pub n_audio_layer: i32,
    /// Mel filterbank count — 80 through v3-turbo, 128 for large-v3.
    pub n_mels: i32,
    /// Tensor format, already reduced past the quantisation-version factor.
    pub ftype: i32,
}

impl WhisperHeader {
    /// Whether the model can transcribe anything but English.
    ///
    /// whisper.cpp's own test, verbatim: `n_vocab >= 51865`. The English-only
    /// models stop at 51864 — one token short, which is exactly why this is
    /// worth reading from the file rather than trusting a hand-typed list.
    pub fn is_multilingual(&self) -> bool {
        self.n_vocab >= 51865
    }

    /// The model size, named as whisper.cpp names it.
    ///
    /// Derived from the encoder depth, not the filename: a file renamed by
    /// hand, or a manifest entry pointing at the wrong download, is precisely
    /// the drift this exists to catch.
    pub fn variant(&self) -> &'static str {
        match self.n_audio_layer {
            4 => "tiny",
            6 => "base",
            12 => "small",
            24 => "medium",
            // whisper.cpp distinguishes v3 by its extra vocabulary token.
            32 if self.n_vocab == 51866 => "large-v3",
            32 => "large",
            _ => "unknown",
        }
    }

    /// The language list this model actually supports, in the manifest's own
    /// vocabulary, so the two can be compared.
    pub fn languages(&self) -> Vec<String> {
        if self.is_multilingual() {
            vec!["en".to_string(), "multilingual".to_string()]
        } else {
            vec!["en".to_string()]
        }
    }
}

/// Why a probe could not answer.
///
/// Separate variants because they mean different things to the caller: a file
/// that is not there yet is normal (the model is not downloaded), and a file
/// that is there but unreadable is a fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// No file at that path.
    NotFound,
    /// The file exists but could not be read.
    Unreadable(String),
    /// Read, but it is not a whisper GGML model.
    NotWhisperGgml { magic: u32 },
    /// Fewer than 48 bytes — truncated, or an interrupted download.
    Truncated { bytes: usize },
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no file at that path"),
            Self::Unreadable(e) => write!(f, "unreadable: {e}"),
            Self::NotWhisperGgml { magic } => {
                write!(f, "not a whisper GGML model (magic {magic:#010x})")
            }
            Self::Truncated { bytes } => {
                write!(f, "truncated: {bytes} bytes, need 48")
            }
        }
    }
}

/// Bytes of header: the magic plus eleven `int32` hyperparameters.
const HEADER_BYTES: usize = 4 + 11 * 4;

/// Read a Whisper model's header from disk.
pub fn probe(path: &Path) -> Result<WhisperHeader, ProbeError> {
    let mut file = File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ProbeError::NotFound,
        _ => ProbeError::Unreadable(e.to_string()),
    })?;

    let mut buffer = [0u8; HEADER_BYTES];
    // `read` may return short on any reader, so ask for the whole header.
    let mut filled = 0;
    while filled < HEADER_BYTES {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => return Err(ProbeError::Truncated { bytes: filled }),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(ProbeError::Unreadable(e.to_string())),
        }
    }

    parse(&buffer)
}

/// Parse a header that has already been read.
fn parse(bytes: &[u8; HEADER_BYTES]) -> Result<WhisperHeader, ProbeError> {
    let word = |i: usize| -> i32 {
        let start = i * 4;
        i32::from_le_bytes([
            bytes[start],
            bytes[start + 1],
            bytes[start + 2],
            bytes[start + 3],
        ])
    };

    let magic = word(0) as u32;
    if magic != GGML_MAGIC {
        return Err(ProbeError::NotWhisperGgml { magic });
    }

    Ok(WhisperHeader {
        n_vocab: word(1),
        // words 2..4 are n_audio_ctx, n_audio_state, n_audio_head
        n_audio_layer: word(5),
        // words 6..9 are n_text_ctx, n_text_state, n_text_head, n_text_layer
        n_mels: word(10),
        // whisper.cpp divides the quantisation version out before using it.
        ftype: word(11).rem_euclid(QNT_VERSION_FACTOR),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a header with the given hyperparameters, in whisper.cpp's order.
    fn header_bytes(
        n_vocab: i32,
        n_audio_layer: i32,
        n_mels: i32,
        ftype: i32,
    ) -> [u8; HEADER_BYTES] {
        let words: [i32; 12] = [
            GGML_MAGIC as i32,
            n_vocab,
            1500, // n_audio_ctx
            768,  // n_audio_state
            12,   // n_audio_head
            n_audio_layer,
            448, // n_text_ctx
            768, // n_text_state
            12,  // n_text_head
            12,  // n_text_layer
            n_mels,
            ftype,
        ];
        let mut bytes = [0u8; HEADER_BYTES];
        for (i, word) in words.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    /// The exact header of the real `ggml-small.en.bin`, read from the file
    /// during this work. It is the reason the parser exists, so it is the
    /// case the parser is held to.
    #[test]
    fn the_real_small_en_header_reads_as_english_only() {
        let header = parse(&header_bytes(51864, 12, 80, 1)).unwrap();
        assert_eq!(header.variant(), "small");
        assert!(
            !header.is_multilingual(),
            "small.en stops one token short of multilingual, at 51864"
        );
        assert_eq!(header.languages(), vec!["en"]);
        assert_eq!(header.ftype, 1);
    }

    /// One token more and it is a different model. This boundary is the whole
    /// reason the flag is worth reading rather than typing.
    #[test]
    fn the_multilingual_boundary_is_one_token_wide() {
        assert!(
            !parse(&header_bytes(51864, 12, 80, 1))
                .unwrap()
                .is_multilingual()
        );
        assert!(
            parse(&header_bytes(51865, 12, 80, 1))
                .unwrap()
                .is_multilingual()
        );
    }

    /// Size comes from the encoder depth, so a mislabelled file is caught.
    #[test]
    fn the_variant_comes_from_the_layer_count_not_the_name() {
        for (layers, expected) in [(4, "tiny"), (6, "base"), (12, "small"), (24, "medium")] {
            assert_eq!(
                parse(&header_bytes(51865, layers, 80, 1))
                    .unwrap()
                    .variant(),
                expected
            );
        }
        assert_eq!(
            parse(&header_bytes(51865, 32, 80, 1)).unwrap().variant(),
            "large"
        );
        assert_eq!(
            parse(&header_bytes(51866, 32, 128, 1)).unwrap().variant(),
            "large-v3",
            "v3 is the one with an extra vocabulary token"
        );
        assert_eq!(
            parse(&header_bytes(51865, 7, 80, 1)).unwrap().variant(),
            "unknown"
        );
    }

    /// A GGUF file, or anything else, must be refused rather than parsed into
    /// confident nonsense.
    #[test]
    fn a_file_that_is_not_a_whisper_model_is_refused() {
        let mut bytes = header_bytes(51865, 32, 80, 1);
        // "GGUF" little-endian — the format #107 assumed these files were.
        bytes[0..4].copy_from_slice(&0x4655_4747u32.to_le_bytes());
        assert_eq!(
            parse(&bytes),
            Err(ProbeError::NotWhisperGgml { magic: 0x4655_4747 })
        );
    }

    /// A missing model is normal — it just is not downloaded — and must not
    /// read as a fault.
    #[test]
    fn a_missing_file_is_reported_as_missing() {
        let path = std::path::Path::new("/nonexistent/thoth/no-such-model.bin");
        assert_eq!(probe(path), Err(ProbeError::NotFound));
    }

    /// An interrupted download leaves a short file, which must not be parsed
    /// out of whatever happens to follow it in memory.
    #[test]
    fn a_truncated_file_is_reported_as_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.bin");
        std::fs::write(&path, &header_bytes(51865, 32, 80, 1)[..20]).unwrap();
        assert_eq!(probe(&path), Err(ProbeError::Truncated { bytes: 20 }));
    }

    /// The whole path, through a real file on disk.
    #[test]
    fn a_written_header_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ggml-medium.en.bin");
        let mut contents = header_bytes(51864, 24, 80, 1).to_vec();
        // Real models continue for hundreds of megabytes; the probe must read
        // only the header and stop.
        contents.extend(std::iter::repeat_n(0u8, 4096));
        std::fs::write(&path, contents).unwrap();

        let header = probe(&path).unwrap();
        assert_eq!(header.variant(), "medium");
        assert!(!header.is_multilingual());
    }
}
