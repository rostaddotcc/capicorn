//! Minimal, dependency-free PNG decoder.
//!
//! The crate may not add dependencies (`crates/platform/Cargo.toml` is owned
//! elsewhere) and `tray-icon` exposes no PNG entry point, but the tray API in the
//! locked contract takes `icon_png: &[u8]` — callers embed the bytes with
//! `include_bytes!`. So the ~200 lines of inflate/defilter live here.
//!
//! Supported: 8-bit greyscale, RGB, palette, greyscale+alpha and RGBA, non
//! interlaced, CRC-checked. Everything else is a clean
//! [`PlatformError::Unsupported`] — hostile input never panics, and output is
//! capped at the IHDR-implied size (no decompression bombs).

use crate::PlatformError;

const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
/// Refuse absurd tray icons: 4 KiB a side is far beyond any panel icon.
const MAX_DIM: u32 = 4096;

/// A decoded, straight (non-premultiplied) 8-bit RGBA image.
pub(crate) struct PngImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn png_err(msg: impl Into<String>) -> PlatformError {
    PlatformError::Unsupported(format!("png: {}", msg.into()))
}

fn crc32(kind: &[u8], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in kind.iter().chain(data.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn be32(bytes: &[u8]) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(u32::from_be_bytes(slice))
}

struct Header {
    width: u32,
    height: u32,
    color: u8,
}

impl Header {
    fn bytes_per_pixel(&self) -> usize {
        match self.color {
            0 => 1,
            2 => 3,
            3 => 1,
            4 => 2,
            _ => 4,
        }
    }
}

fn parse_ihdr(data: &[u8]) -> Result<Header, PlatformError> {
    if data.len() != 13 {
        return Err(png_err("IHDR has the wrong length"));
    }
    let width = be32(data).ok_or_else(|| png_err("truncated width"))?;
    let height = be32(&data[4..]).ok_or_else(|| png_err("truncated height"))?;
    let depth = data[8];
    let color = data[9];
    let compression = data[10];
    let filter = data[11];
    let interlace = data[12];
    if width == 0 || height == 0 || width > MAX_DIM || height > MAX_DIM {
        return Err(png_err(format!("unsupported size {width}x{height}")));
    }
    if depth != 8 {
        return Err(png_err(format!("unsupported bit depth {depth}")));
    }
    if !matches!(color, 0 | 2 | 3 | 4 | 6) {
        return Err(png_err(format!("unsupported colour type {color}")));
    }
    if compression != 0 || filter != 0 || interlace != 0 {
        return Err(png_err("compressed, filtered or interlaced PNG"));
    }
    Ok(Header {
        width,
        height,
        color,
    })
}

fn parse_plte(data: &[u8]) -> Result<Vec<(u8, u8, u8)>, PlatformError> {
    if data.is_empty() || !data.len().is_multiple_of(3) || data.len() > 768 {
        return Err(png_err("malformed PLTE"));
    }
    Ok(data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| (c[0], c[1], c[2]))
        .collect())
}

/// Decode a PNG into 8-bit RGBA.
pub(crate) fn decode(bytes: &[u8]) -> Result<PngImage, PlatformError> {
    if bytes.len() < SIGNATURE.len() || bytes[..SIGNATURE.len()] != SIGNATURE {
        return Err(png_err("not a PNG"));
    }
    let mut pos = SIGNATURE.len();
    let mut header: Option<Header> = None;
    let mut palette: Option<Vec<(u8, u8, u8)>> = None;
    let mut idat: Vec<u8> = Vec::new();

    while pos + 8 <= bytes.len() {
        let len = be32(&bytes[pos..]).ok_or_else(|| png_err("truncated chunk header"))? as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let start = pos + 8;
        let end = start
            .checked_add(len)
            .ok_or_else(|| png_err("chunk length overflow"))?;
        let crc_at = end
            .checked_add(4)
            .ok_or_else(|| png_err("chunk length overflow"))?;
        if crc_at > bytes.len() {
            return Err(png_err("truncated chunk"));
        }
        let data = &bytes[start..end];
        let expected = be32(&bytes[end..]).ok_or_else(|| png_err("truncated CRC"))?;
        if crc32(kind, data) != expected {
            return Err(png_err("chunk CRC mismatch"));
        }
        match kind {
            b"IHDR" => header = Some(parse_ihdr(data)?),
            b"PLTE" => palette = Some(parse_plte(data)?),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        pos = crc_at;
    }

    let header = header.ok_or_else(|| png_err("missing IHDR"))?;
    if idat.is_empty() {
        return Err(png_err("missing IDAT"));
    }
    let bpp = header.bytes_per_pixel();
    let stride = header.width as usize * bpp;
    let raw_len = (stride + 1) * header.height as usize;
    let raw = inflate(&idat, raw_len)?;
    if raw.len() != raw_len {
        return Err(png_err("pixel data length mismatch"));
    }
    let pixels = unfilter(&raw, stride, header.height as usize, bpp)?;

    let (width, height) = (header.width, header.height);
    let count = (width as usize) * (height as usize);
    let mut rgba = Vec::with_capacity(count * 4);
    match header.color {
        0 => {
            for &g in &pixels {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        2 => {
            for px in pixels.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        3 => {
            let palette = palette.ok_or_else(|| png_err("palette image without PLTE"))?;
            for &idx in &pixels {
                let (r, g, b) = palette
                    .get(idx as usize)
                    .copied()
                    .ok_or_else(|| png_err("palette index out of range"))?;
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        4 => {
            for px in pixels.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        _ => rgba = pixels,
    }

    Ok(PngImage {
        width,
        height,
        rgba,
    })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i32::from(a) + i32::from(b) - i32::from(c);
    let pa = (p - i32::from(a)).abs();
    let pb = (p - i32::from(b)).abs();
    let pc = (p - i32::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn unfilter(
    raw: &[u8],
    stride: usize,
    height: usize,
    bpp: usize,
) -> Result<Vec<u8>, PlatformError> {
    let mut out = vec![0u8; stride * height];
    for y in 0..height {
        let row_start = y * (stride + 1);
        let filter = raw[row_start];
        let src = &raw[row_start + 1..row_start + 1 + stride];
        let (row_at, prev_at) = (y * stride, y.saturating_sub(1) * stride);
        for x in 0..stride {
            let left = if x >= bpp { out[row_at + x - bpp] } else { 0 };
            let up = if y > 0 { out[prev_at + x] } else { 0 };
            let up_left = if y > 0 && x >= bpp {
                out[prev_at + x - bpp]
            } else {
                0
            };
            out[row_at + x] = match filter {
                0 => src[x],
                1 => src[x].wrapping_add(left),
                2 => src[x].wrapping_add(up),
                3 => src[x].wrapping_add((u16::from(left) + u16::from(up)).wrapping_div(2) as u8),
                4 => src[x].wrapping_add(paeth(left, up, up_left)),
                other => return Err(png_err(format!("unknown filter type {other}"))),
            };
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- inflate (RFC 1951)

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// Order the code lengths of the code-length alphabet are stored in.
const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            count: 0,
        }
    }

    fn bits(&mut self, n: u32) -> Result<u32, PlatformError> {
        while self.count < n {
            let byte = *self
                .data
                .get(self.pos)
                .ok_or_else(|| png_err("truncated deflate stream"))?;
            self.pos += 1;
            self.buf |= u32::from(byte) << self.count;
            self.count += 8;
        }
        let mask = if n == 0 { 0 } else { (1u32 << n) - 1 };
        let value = self.buf & mask;
        self.buf >>= n;
        self.count -= n;
        Ok(value)
    }

    fn align_to_byte(&mut self) {
        let drop = self.count % 8;
        self.buf >>= drop;
        self.count -= drop;
    }
}

struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn from_lengths(lengths: &[u8]) -> Result<Self, PlatformError> {
        let mut counts = [0u16; 16];
        for &len in lengths {
            if usize::from(len) >= counts.len() {
                return Err(png_err("code length out of range"));
            }
            counts[usize::from(len)] += 1;
        }
        counts[0] = 0;
        let mut left: i32 = 1;
        for &count in counts.iter().skip(1) {
            left <<= 1;
            left -= i32::from(count);
            if left < 0 {
                return Err(png_err("oversubscribed huffman tree"));
            }
        }
        let mut offsets = [0u16; 16];
        let mut total: u16 = 0;
        for len in 1..16 {
            offsets[len] = total;
            total += counts[len];
        }
        let mut symbols = vec![0u16; usize::from(total)];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len != 0 {
                let slot = usize::from(offsets[usize::from(len)]);
                offsets[usize::from(len)] += 1;
                if let Some(cell) = symbols.get_mut(slot) {
                    *cell = symbol as u16;
                }
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, br: &mut BitReader<'_>) -> Result<u16, PlatformError> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= br.bits(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                let slot = index + (code - first);
                return usize::try_from(slot)
                    .ok()
                    .and_then(|i| self.symbols.get(i).copied())
                    .ok_or_else(|| png_err("invalid huffman code"));
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(png_err("invalid huffman code"))
    }
}

fn fixed_tables() -> Result<(Huffman, Huffman), PlatformError> {
    let mut lit = [0u8; 288];
    for (i, len) in lit.iter_mut().enumerate() {
        *len = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let dist = [5u8; 30];
    Ok((Huffman::from_lengths(&lit)?, Huffman::from_lengths(&dist)?))
}

fn dynamic_tables(br: &mut BitReader<'_>) -> Result<(Huffman, Huffman), PlatformError> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(png_err("too many huffman codes"));
    }
    let mut clen = [0u8; 19];
    for &slot in CLEN_ORDER.iter().take(hclen) {
        clen[slot] = br.bits(3)? as u8;
    }
    let clen_tree = Huffman::from_lengths(&clen)?;

    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < lengths.len() {
        let symbol = clen_tree.decode(br)?;
        match symbol {
            0..=15 => {
                lengths[i] = symbol as u8;
                i += 1;
            }
            16 => {
                let prev = match i.checked_sub(1).and_then(|p| lengths.get(p)) {
                    Some(v) => *v,
                    None => return Err(png_err("repeat with no previous length")),
                };
                let repeat = 3 + br.bits(2)? as usize;
                for _ in 0..repeat {
                    if i >= lengths.len() {
                        return Err(png_err("code length repeat overrun"));
                    }
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 | 18 => {
                let repeat = if symbol == 17 {
                    3 + br.bits(3)? as usize
                } else {
                    11 + br.bits(7)? as usize
                };
                i += repeat;
                if i > lengths.len() {
                    return Err(png_err("code length repeat overrun"));
                }
            }
            _ => return Err(png_err("bad code length symbol")),
        }
    }
    if lengths.get(256).copied().unwrap_or(0) == 0 {
        return Err(png_err("missing end-of-block code"));
    }
    let lit = Huffman::from_lengths(&lengths[..hlit])?;
    let dist = Huffman::from_lengths(&lengths[hlit..])?;
    Ok((lit, dist))
}

fn inflate_block(
    br: &mut BitReader<'_>,
    lit: &Huffman,
    dist: &Huffman,
    out: &mut Vec<u8>,
    limit: usize,
) -> Result<(), PlatformError> {
    loop {
        let symbol = lit.decode(br)?;
        if symbol < 256 {
            if out.len() >= limit {
                return Err(png_err("image data larger than expected"));
            }
            out.push(symbol as u8);
        } else if symbol == 256 {
            return Ok(());
        } else {
            let index = usize::from(symbol - 257);
            let (base, extra) = match (LENGTH_BASE.get(index), LENGTH_EXTRA.get(index)) {
                (Some(b), Some(e)) => (*b, *e),
                _ => return Err(png_err("invalid length symbol")),
            };
            let len = usize::from(base) + br.bits(extra)? as usize;
            let dsym = usize::from(dist.decode(br)?);
            let (dbase, dextra) = match (DIST_BASE.get(dsym), DIST_EXTRA.get(dsym)) {
                (Some(b), Some(e)) => (*b, *e),
                _ => return Err(png_err("invalid distance symbol")),
            };
            let back = usize::from(dbase) + br.bits(dextra)? as usize;
            if back > out.len() || back == 0 {
                return Err(png_err("distance beyond start of output"));
            }
            if out.len() + len > limit {
                return Err(png_err("image data larger than expected"));
            }
            let start = out.len() - back;
            for k in 0..len {
                let byte = out[start + k];
                out.push(byte);
            }
        }
    }
}

fn inflate(deflate: &[u8], limit: usize) -> Result<Vec<u8>, PlatformError> {
    // PNG wraps the deflate stream in a zlib header (2 bytes) and an Adler-32.
    let mut br = BitReader::new(deflate);
    let cmf = br.bits(8)?;
    let flg = br.bits(8)?;
    if cmf & 0x0F != 8 {
        return Err(png_err("not a zlib/deflate stream"));
    }
    if (u32::from(cmf as u8) * 256 + u32::from(flg as u8)) % 31 != 0 {
        return Err(png_err("bad zlib header checksum"));
    }
    if flg & 0x20 != 0 {
        return Err(png_err("preset zlib dictionaries are unsupported"));
    }
    let mut out = Vec::with_capacity(limit);
    loop {
        let last = br.bits(1)?;
        let kind = br.bits(2)?;
        match kind {
            0 => {
                br.align_to_byte();
                let len = br.bits(16)?;
                let nlen = br.bits(16)?;
                if len ^ 0xFFFF != nlen {
                    return Err(png_err("corrupt stored block length"));
                }
                for _ in 0..len {
                    let byte = br.bits(8)? as u8;
                    if out.len() >= limit {
                        return Err(png_err("image data larger than expected"));
                    }
                    out.push(byte);
                }
            }
            1 => {
                let (lit, dist) = fixed_tables()?;
                inflate_block(&mut br, &lit, &dist, &mut out, limit)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut br)?;
                inflate_block(&mut br, &lit, &dist, &mut out, limit)?;
            }
            _ => return Err(png_err("invalid deflate block type")),
        }
        if last == 1 {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8×8 RGBA gradient, every row using a different PNG filter (0..4 cycling),
    /// compressed with zlib's dynamic-Huffman blocks.
    const SAMPLE: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 8, 0, 0, 0, 8, 8, 6,
        0, 0, 0, 196, 15, 190, 139, 0, 0, 0, 159, 73, 68, 65, 84, 120, 218, 133, 206, 49, 10, 130,
        96, 0, 197, 241, 103, 95, 36, 97, 20, 38, 57, 5, 31, 210, 16, 132, 83, 139, 155, 131, 99,
        208, 17, 26, 188, 64, 39, 104, 176, 115, 8, 69, 80, 4, 65, 208, 236, 208, 61, 130, 186, 73,
        240, 122, 182, 53, 72, 195, 111, 123, 252, 121, 0, 64, 107, 192, 180, 3, 174, 186, 224,
        166, 7, 150, 3, 176, 26, 130, 143, 17, 232, 184, 182, 30, 120, 104, 210, 210, 0, 174, 245,
        36, 148, 72, 98, 73, 36, 147, 37, 76, 63, 69, 17, 248, 33, 2, 127, 44, 145, 76, 37, 150,
        185, 36, 104, 127, 11, 74, 185, 70, 5, 163, 130, 137, 127, 96, 82, 130, 217, 46, 97, 126,
        88, 179, 56, 158, 184, 63, 63, 121, 191, 4, 124, 93, 23, 124, 223, 182, 116, 102, 85, 125,
        50, 67, 147, 191, 39, 63, 212, 17, 48, 94, 176, 88, 226, 145, 0, 0, 0, 0, 73, 69, 78, 68,
        174, 66, 96, 130,
    ];

    fn expected_rgba() -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..8u32 {
            for x in 0..8u32 {
                out.extend_from_slice(&[
                    ((x * 31 + y * 7) % 256) as u8,
                    ((y * 31 + x * 3) % 256) as u8,
                    ((x * y * 11) % 256) as u8,
                    255,
                ]);
            }
        }
        out
    }

    #[test]
    fn decodes_every_filter_type_and_dynamic_huffman() {
        let img = decode(SAMPLE).expect("sample PNG decodes");
        assert_eq!((img.width, img.height), (8, 8));
        assert_eq!(img.rgba, expected_rgba());
    }

    #[test]
    fn decodes_stored_deflate_blocks() {
        // A 2×1 RGB image in a *stored* (uncompressed) deflate block.
        let raw = [0u8, 255, 0, 0, 0, 255, 0];
        let mut zlib = vec![0x78, 0x01, 0x01, 0x07, 0x00, 0xF8, 0xFF];
        zlib.extend_from_slice(&raw);
        zlib.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        let png = build_png(2, 1, 2, &zlib);
        let img = decode(&png).expect("stored-block PNG decodes");
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert!(decode(&[]).is_err());
        assert!(decode(b"not a png at all").is_err());
        let mut truncated = SAMPLE.to_vec();
        truncated.truncate(60);
        assert!(decode(&truncated).is_err());
        // A single flipped byte breaks the chunk CRC.
        let mut corrupt = SAMPLE.to_vec();
        corrupt[40] ^= 0xFF;
        assert!(decode(&corrupt).is_err());
    }

    #[test]
    fn refuses_interlaced_and_unsupported_sizes() {
        let mut ihdr = vec![0u8, 0, 0, 16, 0, 0, 0, 16, 8, 6, 0, 0, 0];
        ihdr[12] = 1; // Adam7
        assert!(parse_ihdr(&ihdr).is_err());
        let huge = vec![0u8, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 8, 6, 0, 0, 0];
        assert!(parse_ihdr(&huge).is_err());
        let deep = vec![0u8, 0, 0, 8, 0, 0, 0, 8, 16, 6, 0, 0, 0];
        assert!(parse_ihdr(&deep).is_err());
    }

    #[test]
    fn inflate_stops_at_the_expected_size() {
        // The sample's raw size is 8 * (1 + 8 * 4) = 264; ask for less and the
        // decoder must refuse rather than grow (no decompression bombs).
        let idat = idat_of(SAMPLE).expect("IDAT present");
        assert!(inflate(&idat, 16).is_err());
        assert_eq!(inflate(&idat, 264).expect("full size decodes").len(), 264);
    }

    /// Pull the IDAT payload out of a PNG (test helper: mirrors `decode`).
    fn idat_of(png: &[u8]) -> Option<Vec<u8>> {
        let mut pos = SIGNATURE.len();
        let mut out = Vec::new();
        while pos + 8 <= png.len() {
            let len = usize::try_from(be32(&png[pos..])?).ok()?;
            let kind = &png[pos + 4..pos + 8];
            let start = pos + 8;
            let end = start.checked_add(len)?;
            if end + 4 > png.len() {
                return None;
            }
            if kind == b"IDAT" {
                out.extend_from_slice(&png[start..end]);
            }
            pos = end + 4;
        }
        Some(out)
    }

    fn build_png(width: u32, height: u32, color: u8, idat: &[u8]) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, color, 0, 0, 0]);
        out.extend_from_slice(&chunk(b"IHDR", &ihdr));
        out.extend_from_slice(&chunk(b"IDAT", idat));
        out.extend_from_slice(&chunk(b"IEND", &[]));
        out
    }

    fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(kind, data).to_be_bytes());
        out
    }
}
