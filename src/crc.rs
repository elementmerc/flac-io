// FLAC integrity check words.
//
// Two CRCs guard a FLAC stream. The frame header ends in an 8-bit CRC over
// every header byte; the whole frame ends in a 16-bit CRC over every byte of
// the frame up to (but not including) that 16-bit word. Both are plain
// most-significant-bit-first CRCs with no reflection and a zero initial value.

/// CRC-8 with polynomial x^8 + x^2 + x^1 + x^0 (0x07), used on frame headers.
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// CRC-16 with polynomial x^16 + x^15 + x^2 + x^0 (0x8005), used on whole
/// frames.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_empty_is_zero() {
        assert_eq!(crc8(&[]), 0);
    }

    #[test]
    fn crc16_empty_is_zero() {
        assert_eq!(crc16(&[]), 0);
    }

    #[test]
    fn crc8_known_vector() {
        // A single 0x00 byte clocked through leaves the register at zero; a
        // 0x01 byte leaves the low polynomial residue.
        assert_eq!(crc8(&[0x00]), 0x00);
        assert_eq!(crc8(&[0x01]), 0x07);
    }

    #[test]
    fn crc16_known_vector() {
        assert_eq!(crc16(&[0x00]), 0x0000);
        assert_eq!(crc16(&[0x01]), 0x8005);
    }
}
