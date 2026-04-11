/// Salsa20 stream cipher (20 rounds).
///
/// Ported from the reference C# implementation used by CASCLib.  Only
/// decryption is needed for CASC; since Salsa20 is symmetric the same
/// keystream is used for both directions.
///
/// The cipher accepts a 16-byte (128-bit) or 32-byte (256-bit) key and an
/// 8-byte nonce/IV.
pub struct Salsa20 {
    state: [u32; 16],
}

impl Salsa20 {
    /// Initialise with a 16- or 32-byte key and an 8-byte IV.
    ///
    /// # Panics
    ///
    /// Panics if `key.len()` is not 16 or 32, or if `iv.len()` is not 8.
    pub fn new(key: &[u8], iv: &[u8]) -> Self {
        assert!(key.len() == 16 || key.len() == 32, "Salsa20 key must be 16 or 32 bytes");
        assert_eq!(iv.len(), 8, "Salsa20 IV must be 8 bytes");

        let mut s = [0u32; 16];

        // Constants ("expand 32-byte k" or "expand 16-byte k")
        let (c0, c1, c2, c3) = if key.len() == 32 {
            (0x61707865u32, 0x3320646eu32, 0x79622d32u32, 0x6b206574u32)
        } else {
            (0x61707865u32, 0x3120646eu32, 0x79622d36u32, 0x6b206574u32)
        };

        s[0]  = c0;
        s[5]  = c1;
        s[10] = c2;
        s[15] = c3;

        // First half of key (always present)
        s[1] = u32_le(key, 0);
        s[2] = u32_le(key, 4);
        s[3] = u32_le(key, 8);
        s[4] = u32_le(key, 12);

        // Second half of key: uses the first half again for 128-bit keys.
        let key2_off = if key.len() == 32 { 16 } else { 0 };
        s[11] = u32_le(key, key2_off);
        s[12] = u32_le(key, key2_off + 4);
        s[13] = u32_le(key, key2_off + 8);
        s[14] = u32_le(key, key2_off + 12);

        // IV / nonce
        s[6] = u32_le(iv, 0);
        s[7] = u32_le(iv, 4);

        // Counter (starts at 0)
        s[8] = 0;
        s[9] = 0;

        Salsa20 { state: s }
    }

    /// XOR-decrypt (or encrypt — the operation is identical) `data` in place.
    pub fn apply_keystream(&mut self, data: &mut [u8]) {
        let mut keystream = [0u8; 64];
        let mut pos = 0;

        while pos < data.len() {
            self.generate_block(&mut keystream);
            let n = (data.len() - pos).min(64);
            for i in 0..n {
                data[pos + i] ^= keystream[i];
            }
            pos += n;
        }
    }

    /// Generate one 64-byte keystream block and advance the counter.
    fn generate_block(&mut self, out: &mut [u8; 64]) {
        // Work on a copy of the state so we can add it back at the end.
        let mut x = self.state;

        // 20 rounds (10 double-rounds: column then row).
        for _ in 0..10 {
            // Column round
            qr(&mut x, 4,  0,  12, 7);
            qr(&mut x, 8,  4,  0,  9);
            qr(&mut x, 12, 8,  4,  13);
            qr(&mut x, 0,  12, 8,  18);
            qr(&mut x, 9,  5,  1,  7);
            qr(&mut x, 13, 9,  5,  9);
            qr(&mut x, 1,  13, 9,  13);
            qr(&mut x, 5,  1,  13, 18);
            qr(&mut x, 14, 10, 6,  7);
            qr(&mut x, 2,  14, 10, 9);
            qr(&mut x, 6,  2,  14, 13);
            qr(&mut x, 10, 6,  2,  18);
            qr(&mut x, 3,  15, 11, 7);
            qr(&mut x, 7,  3,  15, 9);
            qr(&mut x, 11, 7,  3,  13);
            qr(&mut x, 15, 11, 7,  18);
            // Row round
            qr(&mut x, 1,  0,  3,  7);
            qr(&mut x, 2,  1,  0,  9);
            qr(&mut x, 3,  2,  1,  13);
            qr(&mut x, 0,  3,  2,  18);
            qr(&mut x, 6,  5,  4,  7);
            qr(&mut x, 7,  6,  5,  9);
            qr(&mut x, 4,  7,  6,  13);
            qr(&mut x, 5,  4,  7,  18);
            qr(&mut x, 11, 10, 9,  7);
            qr(&mut x, 8,  11, 10, 9);
            qr(&mut x, 9,  8,  11, 13);
            qr(&mut x, 10, 9,  8,  18);
            qr(&mut x, 12, 15, 14, 7);
            qr(&mut x, 13, 12, 15, 9);
            qr(&mut x, 14, 13, 12, 13);
            qr(&mut x, 15, 14, 13, 18);
        }

        // Add the original state and serialise to bytes.
        for i in 0..16 {
            let word = x[i].wrapping_add(self.state[i]);
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }

        // Increment 64-bit counter (state[8] is low word, state[9] is high).
        self.state[8] = self.state[8].wrapping_add(1);
        if self.state[8] == 0 {
            self.state[9] = self.state[9].wrapping_add(1);
        }
    }
}

/// Salsa20 quarter-round: `x[a] ^= (x[b].wrapping_add(x[c])).rotate_left(rot)`.
#[inline]
fn qr(x: &mut [u32; 16], a: usize, b: usize, c: usize, rot: u32) {
    x[a] ^= x[b].wrapping_add(x[c]).rotate_left(rot);
}

#[inline]
fn u32_le(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Salsa20 is self-inverse: encrypting twice restores the plaintext.
    #[test]
    fn roundtrip() {
        let key = [0xAAu8; 32];
        let iv  = [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        let plaintext = b"Hello, CASC world! This is a test of Salsa20 decryption.";

        let mut buf = plaintext.to_vec();
        Salsa20::new(&key, &iv).apply_keystream(&mut buf);
        Salsa20::new(&key, &iv).apply_keystream(&mut buf);

        assert_eq!(&buf, plaintext);
    }

    /// 16-byte key path should not panic.
    #[test]
    fn short_key() {
        let key = [0xBBu8; 16];
        let iv  = [0u8; 8];
        let mut data = [1u8; 128];
        Salsa20::new(&key, &iv).apply_keystream(&mut data);
    }

    /// Empty data is a no-op.
    #[test]
    fn empty_data() {
        let key = [0u8; 32];
        let iv  = [0u8; 8];
        let mut data: [u8; 0] = [];
        Salsa20::new(&key, &iv).apply_keystream(&mut data);
    }
}
