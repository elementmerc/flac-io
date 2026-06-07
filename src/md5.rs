// A small, self-contained MD5 implementation.
//
// FLAC stores an MD5 digest of the original samples in STREAMINFO. Computing
// the same digest over our decoded samples is the strongest possible decode
// self-check: if the digests match, every sample came back bit for bit. The
// encoder writes this digest too. MD5 is used here purely as a checksum, not
// for any security purpose, so its cryptographic weakness is irrelevant.

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// Streaming MD5 hasher.
pub struct Md5 {
    state: [u32; 4],
    /// Bytes hashed so far (for the length padding).
    length: u64,
    /// Partial block waiting for more input.
    buffer: [u8; 64],
    buffer_len: usize,
}

impl Md5 {
    pub fn new() -> Self {
        Md5 {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            length: 0,
            buffer: [0u8; 64],
            buffer_len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        // Top up an existing partial block first.
        if self.buffer_len > 0 {
            let need = 64 - self.buffer_len;
            let take = need.min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];
            if self.buffer_len == 64 {
                let block = self.buffer;
                self.process(&block);
                self.buffer_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.process(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 16] {
        let bit_len = self.length.wrapping_mul(8);
        // Append the 0x80 terminator then zero-pad to 56 mod 64.
        self.update(&[0x80]);
        // update() bumped length; undo its effect on the recorded length by
        // padding to alignment without counting these bytes is unnecessary
        // because the standard counts only message bytes. Recompute via buffer.
        while self.buffer_len != 56 {
            self.push_pad_byte();
        }
        let len_bytes = bit_len.to_le_bytes();
        // Write the length directly into the buffer and process.
        self.buffer[56..64].copy_from_slice(&len_bytes);
        let block = self.buffer;
        self.process(&block);

        let mut out = [0u8; 16];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    fn push_pad_byte(&mut self) {
        self.buffer[self.buffer_len] = 0;
        self.buffer_len += 1;
        if self.buffer_len == 64 {
            let block = self.buffer;
            self.process(&block);
            self.buffer_len = 0;
        }
    }

    fn process(&mut self, block: &[u8; 64]) {
        let mut m = [0u32; 16];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            m[i] = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = tmp;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot convenience digest.
#[cfg(test)]
pub fn digest(data: &[u8]) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(data);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8; 16]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn empty_string() {
        assert_eq!(hex(&digest(b"")), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn abc() {
        assert_eq!(hex(&digest(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn quick_brown_fox() {
        assert_eq!(
            hex(&digest(b"The quick brown fox jumps over the lazy dog")),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }

    #[test]
    fn long_input_spanning_blocks() {
        // 1000 'a' bytes — exercises the multi-block path and the length field.
        let data = vec![b'a'; 1000];
        // Reference digest of 1000 'a' characters.
        assert_eq!(hex(&digest(&data)), "cabe45dcc9ae5b66ba86600cca6b8ba8");
    }

    #[test]
    fn streaming_matches_one_shot() {
        let data = vec![0x5Au8; 130];
        let mut h = Md5::new();
        h.update(&data[..7]);
        h.update(&data[7..64]);
        h.update(&data[64..]);
        assert_eq!(h.finalize(), digest(&data));
    }
}
