//! Aggressive edge and corner coverage: every bit depth, the exact value
//! boundaries, every channel count, pathological signal shapes, awkward
//! lengths, the sample-rate limits, and the full encoder rejection matrix,
//! each round-tripped through both the native and the Ogg container.
//!
//! The point is to break things: if a round-trip is not bit-exact, or a
//! supposedly-valid input is rejected (or an invalid one accepted), one of
//! these fails.

use flac_io::{decode, encode, encode_ogg, info, FlacAudio, FlacError};

fn make(channels: u8, bps: u8, rate: u32, samples: Vec<Vec<i32>>) -> FlacAudio {
    FlacAudio {
        sample_rate: rate,
        channels,
        bits_per_sample: bps,
        samples,
    }
}

/// Inclusive value range for a signed bit depth.
fn range(bps: u8) -> (i64, i64) {
    ((-(1i64 << (bps - 1))), (1i64 << (bps - 1)) - 1)
}

/// Assert a bit-exact round-trip through both containers, plus info() and
/// determinism, all on the same audio.
fn assert_round_trips(a: &FlacAudio, what: &str) {
    // Native.
    let nat = encode(a).unwrap_or_else(|e| panic!("encode {what}: {e}"));
    let dn = decode(&nat).unwrap_or_else(|e| panic!("decode native {what}: {e}"));
    assert_eq!(dn.samples, a.samples, "native round-trip {what}");
    assert_eq!(dn.sample_rate, a.sample_rate, "native rate {what}");
    assert_eq!(dn.channels, a.channels, "native channels {what}");
    assert_eq!(dn.bits_per_sample, a.bits_per_sample, "native bps {what}");
    assert_eq!(encode(a).unwrap(), nat, "native determinism {what}");

    // Ogg.
    let ogg = encode_ogg(a).unwrap_or_else(|e| panic!("encode_ogg {what}: {e}"));
    let dg = decode(&ogg).unwrap_or_else(|e| panic!("decode ogg {what}: {e}"));
    assert_eq!(dg.samples, a.samples, "ogg round-trip {what}");
    assert_eq!(dg.bits_per_sample, a.bits_per_sample, "ogg bps {what}");
    assert_eq!(encode_ogg(a).unwrap(), ogg, "ogg determinism {what}");

    // info() on both containers agrees with decode.
    for (label, bytes) in [("native", &nat), ("ogg", &ogg)] {
        let i = info(bytes).unwrap_or_else(|e| panic!("info {label} {what}: {e}"));
        assert_eq!(i.sample_rate, a.sample_rate, "info rate {label} {what}");
        assert_eq!(i.channels, a.channels, "info channels {label} {what}");
        assert_eq!(
            i.bits_per_sample, a.bits_per_sample,
            "info bps {label} {what}"
        );
        assert_eq!(
            i.total_samples as usize,
            a.samples_per_channel(),
            "info total {label} {what}"
        );
    }
}

#[test]
fn every_bit_depth_round_trips() {
    // Each depth 4..=32, mono, with a signal that uses the full value range and
    // varied low bits (so no accidental wasted-bit shortcut).
    for bps in 4u8..=32 {
        let (lo, hi) = range(bps);
        let span = (hi - lo) as i128;
        let ch: Vec<i32> = (0..2000usize)
            .map(|n| {
                let v = lo as i128 + ((n as i128 * 2_654_435_761) % (span + 1));
                v as i64 as i32
            })
            .collect();
        let a = make(1, bps, 44100, vec![ch]);
        assert_round_trips(&a, &format!("{bps}-bit"));
    }
}

#[test]
fn value_boundaries_for_every_depth() {
    // The exact minimum and maximum sample for each depth, alternating, which is
    // the largest-magnitude residual the predictors can meet.
    for bps in 4u8..=32 {
        let (lo, hi) = range(bps);
        let ch: Vec<i32> = (0..1500)
            .map(|n| if n % 2 == 0 { lo as i32 } else { hi as i32 })
            .collect();
        let a = make(2, bps, 48000, vec![ch.clone(), ch]);
        assert_round_trips(&a, &format!("{bps}-bit extremes"));
    }
}

#[test]
fn all_channel_counts() {
    for channels in 1u8..=8 {
        let (lo, hi) = range(16);
        let samples: Vec<Vec<i32>> = (0..channels)
            .map(|c| {
                (0..3000usize)
                    .map(|n| {
                        let v = lo + ((n as i64 * (c as i64 * 37 + 11)) % (hi - lo + 1));
                        v as i32
                    })
                    .collect()
            })
            .collect();
        let a = make(channels, 16, 48000, samples);
        assert_round_trips(&a, &format!("{channels} channels"));
    }
}

#[test]
fn pathological_signal_shapes() {
    let (lo64, hi64) = range(16);
    let (lo, hi) = (lo64 as i32, hi64 as i32);
    let span = hi64 - lo64 + 1;
    let n = 9000usize;
    for name in [
        "silence",
        "dc_max",
        "dc_min",
        "alt_extremes",
        "ramp",
        "impulse",
    ] {
        let ch: Vec<i32> = (0..n)
            .map(|k| match name {
                "silence" => 0,
                "dc_max" => hi,
                "dc_min" => lo,
                "alt_extremes" => {
                    if k % 2 == 0 {
                        lo
                    } else {
                        hi
                    }
                }
                "ramp" => (lo64 + (k as i64 % span)) as i32,
                "impulse" => {
                    if k == 4096 {
                        hi
                    } else if k == 5000 {
                        lo
                    } else {
                        0
                    }
                }
                _ => unreachable!(),
            })
            .collect();
        let a = make(1, 16, 44100, vec![ch]);
        assert_round_trips(&a, name);
    }
}

#[test]
fn awkward_lengths() {
    // Around the 4096-sample block boundary and a few primes/odd values.
    let (lo, hi) = range(16);
    for len in [0usize, 1, 2, 3, 7, 4095, 4096, 4097, 8191, 8192, 8193, 9973] {
        let ch: Vec<i32> = (0..len)
            .map(|n| (lo + (n as i64 * 131 % (hi - lo + 1))) as i32)
            .collect();
        let a = make(1, 16, 44100, vec![ch]);
        assert_round_trips(&a, &format!("len {len}"));
    }
}

#[test]
fn sample_rate_boundaries() {
    for rate in [1u32, 8000, 44100, 192_000, 655_350, 1_048_575] {
        let ch: Vec<i32> = (0..2000).map(|n| (n % 1000) - 500).collect();
        let a = make(1, 16, rate, vec![ch]);
        assert_round_trips(&a, &format!("rate {rate}"));
    }
}

#[test]
fn thirty_two_bit_full_scale_both_containers() {
    let ch: Vec<i32> = (0..5000)
        .map(|n| if n % 2 == 0 { i32::MIN } else { i32::MAX })
        .collect();
    let a = make(2, 32, 48000, vec![ch.clone(), ch]);
    assert_round_trips(&a, "32-bit full scale");
}

#[test]
fn single_sample_each_depth_and_container() {
    for bps in [4u8, 8, 12, 16, 20, 24, 32] {
        let (lo, hi) = range(bps);
        for v in [lo as i32, 0, hi as i32] {
            let a = make(1, bps, 44100, vec![vec![v]]);
            assert_round_trips(&a, &format!("single {bps}-bit value {v}"));
        }
    }
}

// ── Encoder rejection matrix: every invalid input must be refused ───────────

#[test]
fn encoder_rejects_invalid_inputs() {
    let good = || make(2, 16, 44100, vec![vec![1, 2, 3], vec![4, 5, 6]]);

    let cases: Vec<(&str, FlacAudio)> = vec![
        ("zero channels", make(0, 16, 44100, vec![])),
        ("nine channels", make(9, 16, 44100, vec![vec![0]; 9])),
        ("bps 3", make(1, 3, 44100, vec![vec![0]])),
        ("bps 33", make(1, 33, 44100, vec![vec![0]])),
        ("rate 0", make(1, 16, 0, vec![vec![0]])),
        ("rate too high", make(1, 16, 1 << 20, vec![vec![0]])),
        ("channel count mismatch", make(2, 16, 44100, vec![vec![0]])),
        (
            "ragged channels",
            make(2, 16, 44100, vec![vec![1, 2, 3], vec![4, 5]]),
        ),
        (
            "sample over range",
            make(1, 8, 44100, vec![vec![200]]), // 8-bit max is 127
        ),
        ("sample under range", make(1, 8, 44100, vec![vec![-200]])),
    ];

    for (name, bad) in cases {
        assert!(
            matches!(encode(&bad), Err(FlacError::InvalidInput(_))),
            "native encode should reject: {name}"
        );
        assert!(
            matches!(encode_ogg(&bad), Err(FlacError::InvalidInput(_))),
            "ogg encode should reject: {name}"
        );
    }

    // The control case is accepted.
    assert!(encode(&good()).is_ok());
    assert!(encode_ogg(&good()).is_ok());
}

#[test]
fn not_flac_inputs_are_rejected_cleanly() {
    for junk in [
        &b""[..],
        &b"x"[..],
        &b"RIFFxxxxWAVE"[..],
        &b"OggS"[..],      // truncated Ogg
        &b"fLa"[..],       // truncated marker
        &[0u8; 64][..],    // all zeros
        &[0xFFu8; 64][..], // all ones
    ] {
        // Must return an error, never panic.
        assert!(decode(junk).is_err(), "decode should reject junk");
        assert!(info(junk).is_err(), "info should reject junk");
    }
}

#[test]
fn cross_container_samples_identical() {
    // The two containers must always decode to identical samples and the same
    // STREAMINFO MD5 (so a file converted between them stays bit-exact).
    for bps in [8u8, 16, 24, 32] {
        let (lo, hi) = range(bps);
        let l: Vec<i32> = (0..7000)
            .map(|n| (lo + (n as i64 * 97 % (hi - lo + 1))) as i32)
            .collect();
        let r: Vec<i32> = (0..7000)
            .map(|n| (hi - (n as i64 * 53 % (hi - lo + 1))) as i32)
            .collect();
        let a = make(2, bps, 96000, vec![l, r]);
        let from_native = decode(&encode(&a).unwrap()).unwrap();
        let from_ogg = decode(&encode_ogg(&a).unwrap()).unwrap();
        assert_eq!(
            from_native.samples, from_ogg.samples,
            "{bps}-bit cross-container"
        );
        assert_eq!(
            info(&encode(&a).unwrap()).unwrap().md5,
            info(&encode_ogg(&a).unwrap()).unwrap().md5,
            "{bps}-bit md5 across containers"
        );
    }
}
