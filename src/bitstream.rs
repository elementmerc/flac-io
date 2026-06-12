// Most-significant-bit-first bit reader over a byte slice.
//
// FLAC packs its fields as a stream of bits, big bit first within each byte.
// Everything the decoder reads (header fields, unary quotients, Rice
// remainders, raw sample words) comes through this reader. It tracks a byte
// cursor and a bit offset inside the current byte so callers can also ask for
// the bytes consumed so far when they need to run a CRC.

use crate::error::FlacError;

/// A guard against a hostile stream asking us to count an unbounded run of
/// zero bits in a unary code. No legitimate FLAC value needs anywhere near
/// this many; a longer run means the input is corrupt or adversarial.
const MAX_UNARY_BITS: u32 = 1 << 20;

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// Index of the next byte to draw bits from.
    byte_pos: usize,
    /// Number of bits already consumed from `data[byte_pos]`, 0 to 7.
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    /// Total bits not yet consumed.
    pub(crate) fn bits_left(&self) -> usize {
        (self.data.len() - self.byte_pos) * 8 - self.bit_pos as usize
    }

    /// True when the cursor sits on a byte boundary.
    pub(crate) fn is_byte_aligned(&self) -> bool {
        self.bit_pos == 0
    }

    /// Index of the next unread byte. Only meaningful when byte aligned.
    pub(crate) fn byte_position(&self) -> usize {
        self.byte_pos
    }

    /// Borrow the underlying bytes (for CRC over a consumed range).
    pub(crate) fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Read `n` bits (0 to 32) most-significant first into a u32.
    pub(crate) fn read_u32(&mut self, n: u32) -> Result<u32, FlacError> {
        debug_assert!(n <= 32);
        if n == 0 {
            return Ok(0);
        }
        if (self.bits_left() as u64) < n as u64 {
            return Err(FlacError::Truncated);
        }
        let mut value: u32 = 0;
        let mut remaining = n;
        while remaining > 0 {
            let cur = self.data[self.byte_pos];
            let avail = 8 - self.bit_pos; // bits left in the current byte
            let take = remaining.min(avail as u32) as u8;
            // Shift the wanted slice down to the low bits of the byte, then mask.
            let shift = avail - take;
            let mask = if take == 8 { 0xFF } else { (1u8 << take) - 1 };
            let bits = (cur >> shift) & mask;
            value = (value << take) | bits as u32;
            self.bit_pos += take;
            if self.bit_pos == 8 {
                self.bit_pos = 0;
                self.byte_pos += 1;
            }
            remaining -= take as u32;
        }
        Ok(value)
    }

    /// Read `n` bits (0 to 64) most-significant first into a u64.
    pub(crate) fn read_u64(&mut self, n: u32) -> Result<u64, FlacError> {
        debug_assert!(n <= 64);
        if n <= 32 {
            return Ok(self.read_u32(n)? as u64);
        }
        let high = self.read_u32(n - 32)? as u64;
        let low = self.read_u32(32)? as u64;
        Ok((high << 32) | low)
    }

    /// Read `n` bits as a two's-complement signed value, sign-extended.
    pub(crate) fn read_signed(&mut self, n: u32) -> Result<i32, FlacError> {
        debug_assert!((1..=32).contains(&n));
        let raw = self.read_u32(n)?;
        if n == 32 {
            return Ok(raw as i32);
        }
        let sign_bit = 1u32 << (n - 1);
        if raw & sign_bit != 0 {
            // Set the bits above n so the value is correctly negative.
            Ok((raw | !((1u32 << n) - 1)) as i32)
        } else {
            Ok(raw as i32)
        }
    }

    /// Read `n` bits (1 to 33) as a two's-complement signed value into an i64.
    ///
    /// Subframe samples need this wider read: a side channel of a 32-bit stream
    /// has an effective depth of 33 bits, one past what [`read_signed`] (which
    /// returns an i32) can represent. The i64 result is narrowed back to i32 by
    /// the decoder only after the inter-channel transform is undone, by which
    /// point each channel's values fit 32 bits again.
    ///
    /// [`read_signed`]: BitReader::read_signed
    pub(crate) fn read_signed_wide(&mut self, n: u32) -> Result<i64, FlacError> {
        debug_assert!((1..=33).contains(&n));
        let raw = self.read_u64(n)?;
        let sign_bit = 1u64 << (n - 1);
        if raw & sign_bit != 0 {
            // Set every bit above n so the value is correctly negative.
            Ok((raw | !((1u64 << n) - 1)) as i64)
        } else {
            Ok(raw as i64)
        }
    }

    /// Read a unary-coded value: the number of zero bits before the first one
    /// bit. The terminating one bit is consumed.
    pub(crate) fn read_unary(&mut self) -> Result<u32, FlacError> {
        let mut count: u32 = 0;
        loop {
            if self.bits_left() == 0 {
                return Err(FlacError::Truncated);
            }
            let bit = self.read_u32(1)?;
            if bit == 1 {
                return Ok(count);
            }
            count += 1;
            if count > MAX_UNARY_BITS {
                return Err(FlacError::CorruptStream(
                    "unary code exceeds the sane length cap".into(),
                ));
            }
        }
    }

    /// Advance to the next byte boundary, discarding any partial bits.
    pub(crate) fn align_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bits_across_byte_boundary() {
        // 0b1010_1100, 0b1111_0000
        let mut r = BitReader::new(&[0xAC, 0xF0]);
        assert_eq!(r.read_u32(3).unwrap(), 0b101);
        assert_eq!(r.read_u32(7).unwrap(), 0b0110011);
        assert_eq!(r.read_u32(6).unwrap(), 0b110000);
    }

    #[test]
    fn read_zero_bits_is_zero() {
        let mut r = BitReader::new(&[0xFF]);
        assert_eq!(r.read_u32(0).unwrap(), 0);
        assert_eq!(r.bits_left(), 8);
    }

    #[test]
    fn full_32_and_64_bit_reads() {
        let mut r = BitReader::new(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        assert_eq!(r.read_u32(32).unwrap(), 0x1234_5678);
        assert_eq!(r.read_u32(32).unwrap(), 0x9ABC_DEF0);

        let mut r2 = BitReader::new(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        assert_eq!(r2.read_u64(64).unwrap(), 0x1234_5678_9ABC_DEF0);
    }

    #[test]
    fn signed_sign_extension() {
        // 0b1110 in 4 bits is -2; 0b0110 is +6.
        let mut r = BitReader::new(&[0b1110_0110]);
        assert_eq!(r.read_signed(4).unwrap(), -2);
        assert_eq!(r.read_signed(4).unwrap(), 6);
    }

    #[test]
    fn signed_full_width() {
        let mut r = BitReader::new(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(r.read_signed(32).unwrap(), -1);
    }

    #[test]
    fn signed_wide_handles_33_bits() {
        // 33 set bits is -1 in 33-bit two's complement.
        let mut r = BitReader::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x80]);
        assert_eq!(r.read_signed_wide(33).unwrap(), -1);

        // The most negative 33-bit value, 1 followed by 32 zeros, is -2^32.
        let mut r = BitReader::new(&[0x80, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(r.read_signed_wide(33).unwrap(), -(1i64 << 32));

        // A positive 33-bit value: 0 then 32 ones is 2^32 - 1.
        let mut r = BitReader::new(&[0x7F, 0xFF, 0xFF, 0xFF, 0x80]);
        assert_eq!(r.read_signed_wide(33).unwrap(), (1i64 << 32) - 1);
    }

    #[test]
    fn unary_counts_leading_zeros() {
        // 0b0001_0010: first unary value is 3 (three zeros then a one),
        // remaining bits 0010 give a unary value of 2.
        let mut r = BitReader::new(&[0b0001_0010]);
        assert_eq!(r.read_unary().unwrap(), 3);
        assert_eq!(r.read_unary().unwrap(), 2);
    }

    #[test]
    fn truncated_read_errors() {
        let mut r = BitReader::new(&[0xFF]);
        assert!(matches!(r.read_u32(9), Err(FlacError::Truncated)));
    }

    #[test]
    fn unary_runs_off_the_end() {
        let mut r = BitReader::new(&[0x00]);
        assert!(matches!(r.read_unary(), Err(FlacError::Truncated)));
    }

    #[test]
    fn align_to_byte_advances_to_boundary() {
        let mut r = BitReader::new(&[0xAC, 0xF0, 0x11]);
        r.read_u32(3).unwrap();
        assert!(!r.is_byte_aligned());
        r.align_to_byte();
        assert!(r.is_byte_aligned());
        assert_eq!(r.byte_position(), 1);
        // Aligning when already on a boundary is a no-op.
        r.align_to_byte();
        assert_eq!(r.byte_position(), 1);
        assert_eq!(r.read_u32(8).unwrap(), 0xF0);
    }
}
