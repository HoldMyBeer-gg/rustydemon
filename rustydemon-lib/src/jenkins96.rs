/// Compute the Bob Jenkins 96-bit hash used by CASC for filename lookup.
///
/// The input string is normalised before hashing: forward slashes are replaced
/// with back slashes and the result is converted to ASCII upper-case.  This
/// matches the convention used throughout CASC root handlers.
///
/// The return value is the 64-bit hash `((c as u64) << 32) | (b as u64)` from
/// the Jenkins lookup3 finalisation, which is what CASC stores in root entries.
///
/// # Example
///
/// ```
/// use rustydemon_lib::jenkins96;
/// let h = jenkins96("interface/glues/models/ui_mainmenu/ui_mainmenu.m2");
/// assert_ne!(h, 0);
/// ```
pub fn jenkins96(s: &str) -> u64 {
    // Normalise: '/' → '\', ASCII upper-case.
    let normalised: Vec<u8> = s
        .bytes()
        .map(|b| {
            if b == b'/' {
                b'\\'
            } else {
                b.to_ascii_uppercase()
            }
        })
        .collect();

    let orig_len = normalised.len() as u32;

    let init = 0xDEAD_BEEFu32.wrapping_add(orig_len);
    let mut a = init;
    let mut b = init;
    let mut c = init;

    if orig_len == 0 {
        return ((c as u64) << 32) | (b as u64);
    }

    // Pad to a multiple of 12 bytes with zeros (matches C# Array.Resize).
    let padded_len = {
        let rem = normalised.len() % 12;
        if rem == 0 {
            normalised.len()
        } else {
            normalised.len() + (12 - rem)
        }
    };

    let mut data = normalised;
    data.resize(padded_len, 0);

    let num_chunks = padded_len / 12;

    // Process all but the last chunk with the mix step.
    for chunk in 0..num_chunks.saturating_sub(1) {
        let off = chunk * 12;
        a = a.wrapping_add(u32_le(&data, off));
        b = b.wrapping_add(u32_le(&data, off + 4));
        c = c.wrapping_add(u32_le(&data, off + 8));
        mix(&mut a, &mut b, &mut c);
    }

    // Final chunk.
    let off = (num_chunks - 1) * 12;
    a = a.wrapping_add(u32_le(&data, off));
    b = b.wrapping_add(u32_le(&data, off + 4));
    c = c.wrapping_add(u32_le(&data, off + 8));
    final_mix(&mut a, &mut b, &mut c);

    ((c as u64) << 32) | (b as u64)
}

/// Compute the FNV-1a based FileDataId hash used for WoW files that have no
/// name in the listfile.
///
/// This allows opening files by numeric `FileDataId` even when the filename is
/// unknown.
pub fn file_data_id_hash(id: u32) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for i in 0..4u32 {
        let byte = (id >> (8 * i)) & 0xFF;
        h = 0x0000_0001_0000_01B3u64.wrapping_mul(byte as u64 ^ h);
    }
    h
}

// ── Internals ─────────────────────────────────────────────────────────────────

#[inline]
fn u32_le(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

#[inline]
fn mix(a: &mut u32, b: &mut u32, c: &mut u32) {
    *a = a.wrapping_sub(*c);
    *a ^= c.rotate_left(4);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a);
    *b ^= a.rotate_left(6);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b);
    *c ^= b.rotate_left(8);
    *b = b.wrapping_add(*a);
    *a = a.wrapping_sub(*c);
    *a ^= c.rotate_left(16);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a);
    *b ^= a.rotate_left(19);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b);
    *c ^= b.rotate_left(4);
    *b = b.wrapping_add(*a);
}

#[inline]
fn final_mix(a: &mut u32, b: &mut u32, c: &mut u32) {
    *c ^= *b;
    *c = c.wrapping_sub(b.rotate_left(14));
    *a ^= *c;
    *a = a.wrapping_sub(c.rotate_left(11));
    *b ^= *a;
    *b = b.wrapping_sub(a.rotate_left(25));
    *c ^= *b;
    *c = c.wrapping_sub(b.rotate_left(16));
    *a ^= *c;
    *a = a.wrapping_sub(c.rotate_left(4));
    *b ^= *a;
    *b = b.wrapping_sub(a.rotate_left(14));
    *c ^= *b;
    *c = c.wrapping_sub(b.rotate_left(24));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_does_not_panic() {
        let _ = jenkins96("");
    }

    #[test]
    fn slash_normalisation() {
        // Forward and back slashes should produce the same hash.
        assert_eq!(
            jenkins96("interface/glues/models/file.m2"),
            jenkins96("INTERFACE\\GLUES\\MODELS\\FILE.M2"),
        );
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(jenkins96("creature/test.m2"), jenkins96("CREATURE/TEST.M2"));
    }

    #[test]
    fn file_data_id_nonzero() {
        assert_ne!(file_data_id_hash(1), 0);
    }

    #[test]
    fn deterministic() {
        let h1 = jenkins96("world/maps/azeroth/azeroth.wdt");
        let h2 = jenkins96("world/maps/azeroth/azeroth.wdt");
        assert_eq!(h1, h2);
    }
}
