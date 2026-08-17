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
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
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
    std::fs::write(dest, &bytes).with_context(|| format!("保存媒体失败: {}", dest.display()))?;
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

/// 用纯 Rust whisper（whisper-candle-core）转写 wav。
/// language 为 "auto" / 空时传 None，让模型自动检测语种。
fn whisper_infer(wav: &PathBuf, model: &str, language: &str) -> Result<String> {
    use std::sync::{Mutex, OnceLock};

    static CACHE: OnceLock<Mutex<Option<(String, whisper_core::WhisperModel)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));

    let device = whisper_core::device("cpu").context("初始化 whisper 设备失败")?;
    let lang_opt = {
        let lang = language.trim();
        if lang.is_empty() || lang.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(lang.to_string())
        }
    };

    let mut guard = cache.lock().unwrap();
    if guard.as_ref().map(|(m, _)| m != model).unwrap_or(true) {
        log::info!("加载 whisper 模型: {model}");
        let loaded = load_whisper_model(model, &device)
            .with_context(|| format!("加载 whisper 模型 {model} 失败"))?;
        *guard = Some((model.to_string(), loaded));
    }
    let (_, wp_model) = guard.as_mut().unwrap();

    let mut options = whisper_core::TranscribeOptions::default();
    options.decode_options.language = lang_opt;
    options.verbose = Some(false);
    let result =
        whisper_core::transcribe_file(wp_model, wav, &options).context("whisper 转录失败")?;
    Ok(result.text.trim().to_string())
}

/// 从 HuggingFace Hub 下载/复用 whisper 模型并加载。
/// 走 `HF_ENDPOINT`（如国内镜像 hf-mirror.com），默认官方 huggingface.co。
/// 模型文件缓存到 `~/.cache/whisper-candle/`（HF_HOME 可覆盖）。
fn load_whisper_model(
    model: &str,
    device: &candle_core::Device,
) -> Result<whisper_core::WhisperModel> {
    use std::io::Read;

    let which: whisper_core::WhichModel = model
        .parse()
        .with_context(|| format!("未知 whisper 模型: {model}"))?;
    let repo = which.hf_repo();
    let endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());

    let cache_root = std::env::var_os("HF_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CACHE_HOME").map(|p| PathBuf::from(p).join("huggingface"))
        })
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|p| PathBuf::from(p).join(".cache").join("huggingface"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache").join("huggingface"))
        })
        .unwrap_or_else(|| PathBuf::from(".whisper_cache"));
    let model_dir = cache_root
        .join("whisper-candle")
        .join(repo.replace('/', "__"));
    std::fs::create_dir_all(&model_dir).ok();

    let fetch = |filename: &str| -> Result<Option<PathBuf>> {
        let dest = model_dir.join(filename);
        if dest.is_file() && dest.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
            return Ok(Some(dest));
        }
        let url = format!("{}/{}/resolve/main/{}", endpoint, repo, filename);
        log::info!("下载 whisper 模型文件: {url}");
        let resp = ureq::get(&url)
            .call()
            .with_context(|| format!("下载 {} 失败", url))?;
        let mut bytes: Vec<u8> = Vec::new();
        resp.into_reader()
            .read_to_end(&mut bytes)
            .context("读取模型响应失败")?;
        if bytes.is_empty() {
            return Ok(None);
        }
        std::fs::write(&dest, &bytes)
            .with_context(|| format!("保存模型文件失败: {}", dest.display()))?;
        Ok(Some(dest))
    };

    let config = fetch("config.json")?.ok_or_else(|| anyhow::anyhow!("下载 config.json 为空"))?;
    let weights = fetch("model.safetensors")?
        .ok_or_else(|| anyhow::anyhow!("下载 model.safetensors 为空"))?;
    let generation_config = fetch("generation_config.json")?;

    let mut loaded = whisper_core::WhisperModel::load(&config, &weights, device)?;
    if let Some(gc) = generation_config {
        loaded.set_alignment_heads_from_file(&gc)?;
    }
    Ok(loaded)
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
    let text = whisper_infer(&wav, &cfg.whisper_model, &cfg.whisper_language)?;
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
