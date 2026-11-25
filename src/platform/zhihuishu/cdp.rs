//! 极简 CDP（Chrome DevTools Protocol）WebSocket 客户端。
//!
//! 只用于「打开浏览器让用户登录，然后取 Cookie」这一个场景：
//! - ws:// 明文，无 TLS/代理；
//! - 文本帧、JSON-RPC，忽略服务端事件；
//! - 不依赖额外 crate（tokio TcpStream + 手写握手/帧）。

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct CdpWs {
    stream: TcpStream,
    buf: Vec<u8>,
    next_id: u64,
}

impl CdpWs {
    /// 连接 `ws://127.0.0.1:<port>/devtools/page/<id>`。
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let rest = ws_url
            .strip_prefix("ws://")
            .context("只支持 ws:// 形式的 CDP 地址")?;
        let (host_port, path) = match rest.split_once('/') {
            Some((hp, p)) => (hp, format!("/{p}")),
            None => (rest, "/".to_string()),
        };
        let stream = TcpStream::connect(host_port)
            .await
            .with_context(|| format!("连接 CDP 失败: {host_port}"))?;
        let key = ws_key();
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        let mut s = Self {
            stream,
            buf: Vec::new(),
            next_id: 0,
        };
        s.stream.write_all(req.as_bytes()).await?;
        // 读取握手响应头
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            let n = s.stream.read(&mut byte).await?;
            if n == 0 {
                bail!("CDP 握手连接被关闭");
            }
            head.push(byte[0]);
            if head.len() > 64 * 1024 {
                bail!("CDP 握手响应过大");
            }
        }
        let head_text = String::from_utf8_lossy(&head);
        if !head_text.starts_with("HTTP/1.1 101") && !head_text.starts_with("HTTP/1.0 101") {
            bail!(
                "CDP 握手失败: {}",
                head_text.lines().next().unwrap_or_default()
            );
        }
        Ok(s)
    }

    async fn send_text(&mut self, text: &str) -> Result<()> {
        let payload = text.as_bytes();
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x81); // FIN + text
        let mask_bit = 0x80u8;
        match payload.len() {
            n if n < 126 => frame.push(mask_bit | n as u8),
            n if n <= u16::MAX as usize => {
                frame.push(mask_bit | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(mask_bit | 127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        frame.extend_from_slice(&mask);
        for (i, b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i % 4]);
        }
        self.stream.write_all(&frame).await?;
        Ok(())
    }

    async fn read_frame(&mut self) -> Result<Vec<u8>> {
        loop {
            let h = self.read_exact(2).await?;
            let opcode = h[0] & 0x0F;
            let masked = h[1] & 0x80 != 0;
            let mut len = (h[1] & 0x7F) as usize;
            if len == 126 {
                let b = self.read_exact(2).await?;
                len = u16::from_be_bytes([b[0], b[1]]) as usize;
            } else if len == 127 {
                let b = self.read_exact(8).await?;
                len = u64::from_be_bytes(b.try_into().unwrap()) as usize;
            }
            let mask = if masked {
                Some(self.read_exact(4).await?)
            } else {
                None
            };
            let mut payload = self.read_exact(len).await?;
            if let Some(m) = mask {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= m[i % 4];
                }
            }
            match opcode {
                0x1 => return Ok(payload),
                0x8 => bail!("CDP WebSocket 被关闭"),
                0x9 => {
                    // ping -> pong（忽略）
                    continue;
                }
                _ => continue,
            }
        }
    }

    async fn read_exact(&mut self, n: usize) -> Result<Vec<u8>> {
        while self.buf.len() < n {
            let mut tmp = [0u8; 4096];
            let got = self.stream.read(&mut tmp).await?;
            if got == 0 {
                bail!("CDP 连接关闭");
            }
            self.buf.extend_from_slice(&tmp[..got]);
        }
        let out: Vec<u8> = self.buf.drain(..n).collect();
        Ok(out)
    }

    /// 发送 JSON-RPC 调用并等待同 id 的结果。
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({"id": id, "method": method, "params": params});
        self.send_text(&msg.to_string()).await?;
        loop {
            let text = self.read_frame().await?;
            let v: Value = serde_json::from_slice(&text).context("CDP 返回非 JSON")?;
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = v.get("error") {
                    bail!("CDP {method} 失败: {err}");
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            // 其它消息是事件，忽略
        }
    }

    /// `Runtime.evaluate` 的便捷封装，返回字符串结果。
    pub async fn eval(&mut self, expr: &str) -> Result<String> {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({"expression": expr, "returnByValue": true, "awaitPromise": true}),
            )
            .await?;
        let v = r.pointer("/result/value").cloned().unwrap_or(Value::Null);
        Ok(match v {
            Value::String(s) => s,
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            other => other.to_string(),
        })
    }
}

fn ws_key() -> String {
    // 16 字节随机（时间/进程/地址混合即可，握手不需要密码学强度）
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut bytes = [0u8; 16];
    let mut x = now ^ (std::process::id() as u128) << 64;
    for b in bytes.iter_mut() {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *b = (x >> 33) as u8;
    }
    STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 需要本机有 CDP 端口时手动跑：
    /// ZHS_CDP_WS=ws://127.0.0.1:9300/devtools/page/<id> cargo test -- --ignored cdp_handshake
    #[tokio::test]
    #[ignore]
    async fn cdp_handshake_against_env() {
        let url = std::env::var("ZHS_CDP_WS").expect("需要 ZHS_CDP_WS");
        let mut ws = CdpWs::connect(&url).await.expect("connect");
        let v = ws.eval("1+1").await.expect("eval");
        assert_eq!(v, "2");
    }
}
