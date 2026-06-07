//! Real-world decoding: every committed fixture is a genuine FLAC file from
//! the reference encoder (libFLAC, via ffmpeg/flac), covering bit depths 8 to
//! 24, one to eight channels, sample rates to 192 kHz, small and large block
//! sizes, and streams with and without padding, seek tables, comments and
//! pictures.
//!
//! A successful `decode` already proves bit-exactness, because it checks the
//! decoded samples against the MD5 the encoder stored in STREAMINFO. On top of
//! that, these tests check that `info` agrees with `decode`, and that the
//! crate's own encoder can round-trip the real-world samples.

use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn all_fixtures() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "flac").unwrap_or(false))
        .collect();
    v.sort();
    assert!(v.len() >= 10, "expected the full fixture corpus");
    v
}

#[test]
fn every_fixture_decodes_and_self_checks() {
    for path in all_fixtures() {
        let bytes = std::fs::read(&path).unwrap();
        let audio =
            flac_io::decode(&bytes).unwrap_or_else(|e| panic!("decode {}: {e}", path.display()));
        // Structural sanity that holds for every stream.
        assert!((1..=8).contains(&audio.channels), "{}", path.display());
        assert!(
            (4..=32).contains(&audio.bits_per_sample),
            "{}",
            path.display()
        );
        assert!(audio.sample_rate > 0, "{}", path.display());
        assert_eq!(
            audio.samples.len(),
            audio.channels as usize,
            "{}",
            path.display()
        );
        let len = audio.samples_per_channel();
        for ch in &audio.samples {
            assert_eq!(ch.len(), len, "ragged channels in {}", path.display());
        }
    }
}

#[test]
fn info_agrees_with_decode_on_every_fixture() {
    for path in all_fixtures() {
        let bytes = std::fs::read(&path).unwrap();
        let info = flac_io::info(&bytes).unwrap();
        let audio = flac_io::decode(&bytes).unwrap();
        assert_eq!(info.sample_rate, audio.sample_rate, "{}", path.display());
        assert_eq!(info.channels, audio.channels, "{}", path.display());
        assert_eq!(
            info.bits_per_sample,
            audio.bits_per_sample,
            "{}",
            path.display()
        );
        // STREAMINFO records the total sample count for these files.
        if info.total_samples > 0 {
            assert_eq!(
                info.total_samples as usize,
                audio.samples_per_channel(),
                "{}",
                path.display()
            );
        }
    }
}

#[test]
fn encoder_round_trips_real_world_samples() {
    // decode == decode(encode(decode)): the crate's own encoder reproduces the
    // real-world samples losslessly, across multichannel and every bit depth.
    for path in all_fixtures() {
        let bytes = std::fs::read(&path).unwrap();
        let audio = flac_io::decode(&bytes).unwrap();
        let re =
            flac_io::encode(&audio).unwrap_or_else(|e| panic!("encode {}: {e}", path.display()));
        let again =
            flac_io::decode(&re).unwrap_or_else(|e| panic!("re-decode {}: {e}", path.display()));
        assert_eq!(
            again.samples,
            audio.samples,
            "round-trip {}",
            path.display()
        );
        assert_eq!(again.sample_rate, audio.sample_rate);
        assert_eq!(again.bits_per_sample, audio.bits_per_sample);
    }
}

#[test]
fn known_fixture_shapes() {
    let cases = [
        ("mono8.flac", 1u8, 8u8),
        ("ch3.flac", 3, 16),
        ("ch8_71.flac", 8, 24),
        ("stereo24_192k.flac", 2, 24),
    ];
    for (name, channels, bps) in cases {
        let bytes = std::fs::read(fixtures_dir().join(name)).unwrap();
        let a = flac_io::decode(&bytes).unwrap();
        assert_eq!(a.channels, channels, "{name}");
        assert_eq!(a.bits_per_sample, bps, "{name}");
    }
}

#[test]
fn rich_and_minimal_metadata_both_parse() {
    // The decoder must skip padding, seek tables, comments and pictures, and
    // also accept a stream with only STREAMINFO.
    for name in ["stereo16_meta.flac", "stereo16_minimal.flac"] {
        let bytes = std::fs::read(fixtures_dir().join(name)).unwrap();
        let a = flac_io::decode(&bytes).unwrap();
        assert_eq!(a.channels, 2, "{name}");
    }
}
