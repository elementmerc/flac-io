//! Decode a FLAC file and print its parameters.
//!
//! Usage: `cargo run --example smoke -- path/to/file.flac`
//! With no argument it decodes the bundled stereo test fixture.

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tests/fixtures/stereo16.flac".to_string());

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("could not read {path}: {e}");
            std::process::exit(1);
        }
    };

    match flac_io::decode(&bytes) {
        Ok(audio) => {
            println!(
                "{path}: {} Hz, {} channel(s), {} bit, {} samples/channel",
                audio.sample_rate,
                audio.channels,
                audio.bits_per_sample,
                audio.samples_per_channel()
            );
        }
        Err(e) => {
            eprintln!("decode failed: {e}");
            std::process::exit(1);
        }
    }
}
