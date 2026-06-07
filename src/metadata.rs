// The stream header: the `fLaC` marker and the metadata block chain.
//
// A FLAC stream opens with the four bytes `fLaC`, then one or more metadata
// blocks. The first block is always STREAMINFO and carries everything the
// decoder needs: block-size bounds, sample rate, channel count, bit depth,
// total sample count, and an MD5 of the original samples. Every other block
// type (padding, seek table, comments, pictures, and so on) is skipped here;
// the decoder only needs STREAMINFO. The audio frames begin on the byte after
// the block whose "last" flag is set.

use crate::bitstream::BitReader;
use crate::error::FlacError;

/// The four-byte FLAC stream marker.
pub const FLAC_MARKER: &[u8; 4] = b"fLaC";

/// The fixed length of a STREAMINFO block body.
const STREAMINFO_LEN: usize = 34;

/// The contents of the STREAMINFO metadata block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub min_block_size: u16,
    pub max_block_size: u16,
    pub min_frame_size: u32,
    pub max_frame_size: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    /// Total samples per channel. Zero means the encoder did not record it.
    pub total_samples: u64,
    /// MD5 of the unencoded samples, or all zero if not recorded.
    pub md5: [u8; 16],
}

/// Result of reading the stream header: the STREAMINFO and the byte offset at
/// which the first audio frame begins.
#[derive(Debug)]
pub struct Header {
    pub stream_info: StreamInfo,
    pub frame_start: usize,
}

/// Parse the `fLaC` marker and the metadata block chain, returning STREAMINFO
/// and the offset of the first frame.
pub fn read_header(bytes: &[u8]) -> Result<Header, FlacError> {
    if bytes.len() < 4 || &bytes[0..4] != FLAC_MARKER {
        return Err(FlacError::NotFlac);
    }
    let mut pos = 4;
    let mut stream_info: Option<StreamInfo> = None;

    loop {
        // Each block header is 4 bytes: 1 last-block bit, 7 type bits, 24 length.
        if pos + 4 > bytes.len() {
            return Err(FlacError::Truncated);
        }
        let header = bytes[pos];
        let is_last = header & 0x80 != 0;
        let block_type = header & 0x7F;
        let length = ((bytes[pos + 1] as usize) << 16)
            | ((bytes[pos + 2] as usize) << 8)
            | bytes[pos + 3] as usize;
        pos += 4;
        if pos + length > bytes.len() {
            return Err(FlacError::Truncated);
        }

        if block_type == 0 {
            // STREAMINFO.
            if length != STREAMINFO_LEN {
                return Err(FlacError::CorruptStream(format!(
                    "STREAMINFO length is {length}, expected {STREAMINFO_LEN}"
                )));
            }
            if stream_info.is_some() {
                return Err(FlacError::CorruptStream(
                    "more than one STREAMINFO block".into(),
                ));
            }
            stream_info = Some(parse_stream_info(&bytes[pos..pos + length])?);
        } else if block_type == 127 {
            return Err(FlacError::CorruptStream(
                "invalid metadata block type 127".into(),
            ));
        }
        // All other block types are skipped.

        pos += length;
        if is_last {
            break;
        }
    }

    let stream_info =
        stream_info.ok_or_else(|| FlacError::CorruptStream("no STREAMINFO block found".into()))?;
    Ok(Header {
        stream_info,
        frame_start: pos,
    })
}

fn parse_stream_info(body: &[u8]) -> Result<StreamInfo, FlacError> {
    let mut r = BitReader::new(body);
    let min_block_size = r.read_u32(16)? as u16;
    let max_block_size = r.read_u32(16)? as u16;
    let min_frame_size = r.read_u32(24)?;
    let max_frame_size = r.read_u32(24)?;
    let sample_rate = r.read_u32(20)?;
    let channels = r.read_u32(3)? as u8 + 1;
    let bits_per_sample = r.read_u32(5)? as u8 + 1;
    let total_samples = r.read_u64(36)?;
    let mut md5 = [0u8; 16];
    for b in md5.iter_mut() {
        *b = r.read_u32(8)? as u8;
    }

    if sample_rate == 0 {
        return Err(FlacError::CorruptStream(
            "STREAMINFO sample rate is zero".into(),
        ));
    }
    if min_block_size < 16 {
        return Err(FlacError::CorruptStream(
            "STREAMINFO minimum block size below 16".into(),
        ));
    }

    Ok(StreamInfo {
        min_block_size,
        max_block_size,
        min_frame_size,
        max_frame_size,
        sample_rate,
        channels,
        bits_per_sample,
        total_samples,
        md5,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid header: marker + one last STREAMINFO block.
    fn synthetic_header() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(FLAC_MARKER);
        // Block header: last flag set, type 0, length 34.
        v.push(0x80);
        v.extend_from_slice(&[0x00, 0x00, 0x22]);
        // STREAMINFO body, 34 bytes.
        // min/max block size 4096.
        v.extend_from_slice(&[0x10, 0x00, 0x10, 0x00]);
        // min/max frame size 0.
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        // sample_rate=44100 (0xAC44) in 20 bits, channels-1=1, bps-1=15.
        // 44100 = 0x0AC44. Pack 20 bits sample rate, 3 bits channels, 5 bits bps.
        // 0000_1010_1100_0100_0100 | 001 | 01111
        // Byte layout: take it bit by bit via a helper below instead.
        let mut bits = Vec::new();
        push_bits(&mut bits, 44100, 20);
        push_bits(&mut bits, 1, 3); // channels - 1 = 1 (stereo)
        push_bits(&mut bits, 15, 5); // bps - 1 = 15 (16-bit)
        push_bits(&mut bits, 88200, 36); // total samples
        let packed = pack(&bits);
        v.extend_from_slice(&packed);
        // MD5 (16 bytes of 0xAB).
        v.extend_from_slice(&[0xAB; 16]);
        v
    }

    fn push_bits(out: &mut Vec<u8>, value: u64, n: u32) {
        for i in (0..n).rev() {
            out.push(((value >> i) & 1) as u8);
        }
    }

    fn pack(bits: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; bits.len().div_ceil(8)];
        for (i, &bit) in bits.iter().enumerate() {
            if bit != 0 {
                out[i / 8] |= 1 << (7 - (i % 8));
            }
        }
        out
    }

    #[test]
    fn rejects_non_flac() {
        assert_eq!(read_header(b"RIFFxxxx").unwrap_err(), FlacError::NotFlac);
        assert_eq!(read_header(b"fL").unwrap_err(), FlacError::NotFlac);
    }

    #[test]
    fn parses_synthetic_streaminfo() {
        let h = read_header(&synthetic_header()).unwrap();
        let si = &h.stream_info;
        assert_eq!(si.min_block_size, 4096);
        assert_eq!(si.max_block_size, 4096);
        assert_eq!(si.sample_rate, 44100);
        assert_eq!(si.channels, 2);
        assert_eq!(si.bits_per_sample, 16);
        assert_eq!(si.total_samples, 88200);
        assert_eq!(si.md5, [0xAB; 16]);
        assert_eq!(h.frame_start, synthetic_header().len());
    }

    #[test]
    fn truncated_header_errors() {
        let full = synthetic_header();
        assert_eq!(read_header(&full[..10]).unwrap_err(), FlacError::Truncated);
    }

    #[test]
    fn rejects_zero_sample_rate() {
        let mut h = synthetic_header();
        // Zero the 20-bit sample rate: it starts at byte 4+4+10 = 18.
        h[18] = 0;
        h[19] = 0;
        h[20] &= 0x0F; // clear the top 4 bits of the sample rate's last nibble
        assert!(matches!(read_header(&h), Err(FlacError::CorruptStream(_))));
    }
}
