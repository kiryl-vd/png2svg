//! A small, safe implementation of zlib (RFC 1950) and DEFLATE (RFC 1951)
//! decompression.
//!
//! The decoder never reads or writes outside of its buffers and never panics on
//! malformed input. The caller gives an upper limit for the output size, so a
//! "zip bomb" cannot make the program allocate an unlimited amount of memory.

use std::fmt;

use super::checksum::adler32;

/// Reasons why a zlib stream cannot be decompressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflateError {
    /// The input ended before the stream was complete.
    UnexpectedEnd,
    /// The stream breaks a rule of the format. The text names the rule.
    Invalid(&'static str),
    /// The stream would produce more bytes than the caller allows.
    OutputTooLarge,
    /// The Adler-32 checksum at the end of the stream does not match the data.
    ChecksumMismatch,
}

impl fmt::Display for InflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InflateError::UnexpectedEnd => write!(f, "compressed data ends too early"),
            InflateError::Invalid(why) => write!(f, "invalid compressed data ({why})"),
            InflateError::OutputTooLarge => {
                write!(f, "compressed data is larger than the image header allows")
            }
            InflateError::ChecksumMismatch => write!(f, "zlib checksum (Adler-32) mismatch"),
        }
    }
}

impl std::error::Error for InflateError {}

/// Decompresses a complete zlib stream (header, DEFLATE data, Adler-32).
///
/// `max_out` is the largest output size that is accepted. The function fails
/// with [`InflateError::OutputTooLarge`] as soon as the output would exceed it.
pub fn zlib_decompress(data: &[u8], max_out: usize) -> Result<Vec<u8>, InflateError> {
    if data.len() < 2 {
        return Err(InflateError::UnexpectedEnd);
    }
    let cmf = data[0];
    let flg = data[1];
    // Compression method 8 is DEFLATE. No other method exists in PNG.
    if cmf & 0x0F != 8 {
        return Err(InflateError::Invalid("unknown compression method"));
    }
    // CINFO is the base-2 logarithm of the window size minus 8 (maximum 7).
    if cmf >> 4 > 7 {
        return Err(InflateError::Invalid("window size is too large"));
    }
    // The 16-bit header value must be a multiple of 31 (FCHECK field).
    if (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
        return Err(InflateError::Invalid("zlib header check failed"));
    }
    // PNG forbids preset dictionaries.
    if flg & 0x20 != 0 {
        return Err(InflateError::Invalid("preset dictionary is not allowed"));
    }

    let (out, used) = inflate_raw(&data[2..], max_out)?;
    let tail = &data[2 + used..];
    if tail.len() < 4 {
        return Err(InflateError::UnexpectedEnd);
    }
    let expected = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
    if adler32(&out) != expected {
        return Err(InflateError::ChecksumMismatch);
    }
    Ok(out)
}

/// Decompresses raw DEFLATE data. Returns the output and the number of input
/// bytes that the DEFLATE stream used (the zlib checksum follows after them).
pub fn inflate_raw(data: &[u8], max_out: usize) -> Result<(Vec<u8>, usize), InflateError> {
    let mut reader = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();

    loop {
        let last = reader.bits(1)? == 1;
        match reader.bits(2)? {
            0 => stored_block(&mut reader, &mut out, max_out)?,
            1 => {
                let (lit, dist) = fixed_tables();
                compressed_block(&mut reader, &mut out, max_out, &lit, &dist)?;
            }
            2 => {
                let (lit, dist) = read_dynamic_tables(&mut reader)?;
                compressed_block(&mut reader, &mut out, max_out, &lit, &dist)?;
            }
            _ => return Err(InflateError::Invalid("reserved block type")),
        }
        if last {
            break;
        }
    }
    Ok((out, reader.bytes_used()))
}

/// Reads bits from a byte slice, least significant bit first, as DEFLATE needs.
struct BitReader<'a> {
    data: &'a [u8],
    /// Index of the next byte that is not yet loaded into `buf`.
    pos: usize,
    /// Bit buffer. The next bit to read is the lowest bit.
    buf: u64,
    /// Number of valid bits in `buf`.
    count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            buf: 0,
            count: 0,
        }
    }

    /// Loads whole bytes into the bit buffer while there is room for them.
    fn refill(&mut self) {
        while self.count <= 56 && self.pos < self.data.len() {
            self.buf |= u64::from(self.data[self.pos]) << self.count;
            self.pos += 1;
            self.count += 8;
        }
    }

    /// Reads `n` bits (0..=32) as an unsigned number.
    fn bits(&mut self, n: u32) -> Result<u32, InflateError> {
        if n == 0 {
            return Ok(0);
        }
        if self.count < n {
            self.refill();
            if self.count < n {
                return Err(InflateError::UnexpectedEnd);
            }
        }
        let value = (self.buf & ((1u64 << n) - 1)) as u32;
        self.buf >>= n;
        self.count -= n;
        Ok(value)
    }

    /// Number of input bytes that were fully or partly consumed so far.
    /// Bytes that sit unused in the bit buffer are not counted.
    fn bytes_used(&self) -> usize {
        self.pos - (self.count / 8) as usize
    }

    /// Drops the bits up to the next byte boundary.
    fn align_to_byte(&mut self) {
        let drop = self.count % 8;
        self.buf >>= drop;
        self.count -= drop;
    }
}

/// Handles a "stored" (uncompressed) block.
fn stored_block(
    reader: &mut BitReader<'_>,
    out: &mut Vec<u8>,
    max_out: usize,
) -> Result<(), InflateError> {
    reader.align_to_byte();
    let len = reader.bits(16)? as usize;
    let nlen = reader.bits(16)? as usize;
    if len != (!nlen & 0xFFFF) {
        return Err(InflateError::Invalid("stored block length check failed"));
    }
    if out.len() + len > max_out {
        return Err(InflateError::OutputTooLarge);
    }
    // After the alignment, the bit buffer holds only whole bytes.
    let mut remaining = len;
    while remaining > 0 && reader.count >= 8 {
        out.push(reader.bits(8)? as u8);
        remaining -= 1;
    }
    if remaining > 0 {
        // The bit buffer is empty now, so we can copy directly from the input.
        let end = reader.pos + remaining;
        if end > reader.data.len() {
            return Err(InflateError::UnexpectedEnd);
        }
        out.extend_from_slice(&reader.data[reader.pos..end]);
        reader.pos = end;
    }
    Ok(())
}

/// Longest Huffman code length in DEFLATE.
const MAX_CODE_BITS: usize = 15;

/// A canonical Huffman decoding table.
///
/// The table is indexed by the next `max_len` bits of the stream (as they come,
/// least significant bit first). Each entry stores `symbol << 4 | code_length`.
/// A length of zero marks a bit pattern that is not a valid code.
struct Huffman {
    table: Vec<u16>,
    max_len: u32,
}

impl Huffman {
    /// Builds the table from the code length of every symbol (0 = unused).
    fn new(lengths: &[u8]) -> Result<Huffman, InflateError> {
        let mut count = [0u16; MAX_CODE_BITS + 1];
        for &len in lengths {
            count[len as usize] += 1;
        }
        count[0] = 0;

        // The longest code length that is used.
        let max_len = (1..=MAX_CODE_BITS)
            .rev()
            .find(|&len| count[len] != 0)
            .unwrap_or(0);
        if max_len == 0 {
            // No codes at all. This is legal for the distance tree of a block
            // that has only literals. Any decode attempt fails.
            return Ok(Huffman {
                table: vec![0; 2],
                max_len: 1,
            });
        }

        // Check that the code lengths do not describe too many codes.
        let mut left: i32 = 1;
        for &c in count.iter().skip(1) {
            left <<= 1;
            left -= i32::from(c);
            if left < 0 {
                return Err(InflateError::Invalid("over-subscribed Huffman code"));
            }
        }
        // An incomplete code is only allowed for a single code of length 1.
        if left > 0 && max_len != 1 {
            return Err(InflateError::Invalid("incomplete Huffman code"));
        }

        // First canonical code of every length.
        let mut next_code = [0u32; MAX_CODE_BITS + 2];
        let mut code = 0u32;
        for len in 1..=MAX_CODE_BITS {
            code = (code + u32::from(count[len - 1])) << 1;
            next_code[len] = code;
        }

        let size = 1usize << max_len;
        let mut table = vec![0u16; size];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let len = len as usize;
            let code = next_code[len];
            next_code[len] += 1;
            // DEFLATE sends Huffman codes starting with the most significant
            // bit, but the bit reader returns the first bit as the lowest bit.
            // Therefore the code is reversed before it is used as a table index.
            let reversed = reverse_bits(code, len as u32) as usize;
            let entry = ((symbol as u16) << 4) | len as u16;
            let mut index = reversed;
            while index < size {
                table[index] = entry;
                index += 1 << len;
            }
        }
        Ok(Huffman {
            table,
            max_len: max_len as u32,
        })
    }

    /// Decodes one symbol from the reader.
    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, InflateError> {
        if reader.count < self.max_len {
            reader.refill();
        }
        let index = (reader.buf & ((1u64 << self.max_len) - 1)) as usize;
        let entry = self.table[index];
        let len = u32::from(entry & 0x0F);
        if len == 0 {
            // Either an invalid code or not enough input bits left.
            return if reader.count < self.max_len {
                Err(InflateError::UnexpectedEnd)
            } else {
                Err(InflateError::Invalid("invalid Huffman code"))
            };
        }
        if len > reader.count {
            return Err(InflateError::UnexpectedEnd);
        }
        reader.buf >>= len;
        reader.count -= len;
        Ok(entry >> 4)
    }
}

/// Reverses the lowest `len` bits of `value`.
fn reverse_bits(value: u32, len: u32) -> u32 {
    let mut result = 0;
    let mut v = value;
    for _ in 0..len {
        result = (result << 1) | (v & 1);
        v >>= 1;
    }
    result
}

/// Base lengths for length symbols 257..=285.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
/// Extra bits for length symbols 257..=285.
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Base distances for distance symbols 0..=29.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
/// Extra bits for distance symbols 0..=29.
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Builds the fixed Huffman tables of block type 1.
fn fixed_tables() -> (Huffman, Huffman) {
    let mut lengths = [0u8; 288];
    for (symbol, len) in lengths.iter_mut().enumerate() {
        *len = match symbol {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    // These fixed length lists are complete codes, so building cannot fail.
    let lit = Huffman::new(&lengths).unwrap_or(Huffman {
        table: vec![0; 2],
        max_len: 1,
    });
    // The fixed distance code has 32 symbols of 5 bits. Symbols 30 and 31 never
    // appear in valid data, and the decoder rejects them later.
    let dist = Huffman::new(&[5u8; 32]).unwrap_or(Huffman {
        table: vec![0; 2],
        max_len: 1,
    });
    (lit, dist)
}

/// Reads the code length tables of a dynamic block (block type 2).
fn read_dynamic_tables(reader: &mut BitReader<'_>) -> Result<(Huffman, Huffman), InflateError> {
    let hlit = reader.bits(5)? as usize + 257;
    let hdist = reader.bits(5)? as usize + 1;
    let hclen = reader.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(InflateError::Invalid("too many length or distance codes"));
    }

    // The code length codes come in a fixed, strange order.
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut cl_lengths = [0u8; 19];
    for &slot in ORDER.iter().take(hclen) {
        cl_lengths[slot] = reader.bits(3)? as u8;
    }
    let cl_table = Huffman::new(&cl_lengths)?;

    // Literal/length and distance code lengths are stored as one sequence.
    let total = hlit + hdist;
    let mut lengths = vec![0u8; total];
    let mut i = 0;
    while i < total {
        let symbol = cl_table.decode(reader)?;
        match symbol {
            0..=15 => {
                lengths[i] = symbol as u8;
                i += 1;
            }
            16 => {
                // Repeat the previous length 3..6 times.
                if i == 0 {
                    return Err(InflateError::Invalid("repeat code without previous length"));
                }
                let prev = lengths[i - 1];
                let repeat = 3 + reader.bits(2)? as usize;
                if i + repeat > total {
                    return Err(InflateError::Invalid("code length repeat is too long"));
                }
                for slot in &mut lengths[i..i + repeat] {
                    *slot = prev;
                }
                i += repeat;
            }
            17 | 18 => {
                // Repeat a zero length 3..10 or 11..138 times.
                let repeat = if symbol == 17 {
                    3 + reader.bits(3)? as usize
                } else {
                    11 + reader.bits(7)? as usize
                };
                if i + repeat > total {
                    return Err(InflateError::Invalid("code length repeat is too long"));
                }
                i += repeat; // The slots are already zero.
            }
            _ => return Err(InflateError::Invalid("bad code length symbol")),
        }
    }
    if lengths[256] == 0 {
        return Err(InflateError::Invalid("block has no end-of-block code"));
    }
    let lit = Huffman::new(&lengths[..hlit])?;
    let dist = Huffman::new(&lengths[hlit..])?;
    Ok((lit, dist))
}

/// Decodes the symbols of one compressed block.
fn compressed_block(
    reader: &mut BitReader<'_>,
    out: &mut Vec<u8>,
    max_out: usize,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<(), InflateError> {
    loop {
        let symbol = lit.decode(reader)? as usize;
        if symbol < 256 {
            if out.len() >= max_out {
                return Err(InflateError::OutputTooLarge);
            }
            out.push(symbol as u8);
        } else if symbol == 256 {
            return Ok(());
        } else {
            let index = symbol - 257;
            if index >= LENGTH_BASE.len() {
                return Err(InflateError::Invalid("bad length symbol"));
            }
            let length = usize::from(LENGTH_BASE[index])
                + reader.bits(u32::from(LENGTH_EXTRA[index]))? as usize;

            let dist_symbol = dist.decode(reader)? as usize;
            if dist_symbol >= DIST_BASE.len() {
                return Err(InflateError::Invalid("bad distance symbol"));
            }
            let distance = usize::from(DIST_BASE[dist_symbol])
                + reader.bits(u32::from(DIST_EXTRA[dist_symbol]))? as usize;
            if distance > out.len() {
                return Err(InflateError::Invalid(
                    "distance reaches before the data start",
                ));
            }
            if out.len() + length > max_out {
                return Err(InflateError::OutputTooLarge);
            }
            let start = out.len() - distance;
            if distance >= length {
                out.extend_from_within(start..start + length);
            } else {
                // The source and the destination overlap, so the bytes must be
                // copied one by one (this repeats a short pattern).
                for k in 0..length {
                    let byte = out[start + k];
                    out.push(byte);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a zlib stream that only uses stored blocks.
    fn stored_zlib(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let mut chunks: Vec<&[u8]> = payload.chunks(65_535).collect();
        if chunks.is_empty() {
            chunks.push(&[]);
        }
        let count = chunks.len();
        for (i, chunk) in chunks.into_iter().enumerate() {
            out.push(u8::from(i + 1 == count));
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
        out.extend_from_slice(&adler32(payload).to_be_bytes());
        out
    }

    #[test]
    fn decodes_stored_blocks() {
        let payload: Vec<u8> = (0..200_000u32).map(|v| (v % 251) as u8).collect();
        let stream = stored_zlib(&payload);
        assert_eq!(zlib_decompress(&stream, payload.len()).unwrap(), payload);
    }

    #[test]
    fn decodes_fixed_huffman_block() {
        // "hello hello hello hello" compressed by zlib (fixed Huffman codes).
        let stream = [
            120u8, 218, 203, 72, 205, 201, 201, 87, 200, 64, 39, 1, 104, 3, 8, 177,
        ];
        let out = zlib_decompress(&stream, 100).unwrap();
        assert_eq!(out, b"hello hello hello hello");
    }

    #[test]
    fn decodes_dynamic_huffman_block() {
        // 1500 bytes of a repeating pattern compressed by zlib at level 9.
        // The first block of this stream is a dynamic Huffman block.
        const STREAM: &[u8] = &[
            0x78, 0xDA, 0xED, 0xCE, 0xC9, 0x01, 0x03, 0x21, 0x08, 0x00, 0xC0, 0x5A, 0x11, 0x51,
            0x14, 0x2F, 0x50, 0xD6, 0xF6, 0x53, 0x43, 0xFE, 0x4E, 0x05, 0x03, 0x81, 0xAA, 0xE2,
            0xA0, 0x33, 0x2A, 0x73, 0x9D, 0x9E, 0x16, 0xED, 0xC6, 0x14, 0x89, 0xFB, 0xC9, 0xC6,
            0x57, 0xBB, 0xB4, 0x61, 0x50, 0x36, 0xDF, 0x25, 0x9C, 0x59, 0x14, 0xAA, 0x0B, 0x1E,
            0x9D, 0x53, 0x3D, 0xB6, 0xDB, 0x70, 0x0F, 0xA9, 0x6D, 0x9E, 0xD8, 0x61, 0x64, 0x38,
            0x66, 0x07, 0x78, 0xE1, 0x4C, 0x9F, 0xCE, 0x31, 0xED, 0xE6, 0x45, 0x56, 0xE3, 0x75,
            0x07, 0x92, 0x9D, 0xAC, 0xA0, 0x9B, 0x9A, 0xA3, 0x1C, 0xF6, 0x9E, 0x31, 0x84, 0xC8,
            0xE3, 0x2B, 0x5F, 0x4F, 0xE0, 0xC7, 0x43, 0x1E, 0x57, 0x60, 0x49, 0x26, 0xCA, 0xA2,
            0xD8, 0xC3, 0x2A, 0x11, 0xE0, 0x05, 0x5F, 0xF0, 0x05, 0x5F, 0xF0, 0x05, 0xFF, 0x0D,
            0xFE, 0x00, 0xD7, 0x8B, 0x77, 0x21,
        ];
        assert_eq!(
            (STREAM[2] >> 1) & 3,
            2,
            "test vector must be a dynamic block"
        );
        let expected: Vec<u8> = (0..1500u32)
            .map(|i| ((i * i + i / 7) % 23) as u8 + 97)
            .collect();
        assert_eq!(zlib_decompress(STREAM, 1500).unwrap(), expected);
        // The same stream fails cleanly when the output limit is too small.
        assert_eq!(
            zlib_decompress(STREAM, 1499),
            Err(InflateError::OutputTooLarge)
        );
    }

    #[test]
    fn rejects_bad_header() {
        assert!(matches!(
            zlib_decompress(&[0x79, 0x01, 0, 0, 0, 0], 10),
            Err(InflateError::Invalid(_))
        ));
        assert!(matches!(
            zlib_decompress(&[0x78], 10),
            Err(InflateError::UnexpectedEnd)
        ));
    }

    #[test]
    fn rejects_wrong_adler() {
        let mut stream = stored_zlib(b"abc");
        let last = stream.len() - 1;
        stream[last] ^= 0xFF;
        assert_eq!(
            zlib_decompress(&stream, 10),
            Err(InflateError::ChecksumMismatch)
        );
    }

    #[test]
    fn respects_output_limit() {
        let stream = stored_zlib(&[7u8; 100]);
        assert_eq!(
            zlib_decompress(&stream, 99),
            Err(InflateError::OutputTooLarge)
        );
        assert!(zlib_decompress(&stream, 100).is_ok());
    }

    #[test]
    fn rejects_truncated_and_garbage_input_without_panic() {
        let stream = stored_zlib(&[1, 2, 3, 4, 5]);
        for cut in 0..stream.len() {
            assert!(zlib_decompress(&stream[..cut], 100).is_err());
        }
        // Pseudo random garbage must never panic.
        let mut seed = 12345u32;
        for _ in 0..500 {
            let mut data = vec![0x78, 0x9C];
            for _ in 0..64 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                data.push((seed >> 24) as u8);
            }
            let _ = zlib_decompress(&data, 1 << 16);
        }
    }

    #[test]
    fn overlapping_copy_repeats_pattern() {
        // Fixed block: literal 'a', then a match of length 5 at distance 1.
        // Built by hand with the fixed Huffman code.
        // bits: BFINAL=1, BTYPE=01, 'a' (0x61 -> 8 bit code 0x30+0x61),
        // length 5 (symbol 259 -> 7 bit code 0b0000011), distance 1 (code 0).
        let mut bits: Vec<u8> = Vec::new();
        let mut push = |value: u32, len: u32, msb_first: bool| {
            for i in 0..len {
                let bit = if msb_first {
                    (value >> (len - 1 - i)) & 1
                } else {
                    (value >> i) & 1
                };
                bits.push(bit as u8);
            }
        };
        push(1, 1, false); // BFINAL
        push(1, 2, false); // BTYPE = 01 (fixed)
        push(0x30 + 0x61, 8, true); // literal 'a'
        push(0b0000011, 7, true); // length symbol 259 = length 5
        push(0, 5, true); // distance symbol 0 = distance 1
        push(0, 7, true); // end of block (symbol 256)
        let mut bytes = vec![0u8; bits.len().div_ceil(8)];
        for (i, bit) in bits.iter().enumerate() {
            bytes[i / 8] |= bit << (i % 8);
        }
        let (out, _) = inflate_raw(&bytes, 100).unwrap();
        assert_eq!(out, b"aaaaaa");
    }
}
