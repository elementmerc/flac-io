// FLAC integrity check words.
//
// Two CRCs guard a FLAC stream. The frame header ends in an 8-bit CRC over
// every header byte; the whole frame ends in a 16-bit CRC over every byte of
// the frame up to (but not including) that 16-bit word. Both are plain
// most-significant-bit-first CRCs with no reflection and a zero initial value.

/// CRC-8 with polynomial x^8 + x^2 + x^1 + x^0 (0x07), used on frame headers.
pub(crate) fn crc8(data: &[u8]) -> u8 {
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
pub(crate) fn crc16(data: &[u8]) -> u16 {
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

/// CRC-32 with polynomial 0x04C11DB7, used on Ogg pages.
///
/// This is the Ogg framing checksum, computed most-significant-bit-first with no
/// input or output reflection and a zero initial value, which makes it a
/// different CRC from the reflected CRC-32 used by zip and PNG. It is taken over
/// a whole Ogg page with the checksum field itself zeroed.
pub(crate) fn ogg_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
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

    #[test]
    fn ogg_crc32_known_vectors() {
        // The standard check string for this CRC variant (poly 0x04C11DB7,
        // init 0, no reflection, no final xor). Cross-checked against a real
        // libFLAC-produced Ogg page in the Ogg integration tests.
        assert_eq!(ogg_crc32(b""), 0x0000_0000);
        assert_eq!(ogg_crc32(b"123456789"), 0x89a1_897f);
    }
}
