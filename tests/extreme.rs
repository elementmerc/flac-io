//! Extreme and boundary conditions: empty streams, single samples, exact block
//! boundaries, the maximum channel count, the minimum and maximum bit depths,
//! full-scale extremes, and streams long enough to push the frame number into
//! its multi-byte coded form.

use flac_io::{decode, encode, FlacAudio};

fn audio(channels: u8, bps: u8, samples: Vec<Vec<i32>>) -> FlacAudio {
    FlacAudio {
        sample_rate: 44100,
        channels,
        bits_per_sample: bps,
        samples,
    }
}

fn round_trip(a: &FlacAudio) -> FlacAudio {
    decode(&encode(a).expect("encode")).expect("decode")
}

#[test]
fn empty_stream_round_trips() {
    let a = audio(1, 16, vec![vec![]]);
    let d = round_trip(&a);
    assert_eq!(d.samples, vec![Vec::<i32>::new()]);
    assert_eq!(d.samples_per_channel(), 0);
}

#[test]
fn single_sample() {
    let a = audio(2, 16, vec![vec![-1], vec![32767]]);
    assert_eq!(round_trip(&a).samples, a.samples);
}

#[test]
fn exact_block_boundaries() {
    for len in [4095usize, 4096, 4097, 8192] {
        let ch: Vec<i32> = (0..len).map(|i| (i as i32 % 1000) - 500).collect();
        let a = audio(1, 16, vec![ch]);
        assert_eq!(round_trip(&a).samples, a.samples, "len {len}");
    }
}

#[test]
fn maximum_channels() {
    let len = 500;
    let samples: Vec<Vec<i32>> = (0..8)
        .map(|c| (0..len).map(|i| ((i + c) % 200) - 100).collect())
        .collect();
    let a = audio(8, 16, samples);
    assert_eq!(round_trip(&a).samples, a.samples);
}

#[test]
fn minimum_bit_depth() {
    // 4-bit samples range from -8 to 7.
    let ch: Vec<i32> = (0..2000).map(|i| (i % 16) - 8).collect();
    let a = audio(1, 4, vec![ch]);
    assert_eq!(round_trip(&a).samples, a.samples);
}

#[test]
fn maximum_bit_depth_full_scale() {
    // 32-bit, alternating the full signed extremes (largest possible residuals).
    let ch: Vec<i32> = (0..3000)
        .map(|i| if i % 2 == 0 { i32::MIN } else { i32::MAX })
        .collect();
    let a = audio(2, 32, vec![ch.clone(), ch]);
    assert_eq!(round_trip(&a).samples, a.samples);
}

#[test]
fn all_silence_and_all_full_scale() {
    let silence = audio(1, 16, vec![vec![0; 5000]]);
    assert_eq!(round_trip(&silence).samples, silence.samples);

    let full = audio(1, 16, vec![vec![32767; 5000]]);
    assert_eq!(round_trip(&full).samples, full.samples);

    let floor = audio(1, 16, vec![vec![-32768; 5000]]);
    assert_eq!(round_trip(&floor).samples, floor.samples);
}

#[test]
fn long_stream_crosses_multibyte_frame_number() {
    // 200 frames of 4096 samples drives the frame number past 127, which the
    // encoder must write and the decoder must read in the multi-byte UTF-8
    // coded form. A failure in that path corrupts every frame after 127.
    let len = 4096 * 200 + 5;
    let ch: Vec<i32> = (0..len).map(|i| ((i * 13) % 4000) as i32 - 2000).collect();
    let a = audio(1, 16, vec![ch]);
    let d = round_trip(&a);
    assert_eq!(d.samples_per_channel(), len);
    assert_eq!(d.samples, a.samples);
}

#[test]
fn twenty_bit_depth() {
    let (lo, hi) = (-(1 << 19), (1 << 19) - 1);
    let ch: Vec<i32> = (0..4000)
        .map(|i| if i % 2 == 0 { lo } else { hi })
        .collect();
    let a = audio(2, 20, vec![ch.clone(), ch]);
    assert_eq!(round_trip(&a).samples, a.samples);
}
