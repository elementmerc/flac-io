// FLAC encoder. The decoder lands first; the encoder is built next.

use crate::error::FlacError;
use crate::FlacAudio;

pub fn encode(_audio: &FlacAudio) -> Result<Vec<u8>, FlacError> {
    Err(FlacError::Unsupported(
        "the FLAC encoder is not yet implemented".into(),
    ))
}
