//! 知到（智慧树）浏览器辅助登录。
//!
//! 登录页有滑块/点选验证码，纯 HTTP 无法自动通过；这里用系统自带的
//! Edge/Chrome（Windows 默认就有）打开登录页：
//! 1. 自动填账号密码、勾选协议、点登录；
//! 2. 用户在浏览器里手动完成验证码；
//! 3. 程序通过 CDP 读取 Cookie，写回配置，之后就像以前一样用 Cookie 跑。
//!
//! 不依赖 Playwright/Selenium/驱动，只需要本机浏览器 + 程序自身。

use super::ZhihuishuConfig;
use super::cdp::CdpWs;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const LOGIN_URL: &str = "https://login.zhihuishu.com/";
const COOKIE_URLS: &[&str] = &[
    "https://login.zhihuishu.com/",
    "https://studyvideoh5.zhihuishu.com/",
    "https://onlineexamh5new.zhihuishu.com/",
    "https://www.zhihuishu.com/",
    // 真正发请求的两个 API 域名：按它们取 Cookie 才等价于浏览器实际发送的头
    "https://studentexam-api.zhihuishu.com/",
    "https://studyservice-api.zhihuishu.com/",
];

pub struct LoginOutcome {
    pub cookie: String,
    pub uuid: String,
}

/// 打开浏览器让用户登录，返回 Cookie。
pub async fn login_with_browser(
    cfg: &ZhihuishuConfig,
    timeout_secs: u64,
) -> Result<LoginOutcome> {
    let exe = find_browser(cfg.browser_path.as_str())?;
    let port = free_port()?;
    let profile = std::env::temp_dir().join(format!("unipus-zhs-profile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    std::fs::create_dir_all(&profile).ok();
    let mut child = Command::new(&exe)
        .args([
            format!("--remote-debugging-port={port}"),
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--new-window".into(),
            LOGIN_URL.into(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("启动浏览器失败: {}", exe.display()))?;

    let result = run_flow(&child, port, cfg, timeout_secs).await;
    // 关闭浏览器并清理临时 profile
    let _ = child.kill();
    let _ = child.wait();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = std::fs::remove_dir_all(&profile);
    result
}

async fn run_flow(
    _child: &Child,
    port: u16,
    cfg: &ZhihuishuConfig,
    timeout_secs: u64,
) -> Result<LoginOutcome> {
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    // 等待 CDP 端口就绪
    let mut ready = false;
    for _ in 0..60 {
        if let Ok(r) = client.get(format!("{base}/json/version")).send().await
            && r.status().is_success()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !ready {
        bail!("浏览器调试端口未就绪（可能被安全软件拦截）");
    }

    // 找到登录页 target
    let ws_url = find_page_ws(&client, &base)
        .await
        .context("没找到登录页标签页，请重试")?;

    let mut cdp = CdpWs::connect(&ws_url).await?;
    cdp.call("Network.enable", json!({})).await.ok();

    // 自动填账号密码（拿不到输入框也不影响，用户可手填）
    if !cfg.username.is_empty() && !cfg.password.is_empty() {
        let js = fill_script(&cfg.username, &cfg.password);
        let _ = cdp.eval(&js).await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = cdp
            .eval(
                "(function(){var c=document.querySelector('input[type=checkbox]');\
                 if(c&&!c.checked){c.click();}return 'ok';})()",
            )
            .await;
        let _ = cdp
            .eval(
                "(function(){var b=[...document.querySelectorAll('button,div,span')]\
                 .find(e=>/(^|\\s)登录(\\s|$)/.test((e.textContent||'').trim())&&e.offsetParent);\
                 if(b){b.click();return 'clicked';}return 'nf';})()",
            )
            .await;
    }
    eprintln!("已打开浏览器，请完成登录/验证码；程序会自动等待并读取登录态…");

    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs.max(30));
    let mut cookie = String::new();
    let mut uuid = String::new();
    let mut last_hint = std::time::Instant::now() - Duration::from_secs(30);
    let mut primed = 0u32;
    let mut printed_names = false;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        // 不依赖某个标签页的 URL：登录成功可能在新标签页完成，
        // 直接看整浏览器的 Cookie，并调用接口校验。
        let cookies = match get_cookies(&mut cdp).await {
            Ok(c) => c,
            Err(e) => {
                if last_hint.elapsed() > Duration::from_secs(10) {
                    eprintln!("读取 Cookie 失败（{e}），尝试重新连接浏览器…");
                    last_hint = std::time::Instant::now();
                }
                if let Some(ws) = find_page_ws(&client, &base).await
                    && let Ok(n) = CdpWs::connect(&ws).await
                {
                    cdp = n;
                    let _ = cdp.call("Network.enable", json!({})).await;
                }
                continue;
            }
        };
        if !cookies.iter().any(|(k, _)| k == "CASLOGC") {
            if last_hint.elapsed() > Duration::from_secs(10) {
                eprintln!("尚未检测到登录 Cookie，请在浏览器完成登录…");
                last_hint = std::time::Instant::now();
            }
            continue;
        }
        if primed < 2 {
            // 登录后浏览器可能停在 AI 中心，没有访问过学习/考试域名，
            // 应用侧 Cookie（jt-cas/SERVERID 等）还没下发；依次打开学习页、考试页。
            let url = if primed == 0 {
                if !cfg.recruit_and_course_id.is_empty() {
                    format!(
                        "https://studyvideoh5.zhihuishu.com/stuStudy?recruitAndCourseId={}",
                        cfg.recruit_and_course_id
                    )
                } else {
                    "https://www.zhihuishu.com/".to_string()
                }
            } else {
                "https://onlineexamh5new.zhihuishu.com/stuExamWeb.html".to_string()
            };
            eprintln!("正在打开页面以激活登录态（{}/2）…", primed + 1);
            let _ = cdp.call("Page.navigate", json!({ "url": url })).await;
            primed += 1;
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        let candidate = cookies
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("; ");
        match cookie_valid(&candidate, cfg).await {
            Ok(()) => {
                cookie = candidate;
                uuid = extract_uuid(&cookies);
                break;
            }
            Err(why) => {
                if last_hint.elapsed() > Duration::from_secs(10) {
                    eprintln!(
                        "登录态尚未生效（Cookie {} 个）：{}",
                        cookies.len(),
                        why
                    );
                    if !printed_names {
                        let mut names: Vec<&str> =
                            cookies.iter().map(|(k, _)| k.as_str()).collect();
                        names.sort_unstable();
                        eprintln!("  当前 Cookie: {}", names.join(","));
                        let href = cdp.eval("location.href").await.unwrap_or_default();
                        eprintln!("  当前页面: {}", href);
                        printed_names = true;
                    }
                    last_hint = std::time::Instant::now();
                }
            }
        }
        if last_hint.elapsed() > Duration::from_secs(10) {
            let has_server = cookies.iter().any(|(k, _)| k == "SERVERID");
            eprintln!(
                "登录态尚未生效（Cookie {} 个，SERVERID={}），继续等待…",
                cookies.len(),
                has_server
            );
            last_hint = std::time::Instant::now();
        }
    }
    if cookie.is_empty() {
        bail!("等待登录超时（{}s），请重试", timeout_secs);
    }
    Ok(LoginOutcome { cookie, uuid })
}

/// 用接口校验 Cookie 是否真的已登录（没配课程时无法校验，视为通过）。
async fn cookie_valid(cookie: &str, cfg: &ZhihuishuConfig) -> Result<(), String> {
    if cfg.recruit_id == 0 || cfg.course_id == 0 {
        return Ok(());
    }
    use super::crypto;
    let sess = super::session::ZsSession::new(cookie, 10).map_err(|e| format!("{e:#}"))?;
    let form = crypto::exam_form(&serde_json::json!({
        "recruitId": cfg.recruit_id.to_string(),
        "courseId": cfg.course_id,
        "flag": 1,
        "pageSize": 1,
        "pageNum": 0,
    }));
    match sess
        .post_form(
            "https://studentexam-api.zhihuishu.com/studentExam/gateway/t/v1/student/getStudentHomework",
            form,
            "https://onlineexamh5new.zhihuishu.com/stuExamWeb.html",
        )
        .await
    {
        Ok(v) => {
            if v.get("status").and_then(Value::as_str) == Some("200") {
                Ok(())
            } else {
                Err(format!("接口返回: {}", v.to_string().chars().take(160).collect::<String>()))
            }
        }
        Err(e) => Err(format!("{e:#}")),
    }
}

async fn find_page_ws(client: &reqwest::Client, base: &str) -> Option<String> {
    for _ in 0..40 {
        if let Ok(r) = client.get(format!("{base}/json/list")).send().await
            && let Ok(list) = r.json::<Value>().await
            && let Some(arr) = list.as_array()
        {
            let mut any = None;
            for t in arr {
                let url = t.get("url").and_then(Value::as_str).unwrap_or("");
                let ws = t.get("webSocketDebuggerUrl").and_then(Value::as_str);
                if let Some(ws) = ws {
                    if url.contains("login.zhihuishu.com") {
                        return Some(ws.to_string());
                    }
                    any = any.or_else(|| Some(ws.to_string()));
                }
            }
            if any.is_some() {
                return any;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}

async fn get_cookies(cdp: &mut CdpWs) -> Result<Vec<(String, String)>> {
    // 优先取整个浏览器的 Cookie：登录后部分关键 Cookie（如 SERVERID）
    // 可能只在 CAS/服务域上，按 URL 过滤会漏掉。
    let mut arr = Vec::new();
    // 先按真实请求的 URL 取（等价于浏览器 Cookie 头），再退回全量
    if let Ok(r) = cdp
        .call("Network.getCookies", json!({ "urls": COOKIE_URLS }))
        .await
        && let Some(list) = r.get("cookies").and_then(Value::as_array)
    {
        arr = list.clone();
    }
    if arr.is_empty()
        && let Ok(r) = cdp.call("Network.getAllCookies", json!({})).await
        && let Some(list) = r.get("cookies").and_then(Value::as_array)
    {
        arr = list
            .iter()
            .filter(|c| {
                c.get("domain")
                    .and_then(Value::as_str)
                    .map(|d| d.contains("zhihuishu"))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
    }
    // 同名 Cookie 可能在不同子域各有一份；浏览器按域名匹配发送，
    // 我们手工拼 Cookie 头时必须去重，优先父域 `.zhihuishu.com`。
    let mut picked: Vec<(String, String, String)> = Vec::new(); // name, domain, value
    for c in arr {
        let name = c.get("name").and_then(Value::as_str).unwrap_or_default();
        let value = c.get("value").and_then(Value::as_str).unwrap_or_default();
        let domain = c.get("domain").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let better = match picked.iter_mut().find(|(n, _, _)| n == name) {
            Some(slot) => {
                if domain.len() < slot.1.len() {
                    slot.1 = domain.to_string();
                    slot.2 = value.to_string();
                }
                false
            }
            None => true,
        };
        if better {
            picked.push((name.to_string(), domain.to_string(), value.to_string()));
        }
    }
    Ok(picked
        .into_iter()
        .map(|(n, _, v)| (n, v))
        .collect::<Vec<_>>())
}

fn extract_uuid(cookies: &[(String, String)]) -> String {
    for (k, v) in cookies {
        if k == "CASLOGC" {
            let decoded = urldecode(v);
            if let Ok(j) = serde_json::from_str::<Value>(&decoded)
                && let Some(u) = j.get("uuid").and_then(Value::as_str)
            {
                return u.to_string();
            }
        }
        if k == "uuid" && !v.is_empty() {
            return v.clone();
        }
    }
    String::new()
}

fn fill_script(user: &str, pass: &str) -> String {
    let user = serde_json::to_string(user).unwrap_or_else(|_| "\"\"".into());
    let pass = serde_json::to_string(pass).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(function(){{
  function set(el,v){{ if(!el) return false;
    var d=Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value');
    d.set.call(el,v); el.dispatchEvent(new Event('input',{{bubbles:true}}));
    el.dispatchEvent(new Event('change',{{bubbles:true}})); return true; }}
  var ins=[...document.querySelectorAll('input')];
  var u=ins.find(i=>/手机号|用户名|账号/.test(i.placeholder||''));
  var p=ins.find(i=>/密码/.test(i.placeholder||''));
  var ok1=set(u,{user}), ok2=set(p,{pass});
  return JSON.stringify({{ok1:ok1,ok2:ok2}});
}})()"#
    )
}

fn find_browser(configured: &str) -> Result<PathBuf> {
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        if p.exists() {
            return Ok(p);
        }
        bail!("config.zhihuishu.browser_path 不存在: {}", p.display());
    }
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application\msedge.exe",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Ok(p);
        }
    }
    bail!("未找到 Edge/Chrome，请在 config.zhihuishu.browser_path 指定浏览器路径")
}

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").context("分配调试端口失败")?;
    Ok(l.local_addr()?.port())
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冒烟：会真的打开浏览器 15 秒后自动关闭（默认 ignore）。
    #[tokio::test]
    #[ignore]
    async fn browser_login_smoke() {
        let cfg = ZhihuishuConfig {
            username: "17539162007".into(),
            password: "x".into(),
            ..ZhihuishuConfig::default()
        };
        let r = login_with_browser(&cfg, 15).await;
        assert!(r.is_err(), "未登录时应超时返回错误");
    }
}
