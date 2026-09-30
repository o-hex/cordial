#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EtcFormat {
    Rgb8,
    Rgb8A1,
    Rgba8,
    R11,
    R11Signed,
    Rg11,
    Rg11Signed,
}

impl EtcFormat {
    pub fn block_bytes(self) -> usize {
        match self {
            EtcFormat::Rgb8 | EtcFormat::Rgb8A1 | EtcFormat::R11 | EtcFormat::R11Signed => 8,
            EtcFormat::Rgba8 | EtcFormat::Rg11 | EtcFormat::Rg11Signed => 16,
        }
    }

    pub fn texel_bytes(self) -> usize {
        match self {
            EtcFormat::Rgb8 | EtcFormat::Rgb8A1 | EtcFormat::Rgba8 => 4,
            EtcFormat::R11 | EtcFormat::R11Signed => 2,
            EtcFormat::Rg11 | EtcFormat::Rg11Signed => 4,
        }
    }
}

const INTENSITY: [[i32; 2]; 8] = [
    [2, 8],
    [5, 17],
    [9, 29],
    [13, 42],
    [18, 60],
    [24, 80],
    [33, 106],
    [47, 183],
];

const DISTANCE: [i32; 8] = [3, 6, 11, 16, 23, 32, 41, 64];

const EAC_MODIFIERS: [[i32; 8]; 16] = [
    [-3, -6, -9, -15, 2, 5, 8, 14],
    [-3, -7, -10, -13, 2, 6, 9, 12],
    [-2, -5, -8, -13, 1, 4, 7, 12],
    [-2, -4, -6, -13, 1, 3, 5, 12],
    [-3, -6, -8, -12, 2, 5, 7, 11],
    [-3, -7, -9, -11, 2, 6, 8, 10],
    [-4, -7, -8, -11, 3, 6, 7, 10],
    [-3, -5, -8, -11, 2, 4, 7, 10],
    [-2, -6, -8, -10, 1, 5, 7, 9],
    [-2, -5, -8, -10, 1, 4, 7, 9],
    [-2, -4, -8, -10, 1, 3, 7, 9],
    [-2, -5, -7, -10, 1, 4, 6, 9],
    [-3, -4, -7, -10, 2, 3, 6, 9],
    [-1, -2, -3, -10, 0, 1, 2, 9],
    [-4, -6, -8, -9, 3, 5, 7, 8],
    [-3, -5, -7, -9, 2, 4, 6, 8],
];

fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn ext4(v: u8) -> i32 {
    i32::from(v) * 17
}

fn ext5(v: i32) -> i32 {
    (v << 3) | (v >> 2)
}

fn ext6(v: u8) -> i32 {
    let v = i32::from(v);
    (v << 2) | (v >> 4)
}

fn ext7(v: u8) -> i32 {
    let v = i32::from(v);
    (v << 1) | (v >> 6)
}

fn signed3(v: u8) -> i32 {
    let v = i32::from(v & 7);
    if v >= 4 {
        v - 8
    } else {
        v
    }
}

fn pixel_index(lo: u32, x: usize, y: usize) -> usize {
    let i = x * 4 + y;
    let msb = (lo >> (16 + i)) & 1;
    let lsb = (lo >> i) & 1;
    ((msb << 1) | lsb) as usize
}

pub fn decode_etc2_rgb(block: &[u8], punchthrough: bool, out: &mut [[u8; 4]; 16]) {
    let b0 = block[0];
    let b1 = block[1];
    let b2 = block[2];
    let b3 = block[3];
    let lo = u32::from_be_bytes([block[4], block[5], block[6], block[7]]);
    let bit33 = (b3 >> 1) & 1 != 0;
    let (differential, opaque) = if punchthrough { (true, bit33) } else { (bit33, true) };

    if !differential {
        let c1 = [ext4(b0 >> 4), ext4(b1 >> 4), ext4(b2 >> 4)];
        let c2 = [ext4(b0 & 0xF), ext4(b1 & 0xF), ext4(b2 & 0xF)];
        subblocks(c1, c2, b3, lo, true, out);
        return;
    }

    let r = i32::from(b0 >> 3);
    let g = i32::from(b1 >> 3);
    let b = i32::from(b2 >> 3);
    let r2 = r + signed3(b0);
    let g2 = g + signed3(b1);
    let b2c = b + signed3(b2);

    if !(0..=31).contains(&r2) {
        let c1 = [
            ext4((((b0 >> 3) & 3) << 2) | (b0 & 3)),
            ext4(b1 >> 4),
            ext4(b1 & 0xF),
        ];
        let c2 = [ext4(b2 >> 4), ext4(b2 & 0xF), ext4(b3 >> 4)];
        let d = DISTANCE[usize::from((((b3 >> 2) & 3) << 1) | (b3 & 1))];
        let paint = [c1, add(c2, d), c2, add(c2, -d)];
        paint_block(&paint, lo, opaque, out);
    } else if !(0..=31).contains(&g2) {
        let r1 = (b0 >> 3) & 0xF;
        let g1 = ((b0 & 7) << 1) | ((b1 >> 4) & 1);
        let bl1 = (b1 & 8) | ((b1 & 3) << 1) | (b2 >> 7);
        let r2h = (b2 >> 3) & 0xF;
        let g2h = ((b2 & 7) << 1) | (b3 >> 7);
        let bl2 = (b3 >> 3) & 0xF;
        let v1 = (u32::from(r1) << 8) | (u32::from(g1) << 4) | u32::from(bl1);
        let v2 = (u32::from(r2h) << 8) | (u32::from(g2h) << 4) | u32::from(bl2);
        let idx = (((b3 >> 2) & 1) << 2) | ((b3 & 1) << 1) | u8::from(v1 >= v2);
        let d = DISTANCE[usize::from(idx)];
        let c1 = [ext4(r1), ext4(g1), ext4(bl1)];
        let c2 = [ext4(r2h), ext4(g2h), ext4(bl2)];
        let paint = [add(c1, d), add(c1, -d), add(c2, d), add(c2, -d)];
        paint_block(&paint, lo, opaque, out);
    } else if !(0..=31).contains(&b2c) {
        let ro = ext6((b0 >> 1) & 0x3F);
        let go = ext7(((b0 & 1) << 6) | ((b1 >> 1) & 0x3F));
        let bo = ext6(((b1 & 1) << 5) | (((b2 >> 3) & 3) << 3) | ((b2 & 3) << 1) | (b3 >> 7));
        let rh = ext6((((b3 >> 2) & 0x1F) << 1) | (b3 & 1));
        let gh = ext7(((lo >> 25) & 0x7F) as u8);
        let bh = ext6(((lo >> 19) & 0x3F) as u8);
        let rv = ext6(((lo >> 13) & 0x3F) as u8);
        let gv = ext7(((lo >> 6) & 0x7F) as u8);
        let bv = ext6((lo & 0x3F) as u8);
        for y in 0..4 {
            for x in 0..4 {
                let (xi, yi) = (x as i32, y as i32);
                let f = |o: i32, h: i32, v: i32| clamp8((xi * (h - o) + yi * (v - o) + 4 * o + 2) >> 2);
                out[y * 4 + x] = [f(ro, rh, rv), f(go, gh, gv), f(bo, bh, bv), 255];
            }
        }
    } else {
        let c1 = [ext5(r), ext5(g), ext5(b)];
        let c2 = [ext5(r2), ext5(g2), ext5(b2c)];
        subblocks(c1, c2, b3, lo, opaque, out);
    }
}

fn add(c: [i32; 3], d: i32) -> [i32; 3] {
    [c[0] + d, c[1] + d, c[2] + d]
}

fn paint_block(paint: &[[i32; 3]; 4], lo: u32, opaque: bool, out: &mut [[u8; 4]; 16]) {
    for y in 0..4 {
        for x in 0..4 {
            let idx = pixel_index(lo, x, y);
            out[y * 4 + x] = if !opaque && idx == 2 {
                [0, 0, 0, 0]
            } else {
                let p = paint[idx];
                [clamp8(p[0]), clamp8(p[1]), clamp8(p[2]), 255]
            };
        }
    }
}

fn subblocks(c1: [i32; 3], c2: [i32; 3], b3: u8, lo: u32, opaque: bool, out: &mut [[u8; 4]; 16]) {
    let cw = [usize::from(b3 >> 5), usize::from((b3 >> 2) & 7)];
    let flip = b3 & 1 != 0;
    for y in 0..4 {
        for x in 0..4 {
            let second = if flip { y >= 2 } else { x >= 2 };
            let (base, table) = if second { (c2, INTENSITY[cw[1]]) } else { (c1, INTENSITY[cw[0]]) };
            let idx = pixel_index(lo, x, y);
            if !opaque && idx == 2 {
                out[y * 4 + x] = [0, 0, 0, 0];
                continue;
            }
            let m = match idx {
                0 if !opaque => 0,
                0 => table[0],
                1 => table[1],
                2 => -table[0],
                _ => -table[1],
            };
            out[y * 4 + x] = [clamp8(base[0] + m), clamp8(base[1] + m), clamp8(base[2] + m), 255];
        }
    }
}

fn eac_parts(block: &[u8]) -> (u8, i32, &'static [i32; 8], u64) {
    let bits = u64::from_be_bytes([
        0, 0, block[2], block[3], block[4], block[5], block[6], block[7],
    ]);
    (block[0], i32::from(block[1] >> 4), &EAC_MODIFIERS[usize::from(block[1] & 0xF)], bits)
}

fn eac_index(bits: u64, x: usize, y: usize) -> usize {
    let i = x * 4 + y;
    ((bits >> (45 - 3 * i)) & 7) as usize
}

pub fn decode_eac_alpha(block: &[u8], out: &mut [[u8; 4]; 16]) {
    let (base, mult, table, bits) = eac_parts(block);
    for y in 0..4 {
        for x in 0..4 {
            out[y * 4 + x][3] = clamp8(i32::from(base) + table[eac_index(bits, x, y)] * mult);
        }
    }
}

pub fn decode_eac11(block: &[u8], signed: bool, out: &mut [u16; 16]) {
    let (base, mult, table, bits) = eac_parts(block);
    for y in 0..4 {
        for x in 0..4 {
            let m = table[eac_index(bits, x, y)];
            let step = if mult == 0 { m } else { m * mult * 8 };
            out[y * 4 + x] = if signed {
                let b = i32::from(base as i8).max(-127);
                let v = (b * 8 + step).clamp(-1023, 1023);
                let mag = v.abs();
                let wide = (mag << 5) | (mag >> 5);
                (if v < 0 { -wide } else { wide }) as i16 as u16
            } else {
                let v = (i32::from(base) * 8 + 4 + step).clamp(0, 2047);
                ((v << 5) | (v >> 6)) as u16
            };
        }
    }
}

pub fn decode_block(format: EtcFormat, block: &[u8], out: &mut [u8; 64]) {
    match format {
        EtcFormat::Rgb8 | EtcFormat::Rgb8A1 | EtcFormat::Rgba8 => {
            let mut px = [[0u8; 4]; 16];
            if format == EtcFormat::Rgba8 {
                decode_etc2_rgb(&block[8..16], false, &mut px);
                decode_eac_alpha(&block[0..8], &mut px);
            } else {
                decode_etc2_rgb(&block[0..8], format == EtcFormat::Rgb8A1, &mut px);
            }
            for (i, p) in px.iter().enumerate() {
                out[i * 4..i * 4 + 4].copy_from_slice(p);
            }
        }
        EtcFormat::R11 | EtcFormat::R11Signed => {
            let mut r = [0u16; 16];
            decode_eac11(&block[0..8], format == EtcFormat::R11Signed, &mut r);
            for (i, v) in r.iter().enumerate() {
                out[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
        EtcFormat::Rg11 | EtcFormat::Rg11Signed => {
            let signed = format == EtcFormat::Rg11Signed;
            let mut r = [0u16; 16];
            let mut g = [0u16; 16];
            decode_eac11(&block[0..8], signed, &mut r);
            decode_eac11(&block[8..16], signed, &mut g);
            for i in 0..16 {
                out[i * 4..i * 4 + 2].copy_from_slice(&r[i].to_le_bytes());
                out[i * 4 + 2..i * 4 + 4].copy_from_slice(&g[i].to_le_bytes());
            }
        }
    }
}

pub struct Surface<'a> {
    pub data: &'a [u8],
    pub row_blocks: usize,
    pub rows_of_blocks: usize,
}

pub fn decode_region(
    format: EtcFormat,
    src: &Surface<'_>,
    width: usize,
    height: usize,
    dst: &mut [u8],
) -> Result<(), String> {
    let bb = format.block_bytes();
    let tb = format.texel_bytes();
    let bw = width.div_ceil(4);
    let bh = height.div_ceil(4);
    if bw > src.row_blocks || bh > src.rows_of_blocks {
        return Err(format!(
            "region {width}x{height} exceeds source {}x{} blocks",
            src.row_blocks, src.rows_of_blocks
        ));
    }
    let need = ((bh - 1) * src.row_blocks + bw) * bb;
    if src.data.len() < need {
        return Err(format!("source has {} bytes, region needs {need}", src.data.len()));
    }
    if dst.len() < width * height * tb {
        return Err(format!("destination has {} bytes, region needs {}", dst.len(), width * height * tb));
    }
    let mut texels = [0u8; 64];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * src.row_blocks + bx) * bb;
            decode_block(format, &src.data[off..off + bb], &mut texels);
            for y in 0..4 {
                let py = by * 4 + y;
                if py >= height {
                    break;
                }
                for x in 0..4 {
                    let px = bx * 4 + x;
                    if px >= width {
                        break;
                    }
                    let d = (py * width + px) * tb;
                    let s = (y * 4 + x) * tb;
                    dst[d..d + tb].copy_from_slice(&texels[s..s + tb]);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type Vector = (&'static str, &'static str, &'static [u8], &'static [u8]);

    const VECTORS: &[Vector] = &[
        ("rgb8", "H", &[0xf5, 0x4, 0xd8, 0xaf, 0x0, 0x36, 0x35, 0xed], &[174, 106, 0, 255, 251, 81, 149, 255, 174, 106, 0, 255, 174, 106, 0, 255, 251, 81, 149, 255, 123, 0, 21, 255, 255, 234, 81, 255, 174, 106, 0, 255, 123, 0, 21, 255, 174, 106, 0, 255, 174, 106, 0, 255, 255, 234, 81, 255, 174, 106, 0, 255, 174, 106, 0, 255, 255, 234, 81, 255, 255, 234, 81, 255]),
        ("rgb8", "T", &[0xfb, 0x65, 0xf6, 0x73, 0xa7, 0xbd, 0x9d, 0xa6], &[255, 102, 119, 255, 255, 102, 119, 255, 249, 96, 113, 255, 255, 108, 125, 255, 255, 108, 125, 255, 249, 96, 113, 255, 255, 102, 119, 255, 255, 102, 119, 255, 249, 96, 113, 255, 255, 102, 85, 255, 249, 96, 113, 255, 255, 102, 85, 255, 255, 102, 119, 255, 249, 96, 113, 255, 255, 108, 125, 255, 249, 96, 113, 255]),
        ("rgb8", "differential", &[0x73, 0xdd, 0x8f, 0xdb, 0xec, 0xc7, 0x77, 0x73], &[9, 116, 34, 255, 221, 255, 246, 255, 221, 255, 246, 255, 221, 255, 246, 255, 9, 116, 34, 255, 221, 255, 246, 255, 221, 255, 246, 255, 9, 116, 34, 255, 107, 165, 99, 255, 34, 92, 26, 255, 34, 92, 26, 255, 34, 92, 26, 255, 173, 231, 165, 255, 107, 165, 99, 255, 107, 165, 99, 255, 107, 165, 99, 255]),
        ("rgb8", "individual", &[0x82, 0xda, 0x96, 0x30, 0x2f, 0xcd, 0x83, 0x79], &[119, 204, 136, 255, 153, 238, 170, 255, 0, 110, 42, 255, 52, 188, 120, 255, 141, 226, 158, 255, 153, 238, 170, 255, 0, 110, 42, 255, 16, 152, 84, 255, 131, 216, 148, 255, 119, 204, 136, 255, 16, 152, 84, 255, 52, 188, 120, 255, 119, 204, 136, 255, 131, 216, 148, 255, 16, 152, 84, 255, 94, 230, 162, 255]),
        ("rgb8", "planar", &[0xb6, 0x10, 0xf3, 0xfa, 0x46, 0xd2, 0x2b, 0x1c], &[109, 16, 93, 255, 143, 30, 96, 255, 176, 43, 99, 255, 210, 57, 102, 255, 99, 34, 98, 255, 133, 48, 101, 255, 166, 61, 104, 255, 200, 75, 107, 255, 89, 52, 103, 255, 123, 66, 106, 255, 156, 79, 109, 255, 190, 93, 112, 255, 79, 70, 108, 255, 113, 84, 111, 255, 146, 97, 114, 255, 180, 111, 117, 255]),
        ("rgb8a1", "differential opaque=0", &[0x73, 0x78, 0x46, 0xec, 0x36, 0xd4, 0x69, 0x61], &[255, 255, 249, 255, 0, 0, 0, 0, 182, 165, 91, 255, 0, 0, 0, 0, 115, 123, 66, 255, 255, 255, 249, 255, 0, 0, 0, 0, 98, 81, 7, 255, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 182, 165, 91, 255, 115, 123, 66, 255, 0, 0, 0, 0, 182, 165, 91, 255, 140, 123, 49, 255]),
        ("rgb8a1", "T opaque=0", &[0xfb, 0x9f, 0x73, 0x50, 0x13, 0xd4, 0x8, 0x47], &[122, 54, 88, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 122, 54, 88, 255, 255, 153, 255, 255, 0, 0, 0, 0, 255, 153, 255, 255, 116, 48, 82, 255, 116, 48, 82, 255, 255, 153, 255, 255, 255, 153, 255, 255, 255, 153, 255, 255, 0, 0, 0, 0, 122, 54, 88, 255, 255, 153, 255, 255]),
        ("rgb8a1", "H opaque=0", &[0x89, 0xfa, 0xca, 0xec, 0x52, 0x43, 0xe8, 0xb7], &[130, 62, 198, 255, 0, 28, 198, 255, 40, 74, 244, 255, 0, 0, 0, 0, 130, 62, 198, 255, 0, 28, 198, 255, 0, 0, 0, 0, 0, 28, 198, 255, 0, 28, 198, 255, 0, 0, 0, 0, 40, 74, 244, 255, 130, 62, 198, 255, 40, 74, 244, 255, 0, 28, 198, 255, 0, 28, 198, 255, 0, 28, 198, 255]),
        ("rgb8a1", "differential opaque=1", &[0x8c, 0x6b, 0xc4, 0x46, 0x7c, 0xeb, 0x7, 0x37], &[111, 78, 169, 255, 169, 136, 227, 255, 124, 149, 182, 255, 102, 127, 160, 255, 111, 78, 169, 255, 111, 78, 169, 255, 124, 149, 182, 255, 102, 127, 160, 255, 169, 136, 227, 255, 131, 98, 189, 255, 90, 115, 148, 255, 102, 127, 160, 255, 131, 98, 189, 255, 131, 98, 189, 255, 102, 127, 160, 255, 112, 137, 170, 255]),
        ("rgb8a1", "T opaque=1", &[0xf2, 0xe1, 0x7e, 0x96, 0xf2, 0xb0, 0x16, 0xc2], &[170, 238, 17, 255, 119, 238, 153, 255, 170, 238, 17, 255, 108, 227, 142, 255, 130, 249, 164, 255, 119, 238, 153, 255, 108, 227, 142, 255, 119, 238, 153, 255, 170, 238, 17, 255, 130, 249, 164, 255, 130, 249, 164, 255, 119, 238, 153, 255, 170, 238, 17, 255, 108, 227, 142, 255, 170, 238, 17, 255, 119, 238, 153, 255]),
        ("rgb8a1", "H opaque=1", &[0x4b, 0x6, 0x3, 0xc7, 0x3e, 0x95, 0xa, 0xa2], &[64, 183, 200, 255, 64, 183, 200, 255, 217, 166, 132, 255, 64, 183, 200, 255, 89, 38, 4, 255, 89, 38, 4, 255, 0, 55, 72, 255, 64, 183, 200, 255, 64, 183, 200, 255, 217, 166, 132, 255, 64, 183, 200, 255, 217, 166, 132, 255, 217, 166, 132, 255, 0, 55, 72, 255, 0, 55, 72, 255, 217, 166, 132, 255]),
        ("rgba8", "eac alpha", &[0x28, 0x68, 0xc1, 0xaf, 0x63, 0xd, 0xe5, 0x50, 0xbf, 0x65, 0xc, 0x94, 0xfd, 0xb9, 0xf1, 0x51], &[127, 42, 0, 82, 127, 42, 0, 94, 175, 5, 124, 28, 175, 5, 124, 0, 205, 120, 18, 28, 169, 84, 0, 70, 255, 109, 228, 0, 175, 5, 124, 70, 205, 120, 18, 0, 247, 162, 60, 46, 231, 61, 180, 0, 175, 5, 124, 0, 169, 84, 0, 0, 169, 84, 0, 0, 231, 61, 180, 82, 175, 5, 124, 28]),
        ("r11", "mult0=False", &[0x12, 0xcc, 0xe7, 0x39, 0x6d, 0xb8, 0x7a, 0x40], &[126, 0, 42, 0, 54, 0, 54, 0, 0, 0, 54, 0, 90, 0, 0, 0, 90, 0, 54, 0, 0, 0, 0, 0, 0, 0, 54, 0, 126, 0, 0, 0]),
        ("r11", "mult0=True", &[0xf9, 0x1, 0x3e, 0xb, 0x85, 0x19, 0xef, 0xcf], &[248, 0, 250, 0, 249, 0, 251, 0, 251, 0, 250, 0, 250, 0, 251, 0, 249, 0, 249, 0, 247, 0, 248, 0, 249, 0, 250, 0, 250, 0, 251, 0]),
        ("r11s", "mult0=False", &[0xb9, 0xe8, 0x77, 0x24, 0xfa, 0xf5, 0xd1, 0x3e], &[0, 0, 0, 0, 182, 0, 28, 0, 126, 0, 0, 0, 126, 0, 70, 0, 154, 0, 182, 0, 0, 0, 182, 0, 0, 0, 0, 0, 126, 0, 154, 0]),
        ("r11s", "mult0=True", &[0x9a, 0x4, 0x1d, 0xc, 0x9f, 0x6a, 0x74, 0x1e], &[25, 0, 26, 0, 24, 0, 24, 0, 27, 0, 24, 0, 24, 0, 25, 0, 24, 0, 24, 0, 26, 0, 24, 0, 25, 0, 27, 0, 27, 0, 26, 0]),
        ("rg11", "mult0=False", &[0x34, 0xdc, 0xd, 0x5b, 0x87, 0x27, 0x1f, 0x5d, 0x71, 0x23, 0xaa, 0x6a, 0x74, 0x99, 0x43, 0xa3], &[13, 119, 91, 119, 0, 115, 169, 105, 0, 101, 130, 105, 0, 123, 91, 123, 0, 115, 13, 123, 130, 101, 0, 115, 91, 123, 169, 115, 0, 115, 91, 87]),
        ("rg11", "mult0=True", &[0x95, 0xe, 0x6b, 0x5d, 0xc7, 0xbe, 0x87, 0xd0, 0xff, 0x3, 0xf3, 0x4a, 0xbb, 0x20, 0x3d, 0x7b], &[148, 255, 150, 255, 150, 255, 148, 255, 148, 255, 150, 254, 150, 255, 150, 255, 150, 255, 149, 255, 150, 255, 148, 255, 150, 255, 150, 253, 149, 253, 149, 253]),
        ("rg11s", "mult0=False", &[0x1d, 0x80, 0xe4, 0x4f, 0xc8, 0x81, 0x9f, 0xd8, 0x5b, 0x46, 0x45, 0x9c, 0xaf, 0xb9, 0x90, 0xb2], &[255, 186, 255, 246, 172, 242, 255, 202, 108, 190, 255, 186, 132, 246, 255, 186, 132, 174, 108, 242, 36, 174, 36, 246, 172, 190, 132, 255, 108, 190, 132, 186]),
        ("rg11s", "mult0=True", &[0x97, 0x0, 0xa2, 0x47, 0xc0, 0x3d, 0x30, 0xf3, 0x3f, 0x0, 0xfa, 0xa7, 0xea, 0x32, 0xe0, 0xae], &[23, 192, 21, 189, 22, 190, 22, 190, 22, 191, 24, 192, 24, 191, 21, 189, 23, 191, 22, 191, 21, 191, 23, 191, 23, 189, 22, 189, 21, 191, 21, 191]),
    ];

    fn format_of(kind: &str) -> EtcFormat {
        match kind {
            "rgb8" => EtcFormat::Rgb8,
            "rgb8a1" => EtcFormat::Rgb8A1,
            "rgba8" => EtcFormat::Rgba8,
            "r11" => EtcFormat::R11,
            "r11s" => EtcFormat::R11Signed,
            "rg11" => EtcFormat::Rg11,
            _ => EtcFormat::Rg11Signed,
        }
    }

    fn eac_to_8bit(v: u16, signed: bool) -> i32 {
        if signed {
            let s = i32::from(v as i16);
            let v11 = if s < 0 { -((-s) >> 5) } else { s >> 5 };
            (v11 + 1023) * 255 / 2046
        } else {
            i32::from(v >> 5) >> 3
        }
    }

    #[test]
    fn every_mode_matches_an_independent_decoder() {
        for &(kind, mode, block, want) in VECTORS {
            let format = format_of(kind);
            let mut out = [0u8; 64];
            decode_block(format, block, &mut out);
            match format {
                EtcFormat::Rgb8 | EtcFormat::Rgb8A1 | EtcFormat::Rgba8 => {
                    assert_eq!(&out[..], want, "{kind} {mode}");
                }
                _ => {
                    let signed = matches!(format, EtcFormat::R11Signed | EtcFormat::Rg11Signed);
                    let channels = format.texel_bytes() / 2;
                    for px in 0..16 {
                        for c in 0..channels {
                            let o = px * format.texel_bytes() + c * 2;
                            let got = eac_to_8bit(u16::from_le_bytes([out[o], out[o + 1]]), signed);
                            let exp = i32::from(want[px * 2 + c]);
                            assert!((got - exp).abs() <= 1, "{kind} {mode} pixel {px} channel {c}: {got} vs {exp}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn eac_extends_to_sixteen_bits_at_both_ends() {
        let mut out = [0u16; 16];
        decode_eac11(&[0xff, 0x1f, 0xdb, 0x6d, 0xb6, 0xdb, 0x6d, 0xb6], false, &mut out);
        assert!(out.iter().all(|&v| v == 0xffff));
        decode_eac11(&[0x00, 0x10, 0, 0, 0, 0, 0, 0], false, &mut out);
        assert!(out.iter().all(|&v| v == 0));
        decode_eac11(&[0x81, 0x13, 0x49, 0x24, 0x92, 0x49, 0x24, 0x92], true, &mut out);
        assert!(out.iter().all(|&v| v as i16 == -32767), "{out:?}");
    }

    #[test]
    fn partial_blocks_at_small_mips_are_cropped_not_overrun() {
        let block = [0x82, 0xda, 0x96, 0x30, 0x2f, 0xcd, 0x83, 0x79];
        let mut full = [0u8; 64];
        decode_block(EtcFormat::Rgb8, &block, &mut full);
        let src = Surface { data: &block, row_blocks: 1, rows_of_blocks: 1 };
        let mut two = [0u8; 2 * 2 * 4];
        decode_region(EtcFormat::Rgb8, &src, 2, 2, &mut two).unwrap();
        assert_eq!(&two[0..8], &full[0..8]);
        assert_eq!(&two[8..16], &full[16..24]);
        let mut one = [0u8; 4];
        decode_region(EtcFormat::Rgb8, &src, 1, 1, &mut one).unwrap();
        assert_eq!(&one, &full[0..4]);
    }

    #[test]
    fn row_length_wider_than_the_region_is_respected() {
        let a = [0x82, 0xda, 0x96, 0x30, 0x2f, 0xcd, 0x83, 0x79];
        let b = [0x73, 0xdd, 0x8f, 0xdb, 0xec, 0xc7, 0x77, 0x73];
        let data: Vec<u8> = [a, b, b, a].concat();
        let src = Surface { data: &data, row_blocks: 2, rows_of_blocks: 2 };
        let mut out = vec![0u8; 4 * 8 * 4];
        decode_region(EtcFormat::Rgb8, &src, 4, 8, &mut out).unwrap();
        let mut fa = [0u8; 64];
        let mut fb = [0u8; 64];
        decode_block(EtcFormat::Rgb8, &a, &mut fa);
        decode_block(EtcFormat::Rgb8, &b, &mut fb);
        assert_eq!(&out[0..64], &fa[..]);
        assert_eq!(&out[64..128], &fb[..]);
    }

    #[test]
    fn a_short_source_is_an_error_not_a_panic() {
        let src = Surface { data: &[0u8; 8], row_blocks: 2, rows_of_blocks: 2 };
        let mut out = vec![0u8; 8 * 8 * 4];
        assert!(decode_region(EtcFormat::Rgb8, &src, 8, 8, &mut out).is_err());
    }
}
