use crate::api::parser::{truncate_text, ChildQ, Module, OptionItem};
use crate::api::session::Session;
use crate::llm;
use anyhow::{bail, Result};

const SYSTEM_PROMPT: &str = "你是一个专业的英语教学助手，擅长分析英语题目。\
请根据题目要求给出准确答案，注意区分不同题型：\
词汇匹配题根据英文选中文或根据中文选英文；选词填空选择最合适的单词；阅读理解基于文章内容作答。";

/// 逐个模块求解，返回每个子题的作答值（按 children 顺序）。
pub async fn solve_module(session: &Session, m: &Module) -> Result<Vec<String>> {
    match m.reply_type.as_str() {
        "singlechoice" | "multichoice" => {
            let mut out = Vec::with_capacity(m.children.len());
            for c in &m.children {
                out.push(solve_child_choice(session, m, c).await?);
            }
            Ok(out)
        }
        "fillblank" | "text-area" => solve_batch(session, m).await,
        "bankedcloze" => solve_banked_cloze(session, m).await,
        other => {
            let mut out = Vec::with_capacity(m.children.len());
            for c in &m.children {
                out.push(solve_child_choice(session, m, c).await?);
            }
            if out.is_empty() {
                bail!("未知 replyType: {}", other);
            }
            Ok(out)
        }
    }
}

fn material_context(m: &Module) -> Option<String> {
    let mut parts = Vec::new();
    if !m.material.is_empty() {
        parts.push(truncate_text(&m.material, 4000));
    }
    if !m.transcript.is_empty() {
        parts.push(format!("【视频/音频字幕】\n{}", truncate_text(&m.transcript, 4000)));
    }
    if !m.direction.is_empty() {
        parts.push(format!("【答题说明】{}", m.direction));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// 若模块含媒体但缺文本，则本地转录并拼接为材料上下文。
async fn media_context(session: &Session, m: &Module) -> Option<String> {
    if m.media_sources.is_empty() {
        return None;
    }
    if !m.material.is_empty() || !m.transcript.is_empty() {
        // 已有文本内容可供回答，跳过转写。
        return None;
    }
    let mut texts: Vec<String> = Vec::new();
    for url in &m.media_sources {
        if let Ok(t) = crate::transcribe::transcribe_media(session, url).await {
            if !t.is_empty() {
                texts.push(t);
            }
        }
    }
    if texts.is_empty() {
        return None;
    }
    let joined = texts.join("\n\n");
    log::info!(
        "转录媒体 {} 条，文本 {} 字符",
        m.media_sources.len(),
        joined.chars().count()
    );
    Some(format!("【音频/视频转写内容】\n{}", truncate_text(&joined, 5000)))
}

async fn choice_prompt(session: &Session, m: &Module, c: &ChildQ, is_multi: bool) -> Result<String> {
    let mut lines: Vec<String> = Vec::new();
    if let Some(ctx) = material_context(m) {
        lines.push(ctx);
        lines.push(String::new());
    } else if let Some(ctx) = media_context(session, m).await {
        lines.push(ctx);
        lines.push(String::new());
    }
    let qtext = if c.question_text.is_empty() {
        "（题目见上方材料）".to_string()
    } else {
        c.question_text.clone()
    };
    lines.push(format!(
        "【{}】{}",
        if is_multi { "多选题" } else { "单选题" },
        qtext
    ));
    for opt in &c.options {
        let label = if opt.name.is_empty() {
            opt.value.clone()
        } else {
            opt.name.clone()
        };
        let txt = if opt.text.is_empty() {
            opt.value.clone()
        } else {
            opt.text.clone()
        };
        lines.push(format!("{}: {}", label, txt));
    }
    lines.push(String::new());
    if is_multi {
        lines.push("请只回答选项字母，多个用逗号分隔（如 A,B,C）。".to_string());
    } else {
        lines.push("请只回答一个选项字母（如 A）。".to_string());
    }
    Ok(lines.join("\n"))
}

async fn solve_child_choice(session: &Session, m: &Module, c: &ChildQ) -> Result<String> {
    if !session.cfg().use_llm() {
        return Ok(random_choice(c));
    }
    let is_multi = c.reply_type == "multichoice";
    let prompt = choice_prompt(session, m, c, is_multi).await?;
    match llm::ask(session, SYSTEM_PROMPT, &prompt).await {
        Ok(ans) => Ok(if is_multi {
            parse_multi(&ans, &c.options)
        } else {
            parse_single(&ans, &c.options)
        }),
        Err(e) => {
            if session.cfg().fallback_on_llm_failure {
                log::warn!("LLM 失败，随机作答: {:#}", e);
                Ok(random_choice(c))
            } else {
                Err(e)
            }
        }
    }
}

async fn solve_batch(session: &Session, m: &Module) -> Result<Vec<String>> {
    let count = m.children.len();
    if !session.cfg().use_llm() {
        return Ok(vec!["answer".to_string(); count]);
    }
    let mut lines: Vec<String> = Vec::new();
    if let Some(ctx) = material_context(m) {
        lines.push(ctx);
        lines.push(String::new());
    } else if let Some(ctx) = media_context(session, m).await {
        lines.push(ctx);
        lines.push(String::new());
    }
    lines.push(format!(
        "【{}题】共 {} 小题，请依次作答：",
        if m.reply_type == "fillblank" { "填空" } else { "简答" },
        count
    ));
    for (i, c) in m.children.iter().enumerate() {
        let q = if c.question_text.is_empty() {
            String::new()
        } else {
            c.question_text.clone()
        };
        lines.push(format!(
            "{}. {}",
            i + 1,
            if q.is_empty() {
                "(见上方材料)".to_string()
            } else {
                q
            }
        ));
    }
    lines.push(String::new());
    lines.push("请按题号回答，格式：1.答案 2.答案 ...".to_string());
    lines.push("如果不是翻译题，请只用英文回答。".to_string());
    let prompt = lines.join("\n");
    match llm::ask(session, SYSTEM_PROMPT, &prompt).await {
        Ok(ans) => Ok(parse_banked(&ans, count)),
        Err(e) => {
            if session.cfg().fallback_on_llm_failure {
                log::warn!("LLM 失败，填空兜底: {:#}", e);
                Ok(vec!["answer".to_string(); count])
            } else {
                Err(e)
            }
        }
    }
}

/// 选词填空：材料中带编号空格 ____n____，每空对应一个 child，词库来自选项。
/// 一次 LLM 调用按编号填空，答案值提交单词本身（与平台抓包一致）。
async fn solve_banked_cloze(session: &Session, m: &Module) -> Result<Vec<String>> {
    let count = m.children.len();
    let words = word_bank(m);
    if !session.cfg().use_llm() || words.is_empty() {
        return Ok(shuffled_words(&words, count));
    }
    let mut lines: Vec<String> = Vec::new();
    if let Some(ctx) = material_context(m) {
        lines.push(ctx);
        lines.push(String::new());
    } else if let Some(ctx) = media_context(session, m).await {
        lines.push(ctx);
        lines.push(String::new());
    }
    lines.push("【选词填空】短文中共有 {} 个编号空格（____1____、____2____…），词库如下：".replace("{}", &count.to_string()));
    lines.push(words.join("、"));
    lines.push(String::new());
    lines.push("请为每个编号空格选择最合适的单词，每个词限用一次。".to_string());
    lines.push("请按题号回答，格式：1.单词 2.单词 3.单词 ...（只输出单词本身）".to_string());
    let prompt = lines.join("\n");
    match llm::ask(session, SYSTEM_PROMPT, &prompt).await {
        Ok(ans) => Ok(canonicalize_banked(&parse_banked(&ans, count), &words, count)),
        Err(e) => {
            if session.cfg().fallback_on_llm_failure {
                log::warn!("LLM 失败，选词填空随机兜底: {:#}", e);
                Ok(shuffled_words(&words, count))
            } else {
                Err(e)
            }
        }
    }
}

/// 词库：按 children 顺序去重收集选项单词（value 优先，回退 name/text）。
fn word_bank(m: &Module) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in &m.children {
        for o in &c.options {
            let w = if o.value.is_empty() { o.name.clone() } else { o.value.clone() };
            let w = if w.is_empty() { o.text.clone() } else { w };
            if !w.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&w)) {
                out.push(w);
            }
        }
    }
    out
}

/// 兜底：把词库随机打散后按空序分配（不重复）。
fn shuffled_words(words: &[String], count: usize) -> Vec<String> {
    use rand::seq::SliceRandom;
    let mut pool = words.to_vec();
    pool.shuffle(&mut rand::thread_rng());
    (0..count).map(|i| pool.get(i).cloned().unwrap_or_default()).collect()
}

/// 规整解析出的答案：去尾部标点、忽略大小写归一到词库原词，缺位留空。
fn canonicalize_banked(parts: &[String], words: &[String], count: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let raw = parts.get(i).map(|s| s.as_str()).unwrap_or("");
        let clean = raw
            .trim()
            .trim_end_matches(['.', ',', '，', '。', '、', ';', '；'])
            .trim();
        let lower = clean.to_lowercase();
        let word = words
            .iter()
            .find(|w| w.to_lowercase() == lower)
            .cloned()
            .unwrap_or_else(|| clean.to_string());
        out.push(word);
    }
    out
}

fn valid_labels(options: &[OptionItem]) -> Vec<String> {
    options
        .iter()
        .map(|o| if o.name.is_empty() { o.value.clone() } else { o.name.clone() })
        .collect()
}

fn parse_single(ans: &str, options: &[OptionItem]) -> String {
    let labels = valid_labels(options);
    let upper = ans.to_uppercase();
    for lab in &labels {
        if lab.len() == 1 && upper.contains(lab.as_str()) {
            return lab.clone();
        }
    }
    upper
        .chars()
        .find(|c| c.is_ascii_uppercase())
        .map(|c| c.to_string())
        .unwrap_or_else(|| "A".to_string())
}

fn parse_multi(ans: &str, options: &[OptionItem]) -> String {
    let labels = valid_labels(options);
    let upper: String = ans.to_uppercase().chars().collect();
    let mut picked: Vec<String> = Vec::new();
    for lab in &labels {
        if lab.len() == 1 && upper.contains(lab.as_str()) {
            picked.push(lab.clone());
        }
    }
    if picked.is_empty() {
        return "A".to_string();
    }
    picked.join(",")
}

fn parse_banked(ans: &str, expected: usize) -> Vec<String> {
    let mut result = vec![String::new(); expected];
    let parts = split_numbered(ans);
    for (i, content) in parts.into_iter().enumerate() {
        if i < expected {
            result[i] = content;
        }
    }
    if result.iter().all(|r| r.is_empty()) {
        let lines: Vec<String> = ans
            .lines()
            .map(|l| strip_number_prefix(l.trim()))
            .filter(|l| !l.is_empty())
            .collect();
        for (i, line) in lines.into_iter().take(expected).enumerate() {
            result[i] = line;
        }
    }
    result
}

/// 解析 "1.xxx 2.yyy" 或 "1)、xxx" 这类编号答案，返回按序内容。
fn split_numbered(ans: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = ans.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < n && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < n && (bytes[i] == b'.' || bytes[i] == b')' || bytes[i] == b']' || bytes[i] == b'>') {
                let _ = start;
                i += 1;
                while i < n && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                let val_start = i;
                while i < n {
                    if bytes[i].is_ascii_digit() {
                        let j = i;
                        let mut j2 = j;
                        while j2 < n && bytes[j2].is_ascii_digit() {
                            j2 += 1;
                        }
                        if j2 < n && (bytes[j2] == b'.' || bytes[j2] == b')' || bytes[j2] == b']') {
                            break;
                        }
                    }
                    i += 1;
                }
                let val = ans[val_start..i].trim().to_string();
                if !val.is_empty() {
                    out.push(val);
                }
                continue;
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    if out.is_empty() && !ans.trim().is_empty() {
        out.push(ans.trim().to_string());
    }
    out
}

fn strip_number_prefix(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')' || bytes[i] == b']') {
        s[i + 1..].trim().to_string()
    } else {
        s.trim().to_string()
    }
}

fn random_choice(c: &ChildQ) -> String {
    use rand::Rng;
    match c.reply_type.as_str() {
        "singlechoice" | "multichoice" if c.option_count > 0 => {
            let idx = rand::thread_rng().gen_range(0..c.option_count);
            ((b'A' + idx as u8) as char).to_string()
        }
        _ => "answer".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_works() {
        let opts = vec![
            OptionItem { name: "A".into(), value: "A".into(), text: "x".into() },
            OptionItem { name: "B".into(), value: "B".into(), text: "y".into() },
        ];
        assert_eq!(parse_single("1. B 正确答案是B", &opts), "B");
        assert_eq!(parse_multi("答案是 A、B、D", &opts), "A,B");
    }

    #[test]
    fn parse_banked_order() {
        assert_eq!(split_numbered("1. apple 2. banana"), vec!["apple", "banana"]);
        assert_eq!(parse_banked("1. one 2. two", 2), vec!["one", "two"]);
    }

    #[test]
    fn word_bank_dedupes_options() {
        let opts = |w: &str| OptionItem {
            name: w.into(),
            value: w.into(),
            text: String::new(),
        };
        let m = Module {
            instance_id: "m".into(),
            module_type: "material-banked-cloze".into(),
            direction: String::new(),
            material: String::new(),
            media_sources: Vec::new(),
            transcript: String::new(),
            reply_type: "bankedcloze".into(),
            children: vec![
                ChildQ {
                    question_type: "material-banked-cloze".into(),
                    reply_type: "bankedcloze".into(),
                    question_text: String::new(),
                    options: vec![opts("unique"), opts("strong"), opts("high")],
                    option_count: 3,
                },
                ChildQ {
                    question_type: "material-banked-cloze".into(),
                    reply_type: "bankedcloze".into(),
                    question_text: String::new(),
                    options: vec![opts("Unique"), opts("collective"), opts("strong")],
                    option_count: 3,
                },
            ],
        };
        assert_eq!(word_bank(&m), vec!["unique", "strong", "high", "collective"]);
    }

    #[test]
    fn banked_canonicalize_matches_bank() {
        let words = vec!["unique".to_string(), "collective".to_string(), "high".to_string()];
        let parts = split_numbered("1. Unique 2. Collective 3. HIGH");
        assert_eq!(
            canonicalize_banked(&parts, &words, 3),
            vec!["unique", "collective", "high"]
        );
        let partial = split_numbered("1. unique");
        assert_eq!(canonicalize_banked(&partial, &words, 3), vec!["unique", "", ""]);
    }

    #[test]
    fn shuffled_words_use_each_once() {
        let words = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut all = shuffled_words(&words, 3);
        all.sort();
        assert_eq!(all, vec!["a", "b", "c"]);
        let padded = shuffled_words(&words, 5);
        assert_eq!(padded.iter().filter(|s| s.is_empty()).count(), 2);
    }
}