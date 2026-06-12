// Serialising samples the way FLAC's STREAMINFO MD5 expects.
//
// FLAC's MD5 is taken over the raw samples interleaved by channel, each sample
// written as a little-endian signed integer in the smallest whole number of
// bytes that holds the bit depth (2 bytes for 16-bit, 3 for 24-bit, and so
// on). Both the decoder self-check and the encoder need exactly this byte
// stream, so the logic lives here once.

use crate::md5::Md5;

/// Bytes used per sample for a given bit depth: the depth rounded up to whole
/// bytes.
pub(crate) fn bytes_per_sample(bits_per_sample: u8) -> usize {
    (bits_per_sample as usize).div_ceil(8)
}

/// Compute the FLAC sample MD5 over interleaved channel samples.
///
/// `samples[channel][index]`; every channel vector has the same length.
pub(crate) fn md5_of_samples(samples: &[Vec<i32>], bits_per_sample: u8) -> [u8; 16] {
    let bps_bytes = bytes_per_sample(bits_per_sample);
    let frames = samples.first().map_or(0, |c| c.len());
    let mut hasher = Md5::new();
    let mut buf = [0u8; 4];
    for i in 0..frames {
        for channel in samples {
            let word = channel[i] as u32;
            buf.copy_from_slice(&word.to_le_bytes());
            hasher.update(&buf[..bps_bytes]);
        }
    }
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_per_sample_rounds_up() {
        assert_eq!(bytes_per_sample(8), 1);
        assert_eq!(bytes_per_sample(12), 2);
        assert_eq!(bytes_per_sample(16), 2);
        assert_eq!(bytes_per_sample(20), 3);
        assert_eq!(bytes_per_sample(24), 3);
        assert_eq!(bytes_per_sample(32), 4);
    }

    #[test]
    fn negative_samples_use_twos_complement_low_bytes() {
        // -1 in 16-bit little-endian is 0xFF 0xFF.
        let md5 = md5_of_samples(&[vec![-1i32]], 16);
        let reference = crate::md5::digest(&[0xFF, 0xFF]);
        assert_eq!(md5, reference);
    }

    #[test]
    fn interleaves_channels() {
        // Two channels, one frame: left=1, right=2 at 16-bit ->
        // 01 00 02 00 little-endian.
        let md5 = md5_of_samples(&[vec![1i32], vec![2i32]], 16);
        let reference = crate::md5::digest(&[0x01, 0x00, 0x02, 0x00]);
        assert_eq!(md5, reference);
    }
}
