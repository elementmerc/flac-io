# Security policy

`flac-io` decodes and encodes FLAC audio. The decoder reads files that may come
from anywhere, so it is written to treat every byte as hostile until it has been
checked. This document explains what the crate guards against, what it does not,
and how to report a problem.

## Reporting a vulnerability

If you find a way to make the decoder crash, loop forever, read out of bounds,
or allocate without limit on some input, please report it privately first.

- Email: daniel@themalwarefiles.com
- Please include the input bytes (or a small program that builds them) and the
  observed behaviour.

Please do not open a public issue for a security report until a fix is
available. A normal correctness bug that is not a security issue can go straight
to the public issue tracker.

## Threat model

The unit of distrust is a byte slice handed to `decode` or `info`. The encoder
works on samples the calling program already holds, so its input is trusted; the
decoder's input is not.

What an attacker is assumed to control:

- The entire byte stream: the `fLaC` marker, every metadata block, and every
  audio frame.
- All declared sizes and counts: block sizes, sample totals, partition orders,
  predictor orders, wasted-bit counts, and the coded frame numbers.
- The stored CRCs and the stored MD5 digest.

What the attacker must not be able to do:

- Crash the process (no panic, no abort, no out-of-bounds access).
- Make the decoder run for an unbounded time.
- Make the decoder allocate an unbounded amount of memory.
- Make the decoder return samples that silently disagree with a stream that
  carries a valid MD5.

## What the decoder guarantees

| Guarantee | How it is enforced |
|---|---|
| No memory unsafety | `#![forbid(unsafe_code)]` across the whole crate, so there is no unchecked indexing or pointer arithmetic. |
| No panic on hostile input | Every field is range-checked where it is read; all integer maths on stream-derived values uses checked or wrapping operations, never an operation that can overflow-panic. |
| Bounded time | The bit reader can only move forward through the input, and the one unbounded loop (counting a unary run) has a hard cap. Every read past the end of the data returns a truncation error. |
| Bounded memory | The total decoded sample count is capped near one billion (about four gibibytes of buffer). A STREAMINFO that declares more is rejected before any allocation; a stream that expands past the cap while decoding is rejected mid-stream. |
| Bit-exact output | When the stream stores an MD5 of its samples, the decoder recomputes that digest over the decoded samples and rejects any mismatch. |

## Input validation boundaries

Validation happens where untrusted data enters, then internal code is trusted:

1. **Stream marker and metadata.** The `fLaC` marker is checked first. Each
   metadata block length is bounds-checked with overflow-safe arithmetic before
   the block is read. STREAMINFO must be present, exactly once, with the right
   length, and a non-zero sample rate.
2. **Frame headers.** The sync code, reserved bits, block-size code, sample-rate
   code, channel assignment, and sample-size code are all validated. Reserved
   and invalid codes are rejected. The frame's CRC-8 is checked before any
   samples are read.
3. **Subframes.** The subframe type, the wasted-bit count, the effective bit
   depth, the predictor order, the partition order, and the Rice parameters are
   each range-checked. Orders that exceed the block size, partition counts that
   do not divide the block, and reserved coding methods are rejected.
4. **Whole frame.** The frame's CRC-16 is checked after the samples are decoded.
5. **Ogg pages (for `.oga` input).** Before any FLAC parsing, the Ogg demuxer
   reads the container. Every page field (segment table, body length, offsets)
   is bounds-checked with overflow-safe arithmetic, and every page's CRC-32 is
   verified against the page contents, so a damaged or truncated page is
   rejected rather than read out of bounds. Packets are reassembled from page
   bodies one-to-one, so an Ogg file cannot expand into more FLAC data than it
   contains. The rebuilt FLAC stream then goes through boundaries 1 to 4 above.

## Known limitations

- **Unverified streams.** A FLAC stream may store an all-zero MD5, which by
  convention means "no digest recorded". The decoder cannot bit-exactly verify
  such a stream, so it decodes it without the self-check. Samples from a
  zero-MD5 stream should be treated as unverified.
- **MD5 is a checksum, not a defence.** The MD5 self-check detects accidental
  corruption and decoder mistakes. MD5 is not collision-resistant, so it is not
  evidence that a stream was not deliberately altered by someone who also
  recomputed the digest. It is used here only as an integrity check, never for
  authentication.

## What is out of scope

- The crate does not execute, follow, or act on any metadata it skips (comments,
  pictures, application blocks). It reads their lengths to step over them and
  nothing more.
- For Ogg input, only the FLAC logical stream is read. Other multiplexed streams
  (for example a video track) are stepped over by serial number, not decoded.
