# Test fixture attribution

The fixtures in this directory are used only to test the FLAC decoder and
encoder.

## Synthetic fixtures

Most fixtures (`mono8`, `mono16`, `stereo16`, `stereo24`, `ch3`, `ch6_51`,
`ch8_71`, `stereo24_192k`, `stereo16_bs256`, `stereo16_meta`,
`stereo16_minimal`) are synthetic tones generated with ffmpeg and encoded with
the reference `flac` tool. They contain no third-party content.

## Real-world music fixtures

`realmusic_24_96.flac` and `realmusic_16_44.flac` are short clips (about 0.3
seconds) cut from a recording in the public domain:

- Work: J.S. Bach, Goldberg Variations, BWV 988 (Variatio 4)
- Performer: Kimiko Ishizaka (piano)
- Release: the Open Goldberg Variations
- Rights: released into the public domain under a Creative Commons Zero (CC0)
  dedication
- Source: https://archive.org/details/OpenGoldbergVariations

Under CC0 the recording carries no copyright or related rights, so these clips
and any re-encoding of them may be redistributed without restriction. The
attribution here is a courtesy, not a licence requirement.
