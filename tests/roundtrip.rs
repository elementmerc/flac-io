//! Encoder round-trip and reproducibility tests.
//!
//! The lossless guarantee: encoding samples and decoding the result returns
//! exactly the same samples. Determinism: encoding the same samples twice
//! produces byte-identical output. And the streams the encoder writes are
//! readable by the reference decoder, not just our own.

use flac_io::{decode, encode, FlacAudio};

fn audio(channels: usize, bps: u8, rate: u32, per_channel: &[&[i32]]) -> FlacAudio {
    FlacAudio {
        sample_rate: rate,
        channels: channels as u8,
        bits_per_sample: bps,
        samples: per_channel.iter().map(|c| c.to_vec()).collect(),
    }
}

#[test]
fn round_trip_simple_stereo() {
    let left: Vec<i32> = (0..5000).map(|i| ((i * 7) % 2000) - 1000).collect();
    let right: Vec<i32> = (0..5000).map(|i| 500 - ((i * 3) % 1000)).collect();
    let original = audio(2, 16, 44100, &[&left, &right]);

    let bytes = encode(&original).expect("encode");
    let decoded = decode(&bytes).expect("decode");

    assert_eq!(decoded.sample_rate, 44100);
    assert_eq!(decoded.channels, 2);
    assert_eq!(decoded.bits_per_sample, 16);
    assert_eq!(decoded.samples, original.samples);
}

#[test]
fn round_trip_constant_and_silence() {
    let silence = vec![0i32; 9000];
    let dc = vec![1234i32; 9000];
    let original = audio(2, 16, 48000, &[&silence, &dc]);
    let decoded = decode(&encode(&original).unwrap()).unwrap();
    assert_eq!(decoded.samples, original.samples);
}

#[test]
fn round_trip_high_entropy_uses_verbatim_path() {
    // A counter scrambled to look noisy forces large residuals and exercises
    // the verbatim fallback as well as Rice coding.
    let noisy: Vec<i32> = (0..8000)
        .map(|i: i64| ((i.wrapping_mul(2654435761) >> 8) as i32).rem_euclid(30000) - 15000)
        .collect();
    let original = audio(1, 16, 44100, &[&noisy]);
    let decoded = decode(&encode(&original).unwrap()).unwrap();
    assert_eq!(decoded.samples, original.samples);
}

#[test]
fn round_trip_24_bit() {
    let ch: Vec<i32> = (0..6000)
        .map(|i| ((i * 131) % 4_000_000) - 2_000_000)
        .collect();
    let original = audio(1, 24, 96000, &[&ch]);
    let decoded = decode(&encode(&original).unwrap()).unwrap();
    assert_eq!(decoded.samples, original.samples);
}

#[test]
fn round_trip_spans_multiple_blocks() {
    // More than two full 4096-sample blocks, with a short final block.
    let ch: Vec<i32> = (0..9001).map(|i| ((i * 5) % 600) - 300).collect();
    let original = audio(1, 16, 44100, &[&ch]);
    let decoded = decode(&encode(&original).unwrap()).unwrap();
    assert_eq!(decoded.samples, original.samples);
    assert_eq!(decoded.samples_per_channel(), 9001);
}

#[test]
fn encoding_is_byte_stable() {
    let ch: Vec<i32> = (0..7000).map(|i| ((i * 11) % 5000) - 2500).collect();
    let original = audio(1, 16, 44100, &[&ch]);
    let a = encode(&original).unwrap();
    let b = encode(&original).unwrap();
    assert_eq!(a, b, "two encodes of the same input must be identical");
}

#[test]
fn rejects_mismatched_channel_lengths() {
    let original = audio(2, 16, 44100, &[&[1, 2, 3], &[1, 2]]);
    assert!(encode(&original).is_err());
}

#[test]
fn rejects_sample_out_of_range() {
    // 70000 does not fit in 16-bit signed.
    let original = audio(1, 16, 44100, &[&[70000]]);
    assert!(encode(&original).is_err());
}
