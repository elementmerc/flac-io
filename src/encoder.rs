// FLAC encoder: a correct, deterministic, byte-stable baseline.
//
// The goal here is correctness and reproducibility, not the last byte of
// compression. Each block of samples is encoded per channel with whichever of
// three subframe types is smallest: a constant (all samples equal), a fixed
// polynomial predictor of order 0 to 4 with Rice-coded residuals, or a
// verbatim fallback that always works. Channels are stored independently (no
// mid/side transform). Every choice is a deterministic function of the input,
// so encoding the same samples twice yields byte-identical output.

use crate::bitwriter::BitWriter;
use crate::crc::{crc16, crc8};
use crate::error::FlacError;
use crate::metadata::FLAC_MARKER;
use crate::ogg;
use crate::sample_bytes::md5_of_samples;
use crate::FlacAudio;

/// Samples per block. A fixed value keeps the encoder simple and the output
/// reproducible; 4096 is the common FLAC default.
const BLOCK_SIZE: usize = 4096;

/// Largest Rice parameter we will pick. Beyond this the remainder field is
/// wider than any real residual needs, so we fall back to verbatim instead.
const MAX_RICE_PARAM: u32 = 30;

/// If even the best Rice parameter leaves a quotient longer than this, the
/// residuals are pathological and we use a verbatim subframe instead.
const MAX_RICE_QUOTIENT: u64 = 1 << 20;

pub(crate) fn encode(audio: &FlacAudio) -> Result<Vec<u8>, FlacError> {
    validate(audio)?;

    let channels = audio.channels as usize;
    let bps = audio.bits_per_sample as u32;
    let total = audio.samples_per_channel();

    let mut out = Vec::new();
    out.extend_from_slice(FLAC_MARKER);
    out.extend_from_slice(&streaminfo_block(audio, total, true));

    // Encode frame by frame over fixed-size blocks.
    let mut frame_number: u64 = 0;
    let mut start = 0;
    while start < total {
        let block = (total - start).min(BLOCK_SIZE);
        out.extend_from_slice(&frame_bytes(
            audio,
            channels,
            bps,
            start,
            block,
            frame_number,
        ));
        start += block;
        frame_number += 1;
    }

    // A stream with zero samples still needs a valid (empty) frame section;
    // STREAMINFO alone is a legal FLAC stream, so nothing more is required.
    Ok(out)
}

/// Encode samples into a FLAC stream wrapped in the Ogg container (`.oga`).
///
/// The audio is encoded exactly as [`encode`] does; the only difference is the
/// envelope. The STREAMINFO becomes the FLAC-to-Ogg mapping header packet, each
/// audio frame becomes one Ogg packet, and the packets are paged up with the
/// granule positions and checksums Ogg requires.
pub(crate) fn encode_ogg(audio: &FlacAudio) -> Result<Vec<u8>, FlacError> {
    validate(audio)?;

    let channels = audio.channels as usize;
    let bps = audio.bits_per_sample as u32;
    let total = audio.samples_per_channel();

    // First packet: the mapping header (type byte, "FLAC", version 1.0, then a
    // 16-bit count of the header packets that follow it), then the native
    // `fLaC` signature and the STREAMINFO block (no longer the last metadata
    // block, since a VORBIS_COMMENT follows). One header packet follows: the
    // comment block. Players that wrap FLAC in Ogg expect a comment header the
    // way Vorbis and Opus streams carry one.
    let mut header = Vec::new();
    header.push(0x7F);
    header.extend_from_slice(b"FLAC");
    header.extend_from_slice(&[1, 0]); // mapping version 1.0
    header.extend_from_slice(&1u16.to_be_bytes()); // one following header packet
    header.extend_from_slice(FLAC_MARKER);
    header.extend_from_slice(&streaminfo_block(audio, total, false));

    let mut packets = vec![
        ogg::Packet {
            data: header,
            granule: 0,
        },
        ogg::Packet {
            data: vorbis_comment_block(),
            granule: 0,
        },
    ];

    let mut frame_number: u64 = 0;
    let mut start = 0;
    let mut cumulative: i64 = 0;
    while start < total {
        let block = (total - start).min(BLOCK_SIZE);
        let data = frame_bytes(audio, channels, bps, start, block, frame_number);
        cumulative += block as i64;
        packets.push(ogg::Packet {
            data,
            granule: cumulative,
        });
        start += block;
        frame_number += 1;
    }

    Ok(ogg::mux(&packets))
}

fn validate(audio: &FlacAudio) -> Result<(), FlacError> {
    if audio.channels < 1 || audio.channels > 8 {
        return Err(FlacError::InvalidInput(format!(
            "channel count {} is outside 1 to 8",
            audio.channels
        )));
    }
    if audio.samples.len() != audio.channels as usize {
        return Err(FlacError::InvalidInput(
            "channel count does not match the number of sample vectors".into(),
        ));
    }
    if audio.bits_per_sample < 4 || audio.bits_per_sample > 32 {
        return Err(FlacError::InvalidInput(format!(
            "bit depth {} is outside 4 to 32",
            audio.bits_per_sample
        )));
    }
    if audio.sample_rate == 0 || audio.sample_rate >= (1 << 20) {
        return Err(FlacError::InvalidInput(format!(
            "sample rate {} is outside 1 to 1048575",
            audio.sample_rate
        )));
    }
    let len = audio.samples_per_channel();
    for (i, channel) in audio.samples.iter().enumerate() {
        if channel.len() != len {
            return Err(FlacError::InvalidInput(format!(
                "channel {i} has {} samples but channel 0 has {len}",
                channel.len()
            )));
        }
    }
    // Every sample must fit the declared bit depth.
    let bps = audio.bits_per_sample as u32;
    let (lo, hi) = signed_range(bps);
    for (i, channel) in audio.samples.iter().enumerate() {
        for &s in channel {
            let s = s as i64;
            if s < lo || s > hi {
                return Err(FlacError::InvalidInput(format!(
                    "channel {i} has a sample outside the {bps}-bit range"
                )));
            }
        }
    }
    Ok(())
}

fn signed_range(bits: u32) -> (i64, i64) {
    if bits >= 64 {
        return (i64::MIN, i64::MAX);
    }
    let hi = (1i64 << (bits - 1)) - 1;
    let lo = -(1i64 << (bits - 1));
    (lo, hi)
}

/// Build the STREAMINFO metadata block (its 4-byte header plus 34-byte body,
/// with the sample MD5 included). `last` sets the last-metadata-block flag:
/// true for native FLAC, where STREAMINFO is the only block; false for Ogg,
/// where a comment block follows. Shared by the native and Ogg encoders.
fn streaminfo_block(audio: &FlacAudio, total: usize, last: bool) -> Vec<u8> {
    let mut w = BitWriter::new();
    // The block-size bounds describe the inter-frame block size. A stream whose
    // whole length is a single short frame reports that frame's size; otherwise
    // the nominal block size, since the shorter final frame of a longer stream
    // does not lower the bound. Both bounds are equal because the encoder uses
    // one fixed block size.
    let block = if (1..BLOCK_SIZE).contains(&total) {
        total as u64
    } else {
        BLOCK_SIZE as u64
    };
    w.write_bits(block, 16); // min block size
    w.write_bits(block, 16); // max block size
    w.write_bits(0, 24); // min frame size: unknown
    w.write_bits(0, 24); // max frame size: unknown
    w.write_bits(audio.sample_rate as u64, 20);
    w.write_bits((audio.channels - 1) as u64, 3);
    w.write_bits((audio.bits_per_sample - 1) as u64, 5);
    w.write_bits(total as u64, 36);
    let body = w.into_bytes();

    let md5 = md5_of_samples(&audio.samples, audio.bits_per_sample);

    // Metadata block header: last-block flag, type 0 (STREAMINFO), length 34.
    let mut block = Vec::with_capacity(38);
    block.push(if last { 0x80 } else { 0x00 });
    block.extend_from_slice(&[0x00, 0x00, 0x22]);
    block.extend_from_slice(&body);
    block.extend_from_slice(&md5);
    block
}

/// Build a minimal VORBIS_COMMENT metadata block: a fixed vendor string and no
/// user comments, as the last metadata block. Ogg-wrapped FLAC carries a
/// comment header for players that expect one; the vendor string is a fixed
/// literal so the output stays byte-stable across crate versions.
fn vorbis_comment_block() -> Vec<u8> {
    const VENDOR: &[u8] = b"flac-io";
    // Vorbis comment bodies use little-endian lengths (the Vorbis convention),
    // unlike the big-endian fields elsewhere in FLAC metadata.
    let mut body = Vec::new();
    body.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    body.extend_from_slice(VENDOR);
    body.extend_from_slice(&0u32.to_le_bytes()); // user comment count
    let len = body.len() as u32;

    let mut block = Vec::with_capacity(4 + body.len());
    // Header: last-block flag set, type 4 (VORBIS_COMMENT), 24-bit length.
    block.push(0x84);
    block.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
    block.extend_from_slice(&body);
    block
}

fn frame_bytes(
    audio: &FlacAudio,
    channels: usize,
    bps: u32,
    start: usize,
    block: usize,
    frame_number: u64,
) -> Vec<u8> {
    let mut w = BitWriter::new();

    // Frame header. Fixed blocking strategy; explicit 16-bit block size; sample
    // rate and sample depth taken from STREAMINFO; independent channels.
    w.write_bits(0x3FFE, 14); // sync
    w.write_bits(0, 1); // mandatory zero
    w.write_bits(0, 1); // blocking strategy: fixed block size
    w.write_bits(0b0111, 4); // block size: read 16 bits at end of header
    w.write_bits(0b0000, 4); // sample rate: from STREAMINFO
    w.write_bits((channels - 1) as u64, 4); // independent channels
    w.write_bits(0b000, 3); // sample size: from STREAMINFO
    w.write_bits(0, 1); // mandatory zero
    write_utf8_coded(&mut w, frame_number);
    w.write_bits((block - 1) as u64, 16); // block size minus one

    // CRC-8 over the header bytes so far (the header is byte aligned here).
    debug_assert!(w.is_byte_aligned());
    let header = w.bytes().to_vec();
    w.write_bits(crc8(&header) as u64, 8);

    // One subframe per channel.
    for ch in 0..channels {
        let samples = &audio.samples[ch][start..start + block];
        encode_subframe(&mut w, samples, bps);
    }

    // Whole-frame CRC-16 over everything written (now byte aligned).
    w.align_to_byte();
    let frame = w.bytes().to_vec();
    w.write_bits(crc16(&frame) as u64, 16);

    w.into_bytes()
}

/// FLAC's extended-UTF-8 coding of the frame number (here always small enough
/// to need at most a few bytes, but the full range is handled).
fn write_utf8_coded(w: &mut BitWriter, value: u64) {
    if value < 0x80 {
        w.write_bits(value, 8);
        return;
    }
    // Choose the smallest length whose payload bits hold the value. An
    // N-byte sequence carries 5*N + 1 payload bits.
    let mut len = 2u64;
    while len < 7 {
        let payload_bits = 5 * len + 1;
        if value < (1u64 << payload_bits) {
            break;
        }
        len += 1;
    }
    let cont = len - 1;
    let lead_ones = (0xFFu64 << (8 - len)) & 0xFF;
    let first = lead_ones | (value >> (cont * 6));
    w.write_bits(first, 8);
    for i in (0..cont).rev() {
        let six = (value >> (i * 6)) & 0x3F;
        w.write_bits(0x80 | six, 8);
    }
}

fn encode_subframe(w: &mut BitWriter, samples: &[i32], bps: u32) {
    // Constant subframe: every sample identical.
    if samples.iter().all(|&s| s == samples[0]) {
        w.write_bits(0, 1); // padding
        w.write_bits(0, 6); // type: constant
        w.write_bits(0, 1); // no wasted bits
        write_signed(w, samples[0] as i64, bps);
        return;
    }

    // Find the best fixed predictor order, if any beats verbatim.
    let mut best: Option<FixedPlan> = None;
    for order in 0..=4usize {
        if samples.len() <= order {
            continue;
        }
        let residual = fixed_residual(samples, order);
        let Some((param, rice_bits)) = best_rice_param(&residual) else {
            continue; // residuals too large for this order
        };
        let warmup_bits = order as u64 * bps as u64;
        // header(8) + method(2) + partition order(4) + param field + rice
        let param_field = if param <= 14 { 4 } else { 5 };
        let cost = 8 + warmup_bits + 2 + 4 + param_field + rice_bits;
        let improves = match &best {
            None => true,
            Some(b) => cost < b.cost,
        };
        if improves {
            best = Some(FixedPlan {
                order,
                residual,
                param,
                cost,
            });
        }
    }

    let verbatim_cost = 8 + samples.len() as u64 * bps as u64;
    match best {
        Some(plan) if plan.cost < verbatim_cost => write_fixed(w, samples, bps, plan),
        _ => write_verbatim(w, samples, bps),
    }
}

struct FixedPlan {
    order: usize,
    residual: Vec<i64>,
    param: u32,
    cost: u64,
}

fn write_verbatim(w: &mut BitWriter, samples: &[i32], bps: u32) {
    w.write_bits(0, 1); // padding
    w.write_bits(1, 6); // type: verbatim
    w.write_bits(0, 1); // no wasted bits
    for &s in samples {
        write_signed(w, s as i64, bps);
    }
}

fn write_fixed(w: &mut BitWriter, samples: &[i32], bps: u32, plan: FixedPlan) {
    w.write_bits(0, 1); // padding
    w.write_bits(8 + plan.order as u64, 6); // type: fixed, order in low bits
    w.write_bits(0, 1); // no wasted bits
    for &s in &samples[..plan.order] {
        write_signed(w, s as i64, bps);
    }
    // Residual: single partition (order 0), method picked by parameter width.
    let method = if plan.param <= 14 { 0 } else { 1 };
    let param_bits = if method == 0 { 4 } else { 5 };
    w.write_bits(method, 2);
    w.write_bits(0, 4); // partition order 0
    w.write_bits(plan.param as u64, param_bits);
    for &r in &plan.residual {
        write_rice(w, r, plan.param);
    }
}

/// Compute the order-`order` fixed-predictor residuals.
fn fixed_residual(samples: &[i32], order: usize) -> Vec<i64> {
    let s: Vec<i64> = samples.iter().map(|&x| x as i64).collect();
    let n = s.len();
    let mut res = Vec::with_capacity(n - order);
    for i in order..n {
        let pred = match order {
            0 => 0,
            1 => s[i - 1],
            2 => 2 * s[i - 1] - s[i - 2],
            3 => 3 * s[i - 1] - 3 * s[i - 2] + s[i - 3],
            4 => 4 * s[i - 1] - 6 * s[i - 2] + 4 * s[i - 3] - s[i - 4],
            _ => unreachable!("fixed order is 0..=4"),
        };
        res.push(s[i] - pred);
    }
    res
}

/// Pick the Rice parameter that minimises the encoded residual size, returning
/// `(parameter, total_bits)`, or `None` if the residuals are too large to Rice
/// code sanely.
fn best_rice_param(residual: &[i64]) -> Option<(u32, u64)> {
    if residual.is_empty() {
        return Some((0, 0));
    }
    let zigzag: Vec<u64> = residual.iter().map(|&r| zigzag(r)).collect();
    let mut best: Option<(u32, u64)> = None;
    for k in 0..=MAX_RICE_PARAM {
        let mut bits: u64 = 0;
        let mut max_quotient: u64 = 0;
        for &z in &zigzag {
            let q = z >> k;
            max_quotient = max_quotient.max(q);
            bits += q + 1 + k as u64;
        }
        if max_quotient > MAX_RICE_QUOTIENT {
            continue;
        }
        let improves = match best {
            None => true,
            Some((_, b)) => bits < b,
        };
        if improves {
            best = Some((k, bits));
        }
    }
    best
}

fn zigzag(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}

fn write_rice(w: &mut BitWriter, residual: i64, k: u32) {
    let value = zigzag(residual);
    let quotient = value >> k;
    w.write_unary(quotient);
    if k > 0 {
        let remainder = value & ((1u64 << k) - 1);
        w.write_bits(remainder, k);
    }
}

/// Write a signed value in `bits` bits as two's complement (low bits).
fn write_signed(w: &mut BitWriter, value: i64, bits: u32) {
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    w.write_bits((value as u64) & mask, bits);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_is_invertible() {
        for r in [
            -1000i64,
            -1,
            0,
            1,
            2,
            1000,
            i32::MAX as i64,
            i32::MIN as i64,
        ] {
            let z = zigzag(r);
            let back = ((z >> 1) as i64) ^ -((z & 1) as i64);
            assert_eq!(back, r);
        }
    }

    #[test]
    fn fixed_residual_order_one_is_difference() {
        let res = fixed_residual(&[10, 13, 19, 18], 1);
        assert_eq!(res, vec![3, 6, -1]);
    }

    #[test]
    fn best_rice_param_handles_empty() {
        assert_eq!(best_rice_param(&[]), Some((0, 0)));
    }

    #[test]
    fn utf8_coded_writes_round_trip_small() {
        // A single byte for values below 0x80.
        let mut w = BitWriter::new();
        write_utf8_coded(&mut w, 0x42);
        assert_eq!(w.into_bytes(), vec![0x42]);
    }
}
