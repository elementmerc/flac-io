// Top-level decode: header, then every frame, then the whole-stream MD5 check.

use crate::bitstream::BitReader;
use crate::error::FlacError;
use crate::frame::decode_frame;
use crate::metadata::read_header;
use crate::sample_bytes::md5_of_samples;
use crate::FlacAudio;

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
