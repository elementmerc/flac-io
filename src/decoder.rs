// Top-level decode: header, then every frame, then the whole-stream MD5 check.

use crate::bitstream::BitReader;
use crate::error::FlacError;
use crate::frame::decode_frame;
use crate::metadata::read_header;
use crate::sample_bytes::md5_of_samples;
use crate::FlacAudio;

/// Hard ceiling on the total number of samples (summed across every channel) a
/// single decode will produce.
///
/// A crafted stream can either declare a huge sample total in STREAMINFO or
/// pack many maximum-size constant subframes into a tiny input. A constant
/// subframe stores one value but expands to a whole block (up to 65,535
/// samples), so a few kilobytes of input could otherwise ask us to allocate
/// hundreds of gigabytes. At four bytes per decoded sample this cap bounds the
/// output buffer near four gibibytes. Any real music file this crate targets
/// is far below it; a stream that needs more is rejected rather than risked.
const MAX_TOTAL_SAMPLES: u64 = 1 << 30;

pub fn decode(bytes: &[u8]) -> Result<FlacAudio, FlacError> {
    let header = read_header(bytes)?;
    let info = header.stream_info;

    if info.channels < 1 || info.channels > 8 {
        return Err(FlacError::Unsupported(format!(
            "channel count {}",
            info.channels
        )));
    }
    if info.bits_per_sample < 4 || info.bits_per_sample > 32 {
        return Err(FlacError::Unsupported(format!(
            "{}-bit samples",
            info.bits_per_sample
        )));
    }

    let channels = info.channels as u64;

    // Fail fast: a STREAMINFO total that already blows the cap is rejected
    // before a single frame is allocated or decoded.
    if info.total_samples.saturating_mul(channels) > MAX_TOTAL_SAMPLES {
        return Err(FlacError::LimitExceeded(format!(
            "stream declares {} samples across {} channels, above the {MAX_TOTAL_SAMPLES}-sample decode cap",
            info.total_samples, info.channels
        )));
    }

    let mut reader = BitReader::new(&bytes[header.frame_start..]);
    let mut out: Vec<Vec<i32>> = vec![Vec::new(); info.channels as usize];

    // Decode frames until we have the recorded sample count, or the data runs
    // out when the count is unknown (STREAMINFO total of zero).
    loop {
        if info.total_samples > 0 && out[0].len() as u64 >= info.total_samples {
            break;
        }
        // A frame needs at least its sync code plus a CRC; if fewer than two
        // bytes remain we have reached the end of the audio.
        if reader.bits_left() < 16 {
            break;
        }
        decode_frame(&mut reader, &info, &mut out)?;

        // Enforce the cap even when the total is unknown or under-declared, so
        // a long run of expanding constant subframes cannot exhaust memory.
        if (out[0].len() as u64).saturating_mul(channels) > MAX_TOTAL_SAMPLES {
            return Err(FlacError::LimitExceeded(format!(
                "decoded sample count exceeds the {MAX_TOTAL_SAMPLES}-sample decode cap"
            )));
        }
    }

    // When the total is known, trim any samples the final block overshot by.
    if info.total_samples > 0 {
        let total = info.total_samples as usize;
        for channel in &mut out {
            if channel.len() > total {
                channel.truncate(total);
            }
        }
    }

    // The decisive correctness check: our samples must hash to the digest the
    // encoder stored. A zero digest means the encoder did not record one.
    if info.md5 != [0u8; 16] {
        let computed = md5_of_samples(&out, info.bits_per_sample);
        if computed != info.md5 {
            return Err(FlacError::CrcMismatch);
        }
    }

    Ok(FlacAudio {
        sample_rate: info.sample_rate,
        channels: info.channels,
        bits_per_sample: info.bits_per_sample,
        samples: out,
    })
}
