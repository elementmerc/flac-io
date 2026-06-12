#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! A pure-Rust FLAC decoder and encoder with no unsafe code and no C
//! dependency.
//!
//! FLAC (Free Lossless Audio Codec) stores audio so that decoding returns the
//! exact original samples, bit for bit. This crate reads a FLAC byte stream
//! into its raw integer samples and writes raw samples back into a valid FLAC
//! stream, without any lossy intermediate.
//!
//! It exists for steganography, watermarking, forensic analysis, and any audio
//! work that needs the decoded sample plane with a guarantee that a decode
//! followed by an encode preserves the data exactly.
//!
//! # Quick start
//!
//! ```no_run
//! use flac_io::{decode, encode};
//!
//! let bytes = std::fs::read("song.flac").unwrap();
//! let audio = decode(&bytes).unwrap();
//! println!("{} Hz, {} ch, {} bit", audio.sample_rate, audio.channels, audio.bits_per_sample);
//!
//! let out = encode(&audio).unwrap();
//! std::fs::write("song_reencoded.flac", out).unwrap();
//! ```
//!
//! # Sample layout
//!
//! [`FlacAudio::samples`] holds one inner vector per channel, each the same
//! length (the number of samples per channel). Samples are signed integers in
//! the stream's native bit depth, sign-extended into `i32`. For stereo, index
//! 0 is the left channel and index 1 is the right.

mod bitstream;
mod bitwriter;
mod crc;
mod decoder;
mod encoder;
mod error;
mod frame;
mod md5;
mod metadata;
mod ogg;
mod sample_bytes;

pub use error::FlacError;
pub use metadata::StreamInfo;

/// Decoded FLAC audio: the stream parameters plus the samples, one vector per
/// channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlacAudio {
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Number of channels (1 to 8).
    pub channels: u8,
    /// Bits per sample (4 to 32).
    pub bits_per_sample: u8,
    /// Decoded samples, `samples[channel][index]`. Every channel vector has
    /// the same length.
    pub samples: Vec<Vec<i32>>,
}

impl FlacAudio {
    /// Number of samples per channel.
    pub fn samples_per_channel(&self) -> usize {
        self.samples.first().map_or(0, |c| c.len())
    }
}

/// Read just the stream metadata (sample rate, channels, bit depth, total
/// samples, MD5) without decoding any audio.
///
/// The container is detected automatically: both native FLAC and Ogg-wrapped
/// FLAC are accepted. Only the metadata is read, so this stays cheap even for a
/// large file. Use it when you need the format of a stream but not its samples.
///
/// # Errors
///
/// Returns [`FlacError`] if the input is not FLAC, is truncated, its STREAMINFO
/// block is corrupt, or it declares a channel count or bit depth this crate
/// cannot decode (so `info` and [`decode`] agree on which streams are valid).
pub fn info(bytes: &[u8]) -> Result<StreamInfo, FlacError> {
    let native;
    let header = if ogg::is_ogg(bytes) {
        native = ogg::to_native_flac(bytes, true)?;
        metadata::read_header(&native)?
    } else {
        metadata::read_header(bytes)?
    };
    decoder::validate_stream_info(&header.stream_info)?;
    Ok(header.stream_info)
}

/// Decode a FLAC byte stream into its samples and parameters.
///
/// The container is detected automatically: both native FLAC (the `.flac` form,
/// starting with `fLaC`) and Ogg-wrapped FLAC (the `.oga` form, starting with
/// `OggS`) are accepted.
///
/// Decoding is bounded: a crafted stream cannot make this function allocate
/// without limit. The total number of decoded samples (summed across channels)
/// is capped near one billion, which holds the output buffer under about four
/// gibibytes; a stream that needs more returns [`FlacError::LimitExceeded`]
/// rather than risking the process.
///
/// # Errors
///
/// Returns [`FlacError`] if the input is not FLAC, is truncated, is corrupt, a
/// stored CRC does not match, it uses a feature this crate does not implement,
/// or it exceeds the decode size cap.
pub fn decode(bytes: &[u8]) -> Result<FlacAudio, FlacError> {
    decoder::decode(bytes)
}

/// Encode samples into a valid native FLAC byte stream (the `.flac` form).
///
/// # Errors
///
/// Returns [`FlacError::InvalidInput`] if the channel vectors disagree in
/// length, the bit depth or channel count is out of range, or there are no
/// samples.
pub fn encode(audio: &FlacAudio) -> Result<Vec<u8>, FlacError> {
    encoder::encode(audio)
}

/// Encode samples into a FLAC stream wrapped in the Ogg container (the `.oga`
/// form).
///
/// The audio is encoded identically to [`encode`]; only the container differs.
/// Reading is symmetric: [`decode`] auto-detects either container, so there is
/// no separate Ogg decode function.
///
/// # Errors
///
/// Returns [`FlacError::InvalidInput`] under the same conditions as [`encode`].
pub fn encode_ogg(audio: &FlacAudio) -> Result<Vec<u8>, FlacError> {
    encoder::encode_ogg(audio)
}
