use regex::Regex;

/// 移植自 process.ts 的 normalizeVoiceForEdgeReadAloud。
/// Edge read-aloud 的 SSML `<voice name>` 不接受 Azure 风格的冒号命名。
pub fn normalize_voice_for_edge(voice_raw: &str) -> String {
    let v = voice_raw.trim();
    if !v.contains(':') {
        return v.to_string();
    }

    let plain = Regex::new(r"(?i)^(.+):Neural$").unwrap();
    if let Some(c) = plain.captures(v) {
        let base = &c[1];
        return if base.ends_with("Neural") {
            base.to_string()
        } else {
            format!("{base}Neural")
        };
    }

    let variant =
        Regex::new(r"(?i)^(.+):(DragonHDFlashLatestNeural|DragonHD\w+|\w+Neural)$").unwrap();
    if let Some(c) = variant.captures(v) {
        let base = &c[1];
        return if base.ends_with("Neural") {
            base.to_string()
        } else {
            format!("{base}Neural")
        };
    }

    v.replacen(':', "", 1)
}

/// 取音色名的前两段作为语言标签，不足两段时回退 zh-CN。
pub fn voice_to_lang(voice: &str) -> String {
    let parts: Vec<&str> = voice.split('-').collect();
    if parts.len() >= 2 {
        format!("{}-{}", parts[0], parts[1])
    } else {
        "zh-CN".to_string()
    }
}

use crate::tts::backend::{Synthesized, TtsBackend, WordTiming};
use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Once;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// 来自 edge-tts (`rany2/edge-tts`) `constants.py`，2026-09-01 核实值。
/// 见 docs/edge-protocol.md ——brief 里的 `1-130.0.2849.68` 已过时，实测应为此值。
const TRUSTED_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const WSS: &str = "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";
const CHROMIUM_FULL_VERSION: &str = "143.0.3650.75";
const CHROMIUM_MAJOR_VERSION: &str = "143";
const OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

static RUSTLS_PROVIDER_INIT: Once = Once::new();

/// 安装一次 rustls ring CryptoProvider（rustls 0.23 不再自动探测后端）。
/// 多次调用会 panic，用 `Once` 保证幂等——见 docs/edge-protocol.md「遇到并解决的问题」。
fn ensure_rustls_provider() {
    RUSTLS_PROVIDER_INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("安装 rustls ring CryptoProvider 失败");
    });
}

/// 进程级时钟偏移校正值（秒）。首次遇到 403/407 时从服务端 `Date` 头计算并写入，
/// 后续所有 `EdgeBackend` 实例共享同一份校正——本机时钟偏差不会在两次调用之间变化。
static CLOCK_SKEW_SECS: AtomicI64 = AtomicI64::new(0);

fn local_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间早于 UNIX_EPOCH")
        .as_secs() as i64
}

/// 应用当前已知时钟偏移校正后的“服务端视角”unix 秒数。
fn corrected_unix_secs() -> u64 {
    let skew = CLOCK_SKEW_SECS.load(Ordering::Relaxed);
    (local_unix_secs() + skew).max(0) as u64
}

/// 尝试从 403/407 响应的 `Date` 头计算时钟偏移并写入全局状态。
/// 返回 true 表示成功计算出偏移（值得重试一次）。
fn try_correct_clock_skew(
    resp: &tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
) -> bool {
    let status = resp.status();
    if status != 403 && status != 407 {
        return false;
    }
    let Some(date_val) = resp.headers().get("date").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Ok(server_time) = chrono::DateTime::parse_from_rfc2822(date_val) else {
        return false;
    };
    let skew = server_time.timestamp() - local_unix_secs();
    CLOCK_SKEW_SECS.store(skew, Ordering::Relaxed);
    true
}

/// Windows FILETIME ticks（100ns 单位）向下取整到 5 分钟，拼 token 后 SHA256 大写十六进制。
/// 算法已对照 edge-tts drm.py::generate_sec_ms_gec 核实，见 docs/edge-protocol.md。
fn sec_ms_gec(unix_secs: u64) -> String {
    let mut ticks: u128 = unix_secs as u128 + 11_644_473_600;
    ticks -= ticks % 300;
    let ticks_100ns = ticks * 10_000_000;
    let mut h = Sha256::new();
    h.update(format!("{}{}", ticks_100ns, TRUSTED_TOKEN).as_bytes());
    hex::encode_upper(h.finalize())
}

/// SSML 是 XML，文本中的这三个字符必须转义，否则服务端会拒绝整条消息。
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub struct EdgeBackend {
    voice: String,
    /// 保留字段以匹配既定接口形状；SSML 的 `xml:lang` 实测应固定写 `en-US`，
    /// 与音色语言无关（见 docs/edge-protocol.md），因此本字段当前未在协议消息中使用。
    #[allow(dead_code)]
    lang: String,
    timeout: Duration,
}

impl EdgeBackend {
    pub fn new(voice: &str, timeout: Duration) -> Self {
        ensure_rustls_provider();
        let voice = normalize_voice_for_edge(voice);
        let lang = voice_to_lang(&voice);
        Self { voice, lang, timeout }
    }

    fn build_url() -> String {
        format!(
            "{WSS}?TrustedClientToken={TRUSTED_TOKEN}&ConnectionId={}\
             &Sec-MS-GEC={}&Sec-MS-GEC-Version=1-{CHROMIUM_FULL_VERSION}",
            uuid::Uuid::new_v4().simple(),
            sec_ms_gec(corrected_unix_secs())
        )
    }

    /// 只保留经消融实验验证为必需的四个头（Origin/User-Agent/Pragma/Cache-Control）。
    /// `Accept-Encoding`、`Accept-Language`、`Cookie: muid=...` 已实测验证非必需
    /// （逐个去掉、以及三个一起去掉均合成成功），详见 task-6-report.md。
    fn build_request(url: &str) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
        let mut req = url.into_client_request()?;
        let h = req.headers_mut();
        h.insert(
            "Origin",
            "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".parse()?,
        );
        h.insert(
            "User-Agent",
            format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/{CHROMIUM_MAJOR_VERSION}.0.0.0 Safari/537.36 \
                 Edg/{CHROMIUM_MAJOR_VERSION}.0.0.0"
            )
            .parse()?,
        );
        h.insert("Pragma", "no-cache".parse()?);
        h.insert("Cache-Control", "no-cache".parse()?);
        Ok(req)
    }

    /// 建立一次 WebSocket 连接。遇到 403/407 时读取服务端 `Date` 头校正本机时钟偏移后
    /// 重试一次（仅一次，避免死循环）——见顾虑 2：时钟偏移校正。
    async fn connect(&self) -> Result<WsStream> {
        let mut retried = false;
        loop {
            let url = Self::build_url();
            let req = Self::build_request(&url)?;
            match tokio_tungstenite::connect_async(req).await {
                Ok((ws, _resp)) => return Ok(ws),
                Err(tokio_tungstenite::tungstenite::Error::Http(resp)) if !retried => {
                    retried = true;
                    if try_correct_clock_skew(&resp) {
                        continue;
                    }
                    bail!(
                        "连接 Edge read-aloud 端点被拒绝：HTTP {}（时钟偏移校正不适用）",
                        resp.status()
                    );
                }
                Err(e) => return Err(e).context("连接 Edge read-aloud 端点失败"),
            }
        }
    }
}

impl TtsBackend for EdgeBackend {
    async fn synth(&self, text: &str) -> Result<Synthesized> {
        tokio::time::timeout(self.timeout, self.synth_inner(text))
            .await
            .map_err(|_| anyhow::anyhow!("合成超时（{}ms）", self.timeout.as_millis()))?
    }
}

impl EdgeBackend {
    async fn synth_inner(&self, text: &str) -> Result<Synthesized> {
        let mut ws = self.connect().await?;

        let ts = chrono::Utc::now()
            .format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)");

        ws.send(Message::Text(
            format!(
                "X-Timestamp:{ts}\r\nContent-Type:application/json; charset=utf-8\r\n\
                 Path:speech.config\r\n\r\n\
                 {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":\
                 {{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"true\"}},\
                 \"outputFormat\":\"{OUTPUT_FORMAT}\"}}}}}}}}\r\n"
            )
            .into(),
        ))
        .await?;

        let req_id = uuid::Uuid::new_v4().simple().to_string();
        // xml:lang 固定写 'en-US'，与 voice 的实际语言无关——edge-tts mkssml() 的既有
        // 行为，已实测核实（docs/edge-protocol.md）。brief 原假设“需要与音色语言匹配”
        // 是错的，不要改回 self.lang。
        ws.send(Message::Text(
            format!(
                "X-RequestId:{req_id}\r\nContent-Type:application/ssml+xml\r\n\
                 X-Timestamp:{ts}Z\r\nPath:ssml\r\n\r\n\
                 <speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
                 <voice name='{}'><prosody pitch='+0Hz' rate='+0%' volume='+0%'>{}</prosody>\
                 </voice></speak>",
                self.voice,
                xml_escape(text)
            )
            .into(),
        ))
        .await?;

        let mut audio: Vec<u8> = Vec::new();
        let mut timings: Vec<WordTiming> = Vec::new();

        while let Some(msg) = ws.next().await {
            match msg? {
                Message::Binary(b) => {
                    if b.len() < 2 {
                        bail!("二进制帧过短：{} 字节", b.len());
                    }
                    let hdr_len = u16::from_be_bytes([b[0], b[1]]) as usize;
                    if b.len() < 2 + hdr_len {
                        bail!("头部长度越界");
                    }
                    let header = String::from_utf8_lossy(&b[2..2 + hdr_len]);
                    if header.contains("Path:audio") {
                        audio.extend_from_slice(&b[2 + hdr_len..]);
                    }
                }
                Message::Text(t) => {
                    if t.contains("Path:audio.metadata") {
                        collect_word_boundaries(&t, &mut timings);
                    }
                    if t.contains("Path:turn.end") {
                        let _ = ws.close(None).await;
                        break;
                    }
                }
                Message::Close(c) => bail!("服务端关闭连接：{c:?}"),
                _ => {}
            }
        }

        if audio.is_empty() {
            bail!("未收到音频负载（音色 {} 可能无效）", self.voice);
        }
        Ok(Synthesized {
            audio,
            timings: if timings.is_empty() { None } else { Some(timings) },
        })
    }
}

/// 从 audio.metadata 文本帧的 JSON 体中提取 WordBoundary 事件。
/// 首版只收集不使用，为后续词级字幕对齐预留。解析失败静默跳过——
/// 元数据缺失或格式变化不应导致合成失败。
fn collect_word_boundaries(frame: &str, out: &mut Vec<WordTiming>) {
    let Some(body) = frame.split("\r\n\r\n").nth(1) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return;
    };
    let Some(items) = v.get("Metadata").and_then(|m| m.as_array()) else {
        return;
    };
    for item in items {
        if item.get("Type").and_then(|t| t.as_str()) != Some("WordBoundary") {
            continue;
        }
        let Some(d) = item.get("Data") else { continue };
        let offset = d.get("Offset").and_then(|o| o.as_u64()).unwrap_or(0);
        let duration = d.get("Duration").and_then(|o| o.as_u64()).unwrap_or(0);
        let text = d
            .get("text")
            .and_then(|t| t.get("Text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        // Edge 的时间单位是 100 纳秒 tick
        out.push(WordTiming {
            text,
            offset_ms: offset / 10_000,
            duration_ms: duration / 10_000,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_plain_neural_suffix() {
        assert_eq!(
            normalize_voice_for_edge("zh-CN-Yunjian:Neural"),
            "zh-CN-YunjianNeural"
        );
    }

    #[test]
    fn normalize_leaves_valid_name_untouched() {
        assert_eq!(
            normalize_voice_for_edge("zh-CN-YunjianNeural"),
            "zh-CN-YunjianNeural"
        );
    }

    #[test]
    fn normalize_dragon_hd_variant() {
        assert_eq!(
            normalize_voice_for_edge("en-US-Aria:DragonHDFlashLatestNeural"),
            "en-US-AriaNeural"
        );
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(
            normalize_voice_for_edge("  zh-CN-YunjianNeural  "),
            "zh-CN-YunjianNeural"
        );
    }

    #[test]
    fn lang_takes_first_two_segments() {
        assert_eq!(voice_to_lang("zh-CN-YunjianNeural"), "zh-CN");
        assert_eq!(voice_to_lang("en-US-AriaNeural"), "en-US");
        assert_eq!(voice_to_lang("weird"), "zh-CN");
    }

    /// 顾虑 2（Task 0 遗留）：时钟偏移校正。需要网络，默认忽略。
    ///
    /// 做法：先把进程级 `CLOCK_SKEW_SECS` 人为写成一个明显错误的值（模拟本机时钟
    /// 偏差过大），验证：(a) 首次连接会被服务端以 403 拒绝；(b) `connect()` 内部
    /// 读取响应 `Date` 头重新计算偏移并重试一次；(c) 重试后连接成功且
    /// `CLOCK_SKEW_SECS` 被校正为接近 0 的值（因为本机时钟其实是准的）。
    /// 用 `cargo test --lib tts::edge::tests::clock_skew -- --ignored` 单独运行，
    /// 避免与其他用到该全局状态的测试并发互相干扰。
    #[tokio::test]
    #[ignore]
    async fn clock_skew_self_corrects_after_403() {
        ensure_rustls_provider();
        // 人为制造一个 1 小时的错误偏移；实测（见 task-6-report.md）表明这样的偏移
        // 会被服务端以 403 拒绝，且响应带 Date 头。
        CLOCK_SKEW_SECS.store(3600, Ordering::Relaxed);

        let backend = EdgeBackend::new("zh-CN-YunjianNeural", Duration::from_secs(30));
        let ws = backend.connect().await;

        assert!(ws.is_ok(), "校正后应重试成功：{:?}", ws.err());
        let skew_after = CLOCK_SKEW_SECS.load(Ordering::Relaxed);
        assert!(
            skew_after.abs() < 300,
            "校正后的偏移应接近 0（本机时钟本身是准的），实际为 {skew_after}"
        );
    }
}
