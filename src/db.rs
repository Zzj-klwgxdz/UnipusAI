use crate::api::parser::{OptionItem, ParsedGroup, VocabWord};
use crate::dump::DUMP_DIR;
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 数据库文件名（位于 dump_text 目录下）。
pub const DB_FILE: &str = "dump.db";

/// 默认（未登录/旧版）数据库完整路径。
pub fn db_path() -> PathBuf {
    Path::new(DUMP_DIR).join(DB_FILE)
}

/// 按账号返回数据库路径：`dump_text/dump-<open_id>.db`；
/// open_id 为空（未登录/离线）时回退旧 `dump_text/dump.db`。
pub fn db_path_for(open_id: &str) -> PathBuf {
    if open_id.is_empty() {
        db_path()
    } else {
        Path::new(DUMP_DIR).join(format!("dump-{}.db", open_id))
    }
}

/// 建表语句（幂等）。
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tasks (
    id          INTEGER PRIMARY KEY,
    unit_index  INTEGER NOT NULL,
    unit_id     TEXT    NOT NULL,
    unit_label  TEXT,
    group_id    TEXT    NOT NULL,
    tab_type    TEXT    NOT NULL,
    group_type  TEXT    NOT NULL,
    kind        TEXT    NOT NULL,
    required    INTEGER NOT NULL,
    passed      INTEGER NOT NULL,
    raw_content TEXT,
    raw_json    TEXT,
    course_id   TEXT,
    updated_at  TEXT    NOT NULL,
    UNIQUE(unit_id, group_id)
);
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS modules (
    id            INTEGER PRIMARY KEY,
    task_id       INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    idx           INTEGER NOT NULL,
    module_type   TEXT    NOT NULL,
    reply_type    TEXT    NOT NULL,
    instance_id   TEXT    NOT NULL,
    direction     TEXT    NOT NULL,
    material      TEXT    NOT NULL,
    transcript    TEXT    NOT NULL,
    word_bank_json TEXT,
    UNIQUE(task_id, idx)
);
CREATE TABLE IF NOT EXISTS media (
    id        INTEGER PRIMARY KEY,
    module_id INTEGER NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    idx       INTEGER NOT NULL,
    url       TEXT    NOT NULL,
    text      TEXT,
    error     TEXT
);
CREATE TABLE IF NOT EXISTS vocabulary (
    id        INTEGER PRIMARY KEY,
    module_id INTEGER NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    idx       INTEGER NOT NULL,
    name      TEXT    NOT NULL,
    sound     TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS questions (
    id            INTEGER PRIMARY KEY,
    module_id     INTEGER NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    idx           INTEGER NOT NULL,
    reply_type    TEXT    NOT NULL,
    question_type TEXT    NOT NULL,
    question_text TEXT    NOT NULL,
    options_json  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_modules_task     ON modules(task_id);
CREATE INDEX IF NOT EXISTS idx_media_module     ON media(module_id);
CREATE INDEX IF NOT EXISTS idx_vocabulary_module ON vocabulary(module_id);
CREATE INDEX IF NOT EXISTS idx_questions_module ON questions(module_id);
"#;

/// 依赖迁移列的索引（在 ensure_column 之后创建）。
const SCHEMA_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_tasks_unit       ON tasks(course_id, unit_id);
CREATE INDEX IF NOT EXISTS idx_tasks_course     ON tasks(course_id, unit_index);
"#;

/// 打开（必要时创建）默认数据库并初始化 schema（未登录/旧版路径）。
pub fn open() -> Result<Connection> {
    open_path(&db_path())
}

/// 按账号打开（必要时创建）数据库并初始化 schema。
pub fn open_for(open_id: &str) -> Result<Connection> {
    open_path(&db_path_for(open_id))
}

/// 打开指定路径的数据库并初始化 schema。
pub fn open_path(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("打开数据库 {} 失败", path.display()))?;
    init(&conn)?;
    Ok(conn)
}

/// 初始化连接参数与 schema（含旧库列迁移）。
pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
    )?;
    conn.execute_batch(SCHEMA)?;
    // 旧库迁移：为已存在的 tasks 表补充新增列
    ensure_column(conn, "tasks", "raw_json", "raw_json TEXT")?;
    ensure_column(conn, "tasks", "course_id", "course_id TEXT")?;
    conn.execute_batch(SCHEMA_INDEXES)?;
    // 旧数据回填：按 meta 中记录的 course_id 归属
    if let Some(cid) = get_meta(conn, "course_id")?
        && !cid.is_empty()
    {
        conn.execute("UPDATE tasks SET course_id=?1 WHERE course_id IS NULL", [&cid])?;
    }
    Ok(())
}

/// 若表缺少指定列则 ALTER TABLE 添加。
fn ensure_column(conn: &Connection, table: &str, column: &str, ddl: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let exists = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|x| x.ok())
        .any(|name| name == column);
    drop(stmt);
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {} ADD COLUMN {}", table, ddl))?;
    }
    Ok(())
}

/// 本地时间字符串（无法取得时区时退化为 UTC）。
pub fn now_local_string() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let fmt = time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second]"
    );
    now.format(fmt).unwrap_or_default()
}

/// 单条媒体转写结果（module_idx 标识所属模块，url_idx 为模块内序号）。
#[derive(Debug, Clone)]
pub struct MediaEntry {
    pub module_idx: usize,
    pub url_idx: usize,
    pub url: String,
    pub text: Option<String>,
    pub error: Option<String>,
}

/// 入库任务及其内容。
pub struct TaskInput<'a> {
    pub unit_index: usize,
    pub unit_id: &'a str,
    pub unit_label: Option<&'a str>,
    pub group_id: &'a str,
    pub tab_type: &'a str,
    pub group_type: &'a str,
    /// "task"（可解析题目）或 "view-only"（浏览类页面）
    pub kind: &'a str,
    pub required: bool,
    pub passed: bool,
    /// 浏览类页面原始内容（全文）
    pub raw_content: Option<&'a str>,
    /// 解密后完整 JSON（缩进美化；浏览类为 None）
    pub raw_json: Option<&'a str>,
    /// 所属课程 id（多课程隔离）
    pub course_id: &'a str,
    /// 解析出的题目组（浏览类为 None）
    pub group: Option<&'a ParsedGroup>,
    /// 单词卡列表（仅 vocabulary 模块写入）
    pub vocab: &'a [VocabWord],
    /// 媒体转写结果
    pub media: &'a [MediaEntry],
}

fn options_json(opts: &[OptionItem]) -> String {
    let arr: Vec<serde_json::Value> = opts
        .iter()
        .map(|o| serde_json::json!({"name": o.name, "value": o.value, "text": o.text}))
        .collect();
    serde_json::Value::Array(arr).to_string()
}

/// 写入/更新一个任务及其全部内容（单事务；覆盖旧内容）。
pub fn save_task(conn: &mut Connection, t: &TaskInput<'_>) -> Result<i64> {
    let tx = conn.transaction()?;
    let now = now_local_string();
    tx.execute(
        "INSERT INTO tasks (unit_index, unit_id, unit_label, group_id, tab_type, group_type,
                            kind, required, passed, raw_content, raw_json, course_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(unit_id, group_id) DO UPDATE SET
            unit_index=excluded.unit_index,
            unit_label=excluded.unit_label,
            tab_type=excluded.tab_type,
            group_type=excluded.group_type,
            kind=excluded.kind,
            required=excluded.required,
            passed=excluded.passed,
            raw_content=excluded.raw_content,
            raw_json=excluded.raw_json,
            course_id=excluded.course_id,
            updated_at=excluded.updated_at",
        params![
            t.unit_index as i64,
            t.unit_id,
            t.unit_label,
            t.group_id,
            t.tab_type,
            t.group_type,
            t.kind,
            t.required as i64,
            t.passed as i64,
            t.raw_content,
            t.raw_json,
            t.course_id,
            now
        ],
    )?;
    let task_id: i64 = tx.query_row(
        "SELECT id FROM tasks WHERE unit_id=?1 AND group_id=?2",
        params![t.unit_id, t.group_id],
        |r| r.get(0),
    )?;
    // 覆盖旧内容：删除子表行后重插（级联删除 media/vocabulary/questions）
    tx.execute("DELETE FROM modules WHERE task_id=?1", [task_id])?;

    if let Some(group) = t.group {
        for (mi, m) in group.modules.iter().enumerate() {
            tx.execute(
                "INSERT INTO modules (task_id, idx, module_type, reply_type, instance_id,
                                      direction, material, transcript, word_bank_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    task_id,
                    mi as i64,
                    m.module_type,
                    m.reply_type,
                    m.instance_id,
                    m.direction,
                    m.material,
                    m.transcript,
                    options_json(&m.word_bank)
                ],
            )?;
            let module_id = tx.last_insert_rowid();

            if m.module_type == "vocabulary" {
                for (wi, w) in t.vocab.iter().enumerate() {
                    tx.execute(
                        "INSERT INTO vocabulary (module_id, idx, name, sound) VALUES (?1, ?2, ?3, ?4)",
                        params![module_id, wi as i64, w.name, w.sound],
                    )?;
                }
            }
            for e in t.media.iter().filter(|e| e.module_idx == mi) {
                tx.execute(
                    "INSERT INTO media (module_id, idx, url, text, error) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![module_id, e.url_idx as i64, e.url, e.text, e.error],
                )?;
            }
            for (ci, c) in m.children.iter().enumerate() {
                tx.execute(
                    "INSERT INTO questions (module_id, idx, reply_type, question_type,
                                            question_text, options_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        module_id,
                        ci as i64,
                        c.reply_type,
                        c.question_type,
                        c.question_text,
                        options_json(&c.options)
                    ],
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(task_id)
}

/// 任务是否存在。
pub fn task_exists(conn: &Connection, unit_id: &str, group_id: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE unit_id=?1 AND group_id=?2",
        params![unit_id, group_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 已入库任务是否需要重新抓取：缺 raw_json（旧库升级）或 course_id 不匹配（换课/旧数据）。
pub fn task_needs_refresh(
    conn: &Connection,
    unit_id: &str,
    group_id: &str,
    course_id: &str,
) -> Result<bool> {
    let mut stmt = conn
        .prepare("SELECT kind, raw_json, course_id FROM tasks WHERE unit_id=?1 AND group_id=?2")?;
    let mut rows = stmt.query(params![unit_id, group_id])?;
    match rows.next()? {
        Some(r) => {
            let kind: String = r.get(0)?;
            let raw: Option<String> = r.get(1)?;
            let cid: Option<String> = r.get(2)?;
            Ok((kind == "task" && raw.is_none()) || cid.as_deref().unwrap_or("") != course_id)
        }
        None => Ok(false),
    }
}

/// 读取任务状态（是否必修, 是否已完成）。
pub fn task_status(conn: &Connection, unit_id: &str, group_id: &str) -> Result<Option<(bool, bool)>> {
    let mut stmt = conn.prepare(
        "SELECT required, passed FROM tasks WHERE unit_id=?1 AND group_id=?2",
    )?;
    let mut rows = stmt.query(params![unit_id, group_id])?;
    match rows.next()? {
        Some(r) => Ok(Some((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0))),
        None => Ok(None),
    }
}

/// 更新任务完成情况；任务不存在返回 false。
pub fn set_passed(conn: &Connection, unit_id: &str, group_id: &str, passed: bool) -> Result<bool> {
    let n = conn.execute(
        "UPDATE tasks SET passed=?1, updated_at=?2 WHERE unit_id=?3 AND group_id=?4",
        params![passed as i64, now_local_string(), unit_id, group_id],
    )?;
    Ok(n > 0)
}

/// 刷新任务元信息与状态（unit_label 为 None 时保留原值）；任务不存在返回 false。
#[allow(clippy::too_many_arguments)]
pub fn update_status(
    conn: &Connection,
    unit_id: &str,
    group_id: &str,
    unit_index: usize,
    unit_label: Option<&str>,
    tab_type: &str,
    required: bool,
    passed: bool,
) -> Result<bool> {
    let n = conn.execute(
        "UPDATE tasks SET unit_index=?3, unit_label=COALESCE(?4, unit_label), tab_type=?5,
                          required=?6, passed=?7, updated_at=?8
         WHERE unit_id=?1 AND group_id=?2",
        params![
            unit_id,
            group_id,
            unit_index as i64,
            unit_label,
            tab_type,
            required as i64,
            passed as i64,
            now_local_string()
        ],
    )?;
    Ok(n > 0)
}

/// 清空全部数据（--force 全量重建）。
pub fn clear_all(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM tasks", [])?;
    Ok(())
}

/// 任务总数（指定课程）。
pub fn task_count(conn: &Connection, course_id: &str) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE course_id=?1",
        [course_id],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// 写入/更新 meta 键值（课程信息等）。
pub fn save_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// 读取 meta 键值。
pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key=?1")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(r) => Ok(Some(r.get(0)?)),
        None => Ok(None),
    }
}

/// 任务树节点（TUI 从数据库加载）。
#[derive(Debug, Clone)]
pub struct TreeUnit {
    pub unit_index: i64,
    pub unit_id: String,
    pub unit_label: Option<String>,
    pub tasks: Vec<TreeTask>,
}

#[derive(Debug, Clone)]
pub struct TreeTask {
    pub group_id: String,
    pub tab_type: String,
    pub group_type: String,
    pub kind: String,
    pub required: bool,
    pub passed: bool,
}

/// 按单元分组加载指定课程的全部任务（顺序：unit_index, 入库顺序）。
pub fn load_tree(conn: &Connection, course_id: &str) -> Result<Vec<TreeUnit>> {
    let mut stmt = conn.prepare(
        "SELECT unit_index, unit_id, unit_label, group_id, tab_type, group_type, kind,
                required, passed
         FROM tasks WHERE course_id=?1 ORDER BY unit_index, rowid",
    )?;
    let mut rows = stmt.query([course_id])?;
    let mut units: Vec<TreeUnit> = Vec::new();
    while let Some(r) = rows.next()? {
        let unit_index: i64 = r.get(0)?;
        let unit_id: String = r.get(1)?;
        let unit_label: Option<String> = r.get(2)?;
        let task = TreeTask {
            group_id: r.get(3)?,
            tab_type: r.get(4)?,
            group_type: r.get(5)?,
            kind: r.get(6)?,
            required: r.get::<_, i64>(7)? != 0,
            passed: r.get::<_, i64>(8)? != 0,
        };
        match units.last_mut() {
            Some(u) if u.unit_id == unit_id && u.unit_index == unit_index => {
                // 同一单元内任一行的标签都能回填（view-only 行可能没有标签）
                if u.unit_label.is_none() && unit_label.is_some() {
                    u.unit_label = unit_label;
                }
                u.tasks.push(task);
            }
            _ => units.push(TreeUnit {
                unit_index,
                unit_id,
                unit_label,
                tasks: vec![task],
            }),
        }
    }
    Ok(units)
}

/// 库中重建任务组内容所需的子题数据。
#[derive(Debug, Clone)]
pub struct StoredQuestion {
    pub reply_type: String,
    pub question_type: String,
    pub question_text: String,
    pub options_json: String,
}

/// 库中重建任务组内容所需的模块数据。
#[derive(Debug, Clone)]
pub struct StoredModule {
    pub module_type: String,
    pub reply_type: String,
    pub instance_id: String,
    pub direction: String,
    pub material: String,
    pub transcript: String,
    pub word_bank_json: String,
    pub media_urls: Vec<String>,
    pub questions: Vec<StoredQuestion>,
}

/// 从数据库读取的完整任务组数据。
#[derive(Debug, Clone)]
pub struct StoredTask {
    pub unit_id: String,
    pub unit_index: i64,
    pub unit_label: Option<String>,
    pub group_id: String,
    pub tab_type: String,
    pub group_type: String,
    pub kind: String,
    pub required: bool,
    pub passed: bool,
    pub raw_content: Option<String>,
    pub raw_json: Option<String>,
    pub modules: Vec<StoredModule>,
    /// 单词卡（name, sound）
    pub vocab: Vec<(String, String)>,
}

/// 按 group_id 读取任务及其全部内容（用于预览）。
pub fn load_stored(conn: &Connection, group_id: &str) -> Result<Option<StoredTask>> {
    let mut stmt = conn.prepare(
        "SELECT id, unit_id, unit_index, unit_label, group_id, tab_type, group_type, kind,
                required, passed, raw_content, raw_json
         FROM tasks WHERE group_id=?1 ORDER BY id LIMIT 1",
    )?;
    let mut rows = stmt.query([group_id])?;
    let Some(r) = rows.next()? else {
        return Ok(None);
    };
    let task_id: i64 = r.get(0)?;
    let mut task = StoredTask {
        unit_id: r.get(1)?,
        unit_index: r.get(2)?,
        unit_label: r.get(3)?,
        group_id: r.get(4)?,
        tab_type: r.get(5)?,
        group_type: r.get(6)?,
        kind: r.get(7)?,
        required: r.get::<_, i64>(8)? != 0,
        passed: r.get::<_, i64>(9)? != 0,
        raw_content: r.get(10)?,
        raw_json: r.get(11)?,
        modules: Vec::new(),
        vocab: Vec::new(),
    };
    drop(rows);

    let mut mstmt = conn.prepare(
        "SELECT id, module_type, reply_type, instance_id, direction, material, transcript,
                word_bank_json
         FROM modules WHERE task_id=?1 ORDER BY idx",
    )?;
    let mut mrows = mstmt.query([task_id])?;
    while let Some(m) = mrows.next()? {
        let module_id: i64 = m.get(0)?;
        let module_type: String = m.get(1)?;
        let mut media_stmt = conn
            .prepare("SELECT url FROM media WHERE module_id=?1 ORDER BY idx")?;
        let media_urls: Vec<String> = media_stmt
            .query_map([module_id], |x| x.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut q_stmt = conn.prepare(
            "SELECT reply_type, question_type, question_text, options_json
             FROM questions WHERE module_id=?1 ORDER BY idx",
        )?;
        let questions: Vec<StoredQuestion> = q_stmt
            .query_map([module_id], |x| {
                Ok(StoredQuestion {
                    reply_type: x.get(0)?,
                    question_type: x.get(1)?,
                    question_text: x.get(2)?,
                    options_json: x.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        if module_type == "vocabulary" {
            let mut v_stmt = conn
                .prepare("SELECT name, sound FROM vocabulary WHERE module_id=?1 ORDER BY idx")?;
            task.vocab = v_stmt
                .query_map([module_id], |x| Ok((x.get(0)?, x.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
        }
        task.modules.push(StoredModule {
            module_type,
            reply_type: m.get(2)?,
            instance_id: m.get(3)?,
            direction: m.get(4)?,
            material: m.get(5)?,
            transcript: m.get(6)?,
            word_bank_json: m.get::<_, Option<String>>(7)?.unwrap_or_default(),
            media_urls,
            questions,
        });
    }
    Ok(Some(task))
}

/// 从数据库实时生成指定课程的汇总文本（原 _summary.txt 内容）；空库返回 None。
pub fn summary_text(conn: &Connection, course_id: &str) -> Result<Option<String>> {
    let total = task_count(conn, course_id)?;
    if total == 0 {
        return Ok(None);
    }
    let mut stmt = conn.prepare(
        "SELECT unit_index, unit_id, group_id, group_type, required, passed
         FROM tasks WHERE course_id=?1 ORDER BY unit_index, rowid",
    )?;
    let mut rows = stmt.query([course_id])?;

    let mut total_req = 0usize;
    let mut total_req_done = 0usize;
    let mut total_opt = 0usize;
    let mut total_opt_done = 0usize;
    let mut units: BTreeMap<(i64, String), (usize, usize, usize, usize)> = BTreeMap::new();
    let mut entries: Vec<(String, String)> = Vec::new();

    while let Some(r) = rows.next()? {
        let unit_index: i64 = r.get(0)?;
        let unit_id: String = r.get(1)?;
        let group_id: String = r.get(2)?;
        let group_type: String = r.get(3)?;
        let required = r.get::<_, i64>(4)? != 0;
        let passed = r.get::<_, i64>(5)? != 0;
        let agg = units.entry((unit_index, unit_id.clone())).or_insert((0, 0, 0, 0));
        if required {
            total_req += 1;
            agg.0 += 1;
            if passed {
                total_req_done += 1;
                agg.1 += 1;
            }
        } else {
            total_opt += 1;
            agg.2 += 1;
            if passed {
                total_opt_done += 1;
                agg.3 += 1;
            }
        }
        entries.push((
            format!("{:02}_{}/{}/{}", unit_index, unit_id, group_type, group_id),
            format!(
                "{} {}",
                if required { "必修" } else { "选修" },
                if passed { "已完成" } else { "未完成" }
            ),
        ));
    }

    let mut out = String::new();
    out.push_str(&format!("dump-text 数据汇总 (更新时间: {})\n", now_local_string()));
    out.push_str(&format!(
        "任务组: {} 个 | 必修: {} (已完成 {}, 未完成 {}) | 选修: {} (已完成 {}, 未完成 {})",
        total,
        total_req,
        total_req_done,
        total_req - total_req_done,
        total_opt,
        total_opt_done,
        total_opt - total_opt_done
    ));
    let db_display = conn
        .path()
        .map(|s| s.to_string())
        .unwrap_or_else(|| db_path().display().to_string());
    out.push_str(&format!("\n数据库: {}\n", db_display));
    out.push_str("\n按单元:\n");
    for ((idx, uid), (r, rd, o, od)) in &units {
        out.push_str(&format!(
            "  {:02}_{}: 必修 {} (已完成 {}) / 选修 {} (已完成 {})\n",
            idx, uid, r, rd, o, od
        ));
    }
    out.push_str("\n任务清单:\n");
    for (path, status) in &entries {
        out.push_str(&format!("  {} [{}]\n", path, status));
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::parser::{ChildQ, Module};

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init(&c).unwrap();
        c
    }

    fn option(name: &str, value: &str, text: &str) -> OptionItem {
        OptionItem {
            name: name.into(),
            value: value.into(),
            text: text.into(),
        }
    }

    fn module(module_type: &str) -> Module {
        Module {
            instance_id: "inst-1".into(),
            module_type: module_type.into(),
            direction: "请阅读并作答".into(),
            material: "材料".repeat(200),
            media_sources: vec!["http://a/1.mp3".into(), "http://a/2.mp4".into()],
            transcript: "字幕文本".into(),
            reply_type: "singlechoice".into(),
            word_bank: vec![option("wb", "v", "t")],
            children: vec![ChildQ {
                question_type: "basic".into(),
                reply_type: "singlechoice".into(),
                question_text: "题干".repeat(100),
                options: vec![option("A", "a", "选项A"), option("B", "b", "选项B")],
                option_count: 2,
            }],
        }
    }

    fn input<'a>(group: &'a ParsedGroup, vocab: &'a [VocabWord], media: &'a [MediaEntry]) -> TaskInput<'a> {
        TaskInput {
            unit_index: 1,
            unit_id: "u1",
            unit_label: Some("Unit 1"),
            group_id: "g1",
            tab_type: "task",
            group_type: "singlechoice",
            kind: "task",
            required: true,
            passed: false,
            raw_content: None,
            raw_json: Some("{\n  \"demo\": true\n}"),
            course_id: "course-x",
            group: Some(group),
            vocab,
            media,
        }
    }

    #[test]
    fn save_task_roundtrip_stores_full_text() {
        let mut c = conn();
        let group = ParsedGroup {
            modules: vec![module("vocabulary")],
        };
        let vocab = vec![VocabWord {
            name: "apple".into(),
            sound: "http://s/apple.mp3".into(),
        }];
        let media = vec![
            MediaEntry {
                module_idx: 0,
                url_idx: 0,
                url: "http://a/1.mp3".into(),
                text: Some("转写".repeat(6000)),
                error: None,
            },
            MediaEntry {
                module_idx: 0,
                url_idx: 1,
                url: "http://a/2.mp4".into(),
                text: None,
                error: Some("下载失败".into()),
            },
        ];
        let id = save_task(&mut c, &input(&group, &vocab, &media)).unwrap();
        assert!(id > 0);
        assert!(task_exists(&c, "u1", "g1").unwrap());
        assert_eq!(task_status(&c, "u1", "g1").unwrap(), Some((true, false)));

        let module_count: i64 = c
            .query_row("SELECT COUNT(*) FROM modules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(module_count, 1);
        let (material, transcript): (String, String) = c
            .query_row("SELECT material, transcript FROM modules", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(material.chars().count(), 400);
        assert_eq!(transcript, "字幕文本");

        let vocab_count: i64 = c
            .query_row("SELECT COUNT(*) FROM vocabulary", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vocab_count, 1);

        let media_count: i64 = c
            .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
            .unwrap();
        assert_eq!(media_count, 2);
        let full_len: i64 = c
            .query_row(
                "SELECT length(text) FROM media WHERE idx=0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(full_len as usize, "转写".chars().count() * 6000);

        let (qtext, opts): (String, String) = c
            .query_row("SELECT question_text, options_json FROM questions", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(qtext.chars().count(), 200);
        let v: serde_json::Value = serde_json::from_str(&opts).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["name"], "A");
    }

    #[test]
    fn save_task_upsert_keeps_single_row_and_updates_status() {
        let mut c = conn();
        let group = ParsedGroup {
            modules: vec![module("basic")],
        };
        save_task(&mut c, &input(&group, &[], &[])).unwrap();
        let mut t2 = input(&group, &[], &[]);
        t2.passed = true;
        t2.required = false;
        save_task(&mut c, &t2).unwrap();

        assert_eq!(task_count(&c, "course-x").unwrap(), 1);
        assert_eq!(task_status(&c, "u1", "g1").unwrap(), Some((false, true)));
        let module_count: i64 = c
            .query_row("SELECT COUNT(*) FROM modules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(module_count, 1);
    }

    #[test]
    fn set_passed_and_clear_all() {
        let mut c = conn();
        let group = ParsedGroup {
            modules: vec![module("basic")],
        };
        save_task(&mut c, &input(&group, &[], &[])).unwrap();
        assert!(set_passed(&c, "u1", "g1", true).unwrap());
        assert_eq!(task_status(&c, "u1", "g1").unwrap(), Some((true, true)));
        assert!(!set_passed(&c, "u9", "g9", true).unwrap());

        clear_all(&c).unwrap();
        assert_eq!(task_count(&c, "course-x").unwrap(), 0);
        let modules: i64 = c
            .query_row("SELECT COUNT(*) FROM modules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(modules, 0);
    }

    #[test]
    fn view_only_content_roundtrip() {
        let mut c = conn();
        let raw = "页面原文".repeat(3000);
        let t = TaskInput {
            unit_index: 2,
            unit_id: "u2",
            unit_label: None,
            group_id: "g2",
            tab_type: "text",
            group_type: "view-only",
            kind: "view-only",
            required: false,
            passed: true,
            raw_content: Some(raw.as_str()),
            raw_json: None,
            course_id: "course-x",
            group: None,
            vocab: &[],
            media: &[],
        };
        save_task(&mut c, &t).unwrap();
        let raw: String = c
            .query_row("SELECT raw_content FROM tasks WHERE group_id='g2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(raw.chars().count(), 4 * 3000);
    }

    #[test]
    fn summary_text_empty_and_filled() {
        let mut c = conn();
        assert!(summary_text(&c, "course-x").unwrap().is_none());

        let group = ParsedGroup {
            modules: vec![module("basic")],
        };
        save_task(&mut c, &input(&group, &[], &[])).unwrap();
        let mut t2 = input(&group, &[], &[]);
        t2.group_id = "g2";
        t2.tab_type = "text";
        t2.required = false;
        t2.passed = true;
        save_task(&mut c, &t2).unwrap();

        let text = summary_text(&c, "course-x").unwrap().unwrap();
        assert!(text.contains("任务组: 2 个"), "{}", text);
        assert!(text.contains("必修: 1 (已完成 0, 未完成 1)"), "{}", text);
        assert!(text.contains("选修: 1 (已完成 1, 未完成 0)"), "{}", text);
        assert!(text.contains("01_u1: 必修 1 (已完成 0) / 选修 1 (已完成 1)"), "{}", text);
        assert!(text.contains("01_u1/singlechoice/g1 [必修 未完成]"), "{}", text);
    }

    #[test]
    fn init_migrates_missing_raw_json() {
        let c = Connection::open_in_memory().unwrap();
        // 模拟旧版 schema（无 raw_json 列）
        c.execute_batch(
            "CREATE TABLE tasks (
                id INTEGER PRIMARY KEY,
                unit_index INTEGER NOT NULL,
                unit_id TEXT NOT NULL,
                unit_label TEXT,
                group_id TEXT NOT NULL,
                tab_type TEXT NOT NULL,
                group_type TEXT NOT NULL,
                kind TEXT NOT NULL,
                required INTEGER NOT NULL,
                passed INTEGER NOT NULL,
                raw_content TEXT,
                updated_at TEXT NOT NULL,
                UNIQUE(unit_id, group_id)
            );",
        )
        .unwrap();
        init(&c).unwrap();
        let mut stmt = c.prepare("PRAGMA table_info(tasks)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|x| x.ok())
            .collect();
        assert!(cols.contains(&"raw_json".to_string()), "{:?}", cols);
        assert!(cols.contains(&"course_id".to_string()), "{:?}", cols);
        // meta 表同时可用
        save_meta(&c, "k", "v").unwrap();
        assert_eq!(get_meta(&c, "k").unwrap().as_deref(), Some("v"));
    }

    #[test]
    fn db_path_per_account() {
        assert_eq!(db_path_for("").file_name().unwrap(), "dump.db");
        assert_eq!(db_path_for("abc123").file_name().unwrap(), "dump-abc123.db");
        assert!(db_path_for("abc123").starts_with(DUMP_DIR));
    }

    #[test]
    fn two_db_files_isolated() {
        let dir = std::env::temp_dir().join(format!("unipus_db_iso_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p1 = dir.join("acct-a.db");
        let p2 = dir.join("acct-b.db");
        for p in [&p1, &p2] {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_file(p.with_extension("db-wal"));
            let _ = std::fs::remove_file(p.with_extension("db-shm"));
        }
        let mut c1 = open_path(&p1).unwrap();
        let group = ParsedGroup {
            modules: vec![module("basic")],
        };
        save_task(&mut c1, &input(&group, &[], &[])).unwrap();
        let c2 = open_path(&p2).unwrap();
        assert_eq!(task_count(&c1, "course-x").unwrap(), 1);
        assert_eq!(task_count(&c2, "course-x").unwrap(), 0, "双文件必须互相隔离");
        assert!(load_tree(&c2, "course-x").unwrap().is_empty());
        drop(c1);
        drop(c2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_tree_filters_by_course() {
        let mut c = conn();
        let group = ParsedGroup {
            modules: vec![module("basic")],
        };
        save_task(&mut c, &input(&group, &[], &[])).unwrap();
        let mut t2 = input(&group, &[], &[]);
        t2.group_id = "g2";
        t2.course_id = "course-y";
        save_task(&mut c, &t2).unwrap();

        let units = load_tree(&c, "course-x").unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].tasks.len(), 1);
        assert_eq!(units[0].tasks[0].group_id, "g1");

        let units = load_tree(&c, "course-y").unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].tasks[0].group_id, "g2");

        assert!(load_tree(&c, "course-z").unwrap().is_empty());
        assert_eq!(task_count(&c, "course-z").unwrap(), 0);
    }

    #[test]
    fn meta_roundtrip() {
        let c = conn();
        assert!(get_meta(&c, "course_name").unwrap().is_none());
        save_meta(&c, "course_name", "新视野").unwrap();
        save_meta(&c, "course_name", "新视野2").unwrap();
        assert_eq!(get_meta(&c, "course_name").unwrap().as_deref(), Some("新视野2"));
    }

    #[test]
    fn load_tree_orders_and_rebuilds() {
        let mut c = conn();
        let group = ParsedGroup {
            modules: vec![module("basic")],
        };
        let mut t1 = input(&group, &[], &[]);
        t1.unit_index = 2;
        t1.unit_id = "u2";
        t1.unit_label = None;
        t1.group_id = "g2";
        save_task(&mut c, &t1).unwrap();
        // u1 首行无标签（view-only 场景），后续行有标签 → 应回填
        let mut t0 = input(&group, &[], &[]);
        t0.unit_index = 1;
        t0.unit_label = None;
        t0.group_id = "g0";
        save_task(&mut c, &t0).unwrap();
        let mut t2 = input(&group, &[], &[]);
        t2.unit_index = 1;
        t2.passed = true;
        save_task(&mut c, &t2).unwrap();
        let mut t3 = input(&group, &[], &[]);
        t3.unit_index = 1;
        t3.group_id = "g1b";
        t3.required = false;
        save_task(&mut c, &t3).unwrap();

        let units = load_tree(&c, "course-x").unwrap();
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].unit_id, "u1");
        assert_eq!(units[0].unit_label.as_deref(), Some("Unit 1"));
        assert_eq!(units[0].tasks.len(), 3);
        assert_eq!(units[0].tasks[0].group_id, "g0");
        assert_eq!(units[0].tasks[1].group_id, "g1");
        assert!(units[0].tasks[1].passed);
        assert_eq!(units[0].tasks[2].group_id, "g1b");
        assert!(!units[0].tasks[2].required);
        assert_eq!(units[1].unit_id, "u2");
        assert_eq!(units[1].unit_label, None);
        assert_eq!(units[1].tasks.len(), 1);
    }

    #[test]
    fn load_stored_rebuilds_group() {
        let mut c = conn();
        let group = ParsedGroup {
            modules: vec![module("vocabulary")],
        };
        let vocab = vec![VocabWord {
            name: "apple".into(),
            sound: "http://s/a.mp3".into(),
        }];
        let media = vec![MediaEntry {
            module_idx: 0,
            url_idx: 0,
            url: "http://a/1.mp3".into(),
            text: Some("t".into()),
            error: None,
        }];
        save_task(&mut c, &input(&group, &vocab, &media)).unwrap();

        let st = load_stored(&c, "g1").unwrap().unwrap();
        assert_eq!(st.group_id, "g1");
        assert_eq!(st.kind, "task");
        assert_eq!(st.raw_json.as_deref(), Some("{\n  \"demo\": true\n}"));
        assert_eq!(st.modules.len(), 1);
        let m = &st.modules[0];
        assert_eq!(m.module_type, "vocabulary");
        assert_eq!(m.media_urls, vec!["http://a/1.mp3".to_string()]);
        assert_eq!(m.questions.len(), 1);
        let opts: serde_json::Value = serde_json::from_str(&m.questions[0].options_json).unwrap();
        assert_eq!(opts.as_array().unwrap().len(), 2);
        assert_eq!(opts[0]["name"], "A");
        assert_eq!(
            st.vocab,
            vec![("apple".to_string(), "http://s/a.mp3".to_string())]
        );

        assert!(load_stored(&c, "missing").unwrap().is_none());
    }
}
