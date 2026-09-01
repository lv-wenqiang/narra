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

/// 安装 rustls ring CryptoProvider（rustls 0.23 不再自动探测后端）。
///
/// `install_default()` 在进程里**已经有 provider**（无论是不是我们自己装的）时会返回
/// `Err`——这既包括我们重复调用的情况，也包括宿主进程/其他库先一步装好了 provider 的
/// 情况。后一种情况下 `Once` 完全没用（`Once` 只防止“这个函数自己重复执行”，防不住
/// “进程里已经有别人装的 provider”），之前用 `.expect()` 会在真实集成场景里 panic
/// （已实测复现）。忽略错误即可：失败恰恰说明已经有可用的 provider，正是我们想要的
/// 结果，不需要区分“谁装的”。
fn ensure_rustls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
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

/// 转义会被插入单引号包裹的 XML 属性值（如 `<voice name='{}'>`）的字符。
/// 音色名本不应含引号，但若用户传入带 `'` 的字符串又不转义，服务端会因为属性值被
/// 提前截断而报出一个完全指不到"音色名非法"的错误（实测：`"'Neural' is an
/// unexpected token"`）。转义后至少能保证 SSML 结构不被破坏，错误信息更可追溯。
fn xml_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('\'', "&apos;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub struct EdgeBackend {
    voice: String,
    timeout: Duration,
    /// 连接端点。生产环境固定为 `WSS`；测试用例（同模块内的 `tests` 子模块可直接访问
    /// 私有字段）用它注入本地 mock WebSocket 服务端地址，让协议机器（帧解析、超时、
    /// metadata 容错）能在无网环境下被覆盖。
    endpoint: String,
}

impl EdgeBackend {
    pub fn new(voice: &str, timeout: Duration) -> Self {
        let voice = normalize_voice_for_edge(voice);
        Self { voice, timeout, endpoint: WSS.to_string() }
    }

    fn build_url(&self) -> String {
        format!(
            "{}?TrustedClientToken={TRUSTED_TOKEN}&ConnectionId={}\
             &Sec-MS-GEC={}&Sec-MS-GEC-Version=1-{CHROMIUM_FULL_VERSION}",
            self.endpoint,
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
    ///
    /// rustls provider 的安装放在这里（而不是 `new()`）：这是一个进程级全局副作用，
    /// 构造函数不应该有；只在真正要建立连接时才需要它就绪。
    async fn connect(&self) -> Result<WsStream> {
        ensure_rustls_provider();
        let mut retried = false;
        loop {
            let url = self.build_url();
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
                xml_escape_attr(&self.voice),
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
/// 首版只收集不使用，为后续词级字幕对齐预留。解析失败**静默跳过**——
/// 元数据缺失或格式变化不应导致合成失败。
///
/// 注意：跳过必须是真正的跳过，不能变成"编造一条全零/空字符串的幽灵词条"。
/// 三个字段（Offset/Duration/text.Text）必须**全部**成功解析出正确类型才 push；
/// 只要有一个字段类型不对，就整条丢弃，而不是用 `unwrap_or` 填充默认值——
/// 后者会产出一条 `offset_ms=0` 的假词条，恰好会毒化"词级字幕对齐"这个预留用途。
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
        let offset = d.get("Offset").and_then(|o| o.as_u64());
        let duration = d.get("Duration").and_then(|o| o.as_u64());
        let text = d
            .get("text")
            .and_then(|t| t.get("Text"))
            .and_then(|t| t.as_str());
        let (Some(offset), Some(duration), Some(text)) = (offset, duration, text) else {
            continue;
        };
        // Edge 的时间单位是 100 纳秒 tick
        out.push(WordTiming {
            text: text.to_string(),
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

    /// 必修 1：`install_default()` 在进程里已有 provider 时会返回 `Err`。这既覆盖
    /// "我们自己重复调用"，也覆盖"宿主进程/其它库先一步装好了 provider"——旧版用
    /// `.expect()` 会在这两种情况下都 panic（复审已实测复现）。这里直接连续调用
    /// 两次（模拟"已有 provider"的场景），断言不 panic 就是修复生效的证据；
    /// 这条测试本身不需要网络。
    #[test]
    fn ensure_rustls_provider_does_not_panic_when_already_installed() {
        ensure_rustls_provider();
        ensure_rustls_provider(); // 第二次调用时 provider 已存在，install_default() 会返回 Err
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
        // 人为制造一个 1 小时的错误偏移；实测（见 task-6-report.md）表明这样的偏移
        // 会被服务端以 403 拒绝，且响应带 Date 头。（rustls provider 的安装已内置在
        // `connect()` 里，这里不用再手动调用一次。）
        CLOCK_SKEW_SECS.store(3600, Ordering::Relaxed);

        let backend = EdgeBackend::new("zh-CN-YunjianNeural", Duration::from_secs(30));
        let ws = backend.connect().await;

        let skew_after = CLOCK_SKEW_SECS.load(Ordering::Relaxed);
        // 无论断言是否通过都要先复原全局状态，避免污染同进程内其他测试
        // （`CLOCK_SKEW_SECS` 是进程级共享状态，`--test-threads=1` 只是报告里的运行
        // 建议，不是代码层面的保证）。
        CLOCK_SKEW_SECS.store(0, Ordering::Relaxed);

        assert!(ws.is_ok(), "校正后应重试成功：{:?}", ws.err());
        assert!(
            skew_after.abs() < 300,
            "校正后的偏移应接近 0（本机时钟本身是准的），实际为 {skew_after}"
        );
    }

    /// 起一个本地 mock WebSocket 服务端：接受一条连接，把它交给 `handler` 处理。
    /// 返回可直接塞进 `EdgeBackend.endpoint` 的 `ws://127.0.0.1:<port>` 地址。
    ///
    /// 之所以能这样测试协议机器（帧解析/超时/metadata 容错）：`EdgeBackend` 的
    /// `endpoint` 字段是私有的，但这个 `tests` 子模块是 `edge` 模块的后代模块，
    /// Rust 的可见性规则允许它直接用结构体字面量构造 `EdgeBackend`，从而绕开生产
    /// 环境固定为 `WSS` 的 `new()`，指向这个本地服务端——不需要额外的公开 API。
    async fn spawn_mock_server<F, Fut>(handler: F) -> String
    where
        F: FnOnce(
                tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
            ) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定本地端口失败");
        let addr = listener.local_addr().expect("读取本地端口失败");
        tokio::spawn(async move {
            let (stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => return,
            };
            let ws = match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => ws,
                Err(_) => return,
            };
            handler(ws).await;
        });
        // 注意：必须带上末尾的 `/`。`build_url()` 会直接在这个端点后面拼接
        // `?TrustedClientToken=...`；若端点没有路径部分，生成的请求行会变成
        // `GET ?query HTTP/1.1`（缺少 `/`），是非法的 HTTP 请求行，服务端的
        // `accept_async` 会以 "HTTP format error: invalid format" 拒绝——这是
        // 实际调试这两条测试时踩到的坑，生产环境的 `WSS` 常量自带路径不会触发。
        format!("ws://{addr}/")
    }

    /// 必修 2：服务端握手后永久静默（既不发 turn.end 也不关闭连接），验证整体
    /// `synth()` 的 `tokio::time::timeout` 包裹确实生效，而不是永久阻塞在
    /// `ws.next().await` 上。不需要外网，进默认测试集。
    ///
    /// 反向验证过这条测试真的在测超时：临时把 `EdgeBackend::synth` 里的
    /// `tokio::time::timeout(...)` 去掉、直接 `.await self.synth_inner(text)`，
    /// 重跑本测试会挂住直到 `timeout_at` 兜底或手动中断——确认测试在超时保护缺失时
    /// 会失败/挂死，而不是无论如何都通过。
    #[tokio::test]
    async fn synth_times_out_when_server_is_silent() {
        let endpoint = spawn_mock_server(|ws| async move {
            // 接受连接、完成 WebSocket 握手后什么都不做：不发消息，不关闭连接。
            // 用一个远超测试超时的 sleep 占住这个任务，模拟服务端静默挂起。
            //
            // 关键坑：`ws` 必须显式移进这个 `async move` 块并在 sleep 期间保持存活。
            // 若参数写成 `_ws`（或干脆不绑定），`async move` 只捕获块内真正用到的
            // 变量——`_ws` 从未在块内被引用，根本不会被捕获，而是在外层闭包函数体
            // 求值完（也就是构造出这个 async 块的那一刻）就立即被 drop，导致连接
            // 立刻关闭。这样一来客户端看到的就不是"服务端静默挂起"，而是连接被
            // 立即断开——第二条消息发送时报 "Broken pipe"，测试会因为这个无关的
            // 错误碰巧也满足 `is_err()` 而"意外通过"，完全没测到超时保护。
            let _keep_connection_alive = ws;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        })
        .await;

        let backend = EdgeBackend {
            voice: "zh-CN-YunjianNeural".to_string(),
            timeout: Duration::from_millis(300),
            endpoint,
        };

        let result = tokio::time::timeout(Duration::from_secs(5), backend.synth("test"))
            .await
            .expect("synth() 本身应该在其内部超时后返回，而不是被外层测试超时兜住");

        assert!(
            result.is_err(),
            "服务端静默挂起时 synth() 应该超时返回 Err，而不是永久阻塞"
        );
    }

    /// 必修 2：服务端发送若干条畸形 `Path:audio.metadata` 帧（非 JSON、缺
    /// `Metadata` 键、缺 `Data`、字段类型全错），混入一条真正合法的 WordBoundary，
    /// 再发一个合法的二进制音频帧和 `turn.end`。验证：
    /// 1. 畸形帧不会让 `synth()` 整体失败；
    /// 2. 音频负载被完整、正确地拼接出来；
    /// 3. 畸形帧不会被"编造"成幽灵词条——只有那条真正合法的 WordBoundary 被收集到。
    /// 不需要外网，进默认测试集。
    #[tokio::test]
    async fn malformed_metadata_is_skipped_without_failing_synthesis() {
        let audio_bytes = b"FAKE-MP3-AUDIO-PAYLOAD-0123456789".to_vec();
        let expected_audio = audio_bytes.clone();

        let endpoint = spawn_mock_server(move |mut ws| async move {
            // 依次读掉客户端发的 speech.config 和 ssml 两条消息，避免理论上的
            // TCP 背压（虽然消息很小，实际不会触发，但这样更贴近真实交互）。
            let _ = ws.next().await;
            let _ = ws.next().await;

            let malformed_frames = [
                // 1) Path 头之后的正文根本不是 JSON
                "X-Timestamp:t\r\nPath:audio.metadata\r\n\r\nnot valid json",
                // 2) 是 JSON，但没有 Metadata 键
                "X-Timestamp:t\r\nPath:audio.metadata\r\n\r\n{\"foo\":1}",
                // 3) 有 Metadata 数组，但条目缺 Data
                "X-Timestamp:t\r\nPath:audio.metadata\r\n\r\n\
                 {\"Metadata\":[{\"Type\":\"WordBoundary\"}]}",
                // 4) 有 Data，但三个字段类型全错（字符串代替数字、数字代替字符串）
                "X-Timestamp:t\r\nPath:audio.metadata\r\n\r\n\
                 {\"Metadata\":[{\"Type\":\"WordBoundary\",\"Data\":{\
                 \"Offset\":\"nope\",\"Duration\":\"nope\",\"text\":{\"Text\":123}}}]}",
            ];
            for frame in malformed_frames {
                ws.send(Message::Text(frame.to_string().into()))
                    .await
                    .expect("mock 服务端发送畸形 metadata 失败");
            }

            // 混入一条真正合法的 WordBoundary，验证畸形帧不会连累后续正常解析。
            ws.send(Message::Text(
                "X-Timestamp:t\r\nPath:audio.metadata\r\n\r\n\
                 {\"Metadata\":[{\"Type\":\"WordBoundary\",\"Data\":{\
                 \"Offset\":100000,\"Duration\":50000,\"text\":{\"Text\":\"test\"}}}]}"
                    .to_string()
                    .into(),
            ))
            .await
            .expect("mock 服务端发送合法 metadata 失败");

            // 合法的二进制音频帧：2 字节大端头部长度 + 头部文本 + 音频负载。
            let header = b"Path:audio\r\nContent-Type:audio/mpeg\r\n\r\n";
            let mut frame = Vec::new();
            frame.extend_from_slice(&(header.len() as u16).to_be_bytes());
            frame.extend_from_slice(header);
            frame.extend_from_slice(&audio_bytes);
            ws.send(Message::Binary(frame.into()))
                .await
                .expect("mock 服务端发送音频帧失败");

            ws.send(Message::Text("Path:turn.end\r\n\r\n{}".to_string().into()))
                .await
                .expect("mock 服务端发送 turn.end 失败");
        })
        .await;

        let backend = EdgeBackend {
            voice: "zh-CN-YunjianNeural".to_string(),
            timeout: Duration::from_secs(5),
            endpoint,
        };

        let synthesized = backend
            .synth("test")
            .await
            .expect("畸形 metadata 不应导致合成失败");

        assert_eq!(
            synthesized.audio, expected_audio,
            "音频负载应完整、未被畸形帧干扰"
        );

        let timings = synthesized
            .timings
            .expect("应至少收到一条真正合法的 WordBoundary");
        assert_eq!(
            timings.len(),
            1,
            "4 条畸形帧应被整条丢弃，只有 1 条真正合法的 WordBoundary 应被收集，\
             而不是编造出额外的幽灵词条"
        );
        assert_eq!(timings[0].text, "test");
        assert_eq!(timings[0].offset_ms, 10);
        assert_eq!(timings[0].duration_ms, 5);
    }
}
