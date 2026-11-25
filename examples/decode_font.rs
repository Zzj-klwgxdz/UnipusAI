//! 手动验证字体解码：`cargo run --release --example decode_font -- <font.ttf> <text.txt>`
use UnipusAI::platform::chaoxing::font::{REF_TABLE, decode_font, decode_text};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: decode_font <font.ttf> <text.txt>");
        return Ok(());
    }
    let font = std::fs::read(&args[1])?;
    let text = std::fs::read_to_string(&args[2])?;
    let map = decode_font(&font, REF_TABLE)?;
    println!("{}", decode_text(&text, &map));
    Ok(())
}
