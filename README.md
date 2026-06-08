# flac-io

A pure-Rust FLAC decoder and encoder.

FLAC (Free Lossless Audio Codec) stores audio so that decoding gives back the
exact original samples, bit for bit. This crate reads a FLAC byte stream into
its raw integer samples and writes raw samples back into a valid FLAC stream,
without ever decoding to or from a lossy intermediate.

It exists for steganography, watermarking, forensic analysis, and any audio
work that needs direct access to the decoded sample plane with a guarantee that
a decode followed by an encode preserves the data exactly.

## What this crate does

- Decode a FLAC stream to interleaved (or per-channel) integer samples.
- Read the stream metadata: sample rate, channel count, bit depth, total
  samples.
- Encode integer samples back into a valid FLAC stream.
- Round-trip guarantee: decoding a FLAC file and re-encoding the same samples
  produces a stream that decodes to the identical samples.

## What this crate does not do

- It does not resample, dither, or change bit depth.
- It does not decode to floating-point; samples stay as signed integers in
  their native bit depth.
- It does not read Ogg-encapsulated FLAC (only the native FLAC stream format).

## Supported FLAC features

- All four subframe types: constant, verbatim, fixed (orders 0 to 4), and LPC
  (orders 1 to 32).
- Both Rice residual coding methods (4-bit and 5-bit partition parameters).
- All inter-channel decorrelation modes (independent, left/side, right/side,
  mid/side).
- Fixed and variable block-size streams.
- Bit depths from 4 to 32 bits per sample.

## Example

```rust,no_run
use flac_io::{decode, encode};

let bytes = std::fs::read("song.flac").unwrap();

// Decode to samples plus the stream parameters.
let audio = decode(&bytes).unwrap();
println!("{} Hz, {} channels, {} bits", audio.sample_rate, audio.channels, audio.bits_per_sample);

// Re-encode the same samples into a fresh FLAC stream.
let out = encode(&audio).unwrap();
std::fs::write("song_reencoded.flac", out).unwrap();
```

## Safety on untrusted input

This crate is built to decode untrusted input safely. The decoder is the part
that reads files you did not create, so it is hardened accordingly:

- **No `unsafe` code.** The crate sets `#![forbid(unsafe_code)]`, so there are
  no raw pointer tricks that could read or write out of bounds.
- **No panics on hostile input.** Every length, code, and field is validated at
  the point it is read. A malformed, truncated, or deliberately crafted stream
  returns an error; it never crashes the process. This is covered by fuzz-style
  tests that throw millions of random and bit-flipped bytes at the decoder.
- **Bounded memory.** Decoding will not allocate without limit. The total
  number of decoded samples is capped (near one billion, about four gibibytes
  of buffer), so a few kilobytes of crafted input cannot trick the decoder into
  asking for hundreds of gigabytes. A stream that needs more returns a
  `LimitExceeded` error.
- **Bit-exact self-check.** When the stream carries an MD5 of its samples (the
  normal case), the decoder recomputes it and rejects any stream whose samples
  do not match. A stream that records no MD5 (an all-zero digest) skips this
  check, so treat samples from such streams as unverified.

See [`SECURITY.md`](SECURITY.md) for the full threat model and how to report a
vulnerability, and [`docs/architecture.md`](docs/architecture.md) for the
internal design.

## Licence

Licensed under either of Apache License, Version 2.0 or MIT licence at your
option.
