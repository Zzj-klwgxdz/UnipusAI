# UnipusAI —— U校园 AI 版刷课脚本（Rust 版）

本项目是原 Python + Selenium 版（v2.4）的 **Rust 完全重写版**：
不需要浏览器、不需要 WebDriver，纯命令行 + 原生 HTTP 实现，更轻量、更快、更稳定
### 该项目在测试阶段，可能存在诸多问题，欢迎各位到issue留言
> 原 Python 版本（`Unipus_v2.4.py`、`AudioRecognizer.py`、`EnvironmentChecker.py` 等）及 PyInstaller 打包产物均已从仓库移除，仅保留 Rust 实现。

## 主要功能

- **全自动刷课**：遍历课程全部单元/任务组，自动解析并作答提交，跳过已通过的章节。
- **AI 答题**：接入任意 OpenAI 兼容接口（DeepSeek / Moonshot / Kimi 等），覆盖选择、填空、简答等常见题型。
- **本地语音/视频转写**：对无内嵌字幕的音频/视频模块，用 ffmpeg + OpenAI Whisper 本地转写后作答，不依赖在线语音识别服务。
- **纯命令行工具**：提供 `progress` / `run` / `group` / `debug` / `test-types` / `transcribe` / `dump-text` 等命令，方便调试与验证。

## 技术栈

| 组件 | 用途 |
| --- | --- |
| Rust (edition 2024) | 主语言，Tokio 异步运行时 |
| reqwest | HTTP 客户端（rustls、cookie、gzip/brotli） |
| aes / ecb / hex | 题目内容 AES-128-ECB 解密 |
| serde / serde_json | 配置与接口数据序列化 |
| OpenAI Whisper (Python CLI) | 本地语音转写 |
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
│   ├── submit.rs      # 构造提交/标记已看 payload 并上报
│   └── user_module.rs # 用户作答记录查询（预留）
└── core/
    ├── planner.rs     # 学习计划：按 learning_strategy 列出待完成任务
    └── runner.rs      # 执行器：逐任务组解析→作答→提交
```

## 核心实现原理

### 1. 登录态
程序不实现浏览器登录。从浏览器复制登录后的凭证填入 `config.json`：

- `cookie`：浏览器请求头里的 `Cookie`。
- `authorization`：登录后的 JWT（开发者工具 Network 里任意 `ucontent.unipus.cn` 请求的 `Authorization` 头）。
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
- **LLM 失败兜底**：可配置随机作答或返回占位答案。

### 6. 媒体转写（`transcribe.rs`）
当模块**既有媒体又没有文本/字幕**时才触发：

```
```
vtt/srt 字幕 → 直接下载并解析纯文本
音频/视频   → 下载 → ffmpeg 转 16k 单声道 wav → 本地 whisper CLI 转写
```

转变语言为 `auto`（或留空）时不传 `--language` 参数，whisper 自动检测语种；也可指定如 `en`/`zh` 强制语言。

结果按 URL 的 SHA1 缓存到 `.media_cache/`，重复转写秒回。内容解密时抽取到的 WEBVTT 字幕会直接作为 `transcript` 使用，无需跑 whisper。

### 7. 提交
`build_answer_payload` 构造 `submit` 接口所需的 `quesDatas`（每模块一个 instance，每子题一个 answer JSON），连同 `courseId`、`openId`、`publish_version` 等一并提交。`text`/`video` 类任务只调"标记已看"接口。

## 使用方法

### 构建

需要 Rust 工具链（建议 latest stable）：

```bash
cargo build --release
```

产物在 `target/release/UnipusAI.exe`。

### 依赖（可选）

- **ffmpeg**：语音转写前置，需加入 PATH。
- **whisper**：本地语音转写，需安装 `openai-whisper`（仓库内 `.venv` 已包含；`which("whisper")` 会先在 PATH 中查找）。

不配置这两者时程序仍可运行，只是带语音无字幕的题目会缺少材料。

### 配置

仓库内不包含真实 `config.json`（含隐私凭证，已被 `.gitignore` 排除）。使用前先把模板复制为 `config.json` 再填入自己的信息：

```powershell
copy config.example.json config.json
```

编辑 `config.json`：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `timeout` | 否 | HTTP 超时秒数，默认 10 |
| `cookie` | **是** | 浏览器登录后的 Cookie |
| `authorization` | **是** | ucontent JWT（Authorization 头） |
| `x_annotator_auth_token` | 否 | 批注鉴权 token（从浏览器复制） |
| `u_school` | 否 | 学校编号 |
| `course_id` | **是** | 课程 id，如 `course-v2:...` |
| `open_id` | **是** | 用户 open id |
| `publish_version` | 是 | 课程发布版本号（会自动更新） |
| `api_key` | 是 | 大模型 API key |
| `base_url` | 是 | 大模型地址，如 `https://api.deepseek.com` |
| `model` | 是 | 模型名 |
| `learning_strategy` | 否 | `learn_all`（全部）/ `learn_all_compusory_course`（仅必修） |
| `max_tokens` / `temperature` | 否 | LLM 参数 |
| `fallback_on_llm_failure` | 否 | LLM 失败时随机/占位作答 |
| `whisper_enabled` | 否 | 是否启用本地语音转写 |
| `whisper_model` | 否 | whisper 模型（tiny/base/small） |
| `whisper_language` | 否 | 转写语言，`auto`（自动检测，默认）/ 可指定 `en`、`zh` |

#### 各项配置如何获取

以 Microsoft Edge（或 Chrome）为例：

1. 浏览器登录 U校园（`ucontent.unipus.cn`），进入任意课程。
2. 按 `F12` 打开开发者工具 → `Network` 面板 → 勾选保留日志并刷新页面。
3. 过滤 `ucontent.unipus.cn` 的请求，双击打开一个常见接口（如含 `course_progress` / `content` 的请求）。

逐项复制如下：

| 配置项 | 从哪取 |
| --- | --- |
| `cookie` | 该请求 `Headers` → Request Headers → `Cookie` 整条值 |
| `authorization` | 同一请求头里的 `Authorization`（登录 JWT，`eyJ...`） |
| `x_annotator_auth_token` | 同一请求头 `x-annotator-auth-token`（若无该头可留空） |
| `u_school` | 同一请求头 `u-school`（学校编号，如 `8320`） |
| `open_id` | 请求 URL 路径中的 open_id 段，或用 [jwt.io](https://jwt.io) 解码 `authorization` 载荷里的 `openId` |
| `course_id` | 请求 URL 路径中的 `course-v2:...` 段（如 `/course/api/v2/course_progress/course-v2:xxx/`） |
| `publish_version` | `course_progress` 接口响应体 `rt.publish_version` 字段 |
| `api_key` / `base_url` / `model` | 大模型厂商控制台申请（DeepSeek / Moonshot / Kimi 等），如 DeepSeek 平台生成 `sk-xxx`，`base_url=https://api.deepseek.com`，`model=deepseek-v4-flash` |
| `learning_strategy` | 固定值二选一：`learn_all`（全部课程）或 `learn_all_compusory_course`（仅必修） |

说明：

- `timeout`、`max_tokens`、`temperature`、`fallback_on_llm_failure`、`whisper_*` 均为可选，按需修改即可。
- `publish_version` 首次运行 `run` 时检测到变更会自动回写 config.json，可不手工改。
- cookie、authorization 等登录凭证有有效期，失效后需按上述步骤重新复制。
![course_id](/imgs/course_id.png)
![x_auth](/imgs/X-Auth.png)
![cookie](/imgs/cookie.png)
![publish_version](/imgs/publish_version.png)
![open_id](/imgs/how%20to%20get%20openid.png)

### 命令
```powershell
cargo run <params>
```
### params
```
progress               打印课程全部单元/任务树（按 learning_strategy 过滤）
run [unitId...]        默认自动完成全课程，也可指定单元
group <groupId>        直接提交指定任务组（LLM 答题）
debug <groupId>        本地求解指定任务组（不提交，用于调试）
test-types             每种题型抽一题测试答题链路（不提交）
transcribe <url>       测试媒体转写链路（下载→ffmpeg→whisper）
dump-text [unitId...]  打印全部题目文本与媒体转写，每任务组一个文件到 dump_text/（不答题）
```

### 转写与文本导出

- `transcribe <url>` 可对任意媒体 URL 单独验证转写链路，结果按 URL 缓存。
- `dump-text` 遍历全课程（或指定单元），把所有模块的材料文本、内嵌字幕、媒体转写文字以及每题的题干与选项写入 `dump_text/<groupId>.txt`，并汇总到 `dump_text/_summary.txt`。启动时会先清空该目录。用于核对题目识别是否完整、媒体转写是否正确。

## 测试

```bash
cargo test
```

覆盖内容解密（ZeroPadding）、多选/单选答案解析、编号填空拆分、LLM 地址归一化、VTT 字幕解析、媒体 URL 提取等。

## 许可证

本项目在 [MIT License](LICENSE) 下发布。
