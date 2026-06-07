# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial FLAC decoder: native stream parsing, STREAMINFO metadata, all
  subframe types, both Rice residual methods, and every inter-channel
  decorrelation mode.
- Initial FLAC encoder producing valid, byte-stable streams.
