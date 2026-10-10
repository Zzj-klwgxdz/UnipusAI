use crate::api::content::{decrypt_content, fetch_content, parse_decrypted};
use crate::api::parser::{
    ChildQ, Module, OptionItem, ParsedGroup, VocabWord, extract_vocabulary, parse_group,
};
use crate::api::session::Session;
use crate::db::{StoredModule, StoredTask};
use anyhow::Result;

/// 任务组只读预览数据（不提交、默认不调用 LLM）。
#[derive(Debug)]
pub struct Preview {
    /// 解密后 JSON（合法 JSON 时）
    pub json_pretty: Option<String>,
    /// 解密原文（非 JSON/空时用于展示）
    pub plain: String,
    /// 可解析出的题目模块；None 表示浏览类页面
    pub group: Option<ParsedGroup>,
    /// 单词卡（vocabulary）
    pub vocab: Vec<VocabWord>,
}

impl Preview {
    /// 是否为浏览类页面（内容为空/非 JSON/无题目模块）。
    pub fn is_view_only(&self) -> bool {
        self.group.is_none()
    }
}

/// 拉取并解析任务组内容（只读，网络版；供 CLI debug 使用）。
pub async fn load_preview(session: &Session, group_id: &str) -> Result<Preview> {
    let rt = fetch_content(session, group_id).await?;
    let plain = decrypt_content(&rt.content, &rt.k)?;
    match parse_decrypted(&plain) {
        Ok(dec) => {
            let json_pretty = serde_json::to_string_pretty(&dec).ok();
            let vocab = extract_vocabulary(&dec);
            let group = parse_group(&dec).ok();
            Ok(Preview {
                json_pretty,
                plain,
                group,
                vocab,
            })
        }
        Err(_) => Ok(Preview {
            json_pretty: None,
            plain,
            group: None,
            vocab: Vec::new(),
        }),
    }
}

/// 从数据库读取任务组预览（TUI 用）；库中无该任务返回 None。
pub fn load_preview_from_db(group_id: &str) -> Result<Option<Preview>> {
    let conn = crate::db::open()?;
    let Some(st) = crate::db::load_stored(&conn, group_id)? else {
        return Ok(None);
    };
    let group = if st.kind == "task" {
        Some(rebuild_group(&st))
    } else {
        None
    };
    let vocab = st
        .vocab
        .iter()
        .map(|(name, sound)| VocabWord {
            name: name.clone(),
            sound: sound.clone(),
        })
        .collect();
    Ok(Some(Preview {
        json_pretty: st.raw_json,
        plain: st.raw_content.unwrap_or_default(),
        group,
        vocab,
    }))
}

/// 解析入库的选项 JSON。
fn parse_options(json: &str) -> Vec<OptionItem> {
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .unwrap_or_default()
        .iter()
        .map(|v| OptionItem {
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            value: v
                .get("value")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            text: v
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

/// 从数据库模块记录重建解析结果。
fn rebuild_group(st: &StoredTask) -> ParsedGroup {
    ParsedGroup {
        modules: st.modules.iter().map(rebuild_module).collect(),
    }
}

fn rebuild_module(m: &StoredModule) -> Module {
    Module {
        instance_id: m.instance_id.clone(),
        module_type: m.module_type.clone(),
        direction: m.direction.clone(),
        material: m.material.clone(),
        media_sources: m.media_urls.clone(),
        transcript: m.transcript.clone(),
        reply_type: m.reply_type.clone(),
        word_bank: parse_options(&m.word_bank_json),
        children: m
            .questions
            .iter()
            .map(|q| {
                let options = parse_options(&q.options_json);
                ChildQ {
                    question_type: q.question_type.clone(),
                    reply_type: q.reply_type.clone(),
                    question_text: q.question_text.clone(),
                    option_count: options.len(),
                    options,
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{MediaEntry, TaskInput};

    #[test]
    fn rebuild_from_stored_task() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        let module = Module {
            instance_id: "i1".into(),
            module_type: "basic".into(),
            direction: "说明".into(),
            material: "材料".into(),
            media_sources: vec!["http://a/1.mp3".into()],
            transcript: "字幕".into(),
            reply_type: "singlechoice".into(),
            word_bank: vec![OptionItem {
                name: "A".into(),
                value: "a".into(),
                text: "选项A".into(),
            }],
            children: vec![ChildQ {
                question_type: "basic".into(),
                reply_type: "singlechoice".into(),
                question_text: "题干".into(),
                options: vec![
                    OptionItem {
                        name: "A".into(),
                        value: "a".into(),
                        text: "选项A".into(),
                    },
                    OptionItem {
                        name: "B".into(),
                        value: "b".into(),
                        text: "选项B".into(),
                    },
                ],
                option_count: 2,
            }],
        };
        let group = ParsedGroup {
            modules: vec![module],
        };
        let media = vec![MediaEntry {
            module_idx: 0,
            url_idx: 0,
            url: "http://a/1.mp3".into(),
            text: None,
            error: Some("x".into()),
        }];
        crate::db::save_task(
            &mut conn,
            &TaskInput {
                unit_index: 1,
                unit_id: "u1",
                unit_label: None,
                group_id: "g1",
                tab_type: "task",
                group_type: "singlechoice",
                kind: "task",
                required: true,
                passed: false,
                raw_content: None,
                raw_json: Some("{}"),
                course_id: "course-x",
                group: Some(&group),
                vocab: &[],
                media: &media,
            },
        )
        .unwrap();

        let st = crate::db::load_stored(&conn, "g1").unwrap().unwrap();
        let rebuilt = rebuild_group(&st);
        assert_eq!(rebuilt.modules.len(), 1);
        let m = &rebuilt.modules[0];
        assert_eq!(m.direction, "说明");
        assert_eq!(m.media_sources, vec!["http://a/1.mp3".to_string()]);
        assert_eq!(m.word_bank.len(), 1);
        assert_eq!(m.children.len(), 1);
        assert_eq!(m.children[0].option_count, 2);
        assert_eq!(m.children[0].options[1].text, "选项B");
    }
}
