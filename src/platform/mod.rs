//! 平台抽象层。
//!
//! 目标：把「网课平台」的差异收敛在各平台的子模块里，上层只面对
//! 统一的数据模型（见 [`types`]）和统一的操作契约（见 [`Platform`]）。
//!
//! 目录约定：
//! ```text
//! src/platform/
//! ├── mod.rs          # 平台枚举、统一入口、trait 契约
//! ├── types.rs        # 平台无关模型（Course/Chapter/TaskPoint…）
//! ├── chaoxing/       # 学习通实现
//! │   ├── mod.rs      # 客户端 + 任务点（mArg）解析
//! │   ├── session.rs  # Cookie 会话 + 重定向 + 抓取
//! │   ├── login.rs    # 账密登录（AES-CBC）
//! │   ├── course.rs   # 课程列表 / 章节树
//! │   ├── font.rs     # 题目字体解密
//! │   ├── video.rs    # 视频进度上报
//! │   ├── quiz.rs     # 章节测验（题目/提交/判分）
//! │   └── cli.rs      # chaoxing 子命令
//! └── zhihuishu/      # 知到 / 智慧树实现
//!     ├── mod.rs      # 客户端（章节/作业/视频学时）
//!     ├── crypto.rs   # AES-CBC 请求体加密
//!     ├── session.rs  # Cookie 会话
//!     ├── login.rs    # 浏览器辅助登录（Edge/Chrome + CDP）
//!     ├── cdp.rs      # 极简 Chrome DevTools Protocol 客户端
//!     └── solve.rs    # 作业逐题 LLM 作答
//! ```
//!
//! 注意：chaoxing / zhihuishu 目前为实验性实现，接口可能随平台更新失效；
//! 阶段状态见 `Readme.md` 与 `ARCHITECTURE.md`。
//! 新增平台时：
//! 1. 在 `config.rs` 增加该平台的配置子结构；
//! 2. 在 `platform/<site>/` 实现客户端；
//! 3. 实现 [`Platform`]（或提供同名方法）并把数据映射为 [`types`]；
//! 4. 在 `AnyPlatform`/CLI 中登记。

pub mod chaoxing;
pub mod zhihuishu;
pub mod types;

pub use types::{Chapter, Course, TaskKind, TaskPoint};

use crate::config::Site;
use anyhow::Result;

/// 各平台客户端的统一契约。
///
/// 使用 `async fn in trait`（Rust 1.75+），因此不能作为 `dyn` 使用；
/// 命令层按 [`Site`] 分支调用具体实现。今后接入新平台时实现本 trait 即可。
#[allow(async_fn_in_trait)]
pub trait Platform: Send {
    /// 平台标识。
    fn site(&self) -> Site;

    /// 确保已登录（必要时自动登录）。
    async fn ensure_login(&mut self) -> Result<()>;

    /// 拉取课程列表。
    async fn fetch_courses(&self) -> Result<Vec<Course>>;

    /// 拉取章节列表。
    async fn fetch_chapters(&self, course: &Course) -> Result<Vec<Chapter>>;

    /// 拉取某章节的任务点。
    async fn fetch_tasks(&self, course: &Course, chapter: &Chapter) -> Result<Vec<TaskPoint>>;
}

/// 运行时多平台分发。
pub enum AnyPlatform {
    Chaoxing(chaoxing::ChaoxingClient),
}

impl AnyPlatform {
    pub fn site(&self) -> Site {
        match self {
            AnyPlatform::Chaoxing(_) => Site::Chaoxing,
        }
    }

    pub fn as_chaoxing_mut(&mut self) -> Option<&mut chaoxing::ChaoxingClient> {
        match self {
            AnyPlatform::Chaoxing(c) => Some(c),
        }
    }

    pub async fn ensure_login(&mut self) -> Result<()> {
        match self {
            AnyPlatform::Chaoxing(c) => c.ensure_login().await,
        }
    }

    pub async fn fetch_courses(&self) -> Result<Vec<Course>> {
        match self {
            AnyPlatform::Chaoxing(c) => c.fetch_courses().await,
        }
    }

    pub async fn fetch_chapters(&self, course: &Course) -> Result<Vec<Chapter>> {
        match self {
            AnyPlatform::Chaoxing(c) => c.fetch_chapters(course).await,
        }
    }

    pub async fn fetch_tasks(&self, course: &Course, chapter: &Chapter) -> Result<Vec<TaskPoint>> {
        match self {
            AnyPlatform::Chaoxing(c) => c.fetch_tasks(course, chapter).await,
        }
    }
}

impl Platform for AnyPlatform {
    fn site(&self) -> Site {
        AnyPlatform::site(self)
    }

    async fn ensure_login(&mut self) -> Result<()> {
        AnyPlatform::ensure_login(self).await
    }

    async fn fetch_courses(&self) -> Result<Vec<Course>> {
        AnyPlatform::fetch_courses(self).await
    }

    async fn fetch_chapters(&self, course: &Course) -> Result<Vec<Chapter>> {
        AnyPlatform::fetch_chapters(self, course).await
    }

    async fn fetch_tasks(&self, course: &Course, chapter: &Chapter) -> Result<Vec<TaskPoint>> {
        AnyPlatform::fetch_tasks(self, course, chapter).await
    }
}

impl Platform for chaoxing::ChaoxingClient {
    fn site(&self) -> Site {
        Site::Chaoxing
    }

    async fn ensure_login(&mut self) -> Result<()> {
        chaoxing::ChaoxingClient::ensure_login(self).await
    }

    async fn fetch_courses(&self) -> Result<Vec<Course>> {
        chaoxing::ChaoxingClient::fetch_courses(self).await
    }

    async fn fetch_chapters(&self, course: &Course) -> Result<Vec<Chapter>> {
        chaoxing::ChaoxingClient::fetch_chapters(self, course).await
    }

    async fn fetch_tasks(&self, course: &Course, chapter: &Chapter) -> Result<Vec<TaskPoint>> {
        chaoxing::ChaoxingClient::fetch_tasks(self, course, chapter).await
    }
}

/// 按配置打开对应平台客户端。
pub fn open(config: &crate::config::Config, config_path: &std::path::Path) -> Result<AnyPlatform> {
    match config.site {
        Site::Chaoxing => Ok(AnyPlatform::Chaoxing(chaoxing::ChaoxingClient::new(
            config.clone(),
            config_path.to_path_buf(),
        )?)),
        Site::Unipusai => anyhow::bail!(
            "平台 unipusai 仍使用原有命令（progress/run/group…）；新平台命令请设置 config.site = \"chaoxing\""
        ),
        Site::Zhihuishu => anyhow::bail!(
            "知到（zhihuishu）请使用 `zhihuishu` 子命令（login/chapters/homeworks/paper/solve/video）"
        ),
    }
}
