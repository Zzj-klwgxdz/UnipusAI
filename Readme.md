# UnipusAI —— U校园 AI 版刷课脚本

本项目是原 Python + Selenium 版（v2.4）的 **Rust 完全重写版**：
不需要浏览器、不需要 WebDriver，纯命令行 + 原生 HTTP 实现，更轻量、更快、更稳定
### 原python项目bug较多，如想用浏览器自动化方案请看[这个](https://github.com/YSJohnson/UnipusAI-Helper),这个项目继承了原python版本的主要功能，并优化了用户体验
### 该项目在测试阶段，可能存在诸多问题，欢迎各位到issue留言
### 因为程序可能对部分题型没有适配完全，所以可能部分题目程序作答提交的成绩为0。请勿无脑使用`run`一键刷题命令，由此导致的一切后果请自行承担
### 本人英语已免修，所以目前只能借用别人的账号调试，并且学业繁忙，更新频率下降
> 原 Python 版本（`Unipus_v2.4.py`、`AudioRecognizer.py`、`EnvironmentChecker.py` 等）已删除。

## 主要功能

- **全自动刷课**：遍历课程全部单元/任务组，自动解析并作答提交，跳过已通过的章节。
- **AI 答题**：接入任意 OpenAI 兼容接口（DeepSeek / Kimi 等），覆盖选择、填空、简答等常见题型。
- **讨论题自动发言**：讨论区（discussion）题型自动生成英文发言并发布到讨论区，再标记任务完成。
- **单词卡/朗读练习**：vocabulary 题型无需作答，自动标记完成；`debug`/`dump-text` 可查看单词表。
- **限频自动重试**：提交命中服务端"操作过于频繁"时，自动等待冷却（递增 180s，最多 5 次）后重试，无需手动干预。
- **本地语音/视频转写**：对无内嵌字幕的音频/视频模块，用 ffmpeg + Whisper 本地转写后作答，不依赖在线语音识别服务。
- **纯命令行工具**：提供 `progress` / `run` / `group` / `debug` / `test-types` / `transcribe` / `dump-text` 等命令，方便调试与验证。
- **爬取课程的目录，题目文本和媒体转写**：`dump-text`工具可以爬取课程题目和媒体转写
- **完成状态跟踪**：dump 文件首行与 `_summary.txt` 记录必修/完成状态，dump 或 run/group 答题后自动刷新

## 示例图片
![dumping](./imgs/dumping.png)
*dump-text*
![running](./imgs/running.png)
*running*
![debug](./imgs/debug.png)
*debug*
## 技术栈

| 组件 | 用途 |
| --- | --- |
| Rust (edition 2024) | 主语言，Tokio 异步运行时 |
| reqwest | HTTP 客户端（rustls、cookie、gzip/brotli） |
| aes / ecb / hex | 题目内容 AES-128-ECB 解密 |
| serde / serde_json | 配置与接口数据序列化 |
| Whisper (whisper-candle-core) | 本地语音转写 |
| FFmpeg | 媒体转 wav 前处理 |

## 项目结构

```
src/
├── main.rs            # CLI 入口与各子命令实现
├── lib.rs             # 模块声明
├── config.rs          # config.json 加载/校验/保存
├── llm.rs             # OpenAI 兼容 LLM 调用（含重试与 reasoning_content 兜底）
├── solve.rs           # 作答策略：选择题/填空/简答 prompt 构造与答案解析
├── transcribe.rs      # 媒体转写：vtt 解析 + ffmpeg + whisper，本地缓存
├── api/
│   ├── session.rs     # HTTP 会话：默认请求头、Cookie/JWT、统一 get/post
│   ├── content.rs     # 拉取任务内容 + AES 解密
│   ├── course.rs      # 课程/单元进度、任务树构建与筛选
│   ├── parser.rs      # 解密后的题目模块/子题解析、HTML 清洗、媒体 URL 提取
│   ├── bbs.rs         # 讨论区接口：查询/创建主题、发表评论（ucloud BBS）
│   ├── submit.rs      # 构造提交/标记已看 payload 并上报
│   └── user_module.rs # 用户作答记录查询（预留）
├── dump.rs            # dump-text 目录结构、文件首行状态、_summary.txt 汇总与 run/group 状态同步
└── core/
    ├── planner.rs     # 学习计划：按 learning_strategy 列出待完成任务
    └── runner.rs      # 执行器：逐任务组解析→作答→提交
```

## 核心实现原理

### 1. 登录态
程序不实现浏览器登录。从浏览器复制登录后的凭证填入 `config.json`：

- `cookie`：浏览器请求头里的 `Cookie`（**必填且唯一需要维护**，其中包含 `jwt=` 登录 token）。
- `authorization`：登录后的 JWT；**可留空或直接删除**，程序会自动使用 cookie 中的 `jwt=`（两者通常是同一个 token）。若两者都填，程序会自动选用 exp 更新的那个，401 时自动回退。
- `x_annotator_auth_token`、`u_school`、`open_id`、`course_id`、`publish_version`：同样从浏览器请求中获取。

`Session` 会为每个请求自动附带这些头，以及固定的 `u-app-id`、`u-platform`、`origin`、`referer` 等。

### 2. 任务发现
- `fetch_course_units` 拉取课程进度，得到全部单元 id。
- 对每个单元 `fetch_unit` 得到任务组（leaf）列表，每个 leaf 含 `tab_type`（`text`/`video`/`task`）、是否必修、是否已通过。
- 按 `learning_strategy` 过滤（`learn_all_compulsory_course` 只处理必修任务）。

### 3. 内容解密
任务内容接口返回的 `content` 是密文：

```
格式: "unipus.<hex>" 或 "<hex>"
密钥: "1a2b3c4d" + k 截取前 16 字节
算法: AES-128-ECB + ZeroPadding
```

`decrypt_content` 按此流程逐块解密、去尾部零填充，得到题目的 JSON。

### 4. 题目解析
`parse_group` 把解密后的 JSON 解析成：

```
ParsedGroup
└── Module（一个模块 = 一道大题）
    ├── module_type / reply_type / direction
    ├── material       阅读/听力材料文本（HTML 去标签）
    ├── media_sources  音频/视频/字幕 URL（用于转写）
    ├── transcript     内嵌 WEBVTT 字幕文本
    └── children       子题列表（题干、选项、option_count）
```

媒体 URL 与内嵌字幕会被单独抽取出来，供转写链路使用。

### 5. 作答策略（`solve.rs`）
- **选择题**（singlechoice / multichoice）：把材料/字幕 + 题干 + 选项拼成 prompt，让 LLM 只回答选项字母；再解析为合法选项（`parse_single` / `parse_multi`）。
- **填空/简答**（fillblank / text-area）：整组拼接为一批题目，要求 LLM 按 `1.xxx 2.xxx` 编号回答，再按序拆分（`parse_banked`）。
- **讨论题**（discussion）：根据答题说明与讨论问题让 LLM 生成 100–150 词英文发言（`solve_discussion`），发帖流程见"讨论题"一节。
- **LLM 失败兜底**：可配置随机作答或返回占位答案。

### 6. 媒体转写（`transcribe.rs`）
当模块**既有媒体又没有文本/字幕**时才触发：

```
vtt/srt 字幕 → 直接下载并解析纯文本
音频/视频   → 下载 → ffmpeg 转 16k 单声道 wav → 本地 whisper 转写（纯 Rust，candle 推理）
```

转变语言为 `auto`（或留空）时不传 `--language` 参数，whisper 自动检测语种；也可指定如 `en`/`zh` 强制语言。

结果按 URL 的 SHA1 缓存到 `.media_cache/`，重复转写秒回。内容解密时抽取到的 WEBVTT 字幕会直接作为 `transcript` 使用，无需跑 whisper。

> 转写使用内置的纯 Rust whisper（`whisper-candle-core`，candle 推理），**无需 Python/.venv**。
> 模型（如 `base`）首次使用时自动从 HuggingFace 下载，缓存到 `~/.cache/whisper-candle/`。
> 国内网络可设置环境变量 `HF_ENDPOINT=https://hf-mirror.com` 走镜像下载。

### 7. 讨论题（`bbs.rs`）
讨论题模块（`replyType=discussion`）的答案不在 submit 接口里，而是发布到讨论区（ucloud BBS）：

```
生成发言（direction + 讨论问题 → LLM 英文发言）
  → 查主题 POST /api/bbs/utopic/page（无主题则先 POST /api/bbs/utopic/add 创建）
  → 查重 POST /api/bbs/ureply/top/page（ownerStatus=true 表示本人已回复，跳过）
  → 发帖 POST /api/bbs/ureply/add {type:2, content, topicId, contentType:"text"}
  → 标记完成（纯讨论组走 submitType=2 的"标记已看"提交）
```

- 讨论题可能挂在 `text`/`video` 叶子下，执行器会先尝试解析内容检测 discussion 再发帖。
- `run`/`group` 会自动完成发帖与标记；`debug <groupId>` 只读预览：显示完整发言草稿与讨论区状态（topicId/是否已回复），不发帖不提交。
- BBS 接口成功返回 `code=1`（非 0）。JWT 自动取 cookie 中的 `jwt=`（或 config 的 authorization，谁的 exp 新用谁，401 自动回退）；两者都过期时提示重新复制 cookie。
- `class_id`（URL 的 `cid`）与 `curricula_id`（URL 的 `cloudCurriculaId`）为讨论题必填配置，缺失时服务端返回"班级Id不能为空"/"Ai版课程Id不能为空"。
- 发帖失败时该任务直接判 FAIL（不标记已看），下次运行会重试。

### 8. 单词卡/朗读练习（vocabulary）
模块类型为 `vocabulary`（`contents[]` 为单词卡：单词、发音、释义、例句）的题型**无需作答**：

- 浏览器在录完音后最终提交的也是 `submitType=2` 的"标记已看"载荷（不含任何录音数据），经实测单独提交即可 `pass=true`。
- 程序对这类模块直接走"标记已看"提交（`task` 叶子同样适用）；`solve`/`debug` 不会因空 `replyType` 报错。
- `debug <groupId>` 显示词表摘要；`dump-text` 会导出含单词卡的 `text`/`video` 任务组（单词 + 官方发音链接）。
- 录音与语音评测走腾讯 SOE WebSocket（`zt.unipus.cn/soe/*` + `wss://speech.unipus.cn/speech/proxy/wss`，含服务端签名），程序未复刻，也不影响任务通过。

### 9. 提交
`build_answer_payload` 构造 `submit` 接口所需的 `quesDatas`（每模块一个 instance，每子题一个 answer JSON），连同 `courseId`、`openId`、`publish_version` 等一并提交。`text`/`video` 类任务、纯讨论组、单词卡等无可答子题的任务只调"标记已看"接口。
提交若命中服务端限频（响应 `code=600001/600002`，或 `msg` 含"操作过于频繁"），程序自动等待冷却（递增 180s，最多 5 次）后重试该次提交，仅重做提交、不重复 LLM 作答。

## 使用方法
### 构建

需要 Rust 工具链：

```bash
cargo build --release
```
**注意使用release模式编译，使用debug模式会导致转写速度大幅下降**
### 依赖

- **ffmpeg**：语音转写前置，需将ffmpeg的bin文件夹加入系统PATH，例如`C:\ffmpeg\bin`
- **whisper 模型**：转写用模型（默认 `base`），首次转写时自动从 HuggingFace 下载并缓存到 `~/.cache/whisper-candle/`；可通过环境变量 `HF_ENDPOINT=https://hf-mirror.com` 使用国内镜像。

不配置这两者时程序仍可运行，但是带语音无字幕的题目会缺少材料。

### 配置

仓库内不包含真实 `config.json`（含隐私凭证，已被 `.gitignore` 排除）。使用前先把模板复制为 `config.json` 再填入自己的信息：

```powershell
copy config.example.json config.json
```

编辑 `config.json`：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `timeout` | 否 | HTTP 超时秒数，默认 10 |
| `cookie` | **是** | 浏览器登录后的 Cookie（含 `jwt=`，是唯一必须维护的登录凭证） |
| `authorization` | 否 | ucontent JWT；**留空即可**，程序会自动用 cookie 中的 `jwt=` 代替 |
| `x_annotator_auth_token` | **是** | 批注鉴权 token |
| `u_school` | **是** | 学校编号 |
| `course_id` | **是** | 课程 id，如 `course-v2:...` |
| `class_id` | 讨论题必填 | 班级 id（页面 URL 的 `cid`），讨论区接口使用 |
| `curricula_id` | 讨论题必填 | AI 版课程 id（页面 URL 的 `cloudCurriculaId`），讨论区接口使用 |
| `open_id` | **是** | 用户 open id |
| `publish_version` | 是 | 课程发布版本号（会自动更新） |
| `api_key` | 是 | 大模型 API key |
| `base_url` | 是 | 大模型地址，如 `https://api.deepseek.com` |
| `model` | 是 | 模型名 |
| `learning_strategy` | 否 | `learn_all`（全部）/ `learn_all_compusory_course`（仅必修） |
| `max_tokens` / `temperature` | 否 | LLM 参数 |
| `fallback_on_llm_failure` | 否 | true表示LLM 失败时随机作答，false表示LLM 失败时直接报错 |
| `whisper_enabled` | 否 | 是否启用本地语音转写 |
| `whisper_model` | 否 | whisper 模型（tiny/base/small） |
| `whisper_language` | 否 | 转写语言，`auto`自动检测/可指定 `en`、`zh` |

#### 各项配置如何获取

以 Microsoft Edge（或 Chrome）为例：

1. 浏览器登录 U校园（`ucontent.unipus.cn`），进入任意课程。
2. 按 `F12` 打开开发者工具 → `Network` 面板 → 勾选保留日志并刷新页面。
3. 过滤 `ucontent.unipus.cn` 的请求，双击打开一个常见接口（如含 `course_progress` / `content` 的请求）。

逐项复制如下：

| 配置项 | 从哪取 |
| --- | --- |
| `cookie` | 该请求 `Headers` → Request Headers → `Cookie` 整条值（含 `jwt=`） |
| `authorization` | 可选。同一请求头里的 `Authorization`（登录 JWT，`eyJ...`）；留空则自动取 cookie 的 `jwt=` |
| `x_annotator_auth_token` | 同一请求头 `x-annotator-auth-token`（若无该头可留空） |
| `u_school` | 主页里的redDot请求里的`u-school`（学校编号，如 `8320`） |
| `open_id` | 请求 URL 路径中的 open_id 段，在`publish_version`同一个页面|
| `course_id` | 请求 URL 路径中的 `course-v2:...` 段（如 `/course/api/v2/course_progress/course-v2:xxx/`） |
| `class_id` / `curricula_id` | 课程页面地址栏 URL 里的 `cid=...` 与 `cloudCurriculaId=...`（如 `pc.html?cid=1840...&cloudCurriculaId=369622`），仅讨论题需要 |
| `publish_version` | `course_progress` 接口响应体 `rt.publish_version` 字段 |
| `api_key` / `base_url` / `model` | 大模型厂商控制台申请（DeepSeek / Moonshot / Kimi 等），如 DeepSeek 平台生成 `sk-xxx`，`base_url=https://api.deepseek.com`，`model=deepseek-v4-flash` |
| `learning_strategy` | 固定值二选一：`learn_all`（全部课程）或 `learn_all_compusory_course`（仅必修） |

**说明：**

- `timeout`、`max_tokens`、`temperature`、`fallback_on_llm_failure`、`whisper_*` 均为可选，按需修改即可。
- 字段中可能有双引号`""`影响（尤其是cookie），导致程序出错，粘贴前需要检查，如果有双引号需要在双引号前加`\`取消转义
- `publish_version` 首次运行 `run` 时检测到变更会自动回写 config.json，可不手工改。
- cookie、authorization 等登录凭证有有效期，失效后需按上述步骤重新复制。**推荐只维护 cookie**：authorization 留空时程序自动使用 cookie 中的 `jwt=`（讨论题 BBS 接口需要 JWT，两者都过期时才会 401）。
![course_id](/imgs/course_id.png)
*course_id*
![x_auth](/imgs/X-Auth.png)
*x_auth*
![cookie](/imgs/cookie.png)
*cookie*
![publish_version](/imgs/publish_version.png)
*publish_version*
![u_school](/imgs/u_school.png)
*u_school*
![class_id](/imgs/class_id.png)
*class_id*
### 命令

#### 运行方式

| 环境 | 命令 |
| --- | --- |
| 源码目录（PowerShell / cmd） | `cargo run --release <命令> [参数]` |
| exe 目录（PowerShell） | `.\UnipusAI.exe <命令> [参数]` |
| exe 目录（cmd） | `UnipusAI <命令> [参数]` |

#### 命令一览

| 命令 | 说明 |
| --- | --- |
| `progress [--names]` | 打印课程全部单元/任务树（按 `learning_strategy` 过滤） |
| `run [--names] [--interval <毫秒>] [unitId...]` | 默认自动完成全课程，也可指定单元 |
| `group <groupId> [--force]` | 直接提交指定任务组（LLM 答题；讨论题自动发帖）；已通过任务默认跳过（不调用 LLM、不提交），`--force` 强制重做 |
| `debug <groupId> [--force]` | 本地求解指定任务组（不提交，用于调试；讨论题显示完整草稿与讨论区状态，单词卡显示词表）；已通过任务默认只做解析预览（不调用 LLM），`--force` 强制生成 |
| `test-types` | 每种题型抽一题测试答题链路（不提交） |
| `transcribe <url>` | 测试媒体转写链路（下载 → ffmpeg → whisper） |
| `dump-text [--names] [--force] [unitId...]` | 打印全部题目文本与媒体转写（不答题）；输出到 `dump_text/{单元序号}_{unitId}/{题型}/{groupId}.txt`，所有叶子全量导出、浏览类页面归入 `view-only/`；文件首行含必修/完成状态（每次刷新，`run`/`group` 完成后自动同步），汇总见 `_summary.txt` |

#### 参数说明

| 参数 | 适用命令 | 说明 |
| --- | --- | --- |
| `--names` | `progress` / `run` / `dump-text` | 显示课程名与单元名（如 新视野大学英语(第四版)读写教程 / U1 Pre-reading activities），结果缓存到 `.unit_labels.json`，不传则不额外请求 |
| `--interval <毫秒>` | `run` | 两次提交间隔，默认 3000ms，如 `--interval 5000` 或 `--interval=5000` |
| `--force` | `dump-text` / `group` / `debug` | dump-text：清空 `dump_text/` 并全量重新生成；group/debug：忽略"已通过"跳过，强制重做/生成 |
| `<unitId...>` | `run` / `dump-text` | 只处理指定单元（可多个）；省略则处理全部单元 |

### 转写与文本导出

- `transcribe <url>` 可对任意媒体 URL 单独验证转写链路，结果按 URL 缓存。
- `dump-text` 遍历全课程（或指定单元），按 `dump_text/{单元序号}_{unitId}/{题型}/{groupId}.txt` 归档：**所有叶子全量导出**；题型目录取 `reply_type`（空则回退 `module_type`）；内容为空/非 JSON/无题目模块的**浏览类页面**归入 `{单元}/view-only/`（记录状态与说明，附原始内容如有）。
  - 每个文件**首行含必修/完成状态**（如 `... (task) | 必修 | 未完成 ====`）：已存在的文件每次运行只刷新状态、不重新抓题（旧版扁平结构升级后建议先执行一次 `--force`）；缺失的才抓取生成（含媒体转写）。
  - `run`/`group` 答题提交成功后，会自动把对应文件首行更新为"已完成"；浏览类页面（task 叶子）在作答时也会直接走"标记已看"提交。
  - `dump_text/_summary.txt` 为状态汇总（更新时间、必修/选修完成统计、按单元统计、带状态的文件清单），覆盖全部任务，每次 dump 或答题完成后自动刷新。

## 测试

```bash
cargo test
```

覆盖内容解密（ZeroPadding）、多选/单选答案解析、编号填空拆分、LLM 地址归一化、VTT 字幕解析、媒体 URL 提取等。
## 建议使用步骤
1. 先使用dump-text 生成所有题目的转写
2. 再使用group命令对每种题型的任务组进行测试，或者用test-types，可以把输出结果给AI分析
3. 如果全部测试通过，则可以使用run命令一键刷完
4. 如果某种题型的分数很低（注意部分题型本来就没有分），且环境均配置好（尤其是ffmpeg和whisper没有配置好会导致程序无法回答包含视频，音频的题目），则可能是程序bug，请向作者报告
5. 如何报告bug：使用debug命令运行一次存在问题的任务组，附上程序输出，并写上错误描述，题目类型，在issue中提出
## 更新日志
### 26/8/10
- 增加了--names参数,修复了banked_cloze类题目的逻辑
### 26/8/15
- 改进了课程名识别逻辑
### 26/8/21
- 增加服务端限频自动冷却
### 26/8/22
- 新发现`https://uai.unipus.cn/api/cmgt/course/getHomeCourseListByStudent`接口，已应用于课程名的精确识别
### 26/9/19
- 修复了选词填空题目获取不到given_words的问题
### 26/9/23
- 新增讨论题（discussion）支持：自动生成英文发言、查询/创建讨论主题、发表评论并标记完成；
- `authorization` 改为可选（留空自动使用 cookie 中的 `jwt=`，按 exp 选新并在 401 时回退）；
- 新增 `class_id`/`curricula_id` 配置项；
- `run`/`group` 自动处理讨论题，`debug` 显示完整草稿与讨论区状态，`dump-text` 支持导出含讨论题的 text/video 组；
- 新增单词卡（vocabulary）支持：无需作答，自动标记完成（实测提交即可 pass），`debug`/`dump-text` 可查看词表；
- `dump-text` 输出改为按单元/题型分目录(`dump_text/{单元序号}_{unitId}/{题型}/{groupId}.txt`)， dump 文件首行增加必修/完成状态，`_summary.txt` 改为状态汇总；`run`/`group` 答题完成后自动同步完成状态；
- `group`/`debug` 支持 `--force`：已通过任务默认跳过作答（`group` 不调用 LLM 不提交、`debug` 只做解析预览），加 `--force` 可强制重做/生成；
- `dump-text` 全量归档所有叶子（含阅读/视频/浏览类页面，浏览类归入 `{单元}/view-only/`），状态跟踪覆盖全部任务；浏览类 task 叶子自动走"标记已看"提交，`debug` 对其友好提示不再报错
- 新增了对无题目类任务的支持例如[Quotation,纯视频页面,长文阅读页面]，程序直接向服务器发送完成标志（经测试已通过）
> 本条在v3.3版本的release还未应用


## 许可证

本项目在 [GNU GPL v3.0](LICENSE) 下发布。
