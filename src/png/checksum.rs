//! Checksums used by the PNG format: CRC-32 (chunks) and Adler-32 (zlib stream).
//!
//! Both are implemented from scratch so the project has no runtime dependencies.

/// Lookup table for the CRC-32 polynomial `0xEDB88320` (the reflected form of
/// the IEEE 802.3 polynomial). It is built at compile time.
const CRC_TABLE: [u32; 256] = build_crc_table();

/// Builds the 256-entry CRC-32 table. Each entry is the CRC of one byte value
/// processed bit by bit.
const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            // If the lowest bit is set we xor with the polynomial after the shift.
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

/// Calculates the CRC-32 of `data` as defined by the PNG specification.
///
/// The PNG CRC covers the chunk type and the chunk data (but not the length).
/// Use [`crc32_update`] when the input comes in several parts.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

/// Continues a CRC-32 calculation. `crc` is the value returned for the
/// previous part (use `0` to start). The result for the concatenated parts is
/// the same as the result of [`crc32`] for the whole data.
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    // The register is kept inverted between calls, as the standard requires.
    let mut c = !crc;
    for &byte in data {
        c = CRC_TABLE[((c ^ u32::from(byte)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// Calculates the Adler-32 checksum of `data` (the checksum at the end of a
/// zlib stream, see RFC 1950).
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    // 5552 is the largest block length for which the sums cannot overflow
    // a `u32` before the modulo operation.
    const BLOCK: usize = 5552;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for block in data.chunks(BLOCK) {
        for &byte in block {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_of_empty_input_is_zero() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn crc32_matches_known_vectors() {
        // Well known check value of the CRC-32/ISO-HDLC algorithm.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        // CRC of the IEND chunk type, which every PNG file ends with.
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn crc32_update_equals_one_shot() {
        let data = b"The quick brown fox jumps over the lazy dog";
        let (left, right) = data.split_at(10);
        assert_eq!(crc32_update(crc32(left), right), crc32(data));
    }

    #[test]
    fn adler32_matches_known_vectors() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn adler32_handles_long_input_without_overflow() {
        // 100 000 bytes of 0xFF would overflow a naive implementation.
        let data = vec![0xFFu8; 100_000];
        let mut a: u64 = 1;
        let mut b: u64 = 0;
        for &x in &data {
            a = (a + u64::from(x)) % 65_521;
            b = (b + a) % 65_521;
        }
        assert_eq!(u64::from(adler32(&data)), (b << 16) | a);
    }
}
