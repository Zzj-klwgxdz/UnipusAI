//! 学习通视频：元信息（dtoken）与进度上报。
//!
//! `enc` 公式已用 5 次真实请求验证：
//! ```text
//! md5("["clazzId"]["userid"]["jobid"]["objectId"]"
//!     + "[" playingTime*1000 "][" d_yHJ!$pdA~5 "][" duration*1000 "][" clipTime "]")
//! ```

use crate::platform::chaoxing::session::Session;
use anyhow::{Context, Result};
use md5::{Digest, Md5};
use serde_json::Value;

const STATUS_URL: &str = "https://mooc1.chaoxing.com/ananas/status";
const REPORT_BASE: &str = "https://mooc1.chaoxing.com/mooc-ans/multimedia/log/a";
/// 进度 enc 的固定盐。
pub const SALT: &str = "d_yHJ!$pdA~5";

/// 视频元信息。
#[derive(Debug, Clone, Default)]
pub struct VideoStatus {
    pub object_id: String,
    pub dtoken: String,
    pub duration: u64,
    pub http: String,
    pub filename: String,
}

/// 拉取视频元信息（含 dtoken）。
pub async fn fetch_status(session: &Session, object_id: &str, cfid: &str) -> Result<VideoStatus> {
    let url = if cfid.is_empty() {
        format!("https://mooc1.chaoxing.com/ananas/status/{}", object_id)
    } else {
        format!("{}/{}", STATUS_URL, object_id)
    };
    let params: Vec<(&str, String)> = if cfid.is_empty() {
        vec![("flag", "normal".into()), ("ro", "0".into())]
    } else {
        vec![
            ("k", cfid.to_string()),
            ("flag", "normal".into()),
            ("ro", "0".into()),
        ]
    };
    let v = session.get_json_params(&url, &params).await?;
    let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("");
    if status != "success" {
        anyhow::bail!(
            "视频状态接口返回异常：{}",
            v.get("msg").and_then(|x| x.as_str()).unwrap_or(status)
        );
    }
    Ok(VideoStatus {
        object_id: object_id.to_string(),
        dtoken: v
            .get("dtoken")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        duration: v.get("duration").and_then(|x| x.as_u64()).unwrap_or(0),
        http: v
            .get("http")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        filename: v
            .get("filename")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// 计算进度上报的 `enc`。
pub fn progress_enc(
    clazz_id: &str,
    userid: &str,
    jobid: &str,
    object_id: &str,
    playing_time_ms: u64,
    duration_ms: u64,
    clip_time: &str,
) -> String {
    let raw = format!(
        "[{}][{}][{}][{}][{}][{}][{}][{}]",
        clazz_id, userid, jobid, object_id, playing_time_ms, SALT, duration_ms, clip_time
    );
    format!("{:x}", Md5::digest(raw.as_bytes()))
}

/// 一次进度上报的参数。
#[derive(Debug, Clone)]
pub struct Report {
    pub clazz_id: String,
    pub userid: String,
    pub jobid: String,
    pub object_id: String,
    /// 课程 cfid（mArg.defaults.cFid），视频状态接口的 `k` 参数。
    pub cfid: String,
    pub course_id: String,
    pub cpi: String,
    pub other_info: String,
    pub duration: u64,
    pub att_duration: i64,
    pub att_duration_enc: String,
    pub video_face_capture_enc: String,
    /// 已播放秒数。
    pub playing_time: u64,
    /// 上报区间，如 `0_896`。
    pub clip_time: String,
    /// 拖动标记：首次上报 3，之后 0。
    pub isdrag: u32,
}

/// 上报一次播放进度，返回服务端 JSON（含 `isPassed`）。
///
/// 注意：服务端按**真实观看时长**累计，单次跳变通常不会通过；
/// 需要真正完成时按 `--pace` 逐步上报。
pub async fn report(session: &Session, r: &Report) -> Result<Value> {
    let status = fetch_status(session, &r.object_id, &r.cfid).await?;
    if status.dtoken.is_empty() {
        anyhow::bail!("未获取到 dtoken，无法上报进度");
    }
    report_with_dtoken(session, &status.dtoken, r).await
}

/// 用已知 dtoken 上报（批量/循环时避免重复请求 status）。
pub async fn report_with_dtoken(session: &Session, dtoken: &str, r: &Report) -> Result<Value> {
    if r.userid.is_empty() || r.jobid.is_empty() || r.object_id.is_empty() {
        anyhow::bail!("进度上报缺少 userid/jobid/objectId");
    }
    // mArg 里的 otherInfo 末尾可能带 "&courseId=..."（页面拼 URL 时会被拆成独立参数），
    // 这里先剥掉，避免 percent-encode 后把 & 也编码进去导致服务端校验失败。
    let other_info = r
        .other_info
        .split("&courseId=")
        .next()
        .unwrap_or_default()
        .to_string();
    let url = format!("{}/{}/{}", REPORT_BASE, r.cpi, dtoken);
    let playing_ms = r.playing_time * 1000;
    let duration_ms = r.duration * 1000;
    let enc = progress_enc(
        &r.clazz_id,
        &r.userid,
        &r.jobid,
        &r.object_id,
        playing_ms,
        duration_ms,
        &r.clip_time,
    );
    let rt = format!("{:.3}", rand::random::<f64>() * 0.1 + 0.85);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let params: Vec<(&str, String)> = vec![
        ("clazzId", r.clazz_id.clone()),
        ("playingTime", r.playing_time.to_string()),
        ("duration", r.duration.to_string()),
        ("clipTime", r.clip_time.clone()),
        ("objectId", r.object_id.clone()),
        ("otherInfo", other_info),
        ("courseId", r.course_id.clone()),
        ("jobid", r.jobid.clone()),
        ("userid", r.userid.clone()),
        ("isdrag", r.isdrag.to_string()),
        ("view", "pc".to_string()),
        ("enc", enc),
        ("rt", rt),
        ("videoFaceCaptureEnc", r.video_face_capture_enc.clone()),
        ("dtype", "Video".to_string()),
        ("_t", now_ms.to_string()),
        ("attDuration", r.att_duration.to_string()),
        ("attDurationEnc", r.att_duration_enc.clone()),
        ("courseEngineInfo", "false".to_string()),
    ];
    // 不做自动重试：同一 _t 的重复请求可能被服务端视为重放而返回 403。
    session
        .get_json_params(&url, &params)
        .await
        .with_context(|| format!("进度上报失败（objectId={}）", r.object_id))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enc_matches_known_formula() {
        // 用可手算的样例验证拼接顺序与毫秒换算
        let got = progress_enc("c", "u", "j", "o", 59000, 896000, "0_896");
        let raw = "[c][u][j][o][59000][d_yHJ!$pdA~5][896000][0_896]";
        let expect = format!("{:x}", Md5::digest(raw.as_bytes()));
        assert_eq!(got, expect);
        assert_eq!(got.len(), 32);
    }
}
