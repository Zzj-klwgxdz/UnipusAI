use anyhow::Result;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct OptionItem {
    pub name: String,
    pub value: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ChildQ {
    /// 子题 question_type，对应 content 中 child 的 "type"（如 basic）
    pub question_type: String,
    /// 子题 reply_type，对应 content 中 child 的 "replyType"（如 singlechoice）
    pub reply_type: String,
    /// 题干（HTML 去标签后的纯文本）
    pub question_text: String,
    /// 选项（name/value/text），选择题使用
    pub options: Vec<OptionItem>,
    /// 选项个数
    pub option_count: usize,
}

#[derive(Debug, Clone)]
pub struct Module {
    /// 模块 id，即提交方的 instanceId
    pub instance_id: String,
    /// 模块内层 content 的 "type"（如 video-popup / basic）
    pub module_type: String,
    /// 模块级 direction 说明（题干/答题要求）
    pub direction: String,
    /// 模块级 contents 拼接后的阅读/听力材料文本
    pub material: String,
    /// 模块级 replyType
    pub reply_type: String,
    /// 模块内音频/视频源 URL（无内嵌文本时需要转写）
    pub media_sources: Vec<String>,
    /// 内嵌字幕文本（contents[].text，如 WEBVTT）
    pub transcript: String,
    pub children: Vec<ChildQ>,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedGroup {
    pub modules: Vec<Module>,
}

fn get_str(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 去掉 HTML 标签并限制行数（用于题干等短文本）。
pub fn strip_html_short(s: &str) -> String {
    let cleaned = strip_html(s);
    let mut lines: Vec<&str> = cleaned.lines().collect();
    lines.truncate(30);
    lines.join("\n")
}

fn direction_text(v: &Value) -> String {
    let dir = v.get("direction");
    if let Some(d) = dir {
        if let Some(s) = d.get("text").and_then(|x| x.as_str()) {
            return strip_html(s);
        }
        if let Some(s) = d.get("pcText").and_then(|x| x.as_str()) {
            return strip_html(s);
        }
    }
    String::new()
}

fn is_media_url(p: &str) -> bool {
    let lower = p.to_ascii_lowercase();
    lower.contains(".mp3")
        || lower.contains(".mp4")
        || lower.contains(".m4a")
        || lower.contains(".wav")
        || lower.contains(".aac")
        || lower.contains(".ogg")
        || lower.contains(".flac")
        || lower.contains("audio/")
        || lower.contains("video/")
}

pub fn clean_url(p: &str) -> String {
    p.split('#').next().unwrap_or(p).trim().to_string()
}

fn collect_material(content: &Value) -> (String, Vec<String>, String) {
    let mut parts: Vec<String> = Vec::new();
    let mut media: Vec<String> = Vec::new();
    let mut transcript: Vec<String> = Vec::new();
    if let Some(arr) = content.get("contents").and_then(|c| c.as_array()) {
        for item in arr {
            let text = item.get("text").and_then(|x| x.as_str()).unwrap_or("");
            let clean = strip_html(text);
            if !clean.is_empty() {
                // WEBVTT 字幕即为转写文本；普通物体文本为阅读/听力材料。
                if text.trim_start().starts_with("WEBVTT") {
                    transcript.push(clean);
                } else {
                    parts.push(clean);
                }
            }
            if let Some(p) = item.get("path").and_then(|x| x.as_str()) {
                if is_media_url(p) {
                    media.push(clean_url(p));
                }
            }
            // 字幕轨道里的 vtt 也算媒体来源（可下载解析）。
            if let Some(subs) = item.get("subtitles").and_then(|s| s.as_array()) {
                for s in subs {
                    if let Some(p) = s.get("path").and_then(|x| x.as_str()) {
                        if !p.is_empty() {
                            media.push(clean_url(p));
                        }
                    }
                }
            }
        }
    }
    // 去重保序
    let mut seen = std::collections::HashSet::new();
    media.retain(|m| seen.insert(m.clone()));
    (
        parts.join("\n\n"),
        media,
        transcript.join("\n\n"),
    )
}

/// 从解密后的 content 解析题目模块。
/// 解密结果为数组：每个元素的 "content" 字段是 JSON 字符串，
/// 包含 "type" 与 "children"（真正的子题列表）。
pub fn parse_group(decrypted: &Value) -> Result<ParsedGroup> {
    let mut out = ParsedGroup::default();

    let modules: Vec<&Value> = if let Some(arr) = decrypted.as_array() {
        arr.iter().collect()
    } else {
        vec![decrypted]
    };

    for module in modules {
        let instance_id = get_str(module, &["id"])
            .or_else(|| module.get("id").and_then(|x| x.as_i64()).map(|x| x.to_string()))
            .unwrap_or_default();
        if instance_id.is_empty() {
            continue;
        }
        let content = module
            .get("content")
            .and_then(|c| c.as_str())
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .or_else(|| module.get("content").and_then(|c| c.as_object()).cloned().map(Value::Object))
            .clone();
        let Some(content) = content else {
            continue;
        };
        let module_type = get_str(&content, &["type"]).unwrap_or_default();
        let reply_type = get_str(&content, &["replyType"]).unwrap_or_default();
        let direction = direction_text(&content);
        let (material, media_sources, transcript) = collect_material(&content);
        let children = content
            .get("children")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();

        let mut child_qs = Vec::new();
        for child in children {
            let question_type = get_str(&child, &["type"]).unwrap_or_else(|| module_type.clone());
            let reply_type = get_str(&child, &["replyType"]).unwrap_or_else(|| "text-area".to_string());
            let question_text =
                get_str(&child, &["quesText", "text"]).map(|s| strip_html_short(&s)).unwrap_or_default();
            let option_items: Vec<Value> = child
                .get("options")
                .and_then(|o| o.as_array())
                .cloned()
                .unwrap_or_default();
            let options_len = option_items.len();
            let options = option_items
                .iter()
                .map(|o| OptionItem {
                    name: get_str(o, &["name", "value"]).unwrap_or_default(),
                    value: get_str(o, &["value", "name"]).unwrap_or_default(),
                    text: get_str(o, &["text"]).map(|s| strip_html(&s)).unwrap_or_default(),
                })
                .collect();
            child_qs.push(ChildQ {
                question_type,
                reply_type,
                question_text,
                options,
                option_count: options_len,
            });
        }

        out.modules.push(Module {
            instance_id,
            module_type,
            direction,
            material,
            media_sources,
            transcript,
            reply_type,
            children: child_qs,
        });
    }

    if out.modules.is_empty() {
        return Err(anyhow::anyhow!("未解析到任何模块，content 结构未知"));
    }
    Ok(out)
}

pub fn question_count(group: &ParsedGroup) -> usize {
    group.modules.iter().map(|m| m.children.len()).sum()
}

pub fn truncate_text(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut r: String = s.chars().take(n).collect();
        r.push('…');
        r
    }
}

/// 从解密后的组内容提取可读标签（用于显示单元/任务名）。
/// 优先取模块 contents[] 中媒体项的 `name`（如 "U1 Pre-reading activities.mp3"，去扩展名），
/// 无则回退到模块 direction 文本首行（截断 40 字），再无则返回空字符串。
pub fn extract_group_label(decrypted: &Value) -> String {
    let modules: Vec<&Value> = if let Some(arr) = decrypted.as_array() {
        arr.iter().collect()
    } else {
        vec![decrypted]
    };
    let mut direction = String::new();
    for module in modules {
        let content = module
            .get("content")
            .and_then(|c| c.as_str())
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .or_else(|| module.get("content").and_then(|c| c.as_object()).cloned().map(Value::Object));
        let Some(content) = content else { continue };
        if let Some(arr) = content.get("contents").and_then(|c| c.as_array()) {
            for item in arr {
                if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                    let name = name.trim();
                    if !name.is_empty() && !looks_like_uuid(name) {
                        return strip_media_ext(name).to_string();
                    }
                }
            }
        }
        if direction.is_empty() {
            direction = direction_text(&content);
        }
    }
    if !direction.is_empty() {
        return truncate_text(&direction, 40);
    }
    String::new()
}

fn looks_like_uuid(s: &str) -> bool {
    s.contains('-') && s.chars().all(|c| c.is_alphanumeric() || c == '-')
}

fn strip_media_ext(s: &str) -> &str {
    for ext in [".mp3", ".mp4", ".m4a", ".wav", ".aac", ".ogg", ".flac"] {
        if let Some(rest) = s.strip_suffix(ext) {
            return rest.trim_end();
        }
    }
    s
}

pub fn build_context() -> String {
    "{\"state\":\"submitted\"}".to_string()
}

impl ParsedGroup {
    pub fn child_count(&self) -> usize {
        self.modules.iter().map(|m| m.children.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_url_detect() {
        assert!(is_media_url("https://cdn/x/audio.mp3"));
        assert!(is_media_url("https://cdn/x/a.m4a"));
        assert!(is_media_url("https://cdn/x/v.mp4#duration=10"));
        assert!(!is_media_url("https://cdn/x/a.jpg"));
    }

    #[test]
    fn clean_url_drops_fragment() {
        assert_eq!(
            clean_url("https://cdn/x/v.mp4#duration=100&size=1"),
            "https://cdn/x/v.mp4"
        );
    }

    #[test]
    fn strip_html_no_truncation() {
        let input: String = (0..50).map(|i| format!("line{}\n", i)).collect();
        let out = strip_html(&input);
        assert_eq!(out.lines().count(), 50);
        let short = strip_html_short(&input);
        assert_eq!(short.lines().count(), 30);
    }

    #[test]
    fn group_label_prefers_media_name() {
        let json = r#"[{"id":1,"content":"{\"type\":\"x\",\"contents\":[{\"id\":\"uuid-1\",\"text\":\"q\"},{\"path\":\"http://c/a.mp3\",\"name\":\"U1 Pre-reading activities.mp3\"}]}"}]"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(extract_group_label(&v), "U1 Pre-reading activities");
    }

    #[test]
    fn group_label_falls_back_to_direction() {
        let json = r#"[{"id":1,"content":"{\"type\":\"x\",\"direction\":{\"text\":\"Listen and fill in the blanks.\"},\"contents\":[{\"id\":\"uuid-1\",\"text\":\"q\"}]}"}]"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(extract_group_label(&v), "Listen and fill in the blanks.");
    }

    #[test]
    fn group_label_empty_when_no_hint() {
        let json = r#"[{"id":1,"content":"{\"type\":\"x\",\"contents\":[{\"id\":\"uuid-1\",\"text\":\"q\"}]}"}]"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(extract_group_label(&v), "");
    }
}