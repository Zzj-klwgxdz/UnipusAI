//! 学习通题目字体解密。
//!
//! 未作答的测验页会把部分汉字替换为生僻字，并用内嵌 `font-cxsecret` 渲染真实字形。
//! 该内嵌字体是 Source Han Sans CN Normal（weight 350）的子集，**字形即真实汉字**，
//! 只是 cmap 把字形挂到了随机的替代码点上（每个页面不同）。
//!
//! 解码方式：把内嵌字体里每个码点的字形光栅化为 32×32 二值位图，
//! 与预生成的参考表（Noto Sans SC wght350 的同款特征）做 IoU 匹配。
//!
//! 参考表由 `examples/gen_font_ref.rs` 用 **同一个光栅化器（fontdue）** 生成，
//! 保证两侧特征一致；运行时只需内嵌 ~0.9MB 的 `assets/font_ref.bin`。

use anyhow::{Context, Result, bail};
use std::collections::HashMap;

/// 参考特征表（见 examples/gen_font_ref.rs）。
pub const REF_TABLE: &[u8] = include_bytes!("../../../assets/font_ref.bin");
/// 匹配网格。
pub const GRID: usize = 32;
/// 渲染像素高度。
pub const PX: f32 = 96.0;
/// 二值阈值（0-255）。
pub const BINARY_THRESHOLD: u8 = 40;

/// 参考特征表。
pub struct RefTable<'a> {
    chars: Vec<char>,
    bits: &'a [u8],
    grid: usize,
}

impl<'a> RefTable<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        if data.len() < 12 || &data[..4] != b"CXF1" {
            bail!("字体参考表格式不正确（magic 不符）");
        }
        let count = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let grid = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
        let stride = (grid * grid).div_ceil(8);
        let need = 12 + count * (4 + stride);
        if data.len() < need {
            bail!("字体参考表被截断（需要 {} 字节，实际 {}）", need, data.len());
        }
        let mut chars = Vec::with_capacity(count);
        for i in 0..count {
            let off = 12 + i * (4 + stride);
            let cp = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            chars.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
        }
        Ok(Self {
            chars,
            bits: &data[12..],
            grid,
        })
    }

    fn row(&self, i: usize) -> &[u8] {
        let stride = (self.grid * self.grid).div_ceil(8);
        let off = i * (4 + stride) + 4;
        &self.bits[off..off + stride]
    }

    /// 返回与特征最匹配的字符（IoU 最大）。
    pub fn best(&self, feat: &[u8]) -> Option<(char, f32)> {
        let mut best: Option<(char, f32)> = None;
        for (i, ch) in self.chars.iter().enumerate() {
            let bits = self.row(i);
            let mut inter = 0u32;
            let mut uni = 0u32;
            for (byte_idx, b) in bits.iter().enumerate() {
                let a = *b;
                let c = feat[byte_idx];
                inter += (a & c).count_ones();
                uni += (a | c).count_ones();
            }
            let iou = if uni == 0 {
                0.0
            } else {
                inter as f32 / uni as f32
            };
            if best.is_none_or(|(_, s)| iou > s) {
                best = Some((*ch, iou));
            }
        }
        best
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

/// 从灰度位图提取 32×32 二值特征（打包为 128 字节）。
///
/// 步骤：裁墨迹包围盒 → 居中成正方形 → 双线性缩放到 32×32 → 阈值二值化。
pub fn feature_from_bitmap(bitmap: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || bitmap.len() < width * height {
        return None;
    }
    // 1) 墨迹包围盒
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..height {
        for x in 0..width {
            if bitmap[y * width + x] > BINARY_THRESHOLD {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x == usize::MAX {
        return None;
    }
    let w = max_x - min_x + 1;
    let h = max_y - min_y + 1;
    let side = w.max(h);
    let ox = (side - w) / 2;
    let oy = (side - h) / 2;

    // 2) 采样到 GRID×GRID
    let mut feat = vec![0u8; GRID * GRID];
    let scale = side as f32 / GRID as f32;
    for gy in 0..GRID {
        for gx in 0..GRID {
            // 目标像素中心映射回源图（含居中偏移）
            let sx = min_x as f32 - ox as f32 + (gx as f32 + 0.5) * scale - 0.5;
            let sy = min_y as f32 - oy as f32 + (gy as f32 + 0.5) * scale - 0.5;
            let v = bilinear(bitmap, width, height, sx, sy);
            if v > BINARY_THRESHOLD {
                feat[gy * GRID + gx] = 1;
            }
        }
    }
    Some(pack_bits(&feat))
}

fn bilinear(bitmap: &[u8], width: usize, height: usize, x: f32, y: f32) -> u8 {
    if x < 0.0 || y < 0.0 || x > (width - 1) as f32 || y > (height - 1) as f32 {
        return 0;
    }
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let p00 = bitmap[y0 * width + x0] as f32;
    let p10 = bitmap[y0 * width + x1] as f32;
    let p01 = bitmap[y1 * width + x0] as f32;
    let p11 = bitmap[y1 * width + x1] as f32;
    let top = p00 * (1.0 - fx) + p10 * fx;
    let bottom = p01 * (1.0 - fx) + p11 * fx;
    (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8
}

fn pack_bits(bits: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bits.len().div_ceil(8)];
    for (i, b) in bits.iter().enumerate() {
        if *b != 0 {
            out[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    out
}

/// 解码一个内嵌字体：返回 `替代码点 -> 真实字符`。
pub fn decode_font(ttf: &[u8], table: &[u8]) -> Result<HashMap<char, char>> {
    let refs = RefTable::parse(table)?;
    let face = ttf_parser::Face::parse(ttf, 0).context("解析内嵌字体失败")?;
    let cmap = face.tables().cmap.context("内嵌字体缺少 cmap")?;
    let mut codepoints: Vec<u32> = Vec::new();
    for sub in cmap.subtables {
        if sub.is_unicode() {
            sub.codepoints(|cp| codepoints.push(cp));
        }
    }
    codepoints.sort_unstable();
    codepoints.dedup();

    let font = fontdue::Font::from_bytes(ttf, fontdue::FontSettings::default())
        .map_err(|e| anyhow::anyhow!("加载内嵌字体失败: {e}"))?;
    let mut map = HashMap::new();
    for cp in codepoints {
        let Some(ch) = char::from_u32(cp) else {
            continue;
        };
        let glyph = font.lookup_glyph_index(ch);
        if glyph == 0 {
            continue;
        }
        let (m, bitmap) = font.rasterize_indexed(glyph, PX);
        let Some(feat) = feature_from_bitmap(&bitmap, m.width, m.height) else {
            continue;
        };
        if let Some((best, score)) = refs.best(&feat)
            && score > 0.45
        {
            map.insert(ch, best);
        }
    }
    Ok(map)
}

/// 把文本里出现的替代码点替换为真实字符。
pub fn decode_text(text: &str, map: &HashMap<char, char>) -> String {
    text.chars()
        .map(|c| map.get(&c).copied().unwrap_or(c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_table_is_available_and_parses() {
        let t = RefTable::parse(REF_TABLE).expect("参考表应存在且可解析");
        assert!(t.len() >= 6000, "参考表候选过少: {}", t.len());
    }

    #[test]
    fn feature_is_stable_for_same_bitmap() {
        let mut bitmap = vec![0u8; 64 * 64];
        for y in 10..50 {
            for x in 20..40 {
                bitmap[y * 64 + x] = 255;
            }
        }
        let a = feature_from_bitmap(&bitmap, 64, 64).unwrap();
        let b = feature_from_bitmap(&bitmap, 64, 64).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 128);
    }

    #[test]
    fn empty_bitmap_returns_none() {
        let bitmap = vec![0u8; 16 * 16];
        assert!(feature_from_bitmap(&bitmap, 16, 16).is_none());
    }
}
