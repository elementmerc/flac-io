//! Security regression tests for resource exhaustion and crash safety.
//!
//! These craft hostile streams that earlier escaped the fuzz tests, which only
//! ever mutate real 16-bit and 24-bit fixtures and so never reach a 32-bit
//! side channel or a maximum-size constant subframe. Each test pins a specific
//! hardening guarantee:
//!
//!   - a tiny input cannot make `decode` allocate unbounded memory
//!     (decompression bomb via expanding constant subframes),
//!   - a STREAMINFO that simply *declares* an enormous sample total is
//!     rejected before any allocation,
//!   - a crafted 32-bit side channel with maximal wasted bits never panics.
//!
//! The streams are hand-built with a local bit packer and the two FLAC CRCs so
//! the tests depend only on the public `decode` API.

use flac_io::{decode, FlacError};

// ── Minimal FLAC writer helpers (public-API-only test scaffolding) ──────────

fn crc8(data: &[u8]) -> u8 {
    let mut c: u8 = 0;
    for &b in data {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 {
                (c << 1) ^ 0x07
            } else {
                c << 1
            };
        }
    }
    c
}

fn crc16(data: &[u8]) -> u16 {
    let mut c: u16 = 0;
    for &b in data {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 {
                (c << 1) ^ 0x8005
            } else {
                c << 1
            };
        }
    }
    c
}

struct Bits {
    out: Vec<u8>,
    cur: u8,
    used: u8,
}

impl Bits {
    fn new() -> Self {
        Bits {
            out: Vec::new(),
            cur: 0,
            used: 0,
        }
    }
    fn put(&mut self, v: u64, n: u32) {
        let mut rem = n;
        while rem > 0 {
            let free = 8 - self.used;
            let take = rem.min(free as u32) as u8;
            let shift = rem - take as u32;
            let chunk = ((v >> shift) & ((1u64 << take) - 1)) as u8;
            self.cur |= chunk << (free - take);
            self.used += take;
            if self.used == 8 {
                self.out.push(self.cur);
                self.cur = 0;
                self.used = 0;
            }
            rem -= take as u32;
        }
    }
    fn align(&mut self) {
        if self.used != 0 {
            self.out.push(self.cur);
            self.cur = 0;
            self.used = 0;
        }
    }
    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

/// `fLaC` + a last STREAMINFO block with the given parameters and a zero MD5
/// (so the digest self-check is skipped and the structural guards are what the
/// test exercises).
fn streaminfo(channels_minus_1: u64, bps_minus_1: u64, total_samples: u64, block: u64) -> Vec<u8> {
    let mut s = Bits::new();
    s.put(block, 16); // min block size
    s.put(block, 16); // max block size
    s.put(0, 24); // min frame size
    s.put(0, 24); // max frame size
    s.put(44100, 20); // sample rate
    s.put(channels_minus_1, 3);
    s.put(bps_minus_1, 5);
    s.put(total_samples, 36);
    let body = s.finish();

    let mut v = b"fLaC".to_vec();
    v.push(0x80); // last block, type 0 (STREAMINFO)
    v.extend_from_slice(&[0x00, 0x00, 0x22]); // length 34
    v.extend_from_slice(&body);
    v.extend_from_slice(&[0u8; 16]); // MD5 all zero -> skip the check
    v
}

/// One mono frame: header + CRC-8 + a constant subframe (block-size samples of
/// a single 16-bit value) + CRC-16.
fn constant_frame(frame_number: u64, block_size: u64) -> Vec<u8> {
    let mut h = Bits::new();
    h.put(0x3FFE, 14); // sync
    h.put(0, 1); // reserved
    h.put(0, 1); // fixed block size
    h.put(7, 4); // block size: explicit 16-bit at end of header
    h.put(0, 4); // sample rate from STREAMINFO
    h.put(0, 4); // mono, independent
    h.put(0, 3); // sample size from STREAMINFO
    h.put(0, 1); // reserved
    h.put(frame_number & 0x7F, 8); // single-byte coded number
    h.put(block_size - 1, 16); // block size minus one
    h.align();
    let header = h.out.clone();

    let mut f = Bits::new();
    for &b in &header {
        f.put(b as u64, 8);
    }
    f.put(crc8(&header) as u64, 8);
    // Constant subframe: pad bit, type 0, no wasted bits, a 16-bit value.
    f.put(0, 1);
    f.put(0, 6);
    f.put(0, 1);
    f.put(0, 16);
    f.align();
    let frame = f.out.clone();
    let c16 = crc16(&frame);
    f.put(c16 as u64, 16);
    f.finish()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn constant_subframe_bomb_is_capped_not_oom() {
    // A handful of frames, each a single constant value that expands to 65,535
    // samples. Total samples is declared zero so the decoder would otherwise
    // run to the end of the data, accumulating ~65k samples per ~13 input
    // bytes (an ~18,000x amplification). The cap must turn this into a clean
    // error rather than an allocation the size of the data times that ratio.
    let mut stream = streaminfo(0, 15, 0, 65535);
    for n in 0..40_000u64 {
        stream.extend_from_slice(&constant_frame(n, 65535));
    }
    match decode(&stream) {
        Err(FlacError::LimitExceeded(_)) => {}
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn declared_huge_total_is_rejected_before_allocation() {
    // STREAMINFO alone claims the maximum 36-bit sample total. No frames are
    // present; the guard must fire on the declaration, not by trying to fill a
    // 68-billion-sample buffer.
    let stream = streaminfo(1, 15, (1u64 << 36) - 1, 4096);
    match decode(&stream) {
        Err(FlacError::LimitExceeded(_)) => {}
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn declared_total_just_over_cap_is_rejected() {
    // 2 channels and a per-channel total whose product crosses the cap by one.
    // Confirms the cap is applied to the across-channel product, not per
    // channel.
    let over = (1u64 << 30) / 2 + 1;
    let stream = streaminfo(1, 15, over, 4096);
    assert!(matches!(decode(&stream), Err(FlacError::LimitExceeded(_))));
}

#[test]
fn thirtytwo_bit_side_channel_wasted_bits_never_panics() {
    // 32-bit stereo, mid/side frame whose side subframe declares 32 wasted
    // bits. The side channel's effective depth is 33, so restoring the wasted
    // bits is a shift by 32: in i32 that is an overflow that used to panic. The
    // decoder must return an error (the all-zero side reconstruction fails the
    // structural path or the frame CRC) without crashing.
    let mut v = b"fLaC".to_vec();
    let mut s = Bits::new();
    s.put(192, 16);
    s.put(192, 16);
    s.put(0, 24);
    s.put(0, 24);
    s.put(44100, 20);
    s.put(1, 3); // stereo
    s.put(31, 5); // 32-bit
    s.put(0, 36); // total unknown
    let body = s.finish();
    v.push(0x80);
    v.extend_from_slice(&[0x00, 0x00, 0x22]);
    v.extend_from_slice(&body);
    v.extend_from_slice(&[0u8; 16]);

    let mut h = Bits::new();
    h.put(0x3FFE, 14);
    h.put(0, 1);
    h.put(0, 1);
    h.put(1, 4); // block size code 1 -> 192
    h.put(0, 4); // sample rate from STREAMINFO
    h.put(10, 4); // mid/side
    h.put(7, 3); // 32-bit
    h.put(0, 1);
    h.put(0, 8); // coded number 0
    h.align();
    let header = h.out.clone();

    let mut f = Bits::new();
    for &b in &header {
        f.put(b as u64, 8);
    }
    f.put(crc8(&header) as u64, 8);
    // Subframe 0 (mid, 32-bit): constant value 0.
    f.put(0, 1);
    f.put(0, 6);
    f.put(0, 1);
    f.put(0, 32);
    // Subframe 1 (side, effective 33-bit): constant, wasted flag set, unary
    // value 31 (thirty-one zeros then a one) -> 32 wasted bits.
    f.put(0, 1);
    f.put(0, 6);
    f.put(1, 1);
    for _ in 0..31 {
        f.put(0, 1);
    }
    f.put(1, 1);
    f.put(0, 1); // 1-bit constant value (33 - 32)
    f.align();
    let frame = f.out.clone();
    let c16 = crc16(&frame);
    f.put(c16 as u64, 16);
    v.extend_from_slice(&f.finish());

    // The only guarantee under test is "no panic". Any Ok/Err is acceptable.
    let _ = decode(&v);
}
