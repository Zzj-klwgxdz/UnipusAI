use crate::api::course::GroupTask;
use crate::api::parser::{Module, OptionItem, ParsedGroup, truncate_text};
use crate::api::session::{Session, content_url, submit_url};
use crate::api::submit::{ModuleAnsw, build_answer_payload, empty_answers};
use crate::solve::is_known_reply_type;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// 未适配题型报告目录（相对工作目录）。
pub const REPORT_DIR: &str = "reports";
/// ucontent 前端入口（用于人工复现题目）。
const PAGE_URL: &str = "https://ucontent.unipus.cn/_explorationpc_default/pc.html";
/// 报告中单段 JSON 的最大字符数。
const MAX_JSON_CHARS: usize = 120_000;
/// bundle 检索最多返回的片段数。
const MAX_BUNDLE_HITS: usize = 5;

/// 检测到的未适配题型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unadapted {
    /// 用于文件名/标题的题型名。
    pub type_name: String,
    /// 详细原因（写入报告与日志）。
    pub reason: String,
}

/// 判定组内是否存在未适配题型：只看"需要作答"的模块（有子题）。
/// - 模块 reply_type 非空且不在已知表 → 未适配
/// - 模块 reply_type 为空（容器，如 video-popup/exit-ticket）→ 子题类型全部已知才算适配
pub fn detect(group: &ParsedGroup) -> Option<Unadapted> {
    for m in &group.modules {
        if m.children.is_empty() {
            continue;
        }
        if !m.reply_type.is_empty() && !is_known_reply_type(&m.reply_type) {
            return Some(Unadapted {
                type_name: m.reply_type.clone(),
                reason: format!(
                    "模块 {} 的 reply_type={} 未适配（module_type={}，子题类型: {}）",
                    m.instance_id,
                    m.reply_type,
                    m.module_type,
                    child_types(m)
                ),
            });
        }
        for c in &m.children {
            if !is_known_reply_type(&c.reply_type) {
                let name = if c.reply_type.is_empty() {
                    c.question_type.clone()
                } else {
                    c.reply_type.clone()
                };
                return Some(Unadapted {
                    type_name: name.clone(),
                    reason: format!(
                        "模块 {}（reply_type={}）的子题 reply_type={} 未适配",
                        m.instance_id,
                        if m.reply_type.is_empty() {
                            "<空>"
                        } else {
                            m.reply_type.as_str()
                        },
                        if name.is_empty() { "<空>" } else { name.as_str() }
                    ),
                });
            }
        }
    }
    None
}

fn child_types(m: &Module) -> String {
    let mut seen = BTreeSet::new();
    for c in &m.children {
        seen.insert(if c.reply_type.is_empty() {
            "<空>"
        } else {
            c.reply_type.as_str()
        });
    }
    seen.into_iter().collect::<Vec<_>>().join("/")
}

/// 报告头信息。
#[derive(Debug, Clone, Default)]
pub struct ReportMeta {
    pub group_id: String,
    pub unit_id: String,
    pub tab_type: String,
    pub group_type: String,
    pub required: bool,
    pub passed: bool,
    /// 触发方式（自动检测 / 手动 probe / 批量扫描）。
    pub trigger: String,
    /// 检测原因或手动触发的说明。
    pub reason: String,
}

/// 报告中的账号/课程环境（与 Session 解耦，便于单测）。
#[derive(Debug, Clone, Default)]
pub struct ReportEnv {
    pub course_id: String,
    pub class_id: String,
    pub curricula_id: String,
    pub publish_version: String,
    pub open_id: String,
}

impl ReportEnv {
    pub fn from_session(session: &Session) -> Self {
        let cfg = session.cfg();
        Self {
            course_id: session.course_id().to_string(),
            class_id: cfg.class_id.clone(),
            curricula_id: cfg.curricula_id.clone(),
            publish_version: cfg.publish_version.clone(),
            open_id: session.open_id().to_string(),
        }
    }
}

/// 报告文件名：`reports/<题型>__<group_id>.md`（非法字符替换为 `_`，按组覆盖）。
pub fn report_path(type_name: &str, group_id: &str) -> PathBuf {
    Path::new(REPORT_DIR).join(format!(
        "{}__{}.md",
        sanitize(type_name),
        sanitize(group_id)
    ))
}

fn sanitize(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let t = out.trim_matches('_');
    if t.is_empty() {
        "unknown".to_string()
    } else {
        t.to_string()
    }
}

/// 用空答案构造载荷（自动报告/干跑用；只展示结构，不提交）。
pub fn empty_payload(session: &Session, group_id: &str, group: &ParsedGroup) -> Option<Value> {
    payload_with_answers(session, group_id, &empty_answers(group))
}

/// 用给定作答构造提交载荷 JSON。
pub fn payload_with_answers(
    session: &Session,
    group_id: &str,
    modules: &[ModuleAnsw],
) -> Option<Value> {
    let s = build_answer_payload(session, group_id, modules).ok()?;
    serde_json::from_str(&s).ok()
}

/// 未适配类型无法正常求解：用现有通用兜底（逐子题选择/占位）生成答案，
/// 仅供 `probe --submit` 探测提交接口使用。
pub async fn fallback_answers(session: &Session, group: &ParsedGroup) -> Vec<ModuleAnsw> {
    let mut modules = empty_answers(group);
    for (mi, m) in group.modules.iter().enumerate() {
        if m.children.is_empty() || m.reply_type == "discussion" {
            continue;
        }
        match crate::solve::solve_module(session, m).await {
            Ok(values) => {
                for (ci, v) in values.into_iter().enumerate() {
                    if let Some(c) = modules[mi].children.get_mut(ci) {
                        c.value = v;
                    }
                }
            }
            Err(e) => log::warn!("probe 求解模块 {} 失败（保留空答案）: {:#}", m.instance_id, e),
        }
    }
    let discussion: BTreeSet<&str> = group
        .modules
        .iter()
        .filter(|m| m.reply_type == "discussion")
        .map(|m| m.instance_id.as_str())
        .collect();
    modules.retain(|m| !discussion.contains(m.instance_id.as_str()));
    modules
}

/// 自动检测路径：写一份 dry-run 报告（不含提交响应/bundle 检索）。
pub fn report_auto(
    session: &Session,
    task: &GroupTask,
    un: &Unadapted,
    decrypted: &Value,
    group: &ParsedGroup,
) -> Result<PathBuf> {
    let meta = ReportMeta {
        group_id: task.group_id.clone(),
        unit_id: task.unit_id.clone(),
        tab_type: task.tab_type.clone(),
        group_type: un.type_name.clone(),
        required: task.required,
        passed: task.passed,
        trigger: "自动检测（run/group/TUI，已跳过提交）".to_string(),
        reason: un.reason.clone(),
    };
    let payload = empty_payload(session, &task.group_id, group);
    write_report(
        session,
        &meta,
        decrypted,
        Some(group),
        payload.as_ref(),
        None,
        &[],
    )
}

/// 渲染并写入报告。
#[allow(clippy::too_many_arguments)]
pub fn write_report(
    session: &Session,
    meta: &ReportMeta,
    decrypted: &Value,
    group: Option<&ParsedGroup>,
    payload: Option<&Value>,
    submit_response: Option<&Value>,
    bundle_hits: &[(String, String)],
) -> Result<PathBuf> {
    let env = ReportEnv::from_session(session);
    let text = render_report(&env, meta, decrypted, group, payload, submit_response, bundle_hits);
    let path = report_path(&meta.group_type, &meta.group_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建报告目录 {} 失败", parent.display()))?;
    }
    std::fs::write(&path, text).with_context(|| format!("写入报告 {} 失败", path.display()))?;
    Ok(path)
}

/// 渲染报告正文（纯函数，便于单测）。
pub fn render_report(
    env: &ReportEnv,
    meta: &ReportMeta,
    decrypted: &Value,
    group: Option<&ParsedGroup>,
    payload: Option<&Value>,
    submit_response: Option<&Value>,
    bundle_hits: &[(String, String)],
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# 未适配题型探针报告\n");
    let _ = writeln!(out, "> 本报告由 UnipusAI 自动生成，用于题型适配；不含 cookie/token 等凭证。");
    let _ = writeln!(
        out,
        "> 生成时间: {} | 程序版本: {} | 触发方式: {}\n",
        crate::db::now_local_string(),
        env!("CARGO_PKG_VERSION"),
        meta.trigger
    );

    let _ = writeln!(out, "## 1. 位置与状态\n");
    let _ = writeln!(out, "| 项 | 值 |");
    let _ = writeln!(out, "| --- | --- |");
    let _ = writeln!(out, "| course_id | `{}` |", env.course_id);
    let _ = writeln!(out, "| class_id | `{}` |", env.class_id);
    let _ = writeln!(out, "| curricula_id | `{}` |", env.curricula_id);
    let _ = writeln!(out, "| publish_version | `{}` |", env.publish_version);
    let _ = writeln!(out, "| open_id | `{}` |", env.open_id);
    let _ = writeln!(out, "| unit_id | `{}` |", meta.unit_id);
    let _ = writeln!(out, "| group_id | `{}` |", meta.group_id);
    let _ = writeln!(out, "| tab_type | `{}` |", meta.tab_type);
    let _ = writeln!(out, "| 题型（DB/检测） | `{}` |", meta.group_type);
    let _ = writeln!(
        out,
        "| 必修 / 已通过 | {} / {} |",
        yes_no(meta.required),
        yes_no(meta.passed)
    );
    let _ = writeln!(
        out,
        "| content 接口 | `{}` |",
        content_url(&env.course_id, &meta.group_id)
    );
    let _ = writeln!(out, "| 前端页面 | {}\n", PAGE_URL);

    let _ = writeln!(out, "## 2. 判定结果\n");
    let _ = writeln!(out, "- {}。", meta.reason);
    if meta.trigger.contains("自动") {
        let _ = writeln!(out, "- 运行策略: 未适配题型只报告并跳过，不调用 LLM、不提交。");
    } else {
        let _ = writeln!(out, "- 运行策略: 手动 probe，不影响任务状态。");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## 3. 模块/子题概览\n");
    match group {
        Some(g) => {
            let _ = writeln!(out, "| # | instance_id | module_type | reply_type | 子题 | 模块词库 | 媒体 | 字幕 |");
            let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- | --- | --- |");
            for (i, m) in g.modules.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "| {} | `{}` | {} | {} | {} | {} | {} | {} |",
                    i + 1,
                    m.instance_id,
                    empty_dash(&m.module_type),
                    empty_dash(&m.reply_type),
                    m.children.len(),
                    m.word_bank.len(),
                    m.media_sources.len(),
                    if m.transcript.is_empty() { "-" } else { "有" }
                );
            }
            let _ = writeln!(out);
            for (i, m) in g.modules.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "### 模块 {}（instance_id=`{}`）\n",
                    i + 1,
                    m.instance_id
                );
                if !m.direction.is_empty() {
                    let _ = writeln!(out, "- 答题说明: {}", truncate_text(&one_line(&m.direction), 300));
                }
                if !m.material.is_empty() {
                    let _ = writeln!(
                        out,
                        "- 材料（前 500 字）: {}",
                        truncate_text(&one_line(&m.material), 500)
                    );
                }
                if !m.media_sources.is_empty() {
                    let _ = writeln!(out, "- 媒体: {}", m.media_sources.join(", "));
                }
                let _ = writeln!(out, "- 子题:");
                for (ci, c) in m.children.iter().enumerate() {
                    let _ = writeln!(
                        out,
                        "  {}. reply_type={} question_type={} 题干={} 选项={}",
                        ci + 1,
                        empty_dash(&c.reply_type),
                        empty_dash(&c.question_type),
                        if c.question_text.is_empty() {
                            "（空）".to_string()
                        } else {
                            truncate_text(&one_line(&c.question_text), 160)
                        },
                        options_text(&c.options)
                    );
                }
                let _ = writeln!(out);
            }
        }
        None => {
            let _ = writeln!(out, "- 内容无题目模块（浏览类页面或解密后非 JSON）。\n");
        }
    }

    let (module_keys, child_keys) = ignored_keys(decrypted);
    let _ = writeln!(out, "## 4. 解析器未消费字段（适配线索）\n");
    let _ = writeln!(out, "- 模块级/模块内容未消费键: {}", keys_text(&module_keys));
    let _ = writeln!(out, "- 子题未消费键: {}", keys_text(&child_keys));
    let _ = writeln!(
        out,
        "\n解析器当前消费：模块 `id`/`content`（`type`/`replyType`/`direction`/`contents`/`options`/`children`）；\
         子题 `type`/`replyType`/`quesText`/`text`/`options`。未列出的字段如含答案/规则/编号信息，请重点核对。\n"
    );

    let _ = writeln!(out, "## 5. 提交路径与载荷\n");
    let _ = writeln!(out, "- 提交接口: `{}`（POST JSON）", submit_url());
    let answer_submit = group
        .map(|g| {
            g.modules
                .iter()
                .any(|m| !m.children.is_empty() && m.reply_type != "discussion")
        })
        .unwrap_or(false);
    let _ = writeln!(
        out,
        "- 程序判定路径: {}",
        if answer_submit {
            "作答提交（submitType=1）"
        } else {
            "标记已看（submitType=2）"
        }
    );
    match payload {
        Some(p) => {
            let note = if submit_response.is_some() {
                "实际提交载荷（答案由通用兜底生成，可能不正确）"
            } else {
                "空答案占位，仅展示结构（未提交）"
            };
            let _ = writeln!(out, "- 载荷说明: {}\n", note);
            let _ = writeln!(out, "```json\n{}\n```\n", cap_json(p));
            if let Some(j) = judges_preview(p) {
                let _ = writeln!(
                    out,
                    "thirdPartyJudges（最多 2 条）:\n\n```json\n{}\n```\n",
                    cap_json(&j)
                );
            }
        }
        None => {
            let _ = writeln!(out, "- 无题目模块，未构造提交载荷。\n");
        }
    }

    let _ = writeln!(out, "## 6. 服务端响应（--submit）\n");
    match submit_response {
        Some(r) => {
            let _ = writeln!(out, "```json\n{}\n```\n", cap_json(r));
        }
        None => {
            let _ = writeln!(out, "- 未启用 --submit（干跑，无响应）。\n");
        }
    }

    let _ = writeln!(out, "## 7. ucontent 前端 bundle 检索（--bundle）\n");
    if bundle_hits.is_empty() {
        let _ = writeln!(out, "- 未启用 --bundle，或未在 bundle 中找到该题型名。\n");
    } else {
        for (file, snip) in bundle_hits {
            let _ = writeln!(out, "### {}\n\n```js\n{}\n```\n", file, snip);
        }
    }

    let _ = writeln!(out, "## 8. 适配检查清单\n");
    let _ = writeln!(out, "- [ ] 子题 value 应提交选项标签（A/B）还是选项文本（do/make）？");
    let _ = writeln!(out, "- [ ] children 顺序是否与材料空位编号（`____n____`）一致？是否需要提交 child id？");
    let _ = writeln!(out, "- [ ] 是否需要额外接口（文件上传/录音/BBS 发帖等）才能完成？");
    let _ = writeln!(out, "- [ ] submitType 应使用 1（作答）还是 2（标记已看）？");
    let _ = writeln!(out, "- [ ] 服务端响应中的 score 数组长度/含义？是否按小题判分？");
    let _ = writeln!(out, "- [ ] 是否需要媒体转写/图片 OCR/词库等额外上下文？\n");

    let _ = writeln!(out, "## 9. 完整解密 JSON\n");
    let _ = writeln!(out, "```json\n{}\n```", cap_json(decrypted));
    out
}

fn yes_no(v: bool) -> &'static str {
    if v { "是" } else { "否" }
}

fn empty_dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn keys_text(keys: &[String]) -> String {
    if keys.is_empty() {
        "（无）".to_string()
    } else {
        keys.iter()
            .map(|k| format!("`{}`", k))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn options_text(options: &[OptionItem]) -> String {
    if options.is_empty() {
        return "（无）".to_string();
    }
    options
        .iter()
        .map(|o| {
            let label = if o.name.is_empty() { &o.value } else { &o.name };
            let text = if o.text.is_empty() { &o.value } else { &o.text };
            if label.is_empty() {
                text.clone()
            } else if text.is_empty() {
                label.clone()
            } else {
                format!("{}={}", label, text)
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn cap_json(v: &Value) -> String {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|e| format!("<序列化失败: {}>", e));
    if text.chars().count() <= MAX_JSON_CHARS {
        text
    } else {
        let mut t: String = text.chars().take(MAX_JSON_CHARS).collect();
        t.push_str("\n…（已截断）");
        t
    }
}

fn judges_preview(payload: &Value) -> Option<Value> {
    let s = payload.get("thirdPartyJudges")?.as_str()?;
    if s.trim().is_empty() || s.trim() == "[]" {
        return None;
    }
    let mut v: Value = serde_json::from_str(s).ok()?;
    if let Some(arr) = v.as_array_mut() {
        arr.truncate(2);
    }
    Some(v)
}

/// 统计解析器尚未消费的字段名（模块级 + 子题级），用于提示适配线索。
fn ignored_keys(decrypted: &Value) -> (Vec<String>, Vec<String>) {
    const MODULE_TOP_CONSUMED: &[&str] = &["id", "content"];
    const MODULE_CONTENT_CONSUMED: &[&str] = &[
        "type",
        "replyType",
        "direction",
        "contents",
        "options",
        "children",
    ];
    const CHILD_CONSUMED: &[&str] = &["type", "replyType", "quesText", "text", "options"];

    let modules: Vec<&Value> = match decrypted.as_array() {
        Some(arr) => arr.iter().collect(),
        None => vec![decrypted],
    };
    let mut module_keys = BTreeSet::new();
    let mut child_keys = BTreeSet::new();
    for module in modules {
        if let Some(obj) = module.as_object() {
            for k in obj.keys() {
                if !MODULE_TOP_CONSUMED.contains(&k.as_str()) {
                    module_keys.insert(k.clone());
                }
            }
            let content = module
                .get("content")
                .and_then(|c| {
                    c.as_str()
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                        .or_else(|| c.as_object().cloned().map(Value::Object))
                })
                .unwrap_or(Value::Null);
            if let Some(obj) = content.as_object() {
                for k in obj.keys() {
                    if !MODULE_CONTENT_CONSUMED.contains(&k.as_str()) {
                        module_keys.insert(format!("content.{}", k));
                    }
                }
                if let Some(children) = obj.get("children").and_then(|c| c.as_array()) {
                    for child in children {
                        if let Some(co) = child.as_object() {
                            for k in co.keys() {
                                if !CHILD_CONSUMED.contains(&k.as_str()) {
                                    child_keys.insert(k.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    (
        module_keys.into_iter().collect(),
        child_keys.into_iter().collect(),
    )
}

/// 抓取 ucontent 前端 bundle，检索题型名出现的片段（最多 5 处）。
pub async fn bundle_hits(type_name: &str) -> Vec<(String, String)> {
    if type_name.is_empty() {
        return Vec::new();
    }
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36 Edg/151.0.0.0",
        )
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[probe] 创建 bundle HTTP 客户端失败: {}", e);
            return Vec::new();
        }
    };
    let resp = match client
        .get(crate::api::login::EXPLORATION_PC_URL)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[probe] 获取 ucontent 页面失败: {}", e);
            return Vec::new();
        }
    };
    let html = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            log::warn!("[probe] 读取 ucontent 页面失败: {}", e);
            return Vec::new();
        }
    };
    let base = "https://ucontent.unipus.cn/_explorationpc_default/";
    let mut hits = Vec::new();
    let files = crate::api::login::extract_script_srcs(&html);
    if files.is_empty() {
        log::warn!(
            "[probe] 页面未解析到 JS 脚本（HTML {} 字节，开头: {}）",
            html.len(),
            crate::api::parser::truncate_text(&html.replace('\n', " "), 160)
        );
    }
    log::info!("[probe] bundle 脚本 {} 个，检索题型 {}", files.len(), type_name);
    for file in files {
        if hits.len() >= MAX_BUNDLE_HITS {
            break;
        }
        let url = if file.starts_with("http") {
            file.clone()
        } else {
            format!("{}{}", base, file.trim_start_matches('/'))
        };
        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                log::warn!("[probe] 获取 bundle {} 失败: {}", url, e);
                continue;
            }
        };
        let js = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                log::warn!("[probe] 读取 bundle {} 失败: {}", url, e);
                continue;
            }
        };
        let mut start = 0usize;
        let mut per_file = 0usize;
        while hits.len() < MAX_BUNDLE_HITS && per_file < 3 {
            let Some(idx) = js[start..].find(type_name) else {
                break;
            };
            let idx = start + idx;
            hits.push((url.clone(), snippet(&js, idx, 220, 320)));
            start = idx + type_name.len();
            per_file += 1;
        }
        if per_file > 0 {
            log::info!("[probe] bundle 命中 {} 处: {}", per_file, url);
        }
    }
    if hits.is_empty() {
        log::warn!("[probe] 未在 bundle 中找到题型 {}", type_name);
    }
    hits
}

fn snippet(s: &str, idx: usize, before: usize, after: usize) -> String {
    let mut start = idx.saturating_sub(before);
    while start > 0 && !s.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (idx + after).min(s.len());
    while end > start && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut text = s[start..end].replace(['\r', '\n'], " ");
    if text.chars().count() > 600 {
        text = text.chars().take(600).collect();
    }
    format!("…{}…", text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::parser::ChildQ;

    fn module(reply_type: &str, module_type: &str, child_types: &[&str]) -> Module {
        Module {
            instance_id: "m1".into(),
            module_type: module_type.into(),
            direction: "Choose the best answer.".into(),
            material: "I ____1____ my own decisions.".into(),
            media_sources: Vec::new(),
            transcript: String::new(),
            reply_type: reply_type.into(),
            word_bank: Vec::new(),
            children: child_types
                .iter()
                .map(|t| ChildQ {
                    question_type: "basic-scoop-content".into(),
                    reply_type: (*t).to_string(),
                    question_text: String::new(),
                    options: Vec::new(),
                    option_count: 0,
                })
                .collect(),
        }
    }

    fn env() -> ReportEnv {
        ReportEnv {
            course_id: "course-v2:test".into(),
            class_id: "1".into(),
            curricula_id: "2".into(),
            publish_version: "3".into(),
            open_id: "abc".into(),
        }
    }

    fn meta() -> ReportMeta {
        ReportMeta {
            group_id: "g1".into(),
            unit_id: "u1".into(),
            tab_type: "task".into(),
            group_type: "multiFileUpload".into(),
            required: true,
            passed: false,
            trigger: "自动检测（测试）".into(),
            reason: "模块 m1 的 reply_type=multiFileUpload 未适配".into(),
        }
    }

    #[test]
    fn detect_unknown_module_type() {
        let g = ParsedGroup {
            modules: vec![module("multiFileUpload", "multiFileUpload", &["multiFileUpload"])],
        };
        let un = detect(&g).expect("应判定未适配");
        assert_eq!(un.type_name, "multiFileUpload");
    }

    #[test]
    fn detect_known_and_container_types() {
        let known = ParsedGroup {
            modules: vec![module("dropdownSelection", "basic-scoop-content", &["dropdownSelection"])],
        };
        assert!(detect(&known).is_none());

        // 容器类型（reply_type 为空）由子题类型决定
        let container = ParsedGroup {
            modules: vec![module("", "video-popup", &["singlechoice"])],
        };
        assert!(detect(&container).is_none());

        // 无子题的浏览类模块不参与判定
        let browsing = ParsedGroup {
            modules: vec![module("", "rich-text-read", &[])],
        };
        assert!(detect(&browsing).is_none());
    }

    #[test]
    fn report_contains_structure_payload_and_checklist() {
        let decrypted = serde_json::json!([{
            "id": "m1",
            "content": serde_json::json!({
                "type": "multiFileUpload",
                "replyType": "multiFileUpload",
                "direction": {"text": "<p>Write a letter.</p>"},
                "children": [{"id": "child-1", "type": "multiFileUpload", "replyType": "multiFileUpload", "rule": {"text": ""}}],
                "answers": []
            }).to_string()
        }]);
        let group = ParsedGroup {
            modules: vec![module("multiFileUpload", "multiFileUpload", &["multiFileUpload"])],
        };
        let payload = serde_json::json!({
            "groupId": "g1",
            "thirdPartyJudges": "[{\"reply_type\":\"multiFileUpload\",\"value\":\"\"}]",
            "quesDatas": []
        });
        let text = render_report(&env(), &meta(), &decrypted, Some(&group), Some(&payload), None, &[]);
        assert!(text.contains("# 未适配题型探针报告"));
        assert!(text.contains("multiFileUpload"));
        assert!(text.contains("I ____1____ my own decisions."));
        assert!(text.contains("提交路径与载荷"));
        assert!(text.contains("thirdPartyJudges"));
        assert!(text.contains("child-1"), "未消费字段应包含 child id: {}", text);
        assert!(text.contains("适配检查清单"));
        assert!(text.contains("完整解密 JSON"));
    }

    #[test]
    fn report_handles_missing_group() {
        let decrypted = serde_json::json!({"raw": "not a question group"});
        let text = render_report(&env(), &meta(), &decrypted, None, None, None, &[]);
        assert!(text.contains("无题目模块"));
        assert!(text.contains("未启用 --submit"));
    }

    #[test]
    fn report_renders_submit_response_and_bundle() {
        let decrypted = serde_json::json!({"raw": "x"});
        let resp = serde_json::json!({"code": 0, "data": {"state": {"score": [1, 0]}}});
        let hits = vec![("question-data.js".to_string(), "…MultiFileUpload…".to_string())];
        let text = render_report(
            &env(),
            &meta(),
            &decrypted,
            None,
            None,
            Some(&resp),
            &hits,
        );
        assert!(text.contains("\"score\""));
        assert!(text.contains("question-data.js"));
        assert!(text.contains("MultiFileUpload"));
    }

    #[test]
    fn report_path_is_sanitized_and_stable() {
        let p = report_path("multiFileUpload", "656ec937600159e");
        assert!(p.to_string_lossy().ends_with("multiFileUpload__656ec937600159e.md"));
        assert_eq!(sanitize("a/b c"), "a_b_c");
        assert_eq!(sanitize(""), "unknown");
    }
}
