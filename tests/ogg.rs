//! Ogg-wrapped FLAC: decoding real reference output, and round-tripping our own
//! Ogg encoder. The container is auto-detected by `decode`/`info`; the only new
//! public surface is `encode_ogg`.

use flac_io::{decode, encode, encode_ogg, info, FlacAudio};
use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(path).unwrap()
}

#[test]
fn decodes_real_reference_ogg_bit_exact() {
    // A genuine libFLAC `flac --ogg` file decodes to the same samples as the
    // native FLAC of the same recording. A successful decode also means the
    // STREAMINFO MD5 matched, so it is bit-exact.
    let oga = fixture("realmusic_16_44.oga");
    let native = fixture("realmusic_16_44.flac");
    let from_ogg = decode(&oga).expect("decode .oga");
    let from_native = decode(&native).expect("decode .flac");
    assert_eq!(from_ogg.samples, from_native.samples);
    assert_eq!(from_ogg.sample_rate, from_native.sample_rate);
    assert_eq!(from_ogg.channels, from_native.channels);
    assert_eq!(from_ogg.bits_per_sample, from_native.bits_per_sample);
}

#[test]
fn info_auto_detects_ogg() {
    let oga = fixture("realmusic_16_44.oga");
    let i = info(&oga).expect("info on .oga");
    let d = decode(&oga).unwrap();
    assert_eq!(i.sample_rate, d.sample_rate);
    assert_eq!(i.channels, d.channels);
    assert_eq!(i.bits_per_sample, d.bits_per_sample);
}

fn audio(channels: u8, bps: u8, rate: u32, len: usize) -> FlacAudio {
    let (lo, hi) = (-(1i64 << (bps - 1)), (1i64 << (bps - 1)) - 1);
    let samples = (0..channels)
        .map(|c| {
            (0..len)
                .map(|n| {
                    let v = ((n as i64 * (c as i64 + 3) * 131) % (hi - lo + 1)) + lo;
                    v as i32
                })
                .collect()
        })
        .collect();
    FlacAudio {
        sample_rate: rate,
        channels,
        bits_per_sample: bps,
        samples,
    }
}

#[test]
fn encode_ogg_round_trips_across_configs() {
    let cases = [
        (1u8, 8u8, 8000u32, 1), // single sample
        (1, 16, 44100, 4096),   // exactly one block
        (2, 16, 44100, 10_000), // multiple frames
        (2, 24, 96000, 9000),
        (6, 16, 48000, 5000),
        (8, 16, 48000, 4097), // block boundary + many channels
        (2, 32, 48000, 6000),
    ];
    for (ch, bps, rate, len) in cases {
        let a = audio(ch, bps, rate, len);
        let bytes = encode_ogg(&a).expect("encode_ogg");
        let back = decode(&bytes).expect("decode our .oga");
        assert_eq!(back.samples, a.samples, "round-trip {ch}ch/{bps}bit/{len}");
        assert_eq!(back.sample_rate, rate);
        assert_eq!(back.bits_per_sample, bps);
        // info() agrees on the same bytes.
        let i = info(&bytes).unwrap();
        assert_eq!(i.channels, ch);
        assert_eq!(i.total_samples as usize, len);
    }
}

#[test]
fn encode_ogg_is_deterministic() {
    let a = audio(2, 16, 44100, 12_000);
    assert_eq!(encode_ogg(&a).unwrap(), encode_ogg(&a).unwrap());
}

#[test]
fn empty_stream_round_trips_through_ogg() {
    let a = FlacAudio {
        sample_rate: 44100,
        channels: 1,
        bits_per_sample: 16,
        samples: vec![vec![]],
    };
    let bytes = encode_ogg(&a).expect("encode empty .oga");
    let back = decode(&bytes).expect("decode empty .oga");
    assert_eq!(back.samples_per_channel(), 0);
}

#[test]
fn ogg_and_native_decode_to_the_same_samples() {
    // The two containers carry identical audio.
    let a = audio(2, 16, 44100, 9000);
    let native = decode(&encode(&a).unwrap()).unwrap();
    let ogg = decode(&encode_ogg(&a).unwrap()).unwrap();
    assert_eq!(native.samples, ogg.samples);
}

#[test]
fn large_stream_spanning_many_pages_round_trips() {
    // Long enough that audio frames fill several Ogg pages, exercising the
    // page-packing and granule bookkeeping over many pages.
    let a = audio(2, 16, 44100, 4096 * 30 + 7);
    let bytes = encode_ogg(&a).expect("encode large .oga");
    let back = decode(&bytes).expect("decode large .oga");
    assert_eq!(back.samples, a.samples);
    assert_eq!(back.samples_per_channel(), 4096 * 30 + 7);
}
