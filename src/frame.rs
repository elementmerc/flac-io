// Frame and subframe decoding: the heart of FLAC playback.
//
// After the metadata, a FLAC stream is a sequence of frames. Each frame holds
// a short run of samples for every channel, encoded independently per channel
// as a "subframe", with an optional inter-channel transform that stores the
// difference between channels instead of both channels outright. This module
// decodes one frame at a time into per-channel sample vectors.
//
// The decode path per frame is: read and CRC-check the header, decode one
// subframe per channel, undo any inter-channel transform, then CRC-check the
// whole frame.

use crate::bitstream::BitReader;
use crate::crc::{crc16, crc8};
use crate::error::FlacError;
use crate::metadata::StreamInfo;

/// Hard caps so a corrupt or hostile header cannot make us allocate or loop
/// without bound.
const MAX_BLOCK_SIZE: u32 = 65_535;
const MAX_LPC_ORDER: u32 = 32;
const MAX_PARTITION_ORDER: u32 = 16;

/// How the two channels of a stereo frame were decorrelated before encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelAssignment {
    /// Each channel stored independently. The value is the channel count.
    Independent(u8),
    LeftSide,
    RightSide,
    MidSide,
}

/// The decoded fields of a frame header that the subframe decoder needs.
#[derive(Debug)]
struct FrameHeader {
    block_size: u32,
    channel_assignment: ChannelAssignment,
    bits_per_sample: u8,
}

/// Decode one frame, appending its samples to `out` (one vector per channel).
///
/// `out` must already have one vector per channel. Returns the number of
/// samples per channel that this frame contributed.
pub(crate) fn decode_frame(
    reader: &mut BitReader,
    stream_info: &StreamInfo,
    out: &mut [Vec<i32>],
) -> Result<usize, FlacError> {
    if !reader.is_byte_aligned() {
        return Err(FlacError::CorruptStream(
            "frame did not start on a byte boundary".into(),
        ));
    }
    let header_start = reader.byte_position();
    let header = read_frame_header(reader, stream_info)?;

    // The header is byte aligned by construction; the CRC-8 is the next byte.
    let header_end = reader.byte_position();
    let crc_stored = reader.read_u32(8)? as u8;
    let crc_calc = crc8(&reader.data()[header_start..header_end]);
    if crc_stored != crc_calc {
        return Err(FlacError::CrcMismatch);
    }

    let n = header.block_size as usize;
    if out.len() != channel_count(header.channel_assignment) as usize {
        return Err(FlacError::CorruptStream(
            "frame channel count differs from the stream".into(),
        ));
    }

    // Decode each channel's subframe. Side channels carry one extra bit, so the
    // sample plane is i64 until the inter-channel transform is undone; only then
    // does every channel fit back into i32.
    let mut decoded: Vec<Vec<i64>> = Vec::with_capacity(out.len());
    for ch in 0..out.len() {
        let bps = subframe_bits_per_sample(header.channel_assignment, ch, header.bits_per_sample);
        decoded.push(decode_subframe(reader, n, bps)?);
    }

    undo_channel_decorrelation(header.channel_assignment, &mut decoded);

    // Whole-frame CRC-16 over every byte from the header start to here.
    reader.align_to_byte();
    let frame_end = reader.byte_position();
    let crc16_stored = reader.read_u32(16)? as u16;
    let crc16_calc = crc16(&reader.data()[header_start..frame_end]);
    if crc16_stored != crc16_calc {
        return Err(FlacError::CrcMismatch);
    }

    // Narrow back to i32 for output. After decorrelation each channel holds
    // values within the stream's declared bit depth (at most 32 bits), so the
    // cast is lossless for any valid stream; a stream where it is not is caught
    // by the STREAMINFO MD5 self-check.
    for (ch, samples) in decoded.into_iter().enumerate() {
        out[ch].extend(samples.into_iter().map(|s| s as i32));
    }
    Ok(n)
}

fn channel_count(a: ChannelAssignment) -> u8 {
    match a {
        ChannelAssignment::Independent(c) => c,
        _ => 2,
    }
}

fn subframe_bits_per_sample(a: ChannelAssignment, ch: usize, bps: u8) -> u8 {
    match a {
        ChannelAssignment::LeftSide | ChannelAssignment::MidSide if ch == 1 => bps + 1,
        ChannelAssignment::RightSide if ch == 0 => bps + 1,
        _ => bps,
    }
}

fn read_frame_header(
    reader: &mut BitReader,
    stream_info: &StreamInfo,
) -> Result<FrameHeader, FlacError> {
    // Sync code: 14 bits, 0b11111111111110.
    let sync = reader.read_u32(14)?;
    if sync != 0x3FFE {
        return Err(FlacError::CorruptStream("bad frame sync code".into()));
    }
    let _reserved = reader.read_u32(1)?;
    let _blocking_strategy = reader.read_u32(1)?;
    let block_size_code = reader.read_u32(4)?;
    let sample_rate_code = reader.read_u32(4)?;
    let channel_code = reader.read_u32(4)?;
    let sample_size_code = reader.read_u32(3)?;
    let _reserved2 = reader.read_u32(1)?;

    // The coded frame/sample number. We decode and discard it; sample layout
    // is driven by block sizes, not by this number.
    read_utf8_coded(reader)?;

    let block_size = match block_size_code {
        0 => {
            return Err(FlacError::CorruptStream(
                "reserved block size code 0".into(),
            ))
        }
        1 => 192,
        2..=5 => 576 << (block_size_code - 2),
        6 => reader.read_u32(8)? + 1,
        7 => reader.read_u32(16)? + 1,
        8..=15 => 256 << (block_size_code - 8),
        _ => unreachable!("block size code is 4 bits"),
    };
    if block_size == 0 || block_size > MAX_BLOCK_SIZE {
        return Err(FlacError::CorruptStream("block size out of range".into()));
    }

    // Sample rate is read for stream validity but the value lives in
    // STREAMINFO; we consume the trailing bytes when the code calls for them.
    match sample_rate_code {
        12 => {
            reader.read_u32(8)?;
        }
        13 | 14 => {
            reader.read_u32(16)?;
        }
        15 => {
            return Err(FlacError::CorruptStream(
                "invalid sample rate code 15".into(),
            ))
        }
        _ => {}
    }

    let channel_assignment = match channel_code {
        0..=7 => ChannelAssignment::Independent(channel_code as u8 + 1),
        8 => ChannelAssignment::LeftSide,
        9 => ChannelAssignment::RightSide,
        10 => ChannelAssignment::MidSide,
        _ => {
            return Err(FlacError::CorruptStream(
                "reserved channel assignment".into(),
            ))
        }
    };

    let bits_per_sample = match sample_size_code {
        0 => stream_info.bits_per_sample,
        1 => 8,
        2 => 12,
        3 => {
            return Err(FlacError::CorruptStream(
                "reserved sample size code 3".into(),
            ))
        }
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => unreachable!("sample size code is 3 bits"),
    };

    Ok(FrameHeader {
        block_size,
        channel_assignment,
        bits_per_sample,
    })
}

/// Decode FLAC's extended-UTF-8 coded number (1 to 7 bytes, up to 36 bits).
fn read_utf8_coded(reader: &mut BitReader) -> Result<u64, FlacError> {
    let first = reader.read_u32(8)? as u8;
    if first & 0x80 == 0 {
        return Ok(first as u64);
    }
    let n_following = match first {
        b if b & 0xE0 == 0xC0 => 1,
        b if b & 0xF0 == 0xE0 => 2,
        b if b & 0xF8 == 0xF0 => 3,
        b if b & 0xFC == 0xF8 => 4,
        b if b & 0xFE == 0xFC => 5,
        0xFE => 6,
        _ => {
            return Err(FlacError::CorruptStream(
                "invalid coded number lead byte".into(),
            ))
        }
    };
    let mask = (1u8 << (6 - n_following)) - 1;
    let mut value = (first & mask) as u64;
    for _ in 0..n_following {
        let cont = reader.read_u32(8)? as u8;
        if cont & 0xC0 != 0x80 {
            return Err(FlacError::CorruptStream(
                "invalid coded number continuation byte".into(),
            ));
        }
        value = (value << 6) | (cont & 0x3F) as u64;
    }
    Ok(value)
}

fn decode_subframe(
    reader: &mut BitReader,
    block_size: usize,
    bits_per_sample: u8,
) -> Result<Vec<i64>, FlacError> {
    // Subframe header: a zero padding bit, 6 type bits, a wasted-bits flag.
    let pad = reader.read_u32(1)?;
    if pad != 0 {
        return Err(FlacError::CorruptStream(
            "subframe padding bit was not zero".into(),
        ));
    }
    let type_code = reader.read_u32(6)?;
    let wasted_flag = reader.read_u32(1)?;
    let wasted_bits = if wasted_flag == 1 {
        // Unary count of zeros, plus one.
        reader.read_unary()? + 1
    } else {
        0
    };

    // Effective bit depth after removing the wasted low bits. The cap is 33,
    // not 32: a side channel of a 32-bit stream carries one extra bit. Only a
    // side channel can reach 33 here, because an independent channel's depth
    // comes straight from the frame header and never exceeds 32.
    let bps = (bits_per_sample as u32)
        .checked_sub(wasted_bits)
        .ok_or_else(|| FlacError::CorruptStream("wasted bits exceed sample depth".into()))?;
    if bps == 0 || bps > 33 {
        return Err(FlacError::CorruptStream(
            "effective sample depth out of range".into(),
        ));
    }

    let mut samples = match type_code {
        0 => decode_constant(reader, block_size, bps)?,
        1 => decode_verbatim(reader, block_size, bps)?,
        // 0b001000..=0b001100: fixed predictor, order in the low three bits.
        8..=12 => decode_fixed(reader, block_size, bps, (type_code - 8) as usize)?,
        // 0b100000..=0b111111: LPC, order = low five bits + 1.
        32..=63 => decode_lpc(reader, block_size, bps, (type_code - 32) as usize + 1)?,
        _ => {
            return Err(FlacError::CorruptStream(format!(
                "reserved subframe type code {type_code}"
            )))
        }
    };

    if wasted_bits > 0 {
        // Samples are i64 here, so this shift cannot overflow: `wasted_bits` is
        // at most 32 (a 33-bit side channel with all but one bit wasted) and an
        // i64 absorbs a 33-bit value shifted left by 32.
        for s in &mut samples {
            *s <<= wasted_bits;
        }
    }
    Ok(samples)
}

fn decode_constant(
    reader: &mut BitReader,
    block_size: usize,
    bps: u32,
) -> Result<Vec<i64>, FlacError> {
    let value = reader.read_signed_wide(bps)?;
    Ok(vec![value; block_size])
}

fn decode_verbatim(
    reader: &mut BitReader,
    block_size: usize,
    bps: u32,
) -> Result<Vec<i64>, FlacError> {
    let mut samples = Vec::with_capacity(block_size);
    for _ in 0..block_size {
        samples.push(reader.read_signed_wide(bps)?);
    }
    Ok(samples)
}

fn decode_fixed(
    reader: &mut BitReader,
    block_size: usize,
    bps: u32,
    order: usize,
) -> Result<Vec<i64>, FlacError> {
    if order > block_size {
        return Err(FlacError::CorruptStream(
            "fixed predictor order exceeds block size".into(),
        ));
    }
    // Warmup samples stored verbatim.
    let mut samples: Vec<i64> = Vec::with_capacity(block_size);
    for _ in 0..order {
        samples.push(reader.read_signed_wide(bps)?);
    }
    let residual = read_residual(reader, block_size, order)?;
    restore_fixed(&mut samples, &residual, order);
    Ok(samples)
}

fn restore_fixed(samples: &mut Vec<i64>, residual: &[i64], order: usize) {
    // Wrapping arithmetic throughout: on a valid stream the values fit and
    // wrapping is identical to ordinary arithmetic, but a corrupt stream can
    // drive this integrator past i64, and a parser of untrusted data must wrap
    // rather than panic. The STREAMINFO MD5 check rejects any such result.
    for (i, &r) in residual.iter().enumerate() {
        let idx = order + i;
        // Each arm only indexes the `order` samples behind it, so order 0
        // never reads `samples[idx - 1]`.
        let pred: i64 = match order {
            0 => 0,
            1 => samples[idx - 1],
            2 => 2i64
                .wrapping_mul(samples[idx - 1])
                .wrapping_sub(samples[idx - 2]),
            3 => 3i64
                .wrapping_mul(samples[idx - 1])
                .wrapping_sub(3i64.wrapping_mul(samples[idx - 2]))
                .wrapping_add(samples[idx - 3]),
            4 => 4i64
                .wrapping_mul(samples[idx - 1])
                .wrapping_sub(6i64.wrapping_mul(samples[idx - 2]))
                .wrapping_add(4i64.wrapping_mul(samples[idx - 3]))
                .wrapping_sub(samples[idx - 4]),
            _ => unreachable!("fixed order is 0..=4"),
        };
        samples.push(r.wrapping_add(pred));
    }
}

fn decode_lpc(
    reader: &mut BitReader,
    block_size: usize,
    bps: u32,
    order: usize,
) -> Result<Vec<i64>, FlacError> {
    if order as u32 > MAX_LPC_ORDER || order > block_size {
        return Err(FlacError::CorruptStream(
            "LPC order exceeds the cap or block size".into(),
        ));
    }
    let mut samples: Vec<i64> = Vec::with_capacity(block_size);
    for _ in 0..order {
        samples.push(reader.read_signed_wide(bps)?);
    }

    // Quantised linear-predictor coefficients.
    let precision = reader.read_u32(4)? + 1;
    if precision > 32 {
        return Err(FlacError::CorruptStream(
            "invalid LPC coefficient precision".into(),
        ));
    }
    let shift = reader.read_signed(5)?;
    if shift < 0 {
        return Err(FlacError::CorruptStream(
            "negative LPC shift is not supported".into(),
        ));
    }
    let mut coeffs: Vec<i64> = Vec::with_capacity(order);
    for _ in 0..order {
        coeffs.push(reader.read_signed(precision)? as i64);
    }

    let residual = read_residual(reader, block_size, order)?;
    for (i, &r) in residual.iter().enumerate() {
        let idx = order + i;
        // Wrapping accumulation so a corrupt stream cannot panic on overflow
        // (see the note in `restore_fixed`).
        let mut pred: i64 = 0;
        for (j, &c) in coeffs.iter().enumerate() {
            pred = pred.wrapping_add(c.wrapping_mul(samples[idx - 1 - j]));
        }
        samples.push(r.wrapping_add(pred >> shift));
    }
    Ok(samples)
}

/// Read the residual block (the prediction errors after the warmup samples).
fn read_residual(
    reader: &mut BitReader,
    block_size: usize,
    order: usize,
) -> Result<Vec<i64>, FlacError> {
    let method = reader.read_u32(2)?;
    let param_bits = match method {
        0 => 4,
        1 => 5,
        _ => {
            return Err(FlacError::CorruptStream(
                "reserved residual coding method".into(),
            ))
        }
    };
    let escape = (1u32 << param_bits) - 1;

    let partition_order = reader.read_u32(4)?;
    if partition_order > MAX_PARTITION_ORDER {
        return Err(FlacError::CorruptStream(
            "residual partition order too large".into(),
        ));
    }
    let partitions = 1usize << partition_order;
    if block_size % partitions != 0 {
        return Err(FlacError::CorruptStream(
            "block size not divisible by partition count".into(),
        ));
    }
    let partition_len = block_size / partitions;

    let mut residual: Vec<i64> = Vec::with_capacity(block_size - order);
    for p in 0..partitions {
        // The first partition is shortened by the warmup samples.
        let count = if p == 0 {
            partition_len
                .checked_sub(order)
                .ok_or_else(|| FlacError::CorruptStream("partition smaller than order".into()))?
        } else {
            partition_len
        };

        let param = reader.read_u32(param_bits)?;
        if param == escape {
            // Escape: each residual is a raw signed value of the given width.
            let raw_bits = reader.read_u32(5)?;
            for _ in 0..count {
                let v = if raw_bits == 0 {
                    0
                } else {
                    reader.read_signed(raw_bits)? as i64
                };
                residual.push(v);
            }
        } else {
            for _ in 0..count {
                residual.push(read_rice(reader, param)?);
            }
        }
    }
    Ok(residual)
}

/// Read one Rice-coded residual with parameter `k` and zigzag-decode it.
fn read_rice(reader: &mut BitReader, k: u32) -> Result<i64, FlacError> {
    let quotient = reader.read_unary()? as u64;
    let remainder = if k > 0 { reader.read_u32(k)? as u64 } else { 0 };
    let value = (quotient << k) | remainder;
    // Zigzag: even -> non-negative, odd -> negative.
    Ok(((value >> 1) as i64) ^ -((value & 1) as i64))
}

fn undo_channel_decorrelation(assignment: ChannelAssignment, channels: &mut [Vec<i64>]) {
    match assignment {
        ChannelAssignment::Independent(_) => {}
        ChannelAssignment::LeftSide => {
            // ch0 = left, ch1 = side = left - right -> right = left - side.
            for i in 0..channels[0].len() {
                let left = channels[0][i];
                let side = channels[1][i];
                channels[1][i] = left - side;
            }
        }
        ChannelAssignment::RightSide => {
            // ch0 = side = left - right, ch1 = right -> left = right + side.
            for i in 0..channels[1].len() {
                let side = channels[0][i];
                let right = channels[1][i];
                channels[0][i] = right + side;
            }
        }
        ChannelAssignment::MidSide => {
            // ch0 = mid = (left+right)>>1, ch1 = side = left-right.
            for i in 0..channels[0].len() {
                let mid = channels[0][i];
                let side = channels[1][i];
                // Recover the bit lost in the mid right-shift from the side parity.
                let mid2 = (mid << 1) | (side & 1);
                channels[0][i] = (mid2 + side) >> 1;
                channels[1][i] = (mid2 - side) >> 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_coded_single_byte() {
        let mut r = BitReader::new(&[0x42]);
        assert_eq!(read_utf8_coded(&mut r).unwrap(), 0x42);
    }

    #[test]
    fn utf8_coded_two_byte() {
        // 0b110_00010 0b10_000001 encodes (0b00010 << 6) | 0b000001 = 129.
        let mut r = BitReader::new(&[0xC2, 0x81]);
        assert_eq!(read_utf8_coded(&mut r).unwrap(), 129);
    }

    #[test]
    fn utf8_coded_rejects_bad_continuation() {
        let mut r = BitReader::new(&[0xC2, 0x00]);
        assert!(matches!(
            read_utf8_coded(&mut r),
            Err(FlacError::CorruptStream(_))
        ));
    }

    #[test]
    fn rice_zigzag_roundtrip() {
        // Build a stream with parameter 0 (pure unary) encoding zigzag values.
        // zigzag(0)=0 -> unary "1"; zigzag(1)= -1? value 1 -> -1; value 2 -> 1.
        // Bits: value 0 = '1', value 1 = '01', value 2 = '001', packed
        // most-significant first as 1010_0100.
        let mut r = BitReader::new(&[0b1010_0100]);
        assert_eq!(read_rice(&mut r, 0).unwrap(), 0);
        assert_eq!(read_rice(&mut r, 0).unwrap(), -1);
        assert_eq!(read_rice(&mut r, 0).unwrap(), 1);
    }

    #[test]
    fn fixed_order_two_restores() {
        // A pure quadratic ramp has zero second difference, so order-2 residual
        // is all zeros after the two warmup samples.
        let mut samples: Vec<i64> = vec![0, 1];
        let residual = vec![0i64; 4];
        restore_fixed(&mut samples, &residual, 2);
        assert_eq!(samples, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn mid_side_inverts() {
        // left=100 right=40 -> mid=(140)>>1=70, side=60.
        let mut channels = vec![vec![70i64], vec![60i64]];
        undo_channel_decorrelation(ChannelAssignment::MidSide, &mut channels);
        assert_eq!(channels[0][0], 100);
        assert_eq!(channels[1][0], 40);
    }

    #[test]
    fn left_side_inverts() {
        let mut channels = vec![vec![100i64], vec![60i64]]; // left=100, side=60 -> right=40
        undo_channel_decorrelation(ChannelAssignment::LeftSide, &mut channels);
        assert_eq!(channels[0][0], 100);
        assert_eq!(channels[1][0], 40);
    }

    #[test]
    fn right_side_inverts() {
        let mut channels = vec![vec![60i64], vec![40i64]]; // side=60, right=40 -> left=100
        undo_channel_decorrelation(ChannelAssignment::RightSide, &mut channels);
        assert_eq!(channels[0][0], 100);
        assert_eq!(channels[1][0], 40);
    }

    #[test]
    fn mid_side_inverts_full_32_bit_range() {
        // A 32-bit stereo sample whose side value needs 33 bits: left = i32::MAX,
        // right = i32::MIN. mid = (MAX + MIN) >> 1 = -1 (in 32-bit), side =
        // MAX - MIN = 2^32 - 1, which is the case the i64 plane exists for.
        let left = i32::MAX as i64;
        let right = i32::MIN as i64;
        let mid = (left + right) >> 1;
        let side = left - right;
        let mut channels = vec![vec![mid], vec![side]];
        undo_channel_decorrelation(ChannelAssignment::MidSide, &mut channels);
        assert_eq!(channels[0][0], left);
        assert_eq!(channels[1][0], right);
        // Both reconstructed channels fit back into i32.
        assert_eq!(channels[0][0] as i32, i32::MAX);
        assert_eq!(channels[1][0] as i32, i32::MIN);
    }

    // ── Crafted-stream error-branch coverage ──────────────────────────────
    use crate::bitwriter::BitWriter;

    fn dummy_stream_info() -> StreamInfo {
        StreamInfo {
            min_block_size: 16,
            max_block_size: 4096,
            min_frame_size: 0,
            max_frame_size: 0,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
            total_samples: 0,
            md5: [0; 16],
        }
    }

    /// Build a frame header with explicit field codes, then a one-byte coded
    /// number, into a byte buffer.
    fn frame_header_bytes(
        sync: u64,
        bs_code: u64,
        sr_code: u64,
        ch_code: u64,
        ss_code: u64,
    ) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.write_bits(sync, 14);
        w.write_bits(0, 1); // reserved
        w.write_bits(0, 1); // blocking strategy
        w.write_bits(bs_code, 4);
        w.write_bits(sr_code, 4);
        w.write_bits(ch_code, 4);
        w.write_bits(ss_code, 3);
        w.write_bits(0, 1); // reserved
        w.write_bits(0, 8); // coded number (single byte)
        w.into_bytes()
    }

    fn read_header_err(bytes: &[u8]) -> FlacError {
        let mut r = BitReader::new(bytes);
        read_frame_header(&mut r, &dummy_stream_info()).unwrap_err()
    }

    #[test]
    fn bad_sync_code_is_rejected() {
        let bytes = frame_header_bytes(0x0000, 1, 9, 0, 4);
        assert!(matches!(
            read_header_err(&bytes),
            FlacError::CorruptStream(_)
        ));
    }

    #[test]
    fn reserved_block_size_code_zero_is_rejected() {
        let bytes = frame_header_bytes(0x3FFE, 0, 9, 0, 4);
        assert!(matches!(
            read_header_err(&bytes),
            FlacError::CorruptStream(_)
        ));
    }

    #[test]
    fn invalid_sample_rate_code_is_rejected() {
        let bytes = frame_header_bytes(0x3FFE, 1, 15, 0, 4);
        assert!(matches!(
            read_header_err(&bytes),
            FlacError::CorruptStream(_)
        ));
    }

    #[test]
    fn reserved_channel_assignment_is_rejected() {
        let bytes = frame_header_bytes(0x3FFE, 1, 9, 11, 4);
        assert!(matches!(
            read_header_err(&bytes),
            FlacError::CorruptStream(_)
        ));
    }

    #[test]
    fn reserved_sample_size_code_is_rejected() {
        let bytes = frame_header_bytes(0x3FFE, 1, 9, 0, 3);
        assert!(matches!(
            read_header_err(&bytes),
            FlacError::CorruptStream(_)
        ));
    }

    #[test]
    fn header_reads_explicit_block_and_sample_rate_bytes() {
        // bs_code 7 (16-bit block size) and sr_code 13 (16-bit sample rate)
        // both consume trailing bytes; this exercises those branches.
        let mut w = BitWriter::new();
        w.write_bits(0x3FFE, 14);
        w.write_bits(0, 2);
        w.write_bits(7, 4); // block size from 16-bit trailer
        w.write_bits(13, 4); // sample rate from 16-bit trailer
        w.write_bits(0, 4); // channels: independent mono
        w.write_bits(1, 3); // 8-bit samples
        w.write_bits(0, 1);
        w.write_bits(0, 8); // coded number
        w.write_bits(255, 16); // block size - 1 -> 256 samples
        w.write_bits(44100, 16); // sample rate trailer
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        let h = read_frame_header(&mut r, &dummy_stream_info()).unwrap();
        assert_eq!(h.block_size, 256);
        assert_eq!(h.bits_per_sample, 8);
    }

    #[test]
    fn subframe_nonzero_padding_bit_is_rejected() {
        let mut w = BitWriter::new();
        w.write_bits(1, 1); // padding bit must be zero
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert!(matches!(
            decode_subframe(&mut r, 16, 16),
            Err(FlacError::CorruptStream(_))
        ));
    }

    #[test]
    fn subframe_reserved_type_is_rejected() {
        let mut w = BitWriter::new();
        w.write_bits(0, 1); // padding
        w.write_bits(2, 6); // reserved type code (2..7 are reserved)
        w.write_bits(0, 1); // no wasted bits
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert!(matches!(
            decode_subframe(&mut r, 16, 16),
            Err(FlacError::CorruptStream(_))
        ));
    }

    #[test]
    fn wasted_bits_exceeding_depth_is_rejected() {
        let mut w = BitWriter::new();
        w.write_bits(0, 1); // padding
        w.write_bits(0, 6); // constant subframe
        w.write_bits(1, 1); // wasted-bits flag set
                            // Unary value 16 (sixteen zeros then a one) -> 17 wasted bits, more
                            // than the 16-bit depth.
        for _ in 0..16 {
            w.write_bits(0, 1);
        }
        w.write_bits(1, 1);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert!(matches!(
            decode_subframe(&mut r, 4, 16),
            Err(FlacError::CorruptStream(_))
        ));
    }

    #[test]
    fn constant_subframe_with_wasted_bits_shifts() {
        // A constant subframe with two wasted bits: the stored value is shifted
        // left by two on output.
        let mut w = BitWriter::new();
        w.write_bits(0, 1); // padding
        w.write_bits(0, 6); // constant
        w.write_bits(1, 1); // wasted-bits flag
        w.write_bits(0, 1); // one zero ...
        w.write_bits(1, 1); // ... then the terminator: unary 1 -> 2 wasted bits
                            // Effective depth 16 - 2 = 14 bits; store value 5.
        w.write_bits(5, 14);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        let samples = decode_subframe(&mut r, 3, 16).unwrap();
        assert_eq!(samples, vec![5 << 2; 3]);
    }

    #[test]
    fn reserved_residual_method_is_rejected() {
        // Build a fixed order-0 subframe whose residual uses reserved method 2.
        let mut w = BitWriter::new();
        w.write_bits(0, 1); // padding
        w.write_bits(8, 6); // fixed order 0
        w.write_bits(0, 1); // no wasted bits
        w.write_bits(2, 2); // residual method 2 is reserved
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert!(matches!(
            decode_subframe(&mut r, 4, 16),
            Err(FlacError::CorruptStream(_))
        ));
    }

    #[test]
    fn block_not_divisible_by_partitions_is_rejected() {
        // Fixed order 0, method 0, partition order 2 (4 partitions) but a block
        // size of 3 is not divisible by 4.
        let mut w = BitWriter::new();
        w.write_bits(0, 1); // padding
        w.write_bits(8, 6); // fixed order 0
        w.write_bits(0, 1); // no wasted bits
        w.write_bits(0, 2); // method 0
        w.write_bits(2, 4); // partition order 2 -> 4 partitions
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert!(matches!(
            decode_subframe(&mut r, 3, 16),
            Err(FlacError::CorruptStream(_))
        ));
    }

    #[test]
    fn rice_escape_code_reads_raw_residuals() {
        // Fixed order 0, method 0, single partition, escape parameter (15) with
        // a raw bit width of 4: three residuals stored verbatim.
        let mut w = BitWriter::new();
        w.write_bits(0, 1); // padding
        w.write_bits(8, 6); // fixed order 0
        w.write_bits(0, 1); // no wasted bits
        w.write_bits(0, 2); // method 0
        w.write_bits(0, 4); // partition order 0
        w.write_bits(15, 4); // escape parameter
        w.write_bits(4, 5); // raw width 4 bits
                            // Three 4-bit signed residuals: 1, -1, 2.
        w.write_bits(0b0001, 4);
        w.write_bits(0b1111, 4);
        w.write_bits(0b0010, 4);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        let samples = decode_subframe(&mut r, 3, 16).unwrap();
        // Order-0 fixed predictor: the residuals are the samples directly.
        assert_eq!(samples, vec![1, -1, 2]);
    }
}
