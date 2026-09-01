# Edge 朗读此页（read-aloud）WebSocket 协议 —— 实测记录

> 本文档记录的是 **2026-09-01 在本仓库 `examples/edge_probe.rs` 中实际跑通** 的协议细节，
> 已对照同日的 edge-tts（Python，`github.com/rany2/edge-tts` master 分支）源码核实。
> Task 6 实现时应直接照抄本文档，不要重新推导，也不要照抄 task-0-brief.md 里的原始假设
> （其中部分数值——见下方“与 brief 的差异”——已经过时）。

## 结论

**协议在 Rust（tokio + tokio-tungstenite + rustls）里可以跑通，无需回退到 Azure。**
排查阶梯（brief Step 4 的五步）**完全没用到**——按验证后的参数（而非 brief 原始假设）
第一次运行就成功。

验证结果：
- `probe.mp3`：22320 字节，`ffprobe` 报告 `duration=3.720000`（秒），`bit_rate=48000`
- `ffmpeg -af volumedetect` 确认非静音：`mean_volume: -23.8 dB`，`max_volume: -4.2 dB`
- 收到 8 条 `WordBoundary` 元数据
- 音频流：`mp3`, 24000 Hz, mono，与请求的 `audio-24khz-48kbitrate-mono-mp3` 一致

## 连接 URL

```
wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1
    ?TrustedClientToken=6A5AA1D4EAFF4E9FB37E23D68491D6F4
    &ConnectionId=<uuid v4，去掉横线，小写 32 位 hex>
    &Sec-MS-GEC=<见下方算法>
    &Sec-MS-GEC-Version=1-143.0.3650.75
```

`TrustedClientToken` 的值与 brief 一致，未变化。

## Sec-MS-GEC 签名算法（已核实，与 brief 一致）

对照 edge-tts `drm.py::generate_sec_ms_gec` 确认算法正确：

1. 取当前 UTC Unix 秒数（整数即可，edge-tts 用带小数的 timestamp + 时钟偏移校正，
   探针里用整数秒 —— 差异只影响 5 分钟窗口边界附近的 ticks 值，几乎不影响成功率）。
2. 加 Windows FILETIME 纪元偏移 `11_644_473_600`（1601-01-01 到 1970-01-01 的秒数）。
3. 向下取整到 300 秒（5 分钟）的整数倍：`ticks -= ticks % 300`。
4. 转换为 100 纳秒单位（Windows FILETIME 刻度）：`ticks *= 10_000_000`。
5. 字符串拼接 `"{ticks_100ns}{TRUSTED_CLIENT_TOKEN}"`，SHA256，十六进制大写。

```rust
fn sec_ms_gec(unix_secs: u64) -> String {
    let mut ticks: u128 = unix_secs as u128 + 11_644_473_600;
    ticks -= ticks % 300;
    let ticks_100ns = ticks * 10_000_000;
    let mut h = Sha256::new();
    h.update(format!("{}{}", ticks_100ns, TRUSTED_TOKEN).as_bytes());
    hex::encode_upper(h.finalize())
}
```

注意：brief 原始代码是先把秒数乘以 `10_000_000` 再对 `3_000_000_000`（=300×10^7）取模，
数学上与“先对 300 取模再乘 10^7”等价（整数场景下 `(a*c) mod (b*c) == c*(a mod b)`），
两种写法都对，我改成了先取模再放大，可读性更接近 Python 原实现。

## 必需的 WebSocket 请求头

```
Origin: chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold
User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36 Edg/143.0.0.0
Pragma: no-cache
Cache-Control: no-cache
Accept-Encoding: gzip, deflate, br, zstd
Accept-Language: en-US,en;q=0.9
Cookie: muid=<32 位随机大写 hex>;
```

`Sec-WebSocket-Version: 13` 等标准握手头由 `tokio-tungstenite` 自动生成，不要手动设置
（手动设置会与库内部重复/冲突）。

`Cookie: muid=...` 不确定是否为必需项（edge-tts 会加，探针也加了、且探针一次成功），
未做“去掉这个头是否仍然成功”的对照实验，Task 6 如需精简可自行验证后再去掉。

## 消息 1：speech.config（TEXT 帧）

```
X-Timestamp:<JS 风格时间戳，如 "Mon Sep 01 2026 12:34:56 GMT+0000 (Coordinated Universal Time)">
Content-Type:application/json; charset=utf-8
Path:speech.config

{"context":{"synthesis":{"audio":{"metadataoptions":{"sentenceBoundaryEnabled":"false","wordBoundaryEnabled":"true"},"outputFormat":"audio-24khz-48kbitrate-mono-mp3"}}}}
```

（头部与 JSON 之间是 `\r\n\r\n`；JSON 末尾探针额外加了一个 `\r\n`，跟随 edge-tts 源码，
未验证去掉是否有影响。）

## 消息 2：ssml（TEXT 帧）

```
X-RequestId:<uuid v4，去掉横线>
Content-Type:application/ssml+xml
X-Timestamp:<同上时间戳字符串>Z
Path:ssml

<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>
  <voice name='zh-CN-YunjianNeural'>
    <prosody pitch='+0Hz' rate='+0%' volume='+0%'>这是一段协议验证用的测试文本。</prosody>
  </voice>
</speak>
```

**关键差异点（与 brief 假设不同，务必注意）**：`xml:lang` 固定写死为 `'en-US'`，
**与 `voice name` 的实际语言无关**。这是 edge-tts 官方实现 `mkssml()` 里硬编码的行为
（即便 voice 是 `zh-CN-YunjianNeural`，`xml:lang` 也不改成 `zh-CN`）。探针用中文语音
+ `xml:lang='en-US'` 一次成功，验证了这一点。brief Step 4 排查阶梯第 3 条建议
“检查 xml:lang 与 voice name 是否匹配”，实测应理解为**不需要匹配**，若遇到连接被关闭，
不要往“匹配”方向排查。

`X-Timestamp` 后面多一个字面量 `Z`（`{ts}Z`，其中 `{ts}` 本身已经是不含时区后缀的字符串），
这是 edge-tts 源码注释里说的 "Microsoft Edge bug"，照抄即可，不要"修正"。

## 服务端响应（本次实测收到的顺序）

1. TEXT `Path:turn.start`（JSON，带 `context.serviceTag`）
2. TEXT `Path:response`（JSON，带 `audio.streamId` 等）
3. TEXT `Path:audio.metadata` × N（每条一个或多个 `WordBoundary` JSON 对象，本次共 8 条）
4. BINARY 帧（可能多个）：音频数据
5. TEXT `Path:turn.end`（JSON `{}`），收到即可结束循环

## 二进制帧结构（已核实，与 brief 假设一致）

```
[0..2)   大端 u16：头部文本的字节长度 hdr_len
[2..2+hdr_len)  头部文本（形如 "Path:audio\r\nContent-Type:audio/mpeg\r\n\r\n"）
[2+hdr_len..)   音频负载（仅当头部里 Path 是 audio 且 Content-Type 是 audio/mpeg 时才有数据；
                 流结束时可能有一个 Path:audio 但无 Content-Type、无负载的“空尾帧”，需要
                 兼容，探针里空数据直接被 extend_from_slice 一个空切片，无副作用，不需要
                 额外分支）
```

多个 BINARY 帧的音频负载按接收顺序原样拼接、写入同一个文件即可得到完整 mp3。

## 与 brief 假设的差异汇总

| 项目 | brief 假设 | 实测/核实结果 |
|---|---|---|
| Sec-MS-GEC-Version 里的 Chromium 版本 | `130.0.2849.68` | `143.0.3650.75`（对照 2026-09 时点 edge-tts `constants.py::CHROMIUM_FULL_VERSION`） |
| User-Agent 里的 Chrome/Edg 版本号 | 130 | 143 |
| SSML `xml:lang` | 假设需要与 voice 语言匹配（如 `zh-CN`） | **固定写 `en-US`，与 voice 无关**（edge-tts `mkssml()` 硬编码），brief Step 4 第 3 条排查建议方向有误 |
| Sec-MS-GEC 算法 | 取模写法：先乘 10^7 再对 3×10^9 取模 | 算法等价，已改写成先对 300 取模再乘 10^7（更接近 Python 源码，可读性更好），两种写法都对 |
| `TrustedClientToken` | `6A5AA1D4EAFF4E9FB37E23D68491D6F4` | 一致，未变 |
| 二进制帧结构（2 字节大端头部长度） | 假设如此 | 核实一致 |
| 额外请求头 | brief 只给了 4 个头（Origin/User-Agent/Pragma/Cache-Control） | 额外加了 `Accept-Encoding`、`Accept-Language`、`Cookie: muid=...`（跟随 edge-tts，未做逐一去除的对照实验，不确定是否都必需） |

## 遇到并解决的问题

### 1. rustls 0.23 需要显式安装 CryptoProvider

`cargo add tokio-tungstenite --features rustls-tls-webpki-roots` 只带来 `rustls` 的
`std` feature，不含任何加密后端（`ring` / `aws-lc-rs` 都不启用）。运行时直接 panic：

```
Could not automatically determine the process-level CryptoProvider from Rustls crate features.
```

修复两步：

1. `cargo add rustls --no-default-features --features ring,std,tls12`
   （**不能**用 `cargo add rustls --features ring`——那样会保留 rustls 的默认 feature，
   而 rustls 0.23 默认启用的是 `aws-lc-rs` 后端，其依赖的 `aws-lc-rs` crate 版本要求
   `^1.18`，但本机 cargo 镜像（`rsproxy.cn`）只同步到 `1.17.3`，会导致
   `error: failed to select a version for the requirement `aws-lc-rs = "^1.18"`。
   显式 `--no-default-features` 只留 `ring` 后端即可避免这个依赖，且更轻量。）
2. 程序入口处调用一次：
   ```rust
   rustls::crypto::ring::default_provider()
       .install_default()
       .expect("安装 rustls ring CryptoProvider 失败");
   ```

### 2. 网络环境

本机 `HTTP_PROXY`/`HTTPS_PROXY` 环境变量指向公司代理，但**该代理访问
`speech.platform.bing.com` 很慢（约 9 秒完成 TLS 握手）**；实测本机对
`speech.platform.bing.com` **有直连能力**（不经代理约 0.6 秒完成握手），
且 `tokio::net::TcpStream`（`tokio-tungstenite` 底层用的连接方式）本来就不读取
`HTTP_PROXY`/`HTTPS_PROXY` 环境变量，会直接发起直连——这正好是更快的路径，
探针未做任何代理配置，直接可用。Task 6 实现同理，不需要处理代理逻辑。

`cargo` 依赖下载走的是 `~/.cargo/config.toml` 里配置的 `rsproxy.cn` 镜像
（`crates-io` 被 `replace-with` 到 `rsproxy-sparse`），与运行时访问
`speech.platform.bing.com` 无关，两者是独立的网络路径。

## 依赖版本（Cargo.toml 关键部分）

```toml
[dependencies]
anyhow = "1.0.104"
chrono = "0.4.45"
futures-util = "0.3.34"
hex = "0.4.3"
rustls = { version = "0.23.43", default-features = false, features = ["ring", "std", "tls12"] }
sha2 = "0.11.0"
tokio = { version = "1.53.1", features = ["rt-multi-thread", "macros", "time", "io-util", "net"] }
tokio-tungstenite = { version = "0.30.0", features = ["rustls-tls-webpki-roots"] }
uuid = { version = "1.26.0", features = ["v4"] }
```

## 参考来源

- edge-tts（Python，rany2/edge-tts，master 分支，2026-09-01 抓取）：
  - `src/edge_tts/constants.py`（`TRUSTED_CLIENT_TOKEN`、`CHROMIUM_FULL_VERSION`、
    `WSS_HEADERS`、`SEC_MS_GEC_VERSION`）
  - `src/edge_tts/drm.py`（`generate_sec_ms_gec`、`generate_muid`）
  - `src/edge_tts/communicate.py`（`mkssml`、`ssml_headers_plus_data`、
    speech.config 消息体、二进制帧解析 `get_headers_and_data`）
