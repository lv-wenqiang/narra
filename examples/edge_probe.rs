// 探针：验证 Microsoft Edge “朗读此页”（read-aloud）WebSocket 协议在 Rust 里能否跑通。
//
// 协议细节已对照 2026-09 时点的 edge-tts (Python, rany2/edge-tts@master)
// 源码核实，而非照抄 brief 里未经验证的假设。差异见 docs/edge-protocol.md。

use anyhow::{bail, Result};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message;

const TRUSTED_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const WSS: &str = "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";
// 与 edge-tts 源码 constants.py 的 CHROMIUM_FULL_VERSION 一致（2026-09 核实值，
// brief 中的 130.0.2849.68 已过时）。
const CHROMIUM_FULL_VERSION: &str = "143.0.3650.75";
const CHROMIUM_MAJOR_VERSION: &str = "143";

/// Windows FILETIME ticks（100ns 单位），向下取整到 5 分钟，拼 token 后 SHA256 大写十六进制。
/// 算法已对照 edge-tts drm.py::generate_sec_ms_gec 核实一致。
fn sec_ms_gec(unix_secs: u64) -> String {
    let mut ticks: u128 = unix_secs as u128 + 11_644_473_600;
    ticks -= ticks % 300;
    let ticks_100ns = ticks * 10_000_000;
    let mut h = Sha256::new();
    h.update(format!("{}{}", ticks_100ns, TRUSTED_TOKEN).as_bytes());
    hex::encode_upper(h.finalize())
}

/// 32 位随机十六进制大写字符串，模拟 edge-tts 的 muid Cookie（DRM.generate_muid，
/// 原实现是 secrets.token_hex(16)；这里用一个 UUID v4 的 16 字节代替，随机性等价）。
fn generate_muid() -> String {
    hex::encode_upper(uuid::Uuid::new_v4().as_bytes())
}

#[tokio::main]
async fn main() -> Result<()> {
    // rustls 0.23 需要显式安装 CryptoProvider；用 ring 后端（见 docs/edge-protocol.md
    // 的“遇到并解决的问题”一节）。忽略返回值而不是 `.expect(...)`：install_default()
    // 在进程里已有 provider 时返回 Err，Once/"只调一次"防不住"别人先装好了"这种情况，
    // `.expect(...)` 会在完全正常的场景下把程序 panic 掉（Task 6 复审已实测踩过并修复）。
    let _ = rustls::crypto::ring::default_provider().install_default();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let conn_id = uuid::Uuid::new_v4().simple().to_string();
    let url = format!(
        "{WSS}?TrustedClientToken={TRUSTED_TOKEN}&ConnectionId={conn_id}\
         &Sec-MS-GEC={}&Sec-MS-GEC-Version=1-{CHROMIUM_FULL_VERSION}",
        sec_ms_gec(now)
    );

    let req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        url.as_str(),
    )?;
    let req = {
        let mut r = req;
        let h = r.headers_mut();
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
        h.insert("Accept-Encoding", "gzip, deflate, br, zstd".parse()?);
        h.insert("Accept-Language", "en-US,en;q=0.9".parse()?);
        h.insert("Cookie", format!("muid={};", generate_muid()).parse()?);
        r
    };

    let (mut ws, resp) = tokio_tungstenite::connect_async(req).await?;
    eprintln!("已连接，HTTP 状态：{}", resp.status());

    let ts =
        chrono::Utc::now().format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)");
    ws.send(Message::Text(
        format!(
            "X-Timestamp:{ts}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n\
             {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":\
             {{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"true\"}},\
             \"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}}}}}\r\n"
        )
        .into(),
    ))
    .await?;

    let req_id = uuid::Uuid::new_v4().simple().to_string();
    // 注意：xml:lang 固定为 'en-US'，与 voice 的实际语言无关——这是 edge-tts
    // mkssml() 里的既有行为（即使 voice 是 zh-CN-* 也不改 xml:lang），已核实。
    ws.send(Message::Text(
        format!(
            "X-RequestId:{req_id}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{ts}Z\r\nPath:ssml\r\n\r\n\
             <speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
             <voice name='zh-CN-YunjianNeural'>\
             <prosody pitch='+0Hz' rate='+0%' volume='+0%'>这是一段协议验证用的测试文本。</prosody>\
             </voice></speak>"
        )
        .into(),
    ))
    .await?;

    let mut audio: Vec<u8> = Vec::new();
    let mut word_boundaries = 0usize;
    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Binary(b) => {
                // 前 2 字节为大端头部长度，其后是头部文本，再往后是音频负载
                if b.len() < 2 {
                    bail!("二进制帧过短");
                }
                let hdr_len = u16::from_be_bytes([b[0], b[1]]) as usize;
                if b.len() < 2 + hdr_len {
                    bail!("头部长度越界");
                }
                let header = String::from_utf8_lossy(&b[2..2 + hdr_len]).to_string();
                if header.contains("Path:audio") {
                    audio.extend_from_slice(&b[2 + hdr_len..]);
                }
            }
            Message::Text(t) => {
                eprintln!("[text] {}", &t[..t.len().min(200)]);
                if t.contains("Path:audio.metadata") && t.contains("WordBoundary") {
                    word_boundaries += 1;
                }
                if t.contains("Path:turn.end") {
                    break;
                }
            }
            Message::Close(c) => bail!("服务端关闭连接：{c:?}"),
            _ => {}
        }
    }

    if audio.is_empty() {
        bail!("未收到任何音频负载");
    }
    std::fs::write("probe.mp3", &audio)?;
    eprintln!(
        "成功：probe.mp3 共 {} 字节，收到 {} 条 WordBoundary",
        audio.len(),
        word_boundaries
    );
    Ok(())
}
