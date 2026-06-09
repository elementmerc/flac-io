//! Metadata and decode edge cases, plus an optional heavy local corpus run.
//!
//! Several decoder paths are only reachable through specific STREAMINFO
//! contents: a zero MD5 (which means "skip the check"), an unknown total
//! sample count (decode until the data ends), and a bit depth this crate does
//! not support. These build or patch a stream to exercise exactly those paths.

use flac_io::{decode, encode, FlacAudio};

fn sample_audio() -> FlacAudio {
    let ch: Vec<i32> = (0..6000).map(|i| (i % 800) - 400).collect();
    FlacAudio {
        sample_rate: 44100,
        channels: 1,
        bits_per_sample: 16,
        samples: vec![ch],
    }
}

/// Byte offset of the STREAMINFO body within a stream this crate writes:
/// `fLaC` (4) + metadata block header (4).
const BODY: usize = 8;

#[test]
fn zero_md5_skips_the_check_and_still_decodes() {
    let a = sample_audio();
    let mut bytes = encode(&a).unwrap();
    // The MD5 is the last 16 bytes of the 34-byte STREAMINFO body.
    for b in bytes.iter_mut().skip(BODY + 18).take(16) {
        *b = 0;
    }
    let d = decode(&bytes).expect("decode with zero MD5");
    assert_eq!(d.samples, a.samples);
}

#[test]
fn corrupt_md5_is_detected() {
    let a = sample_audio();
    let mut bytes = encode(&a).unwrap();
    // Flip one MD5 byte to a non-zero wrong value: the check must now fire.
    bytes[BODY + 18] ^= 0xFF;
    // Guard against the 1-in-256 chance the flip produced an all-zero byte
    // that still is not the whole zero digest; the digest is 16 bytes so a
    // single flipped byte is never the zero digest.
    assert!(decode(&bytes).is_err(), "corrupt MD5 must be rejected");
}

#[test]
fn unknown_total_samples_decodes_to_end() {
    let a = sample_audio();
    let mut bytes = encode(&a).unwrap();
    // total_samples is the 36-bit field at body bits 108..143: the low nibble
    // of body byte 13 plus body bytes 14..17. Zero it without touching the
    // bit depth in the high nibble of body byte 13.
    bytes[BODY + 13] &= 0xF0;
    for b in bytes.iter_mut().skip(BODY + 14).take(4) {
        *b = 0;
    }
    let d = decode(&bytes).expect("decode with unknown total");
    // With the total unknown the decoder reads frames until the data ends, so
    // it still recovers every sample.
    assert_eq!(d.samples, a.samples);
}

/// Build a STREAMINFO-only stream with a chosen bit depth, to reach the
/// decoder's "unsupported bit depth" path without any audio frames.
fn header_only(bps_field: u32) -> Vec<u8> {
    let mut bits: Vec<u8> = Vec::new();
    let push = |value: u64, n: u32, bits: &mut Vec<u8>| {
        for i in (0..n).rev() {
            bits.push(((value >> i) & 1) as u8);
        }
    };
    push(16, 16, &mut bits); // min block size
    push(16, 16, &mut bits); // max block size
    push(0, 24, &mut bits); // min frame size
    push(0, 24, &mut bits); // max frame size
    push(44100, 20, &mut bits); // sample rate
    push(0, 3, &mut bits); // channels - 1 (mono)
    push(bps_field as u64, 5, &mut bits); // bits per sample - 1
    push(0, 36, &mut bits); // total samples
    bits.resize(bits.len() + 128, 0); // MD5, all zero

    // Pack bits MSB-first.
    let mut body = vec![0u8; bits.len() / 8];
    for (i, &bit) in bits.iter().enumerate() {
        if bit != 0 {
            body[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    let mut out = b"fLaC".to_vec();
    out.push(0x80); // last block, type 0
    out.extend_from_slice(&[0x00, 0x00, 0x22]); // length 34
    out.extend_from_slice(&body);
    out
}

#[test]
fn unsupported_low_bit_depth_is_rejected() {
    // bps field 2 means 3 bits per sample, below the supported minimum of 4.
    let stream = header_only(2);
    let err = decode(&stream).unwrap_err();
    assert!(
        matches!(err, flac_io::FlacError::Unsupported(_)),
        "expected Unsupported, got {err:?}"
    );
}

#[test]
fn info_rejects_unsupported_bit_depth_like_decode() {
    // info() must agree with decode() on validity: a 3-bit stream is below the
    // supported minimum, so a metadata-only read rejects it too.
    let stream = header_only(2);
    let info_err = flac_io::info(&stream).unwrap_err();
    let decode_err = decode(&stream).unwrap_err();
    assert!(matches!(info_err, flac_io::FlacError::Unsupported(_)));
    assert_eq!(info_err, decode_err);
}

#[test]
fn info_accepts_supported_depth() {
    // A valid 16-bit header reads its metadata without decoding frames.
    let info = flac_io::info(&header_only(15)).expect("info on supported depth");
    assert_eq!(info.bits_per_sample, 16);
    assert_eq!(info.channels, 1);
}

#[test]
fn supported_depth_header_only_decodes_to_silence() {
    // A valid header with no frames decodes to zero samples (16-bit here).
    let stream = header_only(15);
    let d = decode(&stream).expect("header-only decode");
    assert_eq!(d.bits_per_sample, 16);
    assert_eq!(d.samples_per_channel(), 0);
}

/// Heavy local corpus run, off by default. Point `FLAC_IO_CORPUS` at a
/// directory of real `.flac` files and run `cargo test -- --ignored` to decode
/// every one and confirm each passes its own MD5 self-check.
#[test]
#[ignore = "set FLAC_IO_CORPUS to a directory of real .flac files"]
fn decode_local_corpus() {
    let dir =
        std::env::var("FLAC_IO_CORPUS").expect("set FLAC_IO_CORPUS to a directory of .flac files");
    let mut count = 0;
    let mut failed = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read corpus dir") {
        let path = entry.unwrap().path();
        if path.extension().map(|e| e == "flac").unwrap_or(false) {
            count += 1;
            let bytes = std::fs::read(&path).unwrap();
            if let Err(e) = decode(&bytes) {
                failed.push(format!("{}: {e}", path.display()));
            }
        }
    }
    assert!(count > 0, "no .flac files found in {dir}");
    assert!(failed.is_empty(), "decode failures: {failed:#?}");
    eprintln!("decoded {count} real-world files, all bit-exact");
}
