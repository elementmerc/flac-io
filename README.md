# flac-io

A pure-Rust FLAC decoder and encoder.

FLAC, the Free Lossless Audio Codec, is a way to shrink audio files without
losing a single detail. A codec is just a "coder and decoder", the thing that
packs sound into a file and unpacks it again. "Lossless" is the important word:
unlike MP3, which throws away parts of the sound you probably will not notice,
FLAC gives you back the exact audio you started with, down to the last number.

Think of it like a ZIP file, but for sound. When you unzip a document you get
the original file back, character for character. FLAC does the same for audio.

```
   original audio              FLAC file                back again
   (a list of numbers)         (smaller)                (same numbers)
  +-------------------+ encode +-----------+  decode  +-------------------+
  |  -3  5  5  12 ... | -----> |  fLaC.... | -------> |  -3  5  5  12 ... |
  +-------------------+        +-----------+          +-------------------+
                  what comes out is identical to what went in
```

This crate reads a FLAC file into those raw numbers (called samples), and writes
samples back into a valid FLAC file. It never passes through a lossy format in
between, so the numbers always survive the trip exactly.

It is built for work that needs the raw samples directly: hiding data inside
audio (steganography), adding inaudible markers (watermarking), forensic
analysis, and anything else where you need the exact numbers and a promise that
decoding then re-encoding changes nothing.

## What a "sample" is

Sound is a wave. To store it, a computer measures the height of that wave many
thousands of times a second (44,100 times a second for a CD). Each measurement
is one sample: a single whole number. Play the numbers back fast enough and you
hear the sound again. It is the same trick a flipbook uses to turn still
drawings into motion.

This crate hands you those numbers, one list per channel (left and right for
stereo):

```
  samples[0] = left   ->  [ s0  s1  s2  s3  s4  ... ]
  samples[1] = right  ->  [ s0  s1  s2  s3  s4  ... ]
                             |
                             one column = one instant in time
```

Every channel list has the same length, and the numbers are plain signed
integers (they can be negative, because a wave goes up and down).

## What this crate does

You get three things, two doors into a FLAC file and one back out:

```
        bytes of a .flac file               raw samples
       +----------------------+  decode()   +---------------------+
       |  66 4C 61 43 00 ...   | ----------> |  [ -3, 5, 5, 12 ]   |
       |                       |             |  one list / channel |
       |                       |  encode()   |                     |
       |                       | <---------- |                     |
       +----------------------+              +---------------------+
                 ^
                 | info()  ->  just the facts: 44100 Hz, 2 channels, 16-bit
```

- **`decode`** turns FLAC bytes into the raw samples plus the basic facts about
  the audio.
- **`info`** reads just those facts (sample rate, channels, bit depth, total
  length) without decoding the audio, so it stays fast on a huge file.
- **`encode`** turns raw samples back into a valid FLAC file.

The promise that ties them together: decode a file, encode the same samples, and
the result decodes to the identical numbers. Nothing drifts.

### Two containers, read automatically

FLAC comes in two wrappers: the plain `.flac` file, and an `.oga` file where the
same FLAC data rides inside an Ogg container (the box that Vorbis and Opus use
too). You do not have to say which one you have: `decode` and `info` look at the
first few bytes and pick the right path themselves.

```
   .flac  (starts with "fLaC")  --.
                                   >--  decode() / info()  -->  same result
   .oga   (starts with "OggS")  --'
```

Writing is the one case where you choose, because raw samples do not say which
wrapper you want: `encode` writes a `.flac`, `encode_ogg` writes an `.oga`.

## What this crate does not do

- It does not change the audio: no resampling, no volume changes, no dithering,
  no switching the bit depth.
- It does not give you floating-point samples; they stay as whole numbers in the
  file's own bit depth.

So it is a precise in-and-out tool, not an audio editor.

## Which FLAC features it understands

FLAC has a few ways of packing each chunk of audio. This crate handles all of
them, so it reads files from any standard FLAC encoder:

- All four block types: constant, verbatim, fixed (orders 0 to 4), and LPC
  (orders 1 to 32). These are the maths tricks FLAC uses to describe a run of
  samples compactly.
- Both ways of coding the leftover error values (4-bit and 5-bit Rice
  parameters).
- All the stereo tricks where one channel is stored as a difference from
  another (independent, left/side, right/side, mid/side), including at the full
  32-bit depth.
- Files with a fixed block size and files that vary it.
- Bit depths from 4 to 32 bits per sample.
- Both containers: plain native FLAC (`.flac`) and Ogg-wrapped FLAC (`.oga`),
  read and written.

## Example

```rust,no_run
use flac_io::{decode, encode};

let bytes = std::fs::read("song.flac").unwrap();

// Decode to samples plus the stream parameters.
let audio = decode(&bytes).unwrap();
println!("{} Hz, {} channels, {} bits", audio.sample_rate, audio.channels, audio.bits_per_sample);

// Re-encode the same samples into a fresh FLAC file.
let out = encode(&audio).unwrap();
std::fs::write("song_reencoded.flac", out).unwrap();
```

The same `decode` call also reads an Ogg-wrapped `.oga` file with no extra
steps. To write one, swap `encode` for `flac_io::encode_ogg`.

## Safety on untrusted input

A FLAC file might come from anywhere, so the decoder treats every byte as if an
attacker wrote it. Here is what protects you:

```
  bad bytes in  ----> [ check every field ] ----> a clear error, never a crash
                      [ cap the memory     ]
                      [ no unsafe code     ]
```

- **No `unsafe` code.** The crate sets `#![forbid(unsafe_code)]`, so the compiler
  guarantees there are no memory tricks that could read or write out of bounds.
- **No crashes on bad input.** Every length and code is checked the moment it is
  read. A broken, cut-off, or deliberately nasty file returns an error; it never
  takes down your program. Tests throw millions of random and bit-flipped bytes
  at the decoder to keep this honest.
- **Bounded memory.** Decoding cannot ask for unlimited memory. The number of
  samples it will produce is capped (near one billion, about four gibibytes), so
  a few kilobytes of crafted input cannot trick it into trying to allocate
  hundreds of gigabytes. A file that wants more gets a `LimitExceeded` error.
- **Built-in correctness check.** A FLAC file usually stores a fingerprint (an
  MD5 hash) of its samples. The decoder recomputes that fingerprint from what it
  decoded and rejects the file if they disagree, so a successful decode really
  is bit-for-bit correct. A file that stores no fingerprint (all zeros) skips
  this check, so treat its samples as unverified.

See [`SECURITY.md`](SECURITY.md) for the full threat model and how to report a
problem, and [`docs/architecture.md`](docs/architecture.md) for how the code is
built.

## Licence

Licensed under either of Apache License, Version 2.0 or MIT licence at your
option.
