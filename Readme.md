# UnipusAI —— U 校园 AI 版刷课工具
本项目和原 Python版（v2.4）U校园AI版刷课脚本是同一个仓库，但是推翻了Selenium浏览器自动化方案，改为**Rust HTTP请求方案**：
不需要浏览器、不需要 WebDriver，原生 HTTP 实现，终端CLI或TUI界面支持跨平台，更轻量、更快、更稳定
> 原 Python 版本（`Unipus_v2.4.py`、`AudioRecognizer.py`、`EnvironmentChecker.py` 等）已归档到v2.4分支。
- 如想用浏览器自动化方案请看[v2.4分支（bug较多）](https://github.com/Zzj-klwgxdz/UnipusAI/tree/v2.4)或者[这个（更推荐）](https://github.com/YSJohnson/UnipusAI-Helper),这个项目继承了原python版本的主要功能，并优化了用户体验
 - 该项目在测试阶段，可能存在诸多问题，欢迎各位到issue留言
 - 因为程序可能对部分题型没有适配完全，所以可能部分题目程序作答提交的成绩为0。请勿无脑使用一键刷题命令，由此导致的一切后果请自行承担 
 - 如果你是从视频平台过来的，如果不会使用请不要通过评论区或私信向我提问（大概率不回），请向ai（豆包，deepseek等）提问：“阅读这个项目的最新版本`https://github.com/Zzj-klwgxdz/UnipusAI `，我应该如何使用。” 或者通过agent工具直接让它配置好。

以 **U 校园 AI 版** 为主；学习通 / 知到（智慧树）入口仅作保留，当前不稳定、暂不可用。

U 校园与学习通的网络请求由程序直接完成，不依赖浏览器或 WebDriver；知到登录需要调用系统 Edge/Chrome 完成滑块验证（不使用 WebDriver）。

> ⚠️ **重要提示：学习通（chaoxing）与知到（zhihuishu）当前版本不稳定、暂不可用。**
> 请勿用于实际刷课；相关命令、配置与文档仅保留作开发留档，可能随时失效或报错。
> **目前稳定可用的只有 U 校园（unipusai）。**

> **仅供学习交流，请遵守各平台使用规则。** 使用本工具产生的一切后果由使用者自行承担，禁止售卖或商用。

## 相关文档

- [CHANGELOG.md](CHANGELOG.md)：完整更新日志
- [ARCHITECTURE.md](ARCHITECTURE.md)：程序结构、实现原理与开发说明

## 功能概览

- **平台支持**：U 校园（稳定可用）；学习通、知到（不稳定，暂不可用）。
- **自动登录**：U 校园支持账号密码登录并自动维护凭证。
- **AI 答题**：接入任意 OpenAI 兼容接口，覆盖选择、填空、简答、选词填空、下拉选择、讨论等题型。
- **数据归档**：U 校园 `dump-text` 全量入库，TUI 可离线浏览/预览。
- **得分显示**：U 校园每次提交后 CLI 打印得分、TUI 顶栏实时显示最近得分。
- **未适配题型保护**：遇到尚未支持的题型会自动跳过并生成报告，不会乱提交。
- **多账号隔离**：每个平台一个数据库，库内按账号隔离，换账号互不污染。
- **学习通 / 知到**：相关代码与命令为实验性保留，当前不稳定、暂不可用，详见下方说明。

## 支持平台

| 平台 | 登录方式 | 主要功能 | 状态 |
| --- | --- | --- | --- |
| U 校园 AI 版 | 手机号/密码（推荐）或浏览器 Cookie | 课程/任务、AI 作答提交、讨论区、媒体转写、dump 归档 | **稳定可用** |
| 学习通（超星） | 手机号/密码 或 Cookie | 课程/章节/任务点、视频进度、章节测验 | **不稳定，暂不可用** |
| 知到（智慧树） | 浏览器辅助登录（自动填表 + 人工验证码）或 Cookie | 章节、作业列表/整卷/视频学时 | **不稳定，暂不可用** |

## 快速开始

### 1. 获取程序

- 直接下载 Release（Windows x64）。
- 或从源码构建（需要 Rust 工具链）：

```bash
git clone https://github.com/Zzj-klwgxdz/UnipusAI.git
cd UnipusAI
cargo build --release
```

生成的可执行文件在 `target/release/UnipusAI`（Windows 为 `UnipusAI.exe`）。

### 2. 配置文件

把 `config.example.json` 复制为 `config.json`，至少填写：

- 公共项：`api_key`、`base_url`、`model`
- 你要使用的平台子对象

> 注：下方学习通 / 知到配置示例当前不可用，仅作开发留档；请优先使用 U 校园。

**U 校园最小配置：**

```json
{
  "site": "unipusai",
  "api_key": "sk-...",
  "base_url": "https://api.deepseek.com",
  "model": "deepseek-flash",
  "unipusai": {
    "username": "手机号",
    "password": "密码"
  }
}
```

**学习通最小配置（当前不稳定、暂不可用，仅保留）：**

```json
{
  "site": "chaoxing",
  "api_key": "sk-...",
  "base_url": "https://api.deepseek.com",
  "model": "deepseek-flash",
  "chaoxing": {
    "username": "手机号",
    "password": "密码"
  }
}
```

**知到最小配置（当前不稳定、暂不可用，仅保留）：**

```json
{
  "site": "zhihuishu",
  "api_key": "sk-...",
  "base_url": "https://api.deepseek.com",
  "model": "deepseek-flash",
  "zhihuishu": {
    "username": "手机号（可选，浏览器辅助登录）",
    "password": "密码（可选，验证码需人工完成）",
    "browser_path": "",
    "cookie": "浏览器复制的完整 Cookie（可留空，首次登录后自动写入）",
    "recruit_id": 467029,
    "course_id": 1000009484,
    "recruit_and_course_id": "学习页 stuStudy?recruitAndCourseId= 的值",
    "school_id": 365
  }
}
```

配置说明：

- **U 校园**：`username`/`password` 推荐填写；程序会自动完成登录、刷新凭证、选课等，`cookie`/`open_id`/`class_id`/`curricula_id` 等字段无需手填。登录触发滑块时，可改为从浏览器复制 `cookie` 使用。
- **学习通（暂不可用）**：配置入口仅作开发留档，请勿用于实际使用。
- **知到（暂不可用）**：支持浏览器辅助登录（自动打开 Edge/Chrome 填表，人工完成验证码后保存 Cookie）或直接填写 Cookie；配置入口仅作开发留档，请勿用于实际使用。
- 常用公共项：`interval_ms`（提交间隔，默认 3000）、`llm_votes`（同题多次作答投票，默认 1）、`whisper_*`（转写设置）。
- `unipusai.learning_strategy` 默认只做必修（`learn_all_compulsory_course`）；改成其它值则包含选修任务。
- 旧版扁平配置会自动迁移，无需手动改写。

### 3. 启动

- **推荐**：直接运行 `UnipusAI`（无参数）进入 TUI。
- **CLI**：按下方命令使用。

## TUI 使用

启动后顶部为 `F1 U校园` / `F2 学习通` / `F3 知到` 三个标签页。

> ⚠️ 学习通/知到标签页当前不稳定、暂不可用，仅 U 校园标签页可正常使用。

### U 校园

1. 首次进入按 `c` 选择课程。
2. 按 `d` 导出题目到本地数据库（首次必做，之后可离线浏览）。
3. 按 `p` 预览选中任务，检查题目/材料是否完整。
4. `Enter` 运行选中任务（已通过的任务默认跳过，按 `f` 强制重做）；`R` 运行本单元；`A` 运行全课程。
5. 运行中顶栏实时显示最近一次提交得分；`Esc` 取消运行。

### 学习通（当前不稳定、暂不可用）

> ⚠️ 以下流程仅作开发留档，当前版本请勿实际使用。

1. 按 `F2` 进入学习通页，会自动登录并加载课程。
2. `j/k` 选择课程/章节，`Enter` 加载下一级。
3. 在章节任务页：
   - `a`：自动作答并提交当前章节测验（显示得分）；
   - `v`：按真实节奏补报当前章节视频；
   - `A`：全自动跑完整门课（视频 + 测验，自动跳过已完成项）。
4. `t` 刷新任务点，`Esc` 取消/返回。

### 知到（当前不稳定、暂不可用）

> ⚠️ `F3` 知到标签页仅作开发留档，当前请勿实际使用；知到作业作答/提交已整体关闭（服务端要求滑块验证）。

1. `F3` 进入知到页，按 `r` 加载章节与作业列表。
2. `j/k` 选择，`Tab` 在章节/作业之间切换焦点。
3. `v` 补报当前章节视频，`V` 补报全部章节视频；`Esc` 取消。
4. `s` 切换查看已提交作业；`a` 作答作业（当前整体关闭，仅提示不可用）。

### 按键一览

| 按键 | 作用 |
| --- | --- |
| `F1` / `F2` / `F3` | 切换 U 校园 / 学习通 / 知到标签页（`F2`/`F3` 当前不可用） |
| `↑/↓` `j/k` | 移动选择 |
| `←/→` `h/l` | 折叠 / 展开单元 |
| `Enter` | 单元行=展开；任务行=运行 |
| `f` | 强制运行选中任务（忽略"已通过"） |
| `R` / `A` | 运行本单元 / 运行全课程 |
| `p` / `g` | 预览选中任务 / 生成讨论草稿（需确认） |
| `u` | 预览页：重新抓取当前任务并更新入库 |
| `c` | 选择课程（账号下课程列表） |
| `d` | dump-text 总览与导出（导出后自动刷新） |
| `s` / `w` | 设置 / 保存配置 |
| `r` | 刷新任务树 |
| `Esc` | 运行中取消；其他页面返回 |
| `q` | 退出 |
| 鼠标 | 点击选择/按钮，滚轮滚动列表与日志 |

> TUI 的 U 校园任务树和预览来自本地数据库，首次使用请先按 `d` 导出；空库时会提示。
> 学习通与知到标签页当前不稳定、暂不可用，`F2`/`F3` 仅保留入口。

## CLI 使用

### U 校园

| 命令 | 说明 |
| --- | --- |
| `login [--force]` | 查看登录状态；`--force` 用账号密码强制重新登录 |
| `courses` | 列出账号下的课程 |
| `course [序号\|courseId]` | 选择当前课程（写入配置） |
| `progress [--names]` | 打印课程任务树 |
| `dump-text [--names] [--force] [unitId...]` | 导出题目与媒体转写到本地数据库（不答题） |
| `test-types` | 每种题型抽一题测试答题链路（不提交） |
| `group <groupId> [--force]` | 直接作答并提交指定任务组；已通过默认跳过，`--force` 强制重做 |
| `debug <groupId> [--force]` | 本地求解指定任务组（不提交，用于排查） |
| `run [--names] [--interval <毫秒>] [unitId...]` | 自动完成课程（默认全部单元） |
| `probe <groupId> [--submit] [--bundle] [--force]` | 生成未适配题型报告；`--submit` 真实提交一次取判分响应 |
| `probe --all-unknown [--bundle] [--limit N]` | 批量检查未通过且题型未知的任务组 |
| `transcribe <url>` | 测试媒体转写链路 |

**推荐流程：**

```bash
UnipusAI login
UnipusAI courses
UnipusAI course 1
UnipusAI dump-text --names          # 首次先导出题库
UnipusAI test-types                 # 可选：检查各题型是否能正常作答
UnipusAI group <某个任务组id>        # 可选：单任务验证
UnipusAI run --interval 5000        # 正式刷课
```

说明：

- `group`/`debug` 遇到已通过任务默认跳过，`--force` 才会重做。
- 每次作答提交后会打印 `[SCORE] ...` 得分；标记已看/讨论类任务会显示"无判分数据"。
- 未适配题型不会提交，会生成 `reports/` 报告并跳过。

### 学习通（当前不稳定、暂不可用）

> ⚠️ 以下命令仅作开发留档，当前版本请勿实际使用。

命令前缀：`UnipusAI chaoxing`（可简写 `cx`）。

| 命令 | 说明 |
| --- | --- |
| `chaoxing login` | 登录并缓存 Cookie |
| `chaoxing courses` | 课程列表 |
| `chaoxing chapters <课程序号\|course_id>` | 章节列表 |
| `chaoxing tasks <课程> <章节序号\|章节id>` | 任务点列表（视频/测验） |
| `chaoxing video <课程> <章节> <任务> [--dry]` | 只看视频信息 |
| `chaoxing video <课程> <章节> <任务> --pace 60 [--speed 2] [--from 秒]` | 按真实节奏补报视频进度 |
| `chaoxing quiz <课程> <章节> <任务>` | 只抓取并打印题目（不提交） |
| `chaoxing quiz <课程> <章节> <任务> --solve` | LLM 作答预览（不提交） |
| `chaoxing quiz <课程> <章节> <任务> --solve --submit` | LLM 作答并提交，打印本次成绩 |
| `chaoxing quiz <课程> <章节> <任务> --answers "1:C,2:对" --submit` | 使用手工答案提交 |

示例：

```bash
UnipusAI chaoxing login
UnipusAI chaoxing courses
UnipusAI chaoxing chapters 1
UnipusAI chaoxing tasks 1 8
UnipusAI chaoxing video 1 6 1 --pace 60
UnipusAI chaoxing quiz 1 9 2 --solve --submit
```

说明：

- 课程序号/章节序号/任务序号均来自上一条命令的输出。
- 视频默认"直接上报播放完"通常不会通过（服务端按真实观看时长校验），请用 `--pace`；`--speed` 一般不要超过 2。
- `--pace` 会按真实时间等待，可用 `Esc`（TUI）或 Ctrl+C 中断。
- 也可以直接使用 TUI 的学习通页完成同样操作（该页当前不稳定、暂不可用）。

### 知到（智慧树，当前不稳定、暂不可用）

> ⚠️ 以下命令仅作开发留档，当前版本请勿实际使用。

命令前缀：`UnipusAI zhihuishu`（可简写 `zhs` / `zhidao`）。

| 命令 | 说明 |
| --- | --- |
| `zhihuishu login` | 登录（优先验证现有 Cookie；无 Cookie 时打开浏览器登录，需人工完成验证码） |
| `zhihuishu chapters` | 章节 + 小节（含 lessonId/videoId/时长） |
| `zhihuishu homeworks` | 未提交作业 |
| `zhihuishu submitted` | 已提交作业（含分数） |
| `zhihuishu paper <作业id>` | 打印整卷（选项按 A/B/C/D 显示） |
| `zhihuishu solve <作业id> [--dry\|--submit]` | 作业作答（**当前整体关闭**，命令仅提示不可用） |
| `zhihuishu video --dry` | 查看每节已学时长 |
| `zhihuishu video --lesson <id>` | 按真实节奏补报指定小节 |
| `zhihuishu video --step 100000 --sleep 0 --gap 3` | 整门课批量补报（节间间隔 3 秒） |
| `zhihuishu video --open-browser` | 触发验证码时自动打开播放页 |
| `zhihuishu video --wait-verify` | 验证码时打开播放页，人工完成验证后自动重试 |
| `zhihuishu video --no-wait` | 无人值守：触发验证码时只提示并跳过 |

说明：

- 知到平台当前不可用；以下仅作开发留档：登录/播放有滑块或验证码，遇到 `-12` 风控时可在浏览器播放页完成验证。
- **知到作业提交已暂时关闭**（服务端要求滑块验证，`HOMEWORK_SUBMIT_ENABLED=false`）；视频学时、章节/作业浏览仍保留入口。
- 视频补报建议使用保守节奏（`--step 60`，`--gap 5~10`），避免频繁触发风控。
- 因提交关闭，当前不会真正交卷/产生新分数；`submitted` 可查看历史已提交作业的分数。

## 数据、日志与文件

| 路径 | 内容 |
| --- | --- |
| `config.json` | 账号、Cookie、API Key 等配置（**请勿分享/提交到仓库**） |
| `dump_text/unipusai.db` | U 校园题库、任务状态、最近得分（按账号隔离） |
| `dump_text/chaoxing.db` | 学习通测验结果（平台当前不可用，仅保留） |
| `logs/` | 运行日志，自动保留最近 3 天 |
| `reports/` | 未适配题型报告 |
| `.media_cache/` | 媒体转写缓存（可删除，删除后重新转写） |

- 旧版 `dump.db` / `dump-<open_id>.db` 会保留、不会自动迁移；新版数据写入上述平台数据库。
- 换电脑时复制 `config.json` 即可；如需保留题库/成绩，再复制对应的 `dump_text/*.db`。
- 知到的作业/章节数据实时从平台获取，不落库（平台当前不可用）。

## 得分与未适配题型

- **U 校园**：每次提交后 CLI 打印 `[SCORE] ... 提交后得分 100%（各题 ...）`；TUI 顶栏显示"最近提交得分"，并按课程持久化，重启/换课后自动恢复。
- **学习通 / 知到**：平台当前不稳定、暂不可用，相关得分展示暂不保证。
- **未适配题型**：程序会自动跳过并生成 `reports/<题型>__<任务id>.md`。可把报告发给作者或在 issue 中附上，以便后续适配。
- 手动生成报告：`probe <任务id> --bundle`；`--submit` 会用兜底答案真实提交一次并记录响应（仅建议用于未通过任务）。

## 常见问题

**1. 登录失败 / 滑块 / 验证码**
- U 校园：优先使用账号密码；触发滑块时从浏览器复制 `cookie` 到 `unipusai.cookie`。
- 学习通 / 知到：平台当前不稳定、暂不可用，不建议尝试登录。以下内容仅作留档——
  - 学习通：登录失败时可从浏览器复制完整 Cookie 到 `chaoxing.cookie`。
  - 知到：支持浏览器辅助登录（自动填表，人工完成验证码后自动保存 Cookie）或手动 Cookie；视频触发 `-12` 时可在播放页完成验证。

**2. 音频/视频题没有材料，或作答质量差**
- 需要本机安装 **ffmpeg** 并加入 PATH；Whisper 模型首次使用会自动下载。
- 国内网络可设置 `HF_ENDPOINT=https://hf-mirror.com` 加速模型下载。
- 模型可在配置中调整：`whisper_model`（如 `base`/`small`）、`whisper_language`。

**3. 提交太快 / 提示"操作过于频繁"**
- U 校园：增大 `interval_ms`（如 5000~10000），或减少并发操作；程序会自动冷却重试。
- 学习通/知到：平台当前不可用，相关限频/节奏说明仅作开发留档。

**4. 分数为 0 或偏低**
- 确认 `api_key`/`base_url`/`model` 可用（先用 `test-types` 或 `debug` 检查）。
- 部分题型可能尚未适配，程序会生成 `reports/` 报告并跳过。
- 媒体题需要 ffmpeg/Whisper 正常，否则缺少材料。

**5. TUI 任务树为空**
- 先在 TUI 按 `d` 导出，或在 CLI 执行 `dump-text`；之后按 `r` 刷新。
- 确认当前课程选择正确（`c` 或 CLI `course`）。

**6. 换账号/换电脑**
- 每个账号在对应平台数据库中独立存储，换账号不会串数据。
- 换电脑复制 `config.json`；如需保留题库/成绩，再复制 `dump_text/<平台>.db`。

**7. 学习通 / 知到当前不可用**
- 两个平台目前均不稳定、暂不可用；相关命令、配置和文档仅作开发留档，可能随时失效或报错，请勿用于实际刷课。
- 相关功能短期内不保证可用；如遇问题可保留 `logs/` 日志和命令输出，但请以 U 校园功能为主。

## 更新日志

完整更新记录见 [CHANGELOG.md](CHANGELOG.md)；程序结构与实现细节见 [ARCHITECTURE.md](ARCHITECTURE.md)。

## 许可证

GPL-3.0-or-later
