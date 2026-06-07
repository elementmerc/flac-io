//! Adversarial input: decode() must never panic on hostile or damaged bytes.
//! It may return an error, but a parser of untrusted data must not crash. No
//! assertion on the returned value; the test passing IS the no-panic guarantee.

use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(path).unwrap()
}

/// A tiny deterministic pseudo-random generator (no external dependency).
struct Lcg(u64);
impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Lcg(0x1234_5678_9abc_def0);
    for _ in 0..2000 {
        let len = (rng.next_u32() % 512) as usize;
        let data: Vec<u8> = (0..len).map(|_| rng.next_u32() as u8).collect();
        let _ = flac_io::decode(&data);
    }
}

#[test]
fn random_bytes_with_valid_marker_never_panic() {
    // Prefixing the fLaC marker pushes the parser deeper into the metadata and
    // frame code with otherwise random bytes.
    let mut rng = Lcg(0xdead_beef_cafe_f00d);
    for _ in 0..2000 {
        let len = (rng.next_u32() % 512) as usize;
        let mut data = b"fLaC".to_vec();
        data.extend((0..len).map(|_| rng.next_u32() as u8));
        let _ = flac_io::decode(&data);
    }
}

#[test]
fn truncated_real_streams_never_panic() {
    for name in ["stereo16.flac", "mono16.flac", "stereo24.flac"] {
        let full = fixture(name);
        // Every prefix length, including the empty stream.
        for cut in 0..full.len() {
            let _ = flac_io::decode(&full[..cut]);
        }
    }
}

#[test]
fn bit_flipped_real_streams_never_panic() {
    let mut rng = Lcg(0x0badc0de_0badc0de);
    for name in ["stereo16.flac", "mono16.flac", "stereo24.flac"] {
        let full = fixture(name);
        for _ in 0..3000 {
            let mut data = full.clone();
            let pos = (rng.next_u32() as usize) % data.len();
            data[pos] ^= 1 << (rng.next_u32() % 8);
            let _ = flac_io::decode(&data);
        }
    }
}
