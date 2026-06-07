//! Property-based tests with a zero-dependency generator.
//!
//! These assert the invariants that must hold for every input, across many
//! channel counts, bit depths, lengths and signal shapes:
//!   - lossless round-trip: decode(encode(x)) == x
//!   - determinism: encode(x) == encode(x)
//!   - metadata consistency: info(encode(x)) matches x
//!   - re-encode stability: encode(decode(encode(x))) == encode(x)

use flac_io::{decode, encode, info, FlacAudio};

/// Small deterministic PRNG (SplitMix64-style), no external dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Inclusive sample range for a signed bit depth.
fn range(bps: u8) -> (i64, i64) {
    let hi = (1i64 << (bps - 1)) - 1;
    (-(hi + 1), hi)
}

/// Clamp a value into the bit-depth range.
fn fit(v: i64, bps: u8) -> i32 {
    let (lo, hi) = range(bps);
    v.clamp(lo, hi) as i32
}

/// Generate one channel of `len` samples with a chosen shape.
fn gen_channel(rng: &mut Rng, len: usize, bps: u8, shape: u64) -> Vec<i32> {
    let (lo, hi) = range(bps);
    match shape % 6 {
        0 => vec![fit(rng.next() as i64, bps); len], // constant
        1 => (0..len).map(|i| fit(i as i64 * 3 - 7, bps)).collect(), // ramp
        2 => (0..len)
            .map(|i| if i % 2 == 0 { lo as i32 } else { hi as i32 })
            .collect(), // alternating extremes
        3 => (0..len)
            .map(|_| rng.next() as i32)
            .map(|v| fit(v as i64, bps))
            .collect(), // noise
        4 => (0..len)
            .map(|i| {
                if i % 23 == 0 {
                    fit(rng.next() as i64, bps)
                } else {
                    0
                }
            })
            .collect(), // sparse spikes
        _ => (0..len)
            .map(|i| fit(((i as f64 * 0.21).sin() * hi as f64) as i64, bps))
            .collect(), // smooth sine
    }
}

fn gen_audio(rng: &mut Rng) -> FlacAudio {
    let channels = (1 + rng.below(8)) as u8; // 1..=8
    let bps = [4u8, 8, 12, 16, 20, 24, 32][rng.below(7) as usize];
    let lengths = [0usize, 1, 2, 3, 100, 4095, 4096, 4097, 8200, 5000];
    let len = lengths[rng.below(lengths.len() as u64) as usize];
    let rates = [8000u32, 22050, 44100, 48000, 96000, 192000];
    let sample_rate = rates[rng.below(rates.len() as u64) as usize];
    let samples = (0..channels)
        .map(|_| {
            let shape = rng.next();
            gen_channel(rng, len, bps, shape)
        })
        .collect();
    FlacAudio {
        sample_rate,
        channels,
        bits_per_sample: bps,
        samples,
    }
}

#[test]
fn round_trip_identity_holds_for_random_audio() {
    let mut rng = Rng(0xA5A5_1234_DEAD_0001);
    for _ in 0..600 {
        let a = gen_audio(&mut rng);
        let bytes = encode(&a).expect("encode");
        let d = decode(&bytes).expect("decode");
        assert_eq!(d.samples, a.samples, "samples differ");
        assert_eq!(d.channels, a.channels);
        assert_eq!(d.bits_per_sample, a.bits_per_sample);
        assert_eq!(d.sample_rate, a.sample_rate);
    }
}

#[test]
fn encoding_is_deterministic_for_random_audio() {
    let mut rng = Rng(0x1111_2222_3333_4444);
    for _ in 0..400 {
        let a = gen_audio(&mut rng);
        assert_eq!(encode(&a).unwrap(), encode(&a).unwrap());
    }
}

#[test]
fn info_matches_for_random_audio() {
    let mut rng = Rng(0xFEED_FACE_0000_0007);
    for _ in 0..400 {
        let a = gen_audio(&mut rng);
        let bytes = encode(&a).unwrap();
        let si = info(&bytes).unwrap();
        assert_eq!(si.channels, a.channels);
        assert_eq!(si.bits_per_sample, a.bits_per_sample);
        assert_eq!(si.sample_rate, a.sample_rate);
        assert_eq!(si.total_samples as usize, a.samples_per_channel());
    }
}

#[test]
fn re_encode_is_stable_for_random_audio() {
    // A decode then re-encode must reproduce the same bytes: the encoder is a
    // function of the samples alone.
    let mut rng = Rng(0x0C0F_FEE0_0BAD_F00D);
    for _ in 0..300 {
        let a = gen_audio(&mut rng);
        let once = encode(&a).unwrap();
        let twice = encode(&decode(&once).unwrap()).unwrap();
        assert_eq!(once, twice);
    }
}
