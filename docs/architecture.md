# flac-io architecture

This document describes how the crate is put together: the modules, how data
flows through them when decoding and encoding, and the design decisions that
shaped the code. It is written for a new contributor, so it favours being
explicit over being short.

## What the crate is for

`flac-io` turns a FLAC byte stream into raw integer samples and back again, with
a guarantee that the round trip is lossless. It exists for work that needs the
decoded sample plane directly: steganography, watermarking, and forensic
analysis. It deliberately does not resample, dither, change bit depth, or touch
floating point. Samples go in and come out as signed integers in their native
bit depth.

There is no C dependency and no `unsafe` code anywhere in the crate.

## The shape of a FLAC stream

A picture first, because the module map mirrors it.

```
fLaC                              <- 4-byte marker
+----------------------------------+
| metadata block: STREAMINFO       |  rate, channels, bit depth,
|   (always first, fixed 34 bytes) |  total samples, MD5 of samples
+----------------------------------+
| metadata block: padding/seek/... |  skipped; the decoder steps over them
|   (zero or more, any order)      |
+----------------------------------+
| frame 0                          |  a short run of samples for every channel
|   header + CRC-8                 |
|   subframe per channel           |  each channel coded independently
|   CRC-16                         |
+----------------------------------+
| frame 1 ...                      |
+----------------------------------+
```

Each frame holds the same group of samples for every channel. Within a frame,
each channel is a "subframe", encoded with whichever scheme is smallest. An
optional inter-channel transform stores the difference between two channels
instead of both outright (for example "left" and "left minus right").

## Module map

The modules split along the layers of that stream. Lower modules know nothing
about higher ones.

```
lib.rs            public API: decode(), encode(), info(), FlacAudio
  |
  +-- decoder.rs    drives a full decode: header -> every frame -> MD5 check
  |     |
  |     +-- metadata.rs   parse the fLaC marker and the metadata block chain
  |     +-- frame.rs      decode one frame and its subframes; undo decorrelation
  |
  +-- encoder.rs    drives a full encode: STREAMINFO -> frame per block
  |
  +-- ogg.rs          Ogg container: demux .oga to native FLAC, mux native to .oga
  |
  +-- (shared building blocks)
        bitstream.rs     BitReader: pull big-bit-first fields from a byte slice
        bitwriter.rs     BitWriter: push big-bit-first fields into a byte buffer
        crc.rs           CRC-8 and CRC-16 (frame words) and CRC-32 (Ogg pages)
        md5.rs           self-contained MD5 for the sample digest
        sample_bytes.rs  serialise samples the way FLAC's MD5 expects
        error.rs         the single public error type, FlacError
```

| Module | Responsibility | Knows about |
|---|---|---|
| `lib.rs` | Public surface and the `FlacAudio` type | everything below |
| `decoder.rs` | Orchestrate a decode, enforce the size cap, run the MD5 check | metadata, frame, sample_bytes, ogg |
| `encoder.rs` | Orchestrate an encode, choose subframe types, write bytes | bitwriter, crc, sample_bytes, ogg |
| `ogg.rs` | Demux `.oga` to native FLAC and mux native FLAC to `.oga` | crc, metadata |
| `metadata.rs` | The header: marker plus metadata block chain | bitstream |
| `frame.rs` | One frame: header, subframes, residuals, decorrelation | bitstream, crc |
| `bitstream.rs` | Read N bits, signed values, unary codes | nothing |
| `bitwriter.rs` | Write N bits, unary codes, byte alignment | nothing |
| `crc.rs` | The two FLAC CRCs plus the Ogg page CRC-32 | nothing |
| `md5.rs` | Streaming MD5 | nothing |
| `sample_bytes.rs` | Sample-to-bytes layout for the MD5 | md5 |
| `error.rs` | `FlacError` and its messages | nothing |

## Decode data flow

```
bytes ──▶ read_header ──▶ StreamInfo + frame_start
                              │
              size-cap pre-flight (reject absurd declared totals)
                              │
                              ▼
        ┌───────────── decode loop ─────────────┐
        │  while more samples are needed and     │
        │  enough data remains:                  │
        │     decode_frame ──▶ append per channel │
        │     enforce the running size cap        │
        └────────────────────────────────────────┘
                              │
              trim any overshoot past the declared total
                              │
              recompute MD5 over the samples; compare
                              │
                              ▼
                         FlacAudio
```

`decode_frame` itself:

1. Read the frame header (sync code, block size, channel assignment, bit depth)
   and check its CRC-8.
2. Decode one subframe per channel. A subframe is one of:
   - **constant** (one value repeated for the whole block),
   - **verbatim** (every sample stored raw),
   - **fixed** (a polynomial predictor of order 0 to 4 plus Rice residuals),
   - **LPC** (a linear predictor of order 1 to 32 plus Rice residuals).
3. Undo the inter-channel transform if the frame used one.
4. Check the whole-frame CRC-16.

Predictor restoration runs in `i64` with wrapping arithmetic. On a valid stream
the values fit and wrapping behaves like ordinary maths; on a corrupt stream a
runaway integrator wraps instead of panicking, and the MD5 check then rejects
the result.

## Encode data flow

```
FlacAudio ──▶ validate ──▶ write STREAMINFO (params + sample MD5)
                                  │
                  for each fixed-size block of samples:
                        write_frame
                          header + CRC-8
                          per channel: smallest of
                            constant / fixed+Rice / verbatim
                          CRC-16
                                  │
                                  ▼
                               bytes
```

The encoder aims for correctness and reproducibility, not maximum compression.
It uses a fixed block size, stores channels independently (no mid/side), and
picks each subframe type by a deterministic size comparison. Because every
choice is a pure function of the input, encoding the same samples twice produces
byte-identical output. The streams it writes are read back by the reference
`flac` decoder, not only by this crate.

## Ogg encapsulation

FLAC travels in two containers. The native `.flac` stream is the bytes described
above. The Ogg form (`.oga`) carries the same FLAC data inside the generic Ogg
container, cut into "packets" and grouped into "pages", each page checksummed.

```
  .oga:  [OggS page][OggS page]...    each page wraps one or more packets
            packet 0: FLAC mapping header (carries STREAMINFO)
            packet 1: VORBIS_COMMENT (and any other metadata blocks)
            packet 2..: one FLAC audio frame each
```

The crate handles this as a thin layer around the existing codec, because the
FLAC data inside an Ogg packet is byte-for-byte the same as in a native stream:

- **Decode and `info`** detect the container from the first four bytes (`OggS`
  versus `fLaC`). For Ogg, `ogg.rs` walks the pages, verifies each page's
  CRC-32, reassembles the packets, and concatenates them back into a native
  FLAC byte stream, which then goes through the ordinary decode path. The
  reassembly is one-to-one (Ogg adds no compression), so it cannot expand the
  input. `info` stops after the header packets.
- **Encode** is the reverse: `encode_ogg` builds the same STREAMINFO and frames
  the native encoder produces, wraps STREAMINFO in the FLAC-to-Ogg mapping
  header packet, adds a minimal VORBIS_COMMENT (players expect a comment header,
  as Vorbis and Opus carry one), and pages the packets up with the granule
  positions and checksums Ogg requires. A fixed stream serial keeps the output
  byte-stable.

This means the Ogg layer reuses the whole FLAC decoder and the MD5 self-check
unchanged; only the page framing is new.

## The lossless guarantee, and how it is checked

FLAC stores an MD5 of the original samples in STREAMINFO. The strongest possible
decode self-check is to recompute that digest over the decoded samples: if the
two match, every sample came back bit for bit. `sample_bytes.rs` serialises
samples exactly the way FLAC's MD5 expects (interleaved by channel, each sample
little-endian in the smallest whole number of bytes for the depth), so the same
code serves both the decoder's self-check and the encoder's stored digest.

A stream may record an all-zero digest, which means "no MD5 recorded". In that
case the check is skipped and the samples are returned unverified.

## Resource limits and crash safety

The decoder reads untrusted input, so it is bounded on every axis. The concrete
caps live next to the code that enforces them:

| Limit | Value | Where | Why |
|---|---|---|---|
| Unary run length | 2^20 bits | `bitstream.rs` | A corrupt unary code cannot spin forever counting zero bits. |
| Block size | 65,535 samples | `frame.rs` | The maximum a frame header can legally encode; bounds one frame's allocation. |
| LPC order | 32 | `frame.rs` | The maximum legal predictor order. |
| Partition order | 16 | `frame.rs` | Bounds the residual partition count. |
| Total decoded samples | ~2^30 across channels | `decoder.rs` | Holds the whole output buffer near four gibibytes, so a small input cannot expand without limit. |

The total-samples cap is the load-bearing defence against a decompression bomb.
A constant subframe stores a single value but expands to a whole block, an
amplification of tens of thousands to one, so without this cap a few kilobytes
of crafted frames could ask for hundreds of gigabytes. The cap is checked twice:
once against the total declared in STREAMINFO before any allocation (fail fast),
and again against the running count inside the decode loop (so an
under-declared or zero total cannot slip past).

Every read that would pass the end of the input returns a truncation error
rather than reading out of bounds, and the bit reader only ever moves forward,
so decode time is bounded by the input length.

## Error model

There is one public error type, `FlacError`, with a variant per failure class:

- `NotFlac`: the input does not start with `fLaC`.
- `Truncated`: the stream ended in the middle of a field.
- `CorruptStream(reason)`: a structural value is impossible or out of range.
- `Unsupported(what)`: a valid feature this crate does not implement.
- `CrcMismatch`: a stored CRC or the sample MD5 did not match.
- `LimitExceeded(what)`: the stream is valid but exceeds a decoder safety cap.
- `InvalidInput(reason)`: the samples handed to the encoder are inconsistent.

Every variant carries a human-readable message, and the parameterised ones name
the specific fault.

## Testing strategy

The test suite is layered to match the test pyramid:

- **Unit tests** live next to each module (bit reader and writer round trips,
  CRC vectors, MD5 vectors, individual subframe error branches).
- **Round-trip and property tests** assert the lossless and determinism
  invariants across many channel counts, bit depths, lengths, and signal
  shapes, with a zero-dependency generator.
- **Real-world tests** decode committed fixtures produced by the reference
  encoder (bit depths 8 to 24, one to eight channels, rates to 192 kHz, streams
  with padding, seek tables, comments, and pictures), and re-encode their
  samples.
- **Ogg tests** (`tests/ogg.rs`) decode a real `flac --ogg` fixture bit-exact
  and round-trip the crate's own `encode_ogg` across bit depths, channel counts,
  and stream lengths that span many pages.
- **Adversarial tests** throw millions of random and bit-flipped bytes at the
  decoder, including Ogg-marked and truncated/bit-flipped `.oga` input, and
  assert it never panics.
- **Security regression tests** (`tests/security.rs`) pin the resource-cap and
  crash-safety guarantees with hand-crafted hostile streams.

An optional heavy run decodes a local corpus of real `.flac` files when
`FLAC_IO_CORPUS` points at a directory; it is ignored by default.

## Design decisions worth knowing

- **Pure Rust, no `unsafe`.** The crate is meant to decode untrusted input, so
  memory safety is not negotiable. `#![forbid(unsafe_code)]` makes that a
  compile-time guarantee rather than a promise.
- **The MD5 is the correctness oracle.** Rather than trust the decode path, the
  decoder proves it against the stored digest. This is why the predictor maths
  can safely use wrapping arithmetic: any wrong result is caught.
- **Encoder favours reproducibility over ratio.** A byte-stable encoder is far
  more useful for the crate's purpose (watermarking, forensic comparison) than
  one that squeezes out the last few percent, so the encoder is deterministic by
  construction and leaves higher compression to future work.
- **Public samples are `i32`, decode works in `i64`.** The `i32` output is the
  natural fit for FLAC's 4-to-32-bit depths and keeps the public type simple.
  Internally the decoder carries each subframe as `i64` until the inter-channel
  transform is undone, because a side channel of a 32-bit stream needs 33 bits.
  Once the transform is undone every channel fits back into 32 bits, so the
  narrowing cast to `i32` is lossless for any valid stream (and any stream where
  it would not be is caught by the MD5 check). This keeps full 32-bit support,
  side transforms included, without widening the public type.
