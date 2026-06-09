# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-06-09 — The Sound of Music

### Added

- Initial FLAC decoder: native stream parsing, STREAMINFO metadata, all
  subframe types, both Rice residual methods, and every inter-channel
  decorrelation mode.
- Initial FLAC encoder producing valid, byte-stable streams.
- `info()` reads stream metadata (sample rate, channels, bit depth, total
  samples, MD5) without decoding any audio.
- Full 32-bit decoding, including streams that use an inter-channel side
  transform. The decoder carries samples internally at 64-bit width so the
  33-bit side channel of a 32-bit stream decodes bit-exactly.

### Security

- The decoder enforces a hard cap on the total number of decoded samples, so a
  tiny crafted stream can no longer expand into a multi-gigabyte allocation. A
  stream that asks for more returns the new `LimitExceeded` error instead of
  exhausting memory.
- Fixed a decoder crash on a crafted 32-bit side-channel subframe whose wasted
  bit count reached the sample width. Sample restoration now happens at 64-bit
  width so it cannot overflow, and hostile input returns an error rather than
  panicking.

### Documentation

- Added an architecture document and a security policy describing the threat
  model, the input validation boundaries, and the decoder resource caps.
