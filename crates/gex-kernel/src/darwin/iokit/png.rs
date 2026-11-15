//! A PNG of 32-bit pixels, for a host to save what a framebuffer or a
//! surface holds: RGB, eight bits a channel, its image data deflated in
//! stored blocks (no compression to speak of, and no code to get wrong).

fn crc32(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in bytes {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// `width` by `height` pixels of `row_bytes` a row, each a little-endian
/// word `xxRRGGBB` (the framebuffer's `--------RRRRRRRRGGGGGGGGBBBBBBBB`,
/// a surface's `BGRA`), as a PNG.
pub fn encode(width: u32, height: u32, row_bytes: usize, pixels: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((width as usize * 3 + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        for x in 0..width as usize {
            let at = y * row_bytes + x * 4;
            let p = pixels.get(at..at + 4).unwrap_or(&[0; 4]);
            raw.extend_from_slice(&[p[2], p[1], p[0]]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    if blocks.peek().is_none() {
        z.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(b) = blocks.next() {
        z.push(blocks.peek().is_none() as u8);
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = width.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn checksums_are_the_standard_ones() {
        assert_eq!(super::crc32(b"IEND"), 0xae42_6082);
        assert_eq!(super::adler32(b"Wikipedia"), 0x11e6_0398);
        let png = super::encode(2, 1, 8, &[0, 0, 255, 0, 255, 0, 0, 0]);
        assert!(png.starts_with(b"\x89PNG"));
        assert!(png.ends_with(&[0xae, 0x42, 0x60, 0x82]));
    }
}
