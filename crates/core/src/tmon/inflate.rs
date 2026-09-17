//! Bounded, dependency-free zlib and DEFLATE decoding.
//!
//! The decoder intentionally implements only the zlib container used by PNG
//! and the kitty graphics protocol. Preset dictionaries and concatenated zlib
//! streams are rejected.

use std::sync::OnceLock;

const MAX_CODE_BITS: usize = 15;
// A valid 64 MiB image stream normally needs orders of magnitude fewer blocks.
// Keep adversarial zero-output streams from spending effectively unbounded CPU
// while retaining a generous ceiling for unusual encoders.
const MAX_DEFLATE_BLOCKS: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputLimit {
    Exact(usize),
    AtMost(usize),
}

impl OutputLimit {
    fn maximum(self) -> usize {
        match self {
            Self::Exact(size) | Self::AtMost(size) => size,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InflateError {
    InvalidData,
    OutputTooLarge,
    WorkLimitExceeded,
    OutOfMemory,
}

pub(crate) fn decompress_zlib(data: &[u8], limit: OutputLimit) -> Result<Vec<u8>, InflateError> {
    if data.len() < 6 {
        return Err(InflateError::InvalidData);
    }

    let compression_method = data[0] & 0x0f;
    let window_bits = data[0] >> 4;
    let header = u16::from_be_bytes([data[0], data[1]]);
    if compression_method != 8
        || window_bits > 7
        || !header.is_multiple_of(31)
        || data[1] & 0x20 != 0
    {
        return Err(InflateError::InvalidData);
    }

    let trailer = data.len() - 4;
    let expected_checksum = u32::from_be_bytes(
        data[trailer..]
            .try_into()
            .map_err(|_| InflateError::InvalidData)?,
    );
    let window_size = 1usize << (usize::from(window_bits) + 8);
    let output = decompress_deflate(&data[2..trailer], limit, window_size)?;
    if adler32(&output) != expected_checksum {
        return Err(InflateError::InvalidData);
    }
    Ok(output)
}

fn decompress_deflate(
    data: &[u8],
    limit: OutputLimit,
    window_size: usize,
) -> Result<Vec<u8>, InflateError> {
    let mut reader = BitReader::new(data);
    let mut output = BoundedOutput::new(limit.maximum())?;
    let mut block_count = 0usize;
    loop {
        block_count += 1;
        if block_count > MAX_DEFLATE_BLOCKS {
            return Err(InflateError::WorkLimitExceeded);
        }
        let final_block = reader.read_bits(1)? != 0;
        match reader.read_bits(2)? {
            0 => decode_stored_block(&mut reader, &mut output)?,
            1 => {
                let (literals, distances) = fixed_trees()?;
                decode_compressed_block(
                    &mut reader,
                    &mut output,
                    literals,
                    distances,
                    window_size,
                )?;
            }
            2 => {
                let (literals, distances) = dynamic_trees(&mut reader)?;
                decode_compressed_block(
                    &mut reader,
                    &mut output,
                    &literals,
                    &distances,
                    window_size,
                )?;
            }
            _ => return Err(InflateError::InvalidData),
        }
        if final_block {
            break;
        }
    }
    reader.finish()?;
    output.finish(limit)
}

fn decode_stored_block(
    reader: &mut BitReader<'_>,
    output: &mut BoundedOutput,
) -> Result<(), InflateError> {
    reader.align_to_byte();
    let length = usize::from(reader.read_aligned_u16()?);
    let complement = reader.read_aligned_u16()?;
    if (length as u16) != !complement {
        return Err(InflateError::InvalidData);
    }
    output.extend_from_slice(reader.read_aligned_bytes(length)?)
}

fn decode_compressed_block(
    reader: &mut BitReader<'_>,
    output: &mut BoundedOutput,
    literals: &Huffman,
    distances: &Huffman,
    window_size: usize,
) -> Result<(), InflateError> {
    loop {
        match literals.decode(reader)? {
            literal @ 0..=255 => output.push(literal as u8)?,
            256 => return Ok(()),
            length_symbol @ 257..=285 => {
                let length_index = usize::from(length_symbol - 257);
                let length = usize::from(LENGTH_BASE[length_index])
                    + usize::try_from(reader.read_bits(LENGTH_EXTRA[length_index])?)
                        .map_err(|_| InflateError::InvalidData)?;
                let distance_symbol = usize::from(distances.decode(reader)?);
                if distance_symbol >= DISTANCE_BASE.len() {
                    return Err(InflateError::InvalidData);
                }
                let distance = usize::from(DISTANCE_BASE[distance_symbol])
                    + usize::try_from(reader.read_bits(DISTANCE_EXTRA[distance_symbol])?)
                        .map_err(|_| InflateError::InvalidData)?;
                if distance > window_size {
                    return Err(InflateError::InvalidData);
                }
                output.copy_match(distance, length)?;
            }
            _ => return Err(InflateError::InvalidData),
        }
    }
}

fn dynamic_trees(reader: &mut BitReader<'_>) -> Result<(Huffman, Huffman), InflateError> {
    const CODE_LENGTH_ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];

    let literal_count =
        usize::try_from(reader.read_bits(5)?).map_err(|_| InflateError::InvalidData)? + 257;
    // RFC 1951 limits HLIT to 257..=286 literal/length codes. The two larger
    // values represent reserved symbols 286 and 287 and are invalid even when
    // their eventual code lengths would be zero.
    if literal_count > 286 {
        return Err(InflateError::InvalidData);
    }
    let distance_count =
        usize::try_from(reader.read_bits(5)?).map_err(|_| InflateError::InvalidData)? + 1;
    let code_length_count =
        usize::try_from(reader.read_bits(4)?).map_err(|_| InflateError::InvalidData)? + 4;

    let mut code_lengths = [0u8; 19];
    for symbol in CODE_LENGTH_ORDER.iter().take(code_length_count) {
        code_lengths[*symbol] = reader.read_bits(3)? as u8;
    }
    let code_length_tree = Huffman::new(&code_lengths, 7, TreeKind::CodeLengths)?;

    let total = literal_count
        .checked_add(distance_count)
        .ok_or(InflateError::InvalidData)?;
    let mut lengths = Vec::new();
    lengths
        .try_reserve_exact(total)
        .map_err(|_| InflateError::OutOfMemory)?;
    while lengths.len() < total {
        match code_length_tree.decode(reader)? {
            value @ 0..=15 => lengths.push(value as u8),
            16 => {
                let previous = *lengths.last().ok_or(InflateError::InvalidData)?;
                let count = usize::try_from(reader.read_bits(2)?)
                    .map_err(|_| InflateError::InvalidData)?
                    + 3;
                append_repeated(&mut lengths, previous, count, total)?;
            }
            17 => {
                let count = usize::try_from(reader.read_bits(3)?)
                    .map_err(|_| InflateError::InvalidData)?
                    + 3;
                append_repeated(&mut lengths, 0, count, total)?;
            }
            18 => {
                let count = usize::try_from(reader.read_bits(7)?)
                    .map_err(|_| InflateError::InvalidData)?
                    + 11;
                append_repeated(&mut lengths, 0, count, total)?;
            }
            _ => return Err(InflateError::InvalidData),
        }
    }

    let (literal_lengths, distance_lengths) = lengths.split_at(literal_count);
    if literal_lengths.get(256).copied().unwrap_or(0) == 0 {
        return Err(InflateError::InvalidData);
    }
    Ok((
        Huffman::new(literal_lengths, MAX_CODE_BITS, TreeKind::Literals)?,
        Huffman::new(distance_lengths, MAX_CODE_BITS, TreeKind::Distances)?,
    ))
}

fn append_repeated(
    output: &mut Vec<u8>,
    value: u8,
    count: usize,
    maximum: usize,
) -> Result<(), InflateError> {
    let end = output
        .len()
        .checked_add(count)
        .filter(|end| *end <= maximum)
        .ok_or(InflateError::InvalidData)?;
    output.resize(end, value);
    Ok(())
}

fn fixed_code_lengths() -> ([u8; 288], [u8; 32]) {
    let mut literals = [0u8; 288];
    literals[..144].fill(8);
    literals[144..256].fill(9);
    literals[256..280].fill(7);
    literals[280..].fill(8);
    (literals, [5; 32])
}

fn fixed_trees() -> Result<(&'static Huffman, &'static Huffman), InflateError> {
    static TREES: OnceLock<Result<(Huffman, Huffman), InflateError>> = OnceLock::new();
    match TREES.get_or_init(|| {
        let (literal_lengths, distance_lengths) = fixed_code_lengths();
        Ok((
            Huffman::new(&literal_lengths, MAX_CODE_BITS, TreeKind::Literals)?,
            Huffman::new(&distance_lengths, MAX_CODE_BITS, TreeKind::Distances)?,
        ))
    }) {
        Ok((literals, distances)) => Ok((literals, distances)),
        Err(error) => Err(*error),
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12_289, 16_385, 24_577,
];
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

#[derive(Clone, Copy, Debug, Default)]
enum Branch {
    #[default]
    Empty,
    Node(usize),
    Symbol(u16),
}

#[derive(Clone, Copy, Debug, Default)]
struct HuffmanNode {
    branches: [Branch; 2],
}

#[derive(Debug)]
struct Huffman {
    nodes: Vec<HuffmanNode>,
    maximum_length: usize,
    symbol_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TreeKind {
    CodeLengths,
    Literals,
    Distances,
}

impl Huffman {
    fn new(lengths: &[u8], maximum_bits: usize, kind: TreeKind) -> Result<Self, InflateError> {
        if maximum_bits == 0 || maximum_bits > MAX_CODE_BITS {
            return Err(InflateError::InvalidData);
        }
        let mut counts = [0u16; MAX_CODE_BITS + 1];
        let mut maximum_length = 0usize;
        let mut symbol_count = 0usize;
        for length in lengths {
            let length = usize::from(*length);
            if length > maximum_bits {
                return Err(InflateError::InvalidData);
            }
            if length != 0 {
                counts[length] = counts[length]
                    .checked_add(1)
                    .ok_or(InflateError::InvalidData)?;
                maximum_length = maximum_length.max(length);
                symbol_count += 1;
            }
        }

        let mut remaining = 1i32;
        for count in counts.iter().take(maximum_bits + 1).skip(1) {
            remaining = (remaining << 1) - i32::from(*count);
            if remaining < 0 {
                return Err(InflateError::InvalidData);
            }
        }
        if symbol_count == 0 {
            if kind != TreeKind::Distances {
                return Err(InflateError::InvalidData);
            }
        } else if remaining > 0 && (kind == TreeKind::CodeLengths || maximum_length != 1) {
            return Err(InflateError::InvalidData);
        }

        let mut nodes = Vec::new();
        nodes
            .try_reserve(
                symbol_count
                    .saturating_mul(maximum_length)
                    .saturating_add(1),
            )
            .map_err(|_| InflateError::OutOfMemory)?;
        nodes.push(HuffmanNode::default());
        if symbol_count == 0 {
            return Ok(Self {
                nodes,
                maximum_length,
                symbol_count,
            });
        }

        let mut next_code = [0u16; MAX_CODE_BITS + 1];
        let mut code = 0u16;
        for bits in 1..=maximum_bits {
            code = code
                .checked_add(counts[bits - 1])
                .and_then(|value| value.checked_mul(2))
                .ok_or(InflateError::InvalidData)?;
            next_code[bits] = code;
        }

        for (symbol, length) in lengths.iter().copied().enumerate() {
            let length = usize::from(length);
            if length == 0 {
                continue;
            }
            let symbol = u16::try_from(symbol).map_err(|_| InflateError::InvalidData)?;
            let code = next_code[length];
            next_code[length] = next_code[length]
                .checked_add(1)
                .ok_or(InflateError::InvalidData)?;
            insert_code(&mut nodes, code, length, symbol)?;
        }

        Ok(Self {
            nodes,
            maximum_length,
            symbol_count,
        })
    }

    fn is_empty(&self) -> bool {
        self.symbol_count == 0
    }

    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, InflateError> {
        if self.is_empty() {
            return Err(InflateError::InvalidData);
        }
        let mut node = 0usize;
        for _ in 0..self.maximum_length {
            let bit =
                usize::try_from(reader.read_bits(1)?).map_err(|_| InflateError::InvalidData)?;
            match self.nodes[node].branches[bit] {
                Branch::Empty => return Err(InflateError::InvalidData),
                Branch::Node(next) => node = next,
                Branch::Symbol(symbol) => return Ok(symbol),
            }
        }
        Err(InflateError::InvalidData)
    }
}

fn insert_code(
    nodes: &mut Vec<HuffmanNode>,
    code: u16,
    length: usize,
    symbol: u16,
) -> Result<(), InflateError> {
    let mut node = 0usize;
    for offset in (0..length).rev() {
        let bit = usize::from((code >> offset) & 1);
        let final_bit = offset == 0;
        if final_bit {
            if !matches!(nodes[node].branches[bit], Branch::Empty) {
                return Err(InflateError::InvalidData);
            }
            nodes[node].branches[bit] = Branch::Symbol(symbol);
            continue;
        }
        match nodes[node].branches[bit] {
            Branch::Empty => {
                let next = nodes.len();
                nodes.push(HuffmanNode::default());
                nodes[node].branches[bit] = Branch::Node(next);
                node = next;
            }
            Branch::Node(next) => node = next,
            Branch::Symbol(_) => return Err(InflateError::InvalidData),
        }
    }
    Ok(())
}

struct BitReader<'a> {
    data: &'a [u8],
    byte_offset: usize,
    bit_offset: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_offset: 0,
            bit_offset: 0,
        }
    }

    fn read_bits(&mut self, count: u8) -> Result<u32, InflateError> {
        if count > 24 {
            return Err(InflateError::InvalidData);
        }
        let mut value = 0u32;
        for shift in 0..count {
            let byte = *self
                .data
                .get(self.byte_offset)
                .ok_or(InflateError::InvalidData)?;
            value |= u32::from((byte >> self.bit_offset) & 1) << shift;
            self.bit_offset += 1;
            if self.bit_offset == 8 {
                self.bit_offset = 0;
                self.byte_offset += 1;
            }
        }
        Ok(value)
    }

    fn align_to_byte(&mut self) {
        if self.bit_offset != 0 {
            self.bit_offset = 0;
            self.byte_offset = self.byte_offset.saturating_add(1);
        }
    }

    fn read_aligned_u16(&mut self) -> Result<u16, InflateError> {
        if self.bit_offset != 0 {
            return Err(InflateError::InvalidData);
        }
        let end = self
            .byte_offset
            .checked_add(2)
            .filter(|end| *end <= self.data.len())
            .ok_or(InflateError::InvalidData)?;
        let bytes: [u8; 2] = self.data[self.byte_offset..end]
            .try_into()
            .map_err(|_| InflateError::InvalidData)?;
        self.byte_offset = end;
        Ok(u16::from_le_bytes(bytes))
    }

    fn read_aligned_bytes(&mut self, count: usize) -> Result<&'a [u8], InflateError> {
        if self.bit_offset != 0 {
            return Err(InflateError::InvalidData);
        }
        let end = self
            .byte_offset
            .checked_add(count)
            .filter(|end| *end <= self.data.len())
            .ok_or(InflateError::InvalidData)?;
        let bytes = &self.data[self.byte_offset..end];
        self.byte_offset = end;
        Ok(bytes)
    }

    fn finish(mut self) -> Result<(), InflateError> {
        self.align_to_byte();
        if self.byte_offset == self.data.len() {
            Ok(())
        } else {
            Err(InflateError::InvalidData)
        }
    }
}

struct BoundedOutput {
    bytes: Vec<u8>,
    maximum: usize,
}

impl BoundedOutput {
    fn new(maximum: usize) -> Result<Self, InflateError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve(maximum.min(4096))
            .map_err(|_| InflateError::OutOfMemory)?;
        Ok(Self { bytes, maximum })
    }

    fn reserve(&mut self, additional: usize) -> Result<(), InflateError> {
        let required = self
            .bytes
            .len()
            .checked_add(additional)
            .filter(|required| *required <= self.maximum)
            .ok_or(InflateError::OutputTooLarge)?;
        if required > self.bytes.capacity() {
            let doubled = self.bytes.capacity().max(1024).saturating_mul(2);
            let desired = required.max(doubled.min(self.maximum));
            self.bytes
                .try_reserve(desired.saturating_sub(self.bytes.len()))
                .map_err(|_| InflateError::OutOfMemory)?;
        }
        Ok(())
    }

    fn push(&mut self, byte: u8) -> Result<(), InflateError> {
        self.reserve(1)?;
        self.bytes.push(byte);
        Ok(())
    }

    fn extend_from_slice(&mut self, bytes: &[u8]) -> Result<(), InflateError> {
        self.reserve(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn copy_match(&mut self, distance: usize, length: usize) -> Result<(), InflateError> {
        if distance == 0 || distance > self.bytes.len() {
            return Err(InflateError::InvalidData);
        }
        self.reserve(length)?;
        for _ in 0..length {
            let byte = self.bytes[self.bytes.len() - distance];
            self.bytes.push(byte);
        }
        Ok(())
    }

    fn finish(self, limit: OutputLimit) -> Result<Vec<u8>, InflateError> {
        if matches!(limit, OutputLimit::Exact(expected) if self.bytes.len() != expected) {
            return Err(InflateError::InvalidData);
        }
        Ok(self.bytes)
    }
}

fn adler32(data: &[u8]) -> u32 {
    const MODULUS: u32 = 65_521;
    const CHUNK: usize = 5_552;
    let mut a = 1u32;
    let mut b = 0u32;
    for chunk in data.chunks(CHUNK) {
        for byte in chunk {
            a += u32::from(*byte);
            b += a;
        }
        a %= MODULUS;
        b %= MODULUS;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_hex(input: &str) -> Vec<u8> {
        input
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let hex = std::str::from_utf8(pair).unwrap();
                u8::from_str_radix(hex, 16).unwrap()
            })
            .collect()
    }

    fn stored_zlib(data: &[u8]) -> Vec<u8> {
        assert!(data.len() <= u16::MAX as usize);
        let mut output = vec![0x78, 0x01, 0x01];
        let length = data.len() as u16;
        output.extend_from_slice(&length.to_le_bytes());
        output.extend_from_slice(&(!length).to_le_bytes());
        output.extend_from_slice(data);
        output.extend_from_slice(&adler32(data).to_be_bytes());
        output
    }

    #[test]
    fn decodes_stored_fixed_and_dynamic_huffman_streams() {
        let stored = stored_zlib(b"stored block");
        assert_eq!(
            decompress_zlib(&stored, OutputLimit::Exact(12)).unwrap(),
            b"stored block"
        );

        let fixed = decode_hex("7801cb48cdc9c95748cbac484d51c8284d4bcb4dcc5354c8c02208003e120ef5");
        assert_eq!(
            decompress_zlib(&fixed, OutputLimit::AtMost(1024)).unwrap(),
            b"hello fixed huffman! hello fixed huffman!"
        );

        let dynamic = decode_hex(
            "78daedccc51180300000c13fd5e0524ed024b84bf56815cc6d012bc427fe24aff4953df25b7191522aa5b42ecbaaaa9bb6eb87719a9775db0fd3b21dd7f383303204252525252525252525252525252525252525252525252525252525252525e57fcb1333683551",
        );
        let expected =
            b"aaaaaaaaabbbbbbbbcccccccdddddddeeeeeefffffgggghhhiiijjkkllmnopqrstuvwxyz0123456789\n"
                .repeat(100);
        assert_eq!(
            decompress_zlib(&dynamic, OutputLimit::Exact(expected.len())).unwrap(),
            expected
        );
    }

    #[test]
    fn enforces_exact_and_capped_output_before_growth() {
        let stream = stored_zlib(b"bounded output");
        assert_eq!(
            decompress_zlib(&stream, OutputLimit::AtMost(13)),
            Err(InflateError::OutputTooLarge)
        );
        assert_eq!(
            decompress_zlib(&stream, OutputLimit::Exact(13)),
            Err(InflateError::OutputTooLarge)
        );
        assert_eq!(
            decompress_zlib(&stream, OutputLimit::Exact(15)),
            Err(InflateError::InvalidData)
        );
    }

    #[test]
    fn rejects_invalid_headers_checksums_trailers_and_stored_lengths() {
        let valid = stored_zlib(b"check");
        for invalid in [
            valid[..5].to_vec(),
            {
                let mut bytes = valid.clone();
                bytes[0] = 0x79;
                bytes
            },
            {
                let mut bytes = valid.clone();
                bytes[1] ^= 1;
                bytes
            },
            {
                let mut bytes = valid.clone();
                bytes[1] = 0x20;
                bytes
            },
            {
                let mut bytes = valid.clone();
                let last = bytes.len() - 1;
                bytes[last] ^= 1;
                bytes
            },
            {
                let mut bytes = valid.clone();
                bytes[5] ^= 1;
                bytes
            },
            {
                let mut bytes = valid.clone();
                bytes.insert(bytes.len() - 4, 0);
                bytes
            },
        ] {
            assert_eq!(
                decompress_zlib(&invalid, OutputLimit::AtMost(1024)),
                Err(InflateError::InvalidData)
            );
        }
    }

    #[test]
    fn rejects_reserved_blocks_and_invalid_back_references() {
        let reserved = wrap_deflate(&[0b0000_0111], b"");
        assert_eq!(
            decompress_zlib(&reserved, OutputLimit::AtMost(1024)),
            Err(InflateError::InvalidData)
        );

        let mut writer = BitWriter::default();
        writer.bits(1, 1);
        writer.bits(1, 2);
        writer.huffman(1, 7); // Fixed literal/length symbol 257.
        writer.huffman(0, 5); // Distance 1, before any output exists.
        let invalid_distance = wrap_deflate(&writer.finish(), b"");
        assert_eq!(
            decompress_zlib(&invalid_distance, OutputLimit::AtMost(1024)),
            Err(InflateError::InvalidData)
        );
    }

    #[test]
    fn rejects_zero_output_streams_above_the_block_work_limit() {
        let mut writer = BitWriter::default();
        for block in 0..=MAX_DEFLATE_BLOCKS {
            writer.bits(u32::from(block == MAX_DEFLATE_BLOCKS), 1);
            writer.bits(1, 2); // Fixed Huffman block.
            writer.huffman(0, 7); // End-of-block symbol 256.
        }
        let stream = wrap_deflate(&writer.finish(), b"");

        assert_eq!(
            decompress_zlib(&stream, OutputLimit::AtMost(1024)),
            Err(InflateError::WorkLimitExceeded)
        );
    }

    #[test]
    fn rejects_oversubscribed_and_invalid_dynamic_trees() {
        let mut oversubscribed = dynamic_header();
        for _ in 0..4 {
            oversubscribed.bits(1, 3);
        }
        let stream = wrap_deflate(&oversubscribed.finish(), b"");
        assert_eq!(
            decompress_zlib(&stream, OutputLimit::AtMost(1024)),
            Err(InflateError::InvalidData)
        );

        let mut repeat_without_previous = dynamic_header();
        repeat_without_previous.bits(1, 3);
        repeat_without_previous.bits(0, 3);
        repeat_without_previous.bits(0, 3);
        repeat_without_previous.bits(0, 3);
        repeat_without_previous.huffman(0, 1);
        let stream = wrap_deflate(&repeat_without_previous.finish(), b"");
        assert_eq!(
            decompress_zlib(&stream, OutputLimit::AtMost(1024)),
            Err(InflateError::InvalidData)
        );

        // This otherwise-decodable stream advertises HLIT=30 (287
        // literal/length codes). RFC 1951 permits at most 286; reserved symbol
        // 286 is invalid even though its code length is zero here.
        let reserved_literal_count = decode_hex(
            "7801f5c0010400000000100000000000000000010000000000000000000000000000000000000000000080000000000100420042",
        );
        assert_eq!(
            decompress_zlib(&reserved_literal_count, OutputLimit::AtMost(1024)),
            Err(InflateError::InvalidData)
        );
    }

    #[test]
    fn every_truncation_of_a_valid_stream_is_rejected() {
        let stream = decode_hex("7801cb48cdc9c95748cbac484d51c8284d4bcb4dcc5354c8c02208003e120ef5");
        for length in 0..stream.len() {
            assert!(decompress_zlib(&stream[..length], OutputLimit::AtMost(1024)).is_err());
        }
    }

    #[test]
    fn bounded_malformed_inputs_do_not_panic() {
        for byte in u8::MIN..=u8::MAX {
            let _ = decompress_zlib(&wrap_deflate(&[byte], b""), OutputLimit::AtMost(4096));
        }

        let mut state = 0x1234_5678u32;
        for length in 0..64 {
            let mut deflate = Vec::with_capacity(length);
            for _ in 0..length {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                deflate.push(state as u8);
            }
            let _ = decompress_zlib(&wrap_deflate(&deflate, b""), OutputLimit::AtMost(4096));
        }
    }

    fn dynamic_header() -> BitWriter {
        let mut writer = BitWriter::default();
        writer.bits(1, 1);
        writer.bits(2, 2);
        writer.bits(0, 5);
        writer.bits(0, 5);
        writer.bits(0, 4);
        writer
    }

    fn wrap_deflate(deflate: &[u8], uncompressed: &[u8]) -> Vec<u8> {
        let mut output = vec![0x78, 0x01];
        output.extend_from_slice(deflate);
        output.extend_from_slice(&adler32(uncompressed).to_be_bytes());
        output
    }

    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        bit_offset: u8,
    }

    impl BitWriter {
        fn bits(&mut self, value: u32, count: u8) {
            for shift in 0..count {
                if self.bit_offset == 0 {
                    self.bytes.push(0);
                }
                let last = self.bytes.len() - 1;
                self.bytes[last] |= ((value >> shift) as u8 & 1) << self.bit_offset;
                self.bit_offset = (self.bit_offset + 1) % 8;
            }
        }

        fn huffman(&mut self, code: u16, length: u8) {
            for shift in (0..length).rev() {
                self.bits(u32::from((code >> shift) & 1), 1);
            }
        }

        fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }
}
