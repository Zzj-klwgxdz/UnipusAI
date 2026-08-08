use crate::api::session::Session;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;

const CACHE_DIR: &str = ".media_cache";

fn cache_root() -> PathBuf {
    PathBuf::from(CACHE_DIR)
}

fn sha1(s: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(s.as_bytes());
    h.finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn which(cmd: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(cmd);
        if cand.is_file() {
            return Some(cand);
        }
        let exe = dir.join(format!("{}.exe", cmd));
        if exe.is_file() {
            return Some(exe);
        }
    }
    None
}

/// 下载媒体文件，返回本地路径。
async fn download_media(session: &Session, url: &str, dest: &PathBuf) -> Result<()> {
    if dest.is_file() && dest.metadata().map(|m| m.len()).unwrap_or(0) > 1000 {
        return Ok(());
    }
    let bytes = session.get_bytes(url).await?;
    if bytes.is_empty() {
        anyhow::bail!("媒体下载为空: {}", url);
    }
    std::fs::write(dest, &bytes)
        .with_context(|| format!("保存媒体失败: {}", dest.display()))?;
    Ok(())
}

/// 用 ffmpeg 从媒体文件提取 16k 单声道 wav。
fn extract_wav(media: &PathBuf, wav: &PathBuf) -> Result<()> {
    let ffmpeg = which("ffmpeg").ok_or_else(|| anyhow::anyhow!("未找到 ffmpeg"))?;
    let out = std::process::Command::new(ffmpeg)
        .arg("-y")
        .arg("-i")
        .arg(media.to_str().unwrap_or(""))
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg("-f")
        .arg("wav")
        .arg(wav.to_str().unwrap_or(""))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("运行 ffmpeg 失败")?;
    if !out.status.success() {
        anyhow::bail!("ffmpeg 转码失败 status={}", out.status);
    }
    Ok(())
}

/// 调用本地 whisper CLI 转写 wav。
/// language 为 "auto" / 空时省略 --language，让 whisper 自动检测语种。
fn whisper_cli(wav: &PathBuf, model: &str, language: &str) -> Result<String> {
    let whisper = which("whisper")
        .or_else(|| {
            // .venv/Scripts/whisper.exe
            which_venv("whisper")
        })
        .ok_or_else(|| anyhow::anyhow!("未找到 whisper CLI"))?;
    let mut cmd = std::process::Command::new(whisper);
    cmd.arg(wav.to_str().unwrap_or(""))
        .arg("--model")
        .arg(model);
    let lang = language.trim();
    if !lang.is_empty() && !lang.eq_ignore_ascii_case("auto") {
        cmd.arg("--language").arg(lang);
    }
    cmd.arg("--output_format")
        .arg("txt")
        .arg("--output_dir")
        .arg(CACHE_DIR)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let out = cmd.output().context("运行 whisper 失败")?;
    if !out.status.success() {
        anyhow::bail!("whisper 转录失败 status={}", out.status);
    }
    // whisper 的输出文件名为 {wav 原名}.txt
    let txt_path = {
        let name = wav
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("out")
            .trim_end_matches(".wav");
        cache_root().join(format!("{}.txt", name))
    };
    let text = std::fs::read_to_string(&txt_path).unwrap_or_default();
    Ok(text.trim().to_string())
}

fn which_venv(bin: &str) -> Option<std::path::PathBuf> {
    for base in [".venv", "venv", ".virtualenv"] {
        let p = PathBuf::from(base).join("Scripts").join(format!("{}.exe", bin));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// 转录媒体内容为文本。优先读 .vtt 字幕，其次 whisper 转写。
pub async fn transcribe_media(session: &Session, url: &str) -> Result<String> {
    let cfg = session.cfg();
    if !cfg.whisper_enabled {
        return Ok(String::new());
    }
    std::fs::create_dir_all(cache_root()).ok();

    // 字幕文件(.vtt/.srt)直接解析
    let lower = url.to_ascii_lowercase();
    if lower.ends_with(".vtt") || lower.ends_with(".srt") {
        return parse_subtitle(session, url).await;
    }

    let key = sha1(url);
    let cache_txt = cache_root().join(format!("{}.txt", key));
    if cache_txt.is_file() {
        if let Ok(t) = std::fs::read_to_string(&cache_txt) {
            let t = t.trim();
            if !t.is_empty() {
                return Ok(t.to_string());
            }
        }
    }

    let media_ext = if lower.contains(".mp4") || lower.contains("video") {
        "mp4"
    } else if lower.contains(".m4a") {
        "m4a"
    } else if lower.contains(".wav") {
        "wav"
    } else {
        "mp3"
    };
    let media = cache_root().join(format!("{}.{}", key, media_ext));
    let wav = cache_root().join(format!("{}.wav", key));

    download_media(session, url, &media).await?;
    if media_ext == "wav" {
        std::fs::copy(&media, &wav).ok();
    } else {
        extract_wav(&media, &wav)?;
    }
    let text = whisper_cli(&wav, &cfg.whisper_model, &cfg.whisper_language)?;
    std::fs::write(&cache_txt, &text).ok();
    Ok(text)
}

async fn parse_subtitle(session: &Session, url: &str) -> Result<String> {
    let key = sha1(url);
    let dest_txt = cache_root().join(format!("{}.txt", key));
    if let Ok(t) = std::fs::read_to_string(&dest_txt) {
        let t = t.trim();
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    let raw = session.get_bytes(url).await?;
    let s = String::from_utf8_lossy(&raw).to_string();
    let text = vtt_to_text(&s);
    std::fs::write(&dest_txt, &text).ok();
    Ok(text)
}

/// 把 WEBVTT/SRT 转为纯文本（去掉时间轴与序号）。
fn vtt_to_text(s: &str) -> String {
    let mut out = Vec::new();
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("WEBVTT") {
            continue;
        }
        if t.contains("-->") {
            continue;
        }
        if t.chars().all(|c| c.is_digit(10)) {
            continue;
        }
        out.push(t);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vtt_parse_strips_timestamps() {
        let vtt = "WEBVTT\n\n1\n00:00:07.299 --> 00:00:08.564\nHi, everyone!\n\n2\n00:00:08.914 --> 00:00:11.891\nWelcome!\n";
        let text = vtt_to_text(vtt);
        assert_eq!(text, "Hi, everyone!\nWelcome!");
        assert!(!text.contains("-->"));
    }
}