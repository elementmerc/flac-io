// The public error type for the crate.

use std::fmt;

/// Errors returned when decoding or encoding a FLAC stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlacError {
    /// The input does not begin with the `fLaC` stream marker.
    NotFlac,

    /// The stream ended in the middle of a field that was still being read.
    Truncated,

    /// A structural value in the stream is impossible or out of range (an
    /// unknown subframe type, a reserved code, a partition order that does
    /// not divide the block, and so on). The string names the specific fault.
    CorruptStream(String),

    /// The stream uses a feature this crate does not implement (for example a
    /// reserved sample-rate or bit-depth code, or Ogg encapsulation).
    Unsupported(String),

    /// A computed CRC did not match the value stored in the stream, so the
    /// data is damaged.
    CrcMismatch,

    /// The stream is structurally valid but asks the decoder to produce more
    /// than a built-in safety limit allows (for example a sample total or a
    /// run of maximum-size constant subframes that would exhaust memory). The
    /// string names the limit that was hit.
    LimitExceeded(String),

    /// The samples handed to the encoder are inconsistent (channel lengths
    /// differ, bit depth out of range, no channels, and so on). The string
    /// names the specific fault.
    InvalidInput(String),
}

impl fmt::Display for FlacError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlacError::NotFlac => write!(f, "input is not a FLAC stream (missing fLaC marker)"),
            FlacError::Truncated => write!(f, "FLAC stream ended unexpectedly"),
            FlacError::CorruptStream(why) => write!(f, "corrupt FLAC stream: {why}"),
            FlacError::Unsupported(what) => write!(f, "unsupported FLAC feature: {what}"),
            FlacError::CrcMismatch => write!(f, "FLAC CRC check failed; the data is damaged"),
            FlacError::LimitExceeded(what) => {
                write!(f, "FLAC stream exceeds a decoder safety limit: {what}")
            }
            FlacError::InvalidInput(why) => write!(f, "invalid input to the FLAC encoder: {why}"),
        }
    }
}

impl std::error::Error for FlacError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_displays_a_message() {
        let variants = [
            FlacError::NotFlac,
            FlacError::Truncated,
            FlacError::CorruptStream("why".into()),
            FlacError::Unsupported("what".into()),
            FlacError::CrcMismatch,
            FlacError::LimitExceeded("cap".into()),
            FlacError::InvalidInput("why".into()),
        ];
        for v in &variants {
            let s = v.to_string();
            assert!(!s.is_empty());
            // The detail string is carried through for the parameterised ones.
            let _ = format!("{v:?}");
        }
        assert!(FlacError::CorruptStream("partition".into())
            .to_string()
            .contains("partition"));
        // The error implements the standard Error trait.
        let _: &dyn std::error::Error = &FlacError::NotFlac;
    }
}
