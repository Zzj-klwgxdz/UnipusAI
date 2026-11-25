# 程序结构与实现细节

> 面向开发/维护者。使用说明见 [Readme.md](Readme.md)，更新日志见 [CHANGELOG.md](CHANGELOG.md)。
>
> 当前状态：**U 校园（unipusai）稳定可用**；学习通（chaoxing）与知到（zhihuishu）为实验性实现，**当前不稳定、暂不可用**，以下内容仅作开发留档。

## 1. 技术栈

| 组件 | 用途 |
| --- | --- |
| Rust（edition 2024）+ Tokio | 主语言与异步运行时 |
| reqwest | HTTP 客户端（rustls、cookie、gzip/brotli/deflate、http2） |
| aes / cbc / ecb / hex | 平台内容解密、登录参数加密 |
| hmac / sha2 | 本地签发 annotator JWT（HS256） |
| md-5 / sha1 / rand | 平台签名、媒体缓存键、随机作答 |
| serde / serde_json | 配置与接口数据序列化（含宽容反序列化） |
| rusqlite（bundled SQLite） | 每个平台一个本地数据库 |
| ratatui / crossterm | 交互式 TUI（多平台标签页） |
| tokio-util | CancellationToken 任务取消 |
| whisper-candle-core / candle-core | 本地语音转写（纯 Rust） |
| ureq | Whisper 模型下载（支持 HF_ENDPOINT 镜像） |
| regex / ttf-parser / fontdue | 学习通测验题目字体解密 |
| Chrome DevTools Protocol（CDP） | 知到浏览器辅助登录：自动填表并读取 Cookie |
| time | 本地时间戳（日志/凭证有效期） |
| FFmpeg | 媒体转 16k 单声道 wav 前处理 |
| figlet-rs / colored | CLI 启动 Logo 与彩色输出 |
| log / env_logger | 日志框架 |

## 2. 目录结构

```
src/
├── main.rs            # CLI 入口：命令分发、帮助、启动横幅；知到 CLI 也在此
├── lib.rs             # 模块声明
├── config.rs          # config.json 结构、平台子配置、旧配置迁移、校验
├── logging.rs         # 运行日志：logs/ 每次运行独立文件、3 天清理、TUI 静默
├── llm.rs             # OpenAI 兼容接口（重试、reasoning_content 兜底、投票）
├── solve.rs           # U 校园作答策略 + KNOWN_REPLY_TYPES
├── probe.rs           # U 校园未适配题型检测、报告生成、前端 bundle 检索
├── transcribe.rs      # 媒体转写：vtt 解析 + ffmpeg + whisper，本地缓存
├── reporter.rs        # CLI/TUI 共用事件（TaskDone/TaskSkipped/Score…）
├── dump_text.rs       # dump-text 全量抓取（CLI 与 TUI 共用）
├── dump.rs            # dump 目录/题型名/状态同步入口
├── db.rs              # SQLite：每平台一个库、按账号隔离、schema/迁移/查询
├── preview.rs         # 任务组只读预览（debug/TUI 共用）
├── api/               # U 校园 HTTP 实现
│   ├── login.rs       # SSO 登录/刷新、profile、annotator token、bundle 取参
│   ├── session.rs     # Session：默认请求头、Cookie/JWT、401 自愈、URL 构造
│   ├── content.rs     # 拉取任务内容 + AES 解密
│   ├── course.rs      # 课程/单元进度、任务树构建与筛选、课程列表兜底
│   ├── parser.rs      # 解密后的模块/子题解析、HTML 清洗、媒体 URL 提取
│   ├── bbs.rs         # 讨论区接口：查询/创建主题、发表评论
│   ├── submit.rs      # 提交/标记已看 payload、限频识别、得分解析
│   ├── serde_util.rs  # 宽容反序列化助手（null/浮点/数字字符串）
│   └── user_module.rs # 用户作答记录查询（预留）
├── platform/          # 多平台抽象层
│   ├── mod.rs         # Site 分发、AnyPlatform、Platform trait
│   ├── types.rs       # 平台无关模型：Course/Chapter/TaskPoint/TaskKind
│   ├── chaoxing/      # 学习通实现
│   │   ├── mod.rs     # ChaoxingClient（登录/课程/章节/任务/测验/分数）
│   │   ├── cli.rs     # chaoxing 子命令
│   │   ├── session.rs # Cookie 会话、重定向、抓取
│   │   ├── login.rs   # 账密登录（AES-CBC）
│   │   ├── course.rs  # 课程/章节/任务点/cards
│   │   ├── video.rs   # 视频进度上报
│   │   ├── quiz.rs    # 章节测验：解析/pos/提交/分数
│   │   └── font.rs    # 题目字体解密
│   └── zhihuishu/     # 知到实现
│       ├── mod.rs     # ZhihuishuClient（章节/作业/试卷/交卷/视频）
│       ├── session.rs # Cookie 会话
│       ├── crypto.rs  # AES-CBC 加解密
│       ├── login.rs   # 浏览器辅助登录（Edge/Chrome + CDP 读取 Cookie）
│       ├── cdp.rs     # 极简 Chrome DevTools Protocol 客户端
│       └── solve.rs   # 作业逐题 LLM 作答
├── core/
│   ├── planner.rs     # 学习计划：按 learning_strategy 列任务
│   └── runner.rs      # U 校园执行器：解析→求解→提交/跳过、限频重试、得分落库
└── tui/
    ├── mod.rs         # 终端初始化/事件循环/退出恢复
    ├── app.rs         # AppState、PlatformTab、后台任务
    ├── update.rs      # U 校园键盘/鼠标事件与状态更新
    ├── ui.rs          # 标签栏、U 校园各界面渲染
    ├── cx.rs          # 学习通标签页：状态、后台任务、渲染、按键
    ├── zhs.rs         # 知到标签页：章节/作业/视频
    ├── events.rs      # crossterm 输入线程
    └── logger.rs      # log 转发到 TUI 日志面板

assets/
├── font_ref.bin       # 学习通字体解密特征表（0.9MB，必需）
└── font_candidates.txt

examples/
├── decode_font.rs     # 单题字体解密调试
└── gen_font_ref.rs    # 生成字体特征表
```

## 3. 总体架构

```
config.json
  ├─ 公共配置（LLM/转写/间隔/llm_votes）
  └─ unipusai / chaoxing / zhihuishu 子配置
        │
        ├─ U 校园：Session → content 解密 → parse_group → solve_module
        │            → build_answer_payload → submit → 得分/状态 → unipusai.db
        │
        ├─ 学习通：ChaoxingClient → 课程/章节/任务点 → 视频上报 / 测验作答
        │            → quiz_results → chaoxing.db
        │
        └─ 知到：ZhihuishuClient → 章节/作业/试卷 → 视频上报（作业作答/交卷当前关闭）
```

- CLI 与 TUI 共用业务层；U 校园执行流程通过 `reporter::ReportEvent` 向终端/TUI 上报。
- TUI 的 U 校园、学习通（`CxState`）、知到（`ZsState`）标签页各自持有独立状态、日志、运行任务与取消令牌，互不干扰。
- 知到由 CLI 与 TUI 的 `F3` 标签页直接调用，不进入 `Platform` trait/`AnyPlatform`，也暂不落库。

## 4. 配置与数据库

### 4.1 配置

- `config.rs::Config`：顶层公共字段 + `unipusai` / `chaoxing` / `zhihuishu` 子对象。
- 旧版扁平配置在 `load` 时自动迁移到 `unipusai`。
- `site` 为默认平台标识；日常命令由子命令显式区分平台。
- `learning_strategy == "learn_all_compulsory_course"`（兼容旧拼写 `...compusory...`）时只处理必修，其它值包含选修。
- `llm_votes`：同题多次向 LLM 提问取多数，平票时带候选再问一次。

### 4.2 数据库

每个平台一个库：`dump_text/<site>.db`（`db::db_path_for_site`）。

主要表：

- `tasks`：账号、单元索引/单元 id/名称、group_id、tab_type、题型、kind、必修、完成、原始内容、解密 JSON、course_id、更新时间；唯一键 `(account, unit_id, group_id)`。
- `modules`：任务模块（类型/replyType/instanceId/答题说明/材料/字幕/词库）。
- `questions`：子题（replyType/questionType/题干/选项 JSON）。
- `media`：媒体 URL 与转写全文/错误。
- `vocabulary`：单词卡（单词/发音）。
- `quiz_results`：学习通测验结果（账号、课程、章节、任务、分数/题数/已答数）。
- `meta`：`(account, key)`，常用键 `course_id`、`course_name:<course_id>`、`last_score:<course_id>`。

`db::Db { conn, site, account }` 封装按账号隔离的读写。旧版 `dump.db` / `dump-<open_id>.db` 仅保留，不自动迁移；`db::open_for(open_id)` 是旧接口兼容。

## 5. U 校园实现

### 5.1 登录与会话（`api/login.rs`、`api/session.rs`）

- `ensure_login`：
  - jwt 剩余 > 24h：直接使用；
  - 否则先刷新 `refresh_token`，失败再用 `username`/`password` 登录；
  - 检测 jwt 中 `username` 与配置不一致时强制重新登录；
  - 凭证写回 `config.json`。
- 登录请求：SSO 账密字段 AES-128-CBC 加密后提交；返回 jwt/refresh_token/open_id。
- `x_annotator_auth_token`：按前端同算法本地签发 HS256 JWT（默认 1 年、剩余 <30 天续签）；签名参数按需从 ucontent 前端 bundle 提取（支持 401 时重提取重签）。
- `Session` 统一注入认证头；请求 401 时按 "jwt 刷新 → annotator 重签 → 提示手动兜底" 阶梯重试。

### 5.2 内容获取与解析（`api/content.rs`、`api/parser.rs`）

- content 接口返回密文：`unipus.<hex>` 或 `<hex>`；key = `"1a2b3c4d" + k` 前 16 字节，AES-128-ECB + ZeroPadding。
- `parse_group` 输出 `ParsedGroup { modules: Vec<Module> }`：
  - `Module`：instance_id、module_type、reply_type、direction、material、media_sources、transcript、word_bank、children。
  - `ChildQ`：question_type、reply_type、question_text、options、option_count。
- HTML 会去标签；媒体 URL 与 WEBVTT 字幕单独抽取，供转写使用。

### 5.3 作答策略（`solve.rs`）

`KNOWN_REPLY_TYPES`：`singlechoice` / `multichoice` / `fillblank` / `text-area` / `bankedcloze` / `dropdownSelection` / `discussion` / `vocabulary`。

- 选择/多选：材料+题干+选项 → LLM 只回选项字母；`parse_single`/`parse_multi` 规范化。
- 填空/简答：整组批量，按 `1.xxx 2.xxx` 编号输出后用 `parse_banked` 拆分。
- 选词填空：模块词库 + 编号空格，答案归一到词库原词。
- 下拉选择：材料中的 `____n____` 与子题按序对应，一次批量作答，逐空规范化为选项标签。
- 讨论：生成 100–150 词英文发言，发帖到 BBS 后再标记完成。
- vocabulary：无需作答，直接标记已看。
- 空 `reply_type` 的容器模块（video-popup/exit-ticket 等）按子题类型判断。
- LLM 失败可按 `fallback_on_llm_failure` 随机/占位兜底。

### 5.4 媒体转写（`transcribe.rs`）

- 模块既有媒体又无文本/字幕时触发。
- vtt/srt 直接解析；音频/视频下载 → ffmpeg 转 16k 单声道 wav → 本地 whisper 转写。
- 按 URL SHA1 缓存到 `.media_cache/`；Whisper 模型缓存在 `~/.cache/whisper-candle/`。
- TUI 模式下静默第三方库输出（`logging::TUI_ACTIVE`）。

### 5.5 提交与判分（`api/submit.rs`、`core/runner.rs`）

- 载荷：`quesDatas`（每模块一个 instance，每子题 answer JSON）、`groupId`、`isCompleted`、`thirdPartyJudges`、`submitType`、`courseId`、`openId` 等。
- `submitType=1` 作答提交；`submitType=2` 标记已看（text/video、纯讨论、单词卡、浏览类页面）。
- 子题值约定：单选/下拉 `"A"`，多选 `"A,B"`，选词填空单词本身，填空/简答文本。
- 判分解析 `parse_submit_score`：
  - 逐题分：`data.state.score`；
  - 百分比：`data.state.score_pct` / `real_score_pct` / `data.record_grade.scorePct`（0–1 自动 ×100）/ `score_avg` / 逐题均值兜底。
- 限频：`code=600001/600002` 或 `msg` 含"操作过于频繁" → 冷却 180s 递增，最多 5 次。
- 最近得分写入 `meta` 的 `last_score:<course_id>`，CLI/TUI 读取展示。

### 5.6 未适配题型探针（`probe.rs`）

- `detect`：有子题且模块/子题 replyType 不在已知表即判定未适配；空 replyType 容器看子题类型。
- `run`/`group`/TUI 遇到未适配题型：生成 `reports/<题型>__<groupId>.md` 并跳过（不调 LLM、不提交）。
- 报告内容：完整解密 JSON、模块/子题概览、解析器未消费字段、拟提交 payload、适配检查清单；`--submit` 可附服务端响应；`--bundle` 可检索 ucontent 前端 bundle。
- `probe --all-unknown` 从当前账号库筛选未通过且题型未知的任务组批量生成报告。

### 5.7 讨论区（`api/bbs.rs`）

```
生成发言（direction + 讨论问题 → LLM）
  → 查/建主题（utopic）
  → 查重（本人已回复则跳过）
  → 发帖（ureply/add）
  → 标记完成（submitType=2）
```

BBS 成功返回 `code=1`（非 0）；`class_id`/`curricula_id` 为必填。

## 6. 多平台层

### 6.1 抽象与状态

- `platform/mod.rs`：`Platform` trait（登录/课程/章节/任务点）+ `AnyPlatform`。
- 目前 `AnyPlatform` 只包含 Chaoxing；知到由 `platform/zhihuishu` 独立实现，CLI 与 TUI 的 `F3` 标签页直接调用，不进入 trait/`AnyPlatform`。
- 平台数据统一映射为 `platform/types.rs` 的 `Course` / `Chapter` / `TaskPoint`。

### 6.2 学习通（chaoxing，实验性，当前不可用）

- 登录：AES-128-CBC/PKCS7（key=iv `u2oh6Vu^HWe4_AES`）+ Base64。
- 会话：Cookie + 重定向跟随；课程/章节/任务点来自课程页与 `mArg` attachment。
- 视频进度 `enc`：`md5("[clazzId][userid][jobid][objectId][playingTime*1000][d_yHJ!$pdA~5][duration*1000][clipTime]")`；dtoken 会轮换，不能复用。
- 测验 `pos`：复刻 `enc_js_work.js` 的 `getEnc()`（逐字节 XOR + seed）。
- 题目字体加密：内嵌 `font-cxsecret` 子集；用 `assets/font_ref.bin` 特征表做 32×32 位图匹配，`ttf-parser`/`fontdue` 渲染比对。
- 提交响应不含分数，需再请求批阅页解析"本次成绩 N 分"。
- TUI：`F2` 标签页，`CxState` 独立维护课程/章节/任务点/日志/运行状态；`a` 测验、`v` 视频、`A` 全自动、`Esc` 取消。
- 测验结果写入 `chaoxing.db` 的 `quiz_results`，任务列表显示分数。

### 6.3 知到（zhihuishu，实验性，当前不可用）

- 登录：浏览器辅助登录——启动系统 Edge/Chrome（留空 `browser_path` 时自动查找），自动填账号密码/勾选协议，人工完成滑块后用 CDP 读取 Cookie 并写回配置；也可直接填写 Cookie。
- 加解密：AES-128-CBC/PKCS7，IV `1g3qqdh4jvbskb9x`；学习 key `azp53h0kft7qi78q`，作业 key `onbfhdyvz8x7otrp`。
- 章节：`learning/videolist`；进度：`learning/prelearningNote`。
- 视频上报：`learning/saveDatabaseIntervalTimeV2`；`sdsew` 为明文逐字节 XOR `zzpttjd` 后转 hex；total 用服务端累计时长 `studyTotalTime + delta`，位置单独为 HH:MM:SS。
- 作业：`student/getStudentHomework`（flag 1/2）、`student/doHomework`、`answer/saveStudentAnswer`（按 eid）、`answer/submit`（achieveCount）；提交响应 `rt.score` 即得分。
- **作业作答/提交当前整体关闭**：`HOMEWORK_SUBMIT_ENABLED = false`（服务端新增滑块验证，等待适配）；`zhihuishu solve` 会直接提示不可用，TUI `a` 同样只提示；章节/作业浏览、视频学时仍保留入口。
- 风控：429 退避重试；`-12`（字符点选）/`-13`（人脸）验证码可打开播放页等待人工完成后继续。
- TUI：`F3` 标签页，`ZsState` 独立维护章节/作业/视频/日志；`r` 加载、`Tab` 切换焦点、`v` 本章视频、`V` 全部视频、`s` 查看已提交、`a` 作答（当前提交关闭）、`Esc` 取消。

### 6.4 TUI 标签页（`tui/ui.rs`、`tui/cx.rs`、`tui/zhs.rs`）

- `PlatformTab::{Unipus, Chaoxing, Zhihuishu}`；`F1`/`F2`/`F3` 切换。
- U 校园沿用原 `App` 状态与 `ReportEvent`；学习通使用 `CxState` + `CxEvent`，知到使用 `ZsState` + `ZsEvent`，三者独立。
- 学习通页按键：`j/k` 选择、`Enter` 加载、`t` 刷新任务、`a` 章节测验、`v` 视频补报、`A` 全自动、`Esc` 取消/返回。

## 7. 数据、日志与缓存

| 路径 | 说明 |
| --- | --- |
| `dump_text/unipusai.db` | U 校园题库/任务状态/得分 |
| `dump_text/chaoxing.db` | 学习通测验结果 |
| `logs/unipus-*.log` | 运行日志，保留 3 天 |
| `reports/` | 未适配题型探针报告 |
| `.media_cache/` | 转写结果（URL SHA1 命名） |
| `~/.cache/whisper-candle/` | Whisper 模型 |
| `assets/font_ref.bin` | 学习通字体解密必需资产 |

## 8. 构建、测试与开发注意事项

```powershell
# 构建前先结束正在运行的实例，否则 target 被占用（os error 5）
Get-Process UnipusAI -ErrorAction SilentlyContinue | Stop-Process -Force

cargo test
cargo clippy --release
cargo build --release
```

- release 模式用于正式运行（debug 下 Whisper 转写很慢）。
- clippy 当前有 8 个既有基线警告（`api/course.rs`、`api/parser.rs`、`solve.rs`、`transcribe.rs`），新增代码不应引入新警告。
- 仓库未统一 rustfmt：不要运行全仓 `cargo fmt`，避免大量无关 diff。
- 编辑中文源码请使用编辑工具；不要用 PowerShell `Get-Content/Set-Content` 直接改写（历史上曾损坏 UTF-8 中文）。
- 实机提交验证默认只对已通过任务重做；未通过/未知后果的任务需先确认，两次真实提交间隔 ≥3.5s。
- 学习通/知到当前不稳定、暂不可用，修改相关代码前先确认平台接口是否仍有效。
- 知到作业提交开关为 `HOMEWORK_SUBMIT_ENABLED = false`（服务端滑块验证），恢复前不要打开。

### 当前状态与限制

- 存储路径已统一：`main.rs` 的 `cmd_course`、`probe`、`probe --all-unknown` 均改用 `db::open_site(SITE_UNIPUS)` + 账号参数；`db::open_for(open_id)` 仅保留作旧接口兼容。
- `print_help()` 与 TUI 内置帮助已同步 `F1/F2/F3`、`chaoxing`/`zhihuishu` 子命令和当前数据库路径；后续增删命令/键位时需一并更新这两处。
- `platform/mod.rs` 顶部目录注释已与实际的 `chaoxing/`、`zhihuishu/` 文件对齐。
- 知到暂未落库：`SITE_ZHIHUISHU` 已定义但未使用，`dump_text/zhihuishu.db` 不存在；`AnyPlatform` 目前只包含 Chaoxing，知到由 CLI/TUI 直接调用。
- 知到作业作答/提交关闭：`HOMEWORK_SUBMIT_ENABLED = false`（服务端滑块验证），恢复前不要打开；学习通/知到接口仍可能随平台更新失效。
- 旧版 `dump.db` / `dump-<open_id>.db` 保留但不迁移，这是预期行为。
