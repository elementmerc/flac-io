//! Decode real FLAC files produced by the reference encoder and verify the
//! result against the MD5 that the encoder stored in STREAMINFO. A match
//! proves the decode is bit-exact, because FLAC computes that digest over the
//! original samples.

use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn stereo16_decodes_and_self_checks() {
    let audio = flac_io::decode(&fixture("stereo16.flac")).expect("decode stereo16");
    assert_eq!(audio.channels, 2);
    assert_eq!(audio.bits_per_sample, 16);
    assert_eq!(audio.sample_rate, 44100);
    assert!(audio.samples_per_channel() > 0);
    // The MD5 check inside decode() already passed, so reaching here is proof.
}

#[test]
fn mono16_decodes() {
    let audio = flac_io::decode(&fixture("mono16.flac")).expect("decode mono16");
    assert_eq!(audio.channels, 1);
    assert_eq!(audio.bits_per_sample, 16);
}

#[test]
fn stereo24_decodes() {
    let audio = flac_io::decode(&fixture("stereo24.flac")).expect("decode stereo24");
    assert_eq!(audio.channels, 2);
    assert_eq!(audio.bits_per_sample, 24);
    assert_eq!(audio.sample_rate, 48000);
}

#[test]
fn not_flac_rejected() {
    assert!(flac_io::decode(b"not a flac file at all").is_err());
}
