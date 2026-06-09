# Test fixture attribution

The fixtures in this directory are used only to test the FLAC decoder and
encoder.

## Synthetic fixtures

Most fixtures (`mono8`, `mono16`, `stereo16`, `stereo24`, `ch3`, `ch6_51`,
`ch8_71`, `stereo24_192k`, `stereo16_bs256`, `stereo16_meta`,
`stereo16_minimal`) are synthetic tones generated with ffmpeg and encoded with
the reference `flac` tool. They contain no third-party content.

## Real-world music fixtures

`realmusic_24_96.flac`, `realmusic_16_44.flac`, `realmusic_32_96.flac`, and
`realmusic_16_44.oga` are short clips (about 0.3 seconds) cut from a recording in
the public domain:

- Work: J.S. Bach, Goldberg Variations, BWV 988 (Variatio 4)
- Performer: Kimiko Ishizaka (piano)
- Release: the Open Goldberg Variations
- Rights: released into the public domain under a Creative Commons Zero (CC0)
  dedication
- Source: https://archive.org/details/OpenGoldbergVariations

Under CC0 the recording carries no copyright or related rights, so these clips
and any re-encoding of them may be redistributed without restriction. The
attribution here is a courtesy, not a licence requirement.

### How `realmusic_32_96.flac` was made

This clip is the genuine recording stored at 32 bits per sample, kept as a
regression fixture for decoding a 32-bit stream that uses an inter-channel side
transform (the side channel then needs 33 bits). The reference encoder only
takes that path when the side subframe has no wasted low bits, so the real
24-bit samples were placed into a 32-bit container unscaled (rather than
left-shifted, which would leave eight wasted bits and keep the side channel at
25 bits). The result decodes bit-for-bit; it is the same performance, only at a
wider sample depth. To regenerate it:

1. Decode the real samples from `realmusic_24_96.flac`.
2. Write them, unscaled, as little-endian signed 32-bit interleaved PCM.
3. Encode with the reference tool:
   `flac --endian=little --sign=signed --channels=2 --bps=32 \
         --sample-rate=96000 --force-raw-format -o realmusic_32_96.flac raw.pcm`

At least one frame in the result uses the `MID_SIDE` assignment with zero wasted
bits, which is the case the fixture exists to cover.

### How `realmusic_16_44.oga` was made

The same public-domain recording, encoded by the reference tool into the Ogg
container instead of native FLAC, as a fixture for decoding Ogg-wrapped FLAC:
`flac --ogg`. The audio is identical to `realmusic_16_44.flac`; only the
container differs.
