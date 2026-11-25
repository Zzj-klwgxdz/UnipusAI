//! 生成字体匹配参考表（`assets/font_ref.bin`）。
//!
//! 用法：
//! ```text
//! cargo run --release --example gen_font_ref -- <font.ttf> <candidates.txt> <out.bin>
//! ```
//!
//! - `<font.ttf>`：参考字体（Noto Sans SC 实例化到 wght=350，与 Source Han Sans CN Normal 同设计）；
//! - `<candidates.txt>`：候选字符列表（每行一个字符，见 `assets/font_candidates.txt`）；
//! - `<out.bin>`：输出特征表（CXF1 格式，约 0.9MB）。
//!
//! 必须使用与运行时相同的 `fontdue` 光栅化器和 `feature_from_bitmap`，保证特征一致。

use UnipusAI::platform::chaoxing::font::{GRID, PX, feature_from_bitmap};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("用法: gen_font_ref <font.ttf> <candidates.txt> <out.bin>");
        return Ok(());
    }
    let font_data = std::fs::read(&args[1])?;
    let font = fontdue::Font::from_bytes(&font_data[..], fontdue::FontSettings::default())
        .map_err(|e| anyhow::anyhow!("加载参考字体失败: {e}"))?;
    let text = std::fs::read_to_string(&args[2])?;
    let mut chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    chars.sort_unstable();
    chars.dedup();

    let mut entries: Vec<(char, Vec<u8>)> = Vec::new();
    for ch in chars {
        let idx = font.lookup_glyph_index(ch);
        if idx == 0 {
            continue;
        }
        let (m, bitmap) = font.rasterize_indexed(idx, PX);
        let Some(feat) = feature_from_bitmap(&bitmap, m.width, m.height) else {
            continue;
        };
        entries.push((ch, feat));
    }

    let mut out = Vec::with_capacity(12 + entries.len() * (4 + 128));
    out.extend_from_slice(b"CXF1");
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&(GRID as u32).to_le_bytes());
    for (ch, feat) in &entries {
        out.extend_from_slice(&(*ch as u32).to_le_bytes());
        out.extend_from_slice(feat);
    }
    std::fs::write(&args[3], &out)?;
    println!(
        "wrote {} chars -> {} ({} bytes)",
        entries.len(),
        args[3],
        out.len()
    );
    Ok(())
}
