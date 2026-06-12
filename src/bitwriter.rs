// Most-significant-bit-first bit writer, the mirror of `BitReader`.
//
// The encoder builds a FLAC stream a field at a time: fixed-width integers,
// unary run codes, and raw sample words, all packed big bit first. Completed
// bytes flow into an output buffer as they fill, with a partial byte held back
// until more bits arrive or the caller asks to align. Whenever the writer is
// byte aligned the buffer is exactly the bytes emitted so far, which lets the
// caller run a CRC over any range it has written.

pub(crate) struct BitWriter {
    out: Vec<u8>,
    /// Bits accumulated for the byte currently being filled, left aligned.
    current: u8,
    /// How many bits of `current` are used, 0 to 7.
    used: u8,
}

impl BitWriter {
    pub(crate) fn new() -> Self {
        BitWriter {
            out: Vec::new(),
            current: 0,
            used: 0,
        }
    }

    /// True when no partial byte is pending.
    pub(crate) fn is_byte_aligned(&self) -> bool {
        self.used == 0
    }

    /// Borrow the emitted bytes.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.out
    }

    /// Write the low `n` bits of `value`, most significant first.
    pub(crate) fn write_bits(&mut self, value: u64, n: u32) {
        debug_assert!(n <= 64);
        let mut remaining = n;
        while remaining > 0 {
            let free = 8 - self.used; // free bits in the current byte
            let take = remaining.min(free as u32) as u8;
            // The next `take` bits of value, counting down from bit (remaining-1).
            let shift = remaining - take as u32;
            let chunk = ((value >> shift) & ((1u64 << take) - 1)) as u8;
            // Place chunk into current, left aligned after the used bits.
            self.current |= chunk << (free - take);
            self.used += take;
            if self.used == 8 {
                self.out.push(self.current);
                self.current = 0;
                self.used = 0;
            }
            remaining -= take as u32;
        }
    }

    /// Write a unary code: `q` zero bits followed by a single one bit.
    pub(crate) fn write_unary(&mut self, q: u64) {
        let mut left = q;
        while left >= 8 {
            self.write_bits(0, 8);
            left -= 8;
        }
        // `left` zeros then a one: a value of 1 in (left + 1) bits.
        self.write_bits(1, left as u32 + 1);
    }

    /// Pad with zero bits up to the next byte boundary.
    pub(crate) fn align_to_byte(&mut self) {
        if self.used != 0 {
            self.out.push(self.current);
            self.current = 0;
            self.used = 0;
        }
    }

    /// Finish writing and return the buffer. Pads a trailing partial byte.
    pub(crate) fn into_bytes(mut self) -> Vec<u8> {
        self.align_to_byte();
        self.out
    }
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::BitReader;

    #[test]
    fn round_trips_through_the_reader() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        w.write_bits(0b1111000011, 10);
        w.write_bits(0xAB, 8);
        let bytes = w.into_bytes();

        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_u32(3).unwrap(), 0b101);
        assert_eq!(r.read_u32(10).unwrap(), 0b1111000011);
        assert_eq!(r.read_u32(8).unwrap(), 0xAB);
    }

    #[test]
    fn unary_round_trips() {
        let mut w = BitWriter::new();
        for q in [0u64, 1, 5, 9, 20] {
            w.write_unary(q);
        }
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        for q in [0u64, 1, 5, 9, 20] {
            assert_eq!(r.read_unary().unwrap() as u64, q);
        }
    }

    #[test]
    fn align_emits_pending_byte() {
        let mut w = BitWriter::new();
        w.write_bits(0b1, 1);
        assert!(!w.is_byte_aligned());
        w.align_to_byte();
        assert!(w.is_byte_aligned());
        assert_eq!(w.bytes().len(), 1);
        assert_eq!(w.bytes()[0], 0b1000_0000);
    }

    #[test]
    fn wide_64_bit_write() {
        let mut w = BitWriter::new();
        w.write_bits(0x1234_5678_9ABC_DEF0, 64);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_u64(64).unwrap(), 0x1234_5678_9ABC_DEF0);
    }
}
