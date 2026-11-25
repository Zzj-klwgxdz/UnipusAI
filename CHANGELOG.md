# 更新日志

> 按日期倒序排列。更早的提交记录可查看 `git log`。

## 26/11/25

- 同步帮助文案：`print_help` 与 TUI 内置帮助补充 `F1/F2/F3`、`chaoxing`/`zhihuishu` 子命令及当前数据库路径；`platform/mod.rs` 目录注释补齐知到实际文件（`login/cdp/solve`）
- 整理文档：完整更新日志移至 `CHANGELOG.md`，程序结构与实现细节移至 `ARCHITECTURE.md`，`Readme.md` 只保留使用说明与入口

## 26/11/7

- TUI 新增 `F3 知到` 标签页：章节/作业列表、当前章节/全部章节视频补报、已提交作业查看；作业作答入口保留，但当前整体关闭（仅提示不可用）
- 知到新增浏览器辅助登录：自动打开 Edge/Chrome 填表，人工完成验证码后通过 CDP 读取 Cookie 并写回配置（`zhihuishu login` 或首次使用时自动触发）
- 知到作业作答/提交暂时关闭（`HOMEWORK_SUBMIT_ENABLED = false`，服务端要求滑块验证）；知到平台仍属实验性、当前不稳定

## 26/11/6（TUI 多平台 · 知到接入）

- 知到视频批量：整门课 55 节一次跑完（顺序、节间 3s）；并发不可行（服务端串行）；429 自动退避、-12 验证码可 `--open-browser` 转浏览器处理
- 修正 sdsew 语义：total 用服务器累计时长 `studyTotalTime + delta`，位置单独放 HH:MM:SS
- 新增知到/智慧树平台：Cookie 登录、章节/小节、作业列表（未提交/已提交含分数）、整卷读取、LLM 作答+逐题保存+交卷、视频学时上报
- 新增 `zhihuishu` 子配置（cookie/recruit_id/course_id/recruit_and_course_id/school_id/uuid）

## 26/11/6（TUI 多平台）

- 学习通页新增 `A` 全自动：按章节顺序跑完整个课程（视频补报 + 测验作答，自动跳过已完成项）
- 新增 `llm_votes` 配置：同一题向大模型提问 N 次取多数，平票时带候选再问一次
- 学习通作答结果落库（`quiz_results`，按账号隔离），TUI 任务列表显示分数
- 视频任务搬进 TUI：`v` 按真实节奏补报、`Esc` 取消
- TUI 改为浏览器式标签页：`F1` U 校园、`F2` 学习通，两个平台各自独立运行/日志/进度
- 学习通标签页：课程/章节/任务点浏览，`a` 对当前章节测验自动作答并提交
- 数据库改为每个平台一个库（`dump_text/unipusai.db`、`dump_text/chaoxing.db`），库内按账号列隔离

## 26/11/6

- 配置重构：按平台拆分为 `unipusai`/`chaoxing` 子对象，公共配置（模型/转写/间隔）位于顶层；旧配置自动迁移
- 新增学习通（chaoxing）平台模块：登录、课程/章节/任务点、视频进度、测验抓题与字体解密（`chaoxing` 命令，实验性）
- 修复若干 panic：cookie/open_id 等配置含换行等控制字符时启动崩溃（改为跳过并告警）、非 ASCII 日志文件名清理、中文 annotator 密钥展示、TUI 设置项索引越界

> 说明：学习通（chaoxing）与知到（zhihuishu）当前不稳定、暂不可用；相关功能仅作开发留档。

## 26/11/5

- 新增**未适配题型探针报告**：`run`/`group`/TUI 遇到未适配题型时自动抓取并解密结构，生成 `reports/<题型>__<groupId>.md`（完整 JSON、解析器未消费字段、提交载荷、适配清单），并**跳过该任务**（不调用 LLM、不提交）
- 新增 CLI `probe <groupId> [--submit] [--bundle]` 与 `probe --all-unknown [--limit N]`：手动/批量生成报告；`--submit` 用兜底答案真实提交一次并记录判分响应（未通过任务）；`--bundle` 检索 ucontent 前端 bundle 中的题型枚举/组件线索
- 修复 `extract_script_srcs` 只识别带引号 `src="..."` 的问题（ucontent pc.html 使用无引号 `src=...`，此前会导致 annotator 参数/probe bundle 检索取不到脚本）
- 新增**提交得分展示**：解析 submit 响应中的 `data.state.score` / `score_pct` / `record_grade.scorePct`；CLI 每次提交后打印 `[SCORE] ... 提交后得分 100%（各题 1/1/1/1/1/1）`，TUI 顶栏实时显示最近一次得分（100% 绿/部分分黄/0 分红），并按课程持久化到数据库 `meta`（`last_score:<course_id>`），重启或切换课程后自动恢复

## 26/10/19

- 新增**下拉选择（dropdownSelection）**支持：解析材料中的编号空格（`____n____`）并按空位顺序批量作答，答案逐空规范化为该空的合法选项标签（如 A/B），提交格式与单选一致
- 修复 `random_choice` 对带选项的未知题型返回非法占位值 `answer` 的问题

## 26/10/12

- **修复 issue #21：浮点/字符串型字段与 null 结构导致的解析失败**：新增宽容数值/字符串反序列化（缺失/null/浮点四舍五入/数字字符串均可，如 `min_score_pct: 0.75`、`start_time: "1788710400"`、`publish_version: 139753`）；`state`/`strategies` 为显式 null 时按默认值处理（视听说等课程不再因反序列化失败而无法导出/刷课）；TUI 课程结构为空时显示占位提示"本地无该课程数据：按 d 导出"，空库状态文案同步为 `d`

> 本修复在 v3.5 release 尚未应用。

## 26/10/10

- 新增**账号密码自动登录**：`config.json` 只填 `username`/`password` 即可；启动时 jwt 剩余 >24h 直接用、否则 refresh_token 刷新、失败再用账密重登，凭证自动写回 cookie/open_id/refresh_token；运行中接口 401 自动刷新并重试（单飞防并发）；受服务端验证码（极验/图形）限制时给出明确提示并保留 cookie 兜底
- 新增 CLI `login [--force]`（查看/刷新登录态）、`courses`（列出账号课程）、`course <序号|id>`（选择课程，所有命令共用，自动填充 course_id/class_id）
- TUI 新增课程选择界面（`c` 键，未选课程时启动自动打开）；设置页新增 username/password 字段，保存后自动尝试登录
- 数据库 `tasks` 新增 `course_id` 列并按课程隔离（TUI 任务树/dump 汇总只显示当前课程，旧数据按 meta 自动回填）
- `x_annotator_auth_token` 改为**本地自动签发**（前端同算法/同密钥的 HS256 JWT，1 年有效，剩余 <30 天自动续签）——至此凭证类字段全部无需手动填写
- annotator 签名参数改为**按需从 ucontent 前端 bundle 动态提取**（仅 token 需重签或请求 401 时触发），密钥/iss/aud/TTL 轮换后可自动自愈；401 处理升级为"jwt 刷新 → annotator 重提取重签 → 提示手动兜底"阶梯；新增 `annotator [--extract]` 命令查看/强制重提取
- 课程列表新增"我的教材"兜底：部分账号 `getHomeCourseListByStudent` 返回空（已实测），此时自动改用 `/api/cmgt/course/my/bookshelf` 列出班级课程/个人学习条目（含 classId/curriculaId），`courses` 与 TUI 选课均标注来源
- **多账号数据隔离**：数据库改为每账号一个文件 `dump_text/dump-<open_id>.db`（未登录回退旧 `dump.db`），任务/状态/汇总/预览全部按当前账号读写，修复"换账号后进度被识别为旧账号"的问题；旧 `dump.db` 保留原样，可手动改名归属旧账号；`--force` 仅清空当前账号的库
- TUI 主界面底栏新增 `c 课程` 按钮（与快捷键 `c` 等效，支持鼠标点击），按钮宽度调整为每键 12 列；帮助与 Readme 键位同步为 `d` 导出
- **修复进度接口 `strategies` 字段为 null 导致的崩溃**：`end_time`/`start_time`/`min_score_pct` 改为可空并在构造任务时兜底 0；`duration`/`publish_version`/`LeafState` 等同类字段增加"缺失或显式 null 均取默认值"的反序列化兜底，避免部分课程/单元解析失败导致 `progress` 退出

## 26/10/9

- 新增 ratatui 交互式 TUI：无参数启动，支持课程/任务浏览、运行与取消、dump 总览与导出、配置编辑（保存后重建会话）、只读预览与讨论草稿（弹窗确认）、鼠标操作；核心流程重构为 Reporter 事件回调，CLI 行为保持不变，底栏增加状态提示行；dump 页区分"尚无数据（提示导出）"与"读取中"状态，读取失败写入日志面板
- 新增运行日志：每次运行在 `logs/` 生成独立文件（本地时间命名），自动清理 3 天前 `unipus-*.log`；CLI 同时输出到终端，TUI 同时显示在日志面板
- `dump-text` 输出改为 **SQLite 数据库** `dump_text/dump.db`，弃用 `dump_text/` 下的 `.txt` 文件与 `_summary.txt`
- 数据库保存：单元索引/单元名、任务组 id、tab 类型、题型、kind（task/view-only）、必修、完成情况、更新时间；模块类型/replyType/instanceId/答题说明/材料文本/内嵌字幕/词库；媒体 URL 与转写全文（失败存错误）；题目回答类型/题目类型/题干/完整选项；单词卡单词与发音链接
- 所有文本**全文入库**
- `run`/`group`/TUI 完成任务后直接 UPDATE 数据库状态；TUI dump 页改为实时查库生成汇总，空库时提示导出
- TUI 任务树与预览改为全部从数据库加载：启动即显示，可离线浏览；空库提示按 `D` 导出

## 26/9/23

- 新增讨论题（discussion）支持：自动生成英文发言、查询/创建讨论主题、发表评论并标记完成
- `authorization` 改为可选（留空自动使用 cookie 中的 `jwt=`，按 exp 选新并在 401 时回退）
- 新增 `class_id`/`curricula_id` 配置项
- `run`/`group` 自动处理讨论题，`debug` 显示完整草稿与讨论区状态，`dump-text` 支持导出含讨论题的 text/video 组
- 新增单词卡（vocabulary）支持：无需作答，自动标记完成，`debug` 可查看词表
- `dump-text` 输出改为按单元/题型分目录（`dump_text/{单元序号}_{unitId}/{题型}/{groupId}.txt`），dump 文件首行增加必修/完成状态，`_summary.txt` 改为状态汇总；`run`/`group` 答题完成后自动同步完成状态
- `group`/`debug` 支持 `--force`：已通过任务默认跳过作答（`group` 不调用 LLM 不提交、`debug` 只做解析预览），加 `--force` 可强制重做/生成
- `dump-text` 全量归档所有叶子（含阅读/视频/浏览类页面，浏览类归入 `{单元}/view-only/`），状态跟踪覆盖全部任务；浏览类 task 叶子自动走"标记已看"提交，`debug` 对其友好提示不再报错
- 新增了对无题目类任务的支持（如 Quotation、纯视频页面、长文阅读页面），程序直接向服务器发送完成标志

## 26/9/19

- 修复了选词填空题目获取不到 given_words 的问题

## 26/8/22

- 新发现 `https://uai.unipus.cn/api/cmgt/course/getHomeCourseListByStudent` 接口，已应用于课程名的精确识别

## 26/8/21

- 增加服务端限频自动冷却

## 26/8/15

- 改进了课程名识别逻辑

## 26/8/10

- 增加了 `--names` 参数，修复了 banked_cloze 类题目的逻辑
