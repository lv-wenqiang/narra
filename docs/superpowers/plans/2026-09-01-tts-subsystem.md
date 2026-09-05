# panda-video-rs TTS 子系统 Implementation Plan


> **2026-09-05 校订**：本文档里的品牌名、默认标题与探针文案已随项目改名统一替换
> 为当前值（改动前的原文见 git 历史）。其余内容保持当时的记录原样。

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用 Rust 实现「口播文稿 → `audio.mp3` + `audio.vtt`」，功能等价于现有 `packages/tts-node`，替代 `pnpm tts`。

**Architecture:** 单二进制的第一个子命令 `panda tts`。文稿按非空行分段，经 Edge read-aloud WebSocket 并发合成为逐段 mp3，用外部 ffmpeg 做 concat 合并与 `atempo` 加速，再按字符数比例摊分时长生成 WebVTT。纯逻辑（切句、时间格式、VTT 生成）与 IO/网络严格分离，前者可独立单测。

**Tech Stack:** Rust 2021 / tokio / tokio-tungstenite / symphonia / clap / anyhow / sha2 / serde_json；外部依赖仅 ffmpeg。

**Spec:** `docs/superpowers/specs/2026-09-01-panda-video-rs-design.md`

## Global Constraints

- 目标平台首版仅 **Linux（WSL2）**；不做 Windows 交叉编译。
- 外部依赖**只允许 ffmpeg**，且通过 PATH 调用，不静态链接。
- 输出文件名固定：`audio.mp3`、`audio.vtt`；中间文件 `sentence{N}.mp3` 流程结束必须删除。
- 默认音色 `zh-CN-YunjianNeural`；输出格式 `audio-24khz-48kbitrate-mono-mp3`；加速倍率 `1.1`。
- 并发默认 **3**，`EDGE_TTS_BATCH_SIZE` 可覆盖，**上限 8**；每段重试 **3** 次，退避 `(attempt+1) * 2000ms`。
- 单段超时默认 **120000ms**，`EDGE_TTS_TIMEOUT_MS` 可覆盖，**下限 15000ms**；低于下限则回落到默认值。
- 切句最大长度 **30** 字符；句末标点集合为 `。！？`（仅此三个）。
- 字符计数一律用 `chars().count()`（Unicode 标量），不使用字节长度。
- 首版**不使用** WordBoundary 词级时间戳，字幕时长按字符数比例摊分。
- **验收标准是功能正确可用，不要求与 TS 版逐字节一致**；TS 版仅作行为参考，不作测试基线。不依赖 `pnpm`。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `Cargo.toml` | 依赖与二进制定义 |
| `src/main.rs` | CLI 入口与子命令分发 |
| `src/config.rs` | 环境变量与默认值解析 |
| `src/vtt.rs` | 时间格式化、切句、VTT 生成（纯函数） |
| `src/duration.rs` | mp3 时长读取 |
| `src/ffmpeg.rs` | ffmpeg 可用性探测、concat + atempo 合并 |
| `src/tts/mod.rs` | 子模块声明 |
| `src/tts/backend.rs` | `TtsBackend` trait 与数据类型 |
| `src/tts/edge.rs` | Edge read-aloud WebSocket 客户端 |
| `src/tts/pipeline.rs` | 分段、并发、重试、合并、清理的编排 |

---

### Task 0: Edge read-aloud 协议验证（关卡）

**这是探针任务，不是 TDD 任务。** 它的产出是一个答案，不是要保留的代码。若本任务失败，整个 TTS 方案回退到 Azure Speech REST API，Task 6/7 需重写，Task 1–5 不受影响。

**Files:**
- Create: `examples/edge_probe.rs`

**Interfaces:**
- Produces: 一份确认过的协议细节记录，写入 `docs/edge-protocol.md`，供 Task 6 实现时照抄。

- [ ] **Step 1: 初始化 Cargo 项目并加入探针所需依赖**

```bash
cargo init --name panda
cargo add tokio --features rt-multi-thread,macros,time,io-util
cargo add tokio-tungstenite --features rustls-tls-webpki-roots
cargo add futures-util sha2 hex uuid --features uuid/v4
cargo add chrono anyhow
```

- [ ] **Step 2: 写探针程序**

以下协议细节是**待验证的假设**，来自社区实现的通行做法，可能已过时。跑不通时按 Step 4 的排查顺序调整。

```rust
// examples/edge_probe.rs
use anyhow::{bail, Result};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message;

const TRUSTED_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const WSS: &str = "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";

/// Windows FILETIME ticks，向下取整到 5 分钟，拼 token 后 SHA256 大写十六进制。
fn sec_ms_gec(unix_secs: u64) -> String {
    let mut ticks: u128 = (unix_secs as u128 + 11_644_473_600) * 10_000_000;
    ticks -= ticks % 3_000_000_000;
    let mut h = Sha256::new();
    h.update(format!("{}{}", ticks, TRUSTED_TOKEN).as_bytes());
    hex::encode_upper(h.finalize())
}

#[tokio::main]
async fn main() -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let conn_id = uuid::Uuid::new_v4().simple().to_string();
    let url = format!(
        "{WSS}?TrustedClientToken={TRUSTED_TOKEN}&Sec-MS-GEC={}\
         &Sec-MS-GEC-Version=1-130.0.2849.68&ConnectionId={conn_id}",
        sec_ms_gec(now)
    );

    let req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        url.as_str(),
    )?;
    let req = {
        let mut r = req;
        let h = r.headers_mut();
        h.insert("Origin", "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".parse()?);
        h.insert("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
            (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0".parse()?);
        h.insert("Pragma", "no-cache".parse()?);
        h.insert("Cache-Control", "no-cache".parse()?);
        r
    };

    let (mut ws, resp) = tokio_tungstenite::connect_async(req).await?;
    eprintln!("已连接，HTTP 状态：{}", resp.status());

    let ts = chrono::Utc::now().format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)");
    ws.send(Message::Text(format!(
        "X-Timestamp:{ts}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n\
         {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":\
         {{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"true\"}},\
         \"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}}}}}"
    ).into())).await?;

    let req_id = uuid::Uuid::new_v4().simple().to_string();
    ws.send(Message::Text(format!(
        "X-RequestId:{req_id}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{ts}Z\r\nPath:ssml\r\n\r\n\
         <speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='zh-CN'>\
         <voice name='zh-CN-YunjianNeural'>\
         <prosody pitch='+0Hz' rate='+0%' volume='+0%'>这是一段协议验证用的测试文本。</prosody>\
         </voice></speak>"
    ).into())).await?;

    let mut audio: Vec<u8> = Vec::new();
    let mut word_boundaries = 0usize;
    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Binary(b) => {
                // 前 2 字节为大端头部长度，其后是头部文本，再往后是音频负载
                if b.len() < 2 { bail!("二进制帧过短"); }
                let hdr_len = u16::from_be_bytes([b[0], b[1]]) as usize;
                if b.len() < 2 + hdr_len { bail!("头部长度越界"); }
                let header = String::from_utf8_lossy(&b[2..2 + hdr_len]).to_string();
                if header.contains("Path:audio") {
                    audio.extend_from_slice(&b[2 + hdr_len..]);
                }
            }
            Message::Text(t) => {
                if t.contains("Path:audio.metadata") && t.contains("WordBoundary") {
                    word_boundaries += 1;
                }
                if t.contains("Path:turn.end") { break; }
            }
            Message::Close(c) => bail!("服务端关闭连接：{c:?}"),
            _ => {}
        }
    }

    if audio.is_empty() { bail!("未收到任何音频负载"); }
    std::fs::write("probe.mp3", &audio)?;
    eprintln!("成功：probe.mp3 共 {} 字节，收到 {} 条 WordBoundary", audio.len(), word_boundaries);
    Ok(())
}
```

- [ ] **Step 3: 运行探针**

```bash
cargo run --example edge_probe
```

期望：打印 `已连接，HTTP 状态：101`，随后 `成功：probe.mp3 共 N 字节`，N 应为数万量级。

- [ ] **Step 4: 验证音频真实可播放**

```bash
ffprobe -v error -show_entries format=duration,bit_rate -of default=nw=1 probe.mp3
```

期望：`duration` 约 2–4 秒，`bit_rate` 约 48000。若文件存在但 ffprobe 报错，说明拼接逻辑错了（多半是头部长度解析）。

**若 Step 3 失败，按此顺序排查：**

1. **HTTP 403 / 401** → `Sec-MS-GEC` 签名不对。核对 ticks 是否为 10^7 单位、是否对 3×10^9 取模、SHA256 是否大写。
2. **HTTP 403 且签名确认无误** → `Sec-MS-GEC-Version` 里的 Edge 版本号过旧，换成当前 Edge 稳定版版本号重试。
3. **连接成功但立即被关闭** → 检查 `Origin` 头与 SSML 的 `xml:lang`、`voice name` 是否匹配；音色名不能含冒号。
4. **收到 `Path:turn.end` 但无音频** → SSML 格式被拒，先用纯 ASCII 文本排除编码问题。
5. **以上都试过仍失败** → **停止，向用户报告，方案切换到 Azure Speech**。不要继续往下做 Task 6/7。

- [ ] **Step 5: 记录确认过的协议**

把**实际跑通**的 URL 参数、请求头、两条消息的确切格式、二进制帧结构写入 `docs/edge-protocol.md`。Task 6 照抄这份记录，不再重新推导。

- [ ] **Step 6: 提交**

```bash
git add examples/edge_probe.rs docs/edge-protocol.md Cargo.toml Cargo.lock
git commit -m "spike: 验证 Edge read-aloud WebSocket 协议"
```

---

### Task 1: 脚手架与 VTT 时间格式化

**Files:**
- Create: `src/vtt.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Produces: `pub fn format_vtt_time(seconds: f64) -> String`

- [ ] **Step 1: 写失败的测试**

```rust
// src/vtt.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_vtt_time_basics() {
        assert_eq!(format_vtt_time(0.0), "00:00:00.000");
        assert_eq!(format_vtt_time(1.5), "00:00:01.500");
        assert_eq!(format_vtt_time(61.25), "00:01:01.250");
        assert_eq!(format_vtt_time(3661.007), "01:01:01.007");
    }

    #[test]
    fn format_vtt_time_rounds_to_three_decimals() {
        assert_eq!(format_vtt_time(12.3456), "00:00:12.346");
    }

    /// TS 版的已知缺陷：时分由 floor 独立计算，秒进位时不回填。
    /// 注意：此测试已在 Task 3 Step 1 被替换为期望正确进位行为。
    #[test]
    fn format_vtt_time_replicates_ts_carry_bug() {
        assert_eq!(format_vtt_time(59.9996), "00:00:60.000");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test vtt::tests`
Expected: FAIL，编译错误 `cannot find function format_vtt_time`

- [ ] **Step 3: 实现**

```rust
// src/vtt.rs 顶部
/// 移植自 packages/tts-node/src/vtt.ts 的 formatVttTime。
/// 时分用 floor 独立计算，秒取 `seconds % 60` 后四舍五入到 3 位小数——
/// 与 TS 版一致，包括秒进位不回填的缺陷。
pub fn format_vtt_time(seconds: f64) -> String {
    let hours = (seconds / 3600.0).floor() as i64;
    let minutes = ((seconds % 3600.0) / 60.0).floor() as i64;
    let secs = seconds % 60.0;
    let formatted = format!("{:.3}", secs);
    let (int_part, frac_part) = formatted.split_once('.').unwrap_or((formatted.as_str(), "000"));
    format!("{:02}:{:02}:{:0>2}.{}", hours, minutes, int_part, frac_part)
}
```

在 `src/main.rs` 加 `mod vtt;`，并保留一个空的 `fn main() {}` 以便编译。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test vtt::tests`
Expected: 3 个测试全部 PASS

- [ ] **Step 5: 提交**

```bash
git add src/vtt.rs src/main.rs
git commit -m "feat(vtt): 移植 formatVttTime 并复刻 TS 版进位行为"
```

---

### Task 2: 切句 `split_text_for_vtt`

**Files:**
- Modify: `src/vtt.rs`

**Interfaces:**
- Consumes: 无
- Produces: `pub fn split_text_for_vtt(text: &str, max_length: usize) -> Vec<String>`

- [ ] **Step 1: 写失败的测试**

```rust
// 追加到 src/vtt.rs 的 mod tests 中
#[test]
fn split_short_text_returns_whole() {
    assert_eq!(split_text_for_vtt("很短的一句话。", 30), vec!["很短的一句话。"]);
}

#[test]
fn split_at_last_sentence_end_within_limit() {
    // 前 30 字符内从后往前找到「。」，在其后切开
    let text = "第一句话到这里结束。第二句话比较长一些继续往后写下去直到超过限制为止收尾。";
    let got = split_text_for_vtt(text, 30);
    assert_eq!(got[0], "第一句话到这里结束。");
    assert_eq!(got.len(), 2);
}

#[test]
fn split_looks_ahead_when_no_ending_within_limit() {
    // 前 30 字符内无句末标点，向后找到第一个「！」
    let text = "这段文字前面很长一直没有任何标点符号一直写下去写到很后面才终于出现了标点！后面还有一点。";
    let got = split_text_for_vtt(text, 30);
    assert!(got[0].ends_with('！'));
    assert_eq!(got.len(), 2);
}

#[test]
fn split_returns_whole_when_no_sentence_ending_at_all() {
    let text = "这段文字完全没有句末标点符号但是长度已经远远超过了三十个字符的上限了继续写";
    assert_eq!(split_text_for_vtt(text, 30), vec![text]);
}

#[test]
fn split_counts_unicode_scalars_not_bytes() {
    // 30 个汉字 = 90 字节；若误用字节长度会错误触发切分
    let text = "一二三四五六七八九十一二三四五六七八九十一二三四五六七八九十";
    assert_eq!(text.chars().count(), 30);
    assert_eq!(split_text_for_vtt(text, 30), vec![text]);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test vtt::tests::split`
Expected: FAIL，`cannot find function split_text_for_vtt`

- [ ] **Step 3: 实现**

```rust
const SENTENCE_ENDINGS: [char; 3] = ['。', '！', '？'];

/// 移植自 packages/tts-node/src/vtt.ts 的 splitTextForVtt。
/// TS 原版基于 UTF-16 code unit 计数；此处用 Unicode 标量。
/// 中文（BMP 内）两者一致，emoji（星光平面）会有差异，见 tests。
pub fn split_text_for_vtt(text: &str, max_length: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_length {
        return vec![text.to_string()];
    }

    let mut segments: Vec<String> = Vec::new();
    let mut remaining: Vec<char> = chars;

    while remaining.len() > max_length {
        // 1. 在前 max_length 个字符内从后往前找句末标点
        let upper = max_length.min(remaining.len());
        let back = (0..upper).rev().find(|&i| SENTENCE_ENDINGS.contains(&remaining[i]));

        if let Some(i) = back {
            remaining = cut(&mut segments, remaining, i + 1);
            continue;
        }

        // 2. 向后在 [max_length, max_length*2) 内找
        let lookahead = (max_length * 2).min(remaining.len());
        let mut found = (max_length..lookahead)
            .find(|&i| SENTENCE_ENDINGS.contains(&remaining[i]));

        // 3. 仍未找到则扫描剩余全文
        if found.is_none() {
            found = (max_length..remaining.len())
                .find(|&i| SENTENCE_ENDINGS.contains(&remaining[i]));
        }

        match found {
            Some(i) => remaining = cut(&mut segments, remaining, i + 1),
            None => {
                // 全文无句末标点：整段作为一个片段，结束循环
                let whole: String = remaining.iter().collect();
                segments.push(whole.trim().to_string());
                remaining = Vec::new();
            }
        }
    }

    // TS 原版此处不 trim，保持一致
    if !remaining.is_empty() {
        segments.push(remaining.iter().collect());
    }
    segments
}

/// 在 `pos` 处切开：前半 trim 后（非空才）推入 segments，返回 trim 后的后半。
fn cut(segments: &mut Vec<String>, remaining: Vec<char>, pos: usize) -> Vec<char> {
    let head: String = remaining[..pos].iter().collect();
    let head = head.trim();
    if !head.is_empty() {
        segments.push(head.to_string());
    }
    let tail: String = remaining[pos..].iter().collect();
    tail.trim().chars().collect()
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test vtt::tests`
Expected: 全部 PASS

- [ ] **Step 5: 提交**

```bash
git add src/vtt.rs
git commit -m "feat(vtt): 移植 splitTextForVtt 切句逻辑"
```

---

### Task 3: VTT 生成

**Files:**
- Modify: `src/vtt.rs`
- Create: `src/lib.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `format_vtt_time`、`split_text_for_vtt`
- Produces: `pub fn generate_vtt(lines: &[String], durations: &[f64], vtt_max_length: usize) -> String`

> **验收标准变更**：本任务原计划与 TS 版做黄金文件逐字节对拍，现已取消。验收标准改为**功能正确**：时间轴单调递增、时间戳合法、切片不丢字。不再依赖 `pnpm`。

- [ ] **Step 1: 修正 `format_vtt_time` 的进位缺陷**

Task 1 曾刻意复刻 TS 原版的一个缺陷：时分由 floor 独立计算、秒由 `seconds % 60` 得出，导致 `59.9996` 输出 `00:00:60.000`——这是**非法的 WebVTT 时间戳**。当初复刻它的唯一理由是逐字节对拍，该要求已取消，故予以修正。

先把 Task 1 里那个测试改成期望正确行为：

```rust
// src/vtt.rs 的 mod tests：把 format_vtt_time_replicates_ts_carry_bug 整体替换为
#[test]
fn format_vtt_time_carries_into_minutes() {
    // 四舍五入到毫秒后恰好满 60 秒，必须进位而不是输出非法的 :60.000
    assert_eq!(format_vtt_time(59.9996), "00:01:00.000");
    assert_eq!(format_vtt_time(3599.9999), "01:00:00.000");
    assert_eq!(format_vtt_time(59.9994), "00:00:59.999");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test vtt::tests::format_vtt_time_carries_into_minutes`
Expected: FAIL，实际得到 `00:00:60.000`

- [ ] **Step 3: 改实现——先归一化到毫秒再拆分**

```rust
/// 格式化为 WebVTT 时间戳 `HH:MM:SS.mmm`。
/// 先把秒四舍五入到毫秒再拆分时/分/秒，因此进位始终正确
/// （`59.9996` → `00:01:00.000`，而非 TS 原版会产生的非法值 `00:00:60.000`）。
pub fn format_vtt_time(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0).round().max(0.0) as u64;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let secs = total_secs % 60;
    let minutes = (total_secs / 60) % 60;
    let hours = total_secs / 3600;
    format!("{hours:02}:{minutes:02}:{secs:02}.{ms:03}")
}
```

注意 `.max(0.0)`：负数时长在本流程中不应出现，钳制到 0 比产生回绕的巨大数值安全。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test vtt::`
Expected: 原有测试加新测试全部 PASS（`format_vtt_time_basics` 与 `format_vtt_time_rounds_to_three_decimals` 的期望值不受影响，因为它们不涉及进位）

- [ ] **Step 5: 建立 lib.rs 并写 `generate_vtt` 的失败测试**

新建 `src/lib.rs`，内容**只有**一行 `pub mod vtt;`（后续任务各自追加自己的模块声明）。把 `src/main.rs` 里的 `mod vtt;` 删掉。

```rust
// 追加到 src/vtt.rs 的 mod tests
#[test]
fn vtt_starts_with_header_and_numbers_cues_from_one() {
    let lines = vec!["第一段。".to_string(), "第二段。".to_string()];
    let out = generate_vtt(&lines, &[2.0, 3.0], 30);
    assert!(out.starts_with("WEBVTT\n\n"), "缺少 WEBVTT 头：{out}");
    assert!(out.contains("\n1\n00:00:00.000 --> 00:00:02.000\n第一段。\n"));
    assert!(out.contains("\n2\n00:00:02.000 --> 00:00:05.000\n第二段。\n"));
}

#[test]
fn long_paragraph_splits_and_shares_duration_by_char_count() {
    // 一段 40 字、含一个句号，会被切成两片；两片时长按字符数比例摊分
    let text = "第一句话写得比较长一点用来触发切分。第二句话也在这里继续往后写。";
    let lines = vec![text.to_string()];
    let out = generate_vtt(&lines, &[10.0], 30);
    let cues: Vec<&str> = out.lines().filter(|l| l.contains("-->")).collect();
    assert_eq!(cues.len(), 2, "期望切成 2 条 cue，实得 {}：{out}", cues.len());
    // 切片文本不丢字
    assert!(out.contains("第一句话写得比较长一点用来触发切分。"));
    assert!(out.contains("第二句话也在这里继续往后写。"));
}

#[test]
fn timeline_is_monotonically_increasing() {
    let lines = vec![
        "短句。".to_string(),
        "这是一段明显更长的文字用来触发切句逻辑从而产生多条字幕。后面还有一句。".to_string(),
        "结尾。".to_string(),
    ];
    let out = generate_vtt(&lines, &[1.5, 8.25, 2.0], 30);
    let mut prev_end = 0.0_f64;
    let mut cue_count = 0;
    for line in out.lines().filter(|l| l.contains("-->")) {
        let (start, end) = line.split_once(" --> ").unwrap();
        let (s, e) = (parse_ts(start), parse_ts(end));
        assert!(s >= prev_end - 1e-6, "时间轴回退：{line}");
        assert!(e >= s, "cue 结束早于开始：{line}");
        prev_end = e;
        cue_count += 1;
    }
    assert!(cue_count >= 4, "期望至少 4 条 cue，实得 {cue_count}");
}

/// 把 `HH:MM:SS.mmm` 解析回秒，仅测试用。
fn parse_ts(s: &str) -> f64 {
    let p: Vec<&str> = s.trim().split(':').collect();
    let sec: Vec<&str> = p[2].split('.').collect();
    p[0].parse::<f64>().unwrap() * 3600.0
        + p[1].parse::<f64>().unwrap() * 60.0
        + sec[0].parse::<f64>().unwrap()
        + sec[1].parse::<f64>().unwrap() / 1000.0
}
```

- [ ] **Step 6: 运行测试确认失败**

Run: `cargo test vtt::`
Expected: FAIL，`cannot find function generate_vtt`

- [ ] **Step 7: 实现**

```rust
/// 由段落文本与各段时长生成 WebVTT。
/// 段落过长时先切句，再按 `片段字符数 / 原段字符数` 比例摊分该段时长。
/// 片段经过 trim，其字符数之和可能小于原段，故摊分后的总时长可能略小于 duration。
pub fn generate_vtt(lines: &[String], durations: &[f64], vtt_max_length: usize) -> String {
    let mut out: Vec<String> = vec!["WEBVTT".to_string(), String::new()];
    let mut current_time = 0.0_f64;
    let mut vtt_index = 1_usize;

    for (i, text) in lines.iter().enumerate() {
        let duration = durations[i];
        let segments = split_text_for_vtt(text, vtt_max_length);

        let total_chars = text.chars().count().max(1) as f64;
        let segment_durations: Vec<f64> = if segments.len() == 1 {
            vec![duration]
        } else {
            segments
                .iter()
                .map(|s| duration * s.chars().count() as f64 / total_chars)
                .collect()
        };

        for (segment_text, segment_duration) in segments.iter().zip(segment_durations) {
            let start_time = current_time;
            let end_time = current_time + segment_duration;

            out.push(vtt_index.to_string());
            out.push(format!(
                "{} --> {}",
                format_vtt_time(start_time),
                format_vtt_time(end_time)
            ));
            out.push(segment_text.clone());
            out.push(String::new());

            current_time = end_time;
            vtt_index += 1;
        }
    }

    out.join("\n")
}
```

- [ ] **Step 8: 运行测试确认通过**

Run: `cargo test`
Expected: 全部 PASS

- [ ] **Step 9: 提交**

```bash
git add src/vtt.rs src/lib.rs src/main.rs
git commit -m "feat(vtt): 实现 generateVtt 并修正时间戳进位缺陷"
```

---

### Task 4: mp3 时长读取

**Files:**
- Create: `src/duration.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces: `pub fn mp3_duration_seconds(path: &Path) -> f64`

- [ ] **Step 1: 准备测试素材并写失败的测试**

用 ffmpeg 生成一段已知时长的 mp3 当测试素材：

```bash
mkdir -p tests/fixtures
ffmpeg -y -f lavfi -i "sine=frequency=440:duration=3.5" \
  -c:a libmp3lame -b:a 48k -ar 24000 -ac 1 tests/fixtures/tone_3s5.mp3
```

```rust
// tests/duration_test.rs
use std::path::Path;

#[test]
fn reads_mp3_duration_within_tolerance() {
    let d = panda::duration::mp3_duration_seconds(Path::new("tests/fixtures/tone_3s5.mp3"));
    assert!((d - 3.5).abs() < 0.1, "期望约 3.5 秒，实际 {d}");
}

#[test]
fn falls_back_to_byte_estimate_for_unreadable_file() {
    // 非 mp3 内容：解析失败后按 TS 版的 len/16000 估算
    std::fs::write("/tmp/not_audio.mp3", vec![0u8; 32000]).unwrap();
    let d = panda::duration::mp3_duration_seconds(Path::new("/tmp/not_audio.mp3"));
    assert!((d - 2.0).abs() < 1e-9, "期望回退估算 2.0 秒，实际 {d}");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --test duration_test`
Expected: FAIL，模块不存在

- [ ] **Step 3: 实现**

```bash
cargo add symphonia --no-default-features --features mp3
```

```rust
// src/duration.rs
use std::fs::File;
use std::path::Path;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// 读 mp3 时长（秒）。对应 packages/tts-node/src/duration.ts。
/// 解析失败时按 TS 版回退：文件字节数 / 16000。
pub fn mp3_duration_seconds(path: &Path) -> f64 {
    if let Some(d) = probe_duration(path) {
        return d;
    }
    std::fs::metadata(path).map(|m| m.len() as f64 / 16000.0).unwrap_or(0.0)
}

fn probe_duration(path: &Path) -> Option<f64> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?;
    let track_id = track.id;
    let time_base = track.codec_params.time_base?;

    // MP3 通常无 Xing 头，n_frames 缺失，因此累加每个包的时长
    let mut total: u64 = 0;
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() == track_id {
            total += packet.dur();
        }
    }
    if total == 0 {
        return None;
    }
    let t = time_base.calc_time(total);
    Some(t.seconds as f64 + t.frac)
}
```

在 `src/lib.rs` 加 `pub mod duration;`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test --test duration_test`
Expected: 2 个测试 PASS

- [ ] **Step 5: 提交**

```bash
git add src/duration.rs src/lib.rs tests/duration_test.rs tests/fixtures/tone_3s5.mp3
git commit -m "feat(duration): 用 symphonia 读取 mp3 时长，失败回退字节估算"
```

---

### Task 5: ffmpeg 探测与合并加速

**Files:**
- Create: `src/ffmpeg.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `pub fn assert_available() -> anyhow::Result<()>`
  - `pub fn merge_mp3_with_speed(inputs: &[PathBuf], output: &Path, speed: f64) -> anyhow::Result<()>`
  - `pub fn concat_list_body(inputs: &[PathBuf]) -> anyhow::Result<String>`（供测试断言转义）

- [ ] **Step 1: 写失败的测试**

```rust
// tests/ffmpeg_test.rs
use std::path::{Path, PathBuf};

#[test]
fn concat_list_escapes_single_quotes() {
    let body = panda::ffmpeg::concat_list_body(&[PathBuf::from("/tmp/it's here.mp3")]).unwrap();
    assert_eq!(body, r"file '/tmp/it'\''s here.mp3'");
}

#[test]
fn merges_two_clips_and_applies_atempo() {
    // 两段各 2 秒，1.1 倍速后应约 4 / 1.1 ≈ 3.64 秒
    for (i, name) in ["/tmp/m1.mp3", "/tmp/m2.mp3"].iter().enumerate() {
        std::process::Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i",
                   &format!("sine=frequency={}:duration=2", 300 + i * 100),
                   "-c:a", "libmp3lame", "-b:a", "48k", "-ar", "24000", "-ac", "1", name])
            .output().unwrap();
    }
    let out = Path::new("/tmp/merged.mp3");
    panda::ffmpeg::merge_mp3_with_speed(
        &[PathBuf::from("/tmp/m1.mp3"), PathBuf::from("/tmp/m2.mp3")], out, 1.1,
    ).unwrap();

    let d = panda::duration::mp3_duration_seconds(out);
    assert!((d - 3.64).abs() < 0.2, "期望约 3.64 秒，实际 {d}");
    // 中间的 concat 清单文件必须被清理
    assert!(!Path::new("/tmp/merged.mp3.concat.txt").exists());
}

#[test]
fn rejects_empty_input_list() {
    let err = panda::ffmpeg::merge_mp3_with_speed(&[], Path::new("/tmp/x.mp3"), 1.1).unwrap_err();
    assert!(err.to_string().contains("no input files"));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --test ffmpeg_test`
Expected: FAIL，模块不存在

- [ ] **Step 3: 实现**

```rust
// src/ffmpeg.rs
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// 探测 PATH 上是否有可用的 ffmpeg。对应 TS 版 assertFfmpegAvailable。
pub fn assert_available() -> Result<()> {
    let out = Command::new("ffmpeg").arg("-version").output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        _ => bail!(
            "TTS 的合并与变速步骤需要 ffmpeg。请安装 ffmpeg 并确保它在 PATH 上。"
        ),
    }
}

/// 生成 concat demuxer 的清单内容。路径转为绝对路径，单引号按 shell 规则转义。
pub fn concat_list_body(inputs: &[PathBuf]) -> Result<String> {
    let mut lines = Vec::with_capacity(inputs.len());
    for p in inputs {
        let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let s = abs.to_string_lossy().replace('\'', r"'\''");
        lines.push(format!("file '{s}'"));
    }
    Ok(lines.join("\n"))
}

/// 合并多个 mp3 并施加 atempo 变速。参数与 TS 版 mergeMp3WithSpeed 完全一致。
pub fn merge_mp3_with_speed(inputs: &[PathBuf], output: &Path, speed: f64) -> Result<()> {
    if inputs.is_empty() {
        bail!("merge_mp3_with_speed: no input files");
    }

    let list_path = PathBuf::from(format!("{}.concat.txt", output.display()));
    std::fs::write(&list_path, concat_list_body(inputs)?)
        .with_context(|| format!("写 concat 清单失败：{}", list_path.display()))?;

    let result = Command::new("ffmpeg")
        .args([
            "-y", "-f", "concat", "-safe", "0",
            "-i", &list_path.to_string_lossy(),
            "-vn", "-filter:a", &format!("atempo={speed}"),
            "-c:a", "libmp3lame", "-q:a", "2",
            &output.to_string_lossy(),
        ])
        .output();

    let _ = std::fs::remove_file(&list_path);

    let out = result.context("启动 ffmpeg 失败")?;
    if !out.status.success() {
        bail!(
            "ffmpeg 退出码 {:?}：\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}
```

在 `src/lib.rs` 加 `pub mod ffmpeg;`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test --test ffmpeg_test`
Expected: 3 个测试 PASS

- [ ] **Step 5: 提交**

```bash
git add src/ffmpeg.rs src/lib.rs tests/ffmpeg_test.rs
git commit -m "feat(ffmpeg): 探测可用性并实现 concat + atempo 合并"
```

---

### Task 6: Edge TTS 后端

**前置条件：Task 0 必须已通过，且 `docs/edge-protocol.md` 已写好。**

**Files:**
- Create: `src/tts/mod.rs`
- Create: `src/tts/backend.rs`
- Create: `src/tts/edge.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `pub fn normalize_voice_for_edge(voice_raw: &str) -> String`
  - `pub fn voice_to_lang(voice: &str) -> String`
  - `pub struct Synthesized { pub audio: Vec<u8>, pub timings: Option<Vec<WordTiming>> }`
  - `pub struct WordTiming { pub text: String, pub offset_ms: u64, pub duration_ms: u64 }`
  - `pub trait TtsBackend { async fn synth(&self, text: &str) -> anyhow::Result<Synthesized>; }`
  - `pub struct EdgeBackend { voice: String, lang: String, timeout: Duration }`
  - `pub fn EdgeBackend::new(voice: &str, timeout: Duration) -> Self`

- [ ] **Step 1: 写失败的测试（纯函数部分）**

```rust
// src/tts/edge.rs 内的 mod tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_plain_neural_suffix() {
        assert_eq!(normalize_voice_for_edge("zh-CN-Yunjian:Neural"), "zh-CN-YunjianNeural");
    }

    #[test]
    fn normalize_leaves_valid_name_untouched() {
        assert_eq!(normalize_voice_for_edge("zh-CN-YunjianNeural"), "zh-CN-YunjianNeural");
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
        assert_eq!(normalize_voice_for_edge("  zh-CN-YunjianNeural  "), "zh-CN-YunjianNeural");
    }

    #[test]
    fn lang_takes_first_two_segments() {
        assert_eq!(voice_to_lang("zh-CN-YunjianNeural"), "zh-CN");
        assert_eq!(voice_to_lang("en-US-AriaNeural"), "en-US");
        assert_eq!(voice_to_lang("weird"), "zh-CN");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test tts::edge`
Expected: FAIL，函数不存在

- [ ] **Step 3: 实现纯函数**

```rust
// src/tts/edge.rs
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
        return if base.ends_with("Neural") { base.to_string() } else { format!("{base}Neural") };
    }

    let variant = Regex::new(r"(?i)^(.+):(DragonHDFlashLatestNeural|DragonHD\w+|\w+Neural)$").unwrap();
    if let Some(c) = variant.captures(v) {
        let base = &c[1];
        return if base.ends_with("Neural") { base.to_string() } else { format!("{base}Neural") };
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
```

`cargo add regex`。在 `src/tts/mod.rs` 写 `pub mod backend; pub mod edge;`，`src/lib.rs` 加 `pub mod tts;`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test tts::edge`
Expected: 5 个测试 PASS

- [ ] **Step 5: 定义 backend 类型**

```rust
// src/tts/backend.rs
#[derive(Debug, Clone)]
pub struct WordTiming {
    pub text: String,
    pub offset_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug)]
pub struct Synthesized {
    pub audio: Vec<u8>,
    /// Edge 的 WordBoundary 事件。首版收集但不使用，为后续精确字幕对齐预留。
    pub timings: Option<Vec<WordTiming>>,
}

#[allow(async_fn_in_trait)]
pub trait TtsBackend {
    async fn synth(&self, text: &str) -> anyhow::Result<Synthesized>;
}
```

- [ ] **Step 6: 实现 EdgeBackend**

以下是完整实现骨架。**连接参数（URL 查询串、请求头、两条消息的确切格式）必须以 `docs/edge-protocol.md` 中实测通过的版本为准**——若与下面的代码有出入，以文档为准并同步修改这里。

```rust
// src/tts/edge.rs 追加
use crate::tts::backend::{Synthesized, TtsBackend, WordTiming};
use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const TRUSTED_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const WSS: &str = "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";
const OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

pub struct EdgeBackend {
    voice: String,
    lang: String,
    timeout: Duration,
}

impl EdgeBackend {
    pub fn new(voice: &str, timeout: Duration) -> Self {
        let voice = normalize_voice_for_edge(voice);
        let lang = voice_to_lang(&voice);
        Self { voice, lang, timeout }
    }
}

/// Windows FILETIME ticks 向下取整到 5 分钟，拼 token 后 SHA256 大写十六进制。
fn sec_ms_gec(unix_secs: u64) -> String {
    let mut ticks: u128 = (unix_secs as u128 + 11_644_473_600) * 10_000_000;
    ticks -= ticks % 3_000_000_000;
    let mut h = Sha256::new();
    h.update(format!("{}{}", ticks, TRUSTED_TOKEN).as_bytes());
    hex::encode_upper(h.finalize())
}

/// SSML 是 XML，文本中的这三个字符必须转义，否则服务端会拒绝整条消息。
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
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
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let conn_id = uuid::Uuid::new_v4().simple().to_string();
        let url = format!(
            "{WSS}?TrustedClientToken={TRUSTED_TOKEN}&Sec-MS-GEC={}\
             &Sec-MS-GEC-Version=1-130.0.2849.68&ConnectionId={conn_id}",
            sec_ms_gec(now)
        );

        let mut req = url.as_str().into_client_request()?;
        {
            let h = req.headers_mut();
            h.insert("Origin", "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".parse()?);
            h.insert("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0".parse()?);
            h.insert("Pragma", "no-cache".parse()?);
            h.insert("Cache-Control", "no-cache".parse()?);
        }

        let (mut ws, _) = tokio_tungstenite::connect_async(req)
            .await
            .context("连接 Edge read-aloud 端点失败")?;

        let ts = chrono::Utc::now()
            .format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)");

        ws.send(Message::Text(format!(
            "X-Timestamp:{ts}\r\nContent-Type:application/json; charset=utf-8\r\n\
             Path:speech.config\r\n\r\n\
             {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":\
             {{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"true\"}},\
             \"outputFormat\":\"{OUTPUT_FORMAT}\"}}}}}}}}"
        ).into())).await?;

        let req_id = uuid::Uuid::new_v4().simple().to_string();
        ws.send(Message::Text(format!(
            "X-RequestId:{req_id}\r\nContent-Type:application/ssml+xml\r\n\
             X-Timestamp:{ts}Z\r\nPath:ssml\r\n\r\n\
             <speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{}'>\
             <voice name='{}'><prosody pitch='+0Hz' rate='+0%' volume='+0%'>{}</prosody>\
             </voice></speak>",
            self.lang, self.voice, xml_escape(text)
        ).into())).await?;

        let mut audio: Vec<u8> = Vec::new();
        let mut timings: Vec<WordTiming> = Vec::new();

        while let Some(msg) = ws.next().await {
            match msg? {
                Message::Binary(b) => {
                    if b.len() < 2 { bail!("二进制帧过短：{} 字节", b.len()); }
                    let hdr_len = u16::from_be_bytes([b[0], b[1]]) as usize;
                    if b.len() < 2 + hdr_len { bail!("头部长度越界"); }
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
/// 元数据缺失不应导致合成失败。
fn collect_word_boundaries(frame: &str, out: &mut Vec<WordTiming>) {
    let Some(body) = frame.split("\r\n\r\n").nth(1) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return };
    let Some(items) = v.get("Metadata").and_then(|m| m.as_array()) else { return };
    for item in items {
        if item.get("Type").and_then(|t| t.as_str()) != Some("WordBoundary") { continue; }
        let Some(d) = item.get("Data") else { continue };
        let offset = d.get("Offset").and_then(|o| o.as_u64()).unwrap_or(0);
        let duration = d.get("Duration").and_then(|o| o.as_u64()).unwrap_or(0);
        let text = d.get("text").and_then(|t| t.get("Text")).and_then(|t| t.as_str())
            .unwrap_or("").to_string();
        // Edge 的时间单位是 100 纳秒 tick
        out.push(WordTiming {
            text,
            offset_ms: offset / 10_000,
            duration_ms: duration / 10_000,
        });
    }
}
```

补依赖：`cargo add serde_json`（`hex`、`sha2`、`uuid`、`chrono`、`futures-util`、`tokio-tungstenite` 在 Task 0 已加）。

- [ ] **Step 7: 写联网冒烟测试并运行**

```rust
// tests/edge_smoke.rs
// 需要网络，默认忽略；用 `cargo test -- --ignored` 显式运行
#[tokio::test]
#[ignore]
async fn synthesizes_a_short_chinese_line() {
    use panda::tts::backend::TtsBackend;
    let b = panda::tts::edge::EdgeBackend::new(
        "zh-CN-YunjianNeural", std::time::Duration::from_secs(120),
    );
    let r = b.synth("这是一段测试文本。").await.unwrap();
    assert!(r.audio.len() > 5000, "音频过小：{} 字节", r.audio.len());
}
```

Run: `cargo test --test edge_smoke -- --ignored`
Expected: PASS

- [ ] **Step 7: 提交**

```bash
git add src/tts/ src/lib.rs tests/edge_smoke.rs
git commit -m "feat(tts): 实现 Edge read-aloud 后端与音色名规范化"
```

---

### Task 7: 合成流水线编排

**Files:**
- Create: `src/tts/pipeline.rs`
- Modify: `src/tts/mod.rs`

**Interfaces:**
- Consumes: `EdgeBackend`、`TtsBackend`、`mp3_duration_seconds`、`merge_mp3_with_speed`、`generate_vtt`
- Produces:
  - `pub struct ProcessOptions { pub voice: String, pub speed_factor: f64, pub batch_size: usize, pub timeout: Duration }`
  - `pub fn parse_paragraphs(content: &str) -> Vec<String>`
  - `pub async fn process_narration_file(input: &Path, output_dir: &Path, opts: &ProcessOptions) -> anyhow::Result<()>`

- [ ] **Step 1: 写失败的测试**

```rust
// src/tts/pipeline.rs 内的 mod tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_are_non_empty_trimmed_lines() {
        let got = parse_paragraphs("第一段。\n\n  第二段。  \n\t\n第三段。\n");
        assert_eq!(got, vec!["第一段。", "第二段。", "第三段。"]);
    }

    #[test]
    fn paragraphs_of_blank_input_is_empty() {
        assert!(parse_paragraphs("\n\n   \n\t\n").is_empty());
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test tts::pipeline`
Expected: FAIL，函数不存在

- [ ] **Step 3: 实现**

```rust
// src/tts/pipeline.rs
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

use crate::duration::mp3_duration_seconds;
use crate::ffmpeg;
use crate::tts::backend::TtsBackend;
use crate::tts::edge::EdgeBackend;
use crate::vtt::generate_vtt;

const DEFAULT_MAX_RETRIES: u32 = 3;

pub struct ProcessOptions {
    pub voice: String,
    pub speed_factor: f64,
    pub batch_size: usize,
    pub timeout: Duration,
}

/// 非空行即一段。对应 process.ts 的 parseParagraphs。
pub fn parse_paragraphs(content: &str) -> Vec<String> {
    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// 重试包装：失败后退避 (attempt+1) * 2000ms，最多 DEFAULT_MAX_RETRIES 次。
async fn synth_with_retry(backend: &EdgeBackend, text: &str) -> Result<Vec<u8>> {
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..DEFAULT_MAX_RETRIES {
        match backend.synth(text).await {
            Ok(s) => return Ok(s.audio),
            Err(e) => {
                let wait = Duration::from_millis(((attempt + 1) as u64) * 2000);
                eprintln!("  ⚠️  {e} — {}ms 后重试（{}/{}）",
                    wait.as_millis(), attempt + 1, DEFAULT_MAX_RETRIES);
                tokio::time::sleep(wait).await;
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("合成失败且无错误记录")))
}

/// 读文稿 → 并发合成 → 合并加速 → 写 audio.mp3 与 audio.vtt → 清理中间文件。
pub async fn process_narration_file(
    input: &Path,
    output_dir: &Path,
    opts: &ProcessOptions,
) -> Result<()> {
    ffmpeg::assert_available()?;

    let content = std::fs::read_to_string(input)
        .with_context(|| format!("读取文稿失败：{}", input.display()))?;
    let lines = parse_paragraphs(&content);
    if lines.is_empty() {
        bail!("Narration file is empty (no non-empty lines)");
    }

    std::fs::create_dir_all(output_dir)?;

    let backend = Arc::new(EdgeBackend::new(&opts.voice, opts.timeout));
    let sem = Arc::new(Semaphore::new(opts.batch_size.max(1)));
    let total = lines.len();

    println!("🎙️  Edge-TTS (Rust)");
    println!("🔊 音色：{}", opts.voice);
    println!("⏱  单段超时 {}ms · 并发 {}", opts.timeout.as_millis(), opts.batch_size);
    println!("📝 共 {total} 段\n");

    let mut handles = Vec::with_capacity(total);
    for (i, text) in lines.iter().enumerate() {
        let index = i + 1;
        let path = output_dir.join(format!("sentence{index}.mp3"));
        let (backend, sem, text) = (backend.clone(), sem.clone(), text.clone());

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await?;
            let preview: String = text.chars().take(40).collect();
            let ellipsis = if text.chars().count() > 40 { "…" } else { "" };
            println!("[{index}/{total}] {preview}{ellipsis}");

            let audio = synth_with_retry(&backend, &text).await?;
            std::fs::write(&path, &audio)?;
            let dur = mp3_duration_seconds(&path);
            println!("    ✅ {} ({dur:.2}s)\n", path.display());
            Ok::<(usize, PathBuf, f64), anyhow::Error>((index, path, dur))
        }));
    }

    // 按 index 归位，保证顺序与文稿一致（并发完成顺序不确定）
    let mut rows: Vec<(usize, PathBuf, f64)> = Vec::with_capacity(total);
    for h in handles {
        rows.push(h.await??);
    }
    rows.sort_by_key(|r| r.0);

    let temp_paths: Vec<PathBuf> = rows.iter().map(|r| r.1.clone()).collect();
    let durations: Vec<f64> = rows.iter().map(|r| r.2).collect();

    let merged = output_dir.join("audio.mp3");
    println!("🔗 合并并以 atempo {} 加速 → {}", opts.speed_factor, merged.display());
    ffmpeg::merge_mp3_with_speed(&temp_paths, &merged, opts.speed_factor)?;

    let adjusted: Vec<f64> = durations.iter().map(|d| d / opts.speed_factor).collect();

    let vtt_path = output_dir.join("audio.vtt");
    std::fs::write(&vtt_path, generate_vtt(&lines, &adjusted, 30))?;
    println!("📝 VTT：{}", vtt_path.display());

    println!("🗑️  清理 sentence*.mp3…");
    for p in &temp_paths {
        let _ = std::fs::remove_file(p);
    }

    let total_secs: f64 = adjusted.iter().sum();
    println!("\n✅ 完成 — 音频 {}，加速后总长约 {total_secs:.2}s", merged.display());
    Ok(())
}
```

`cargo add tokio --features rt-multi-thread,macros,time,sync`（若前面未全开）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test tts::pipeline`
Expected: 2 个测试 PASS

- [ ] **Step 5: 提交**

```bash
git add src/tts/pipeline.rs src/tts/mod.rs
git commit -m "feat(tts): 实现并发合成、重试与合并编排"
```

---

### Task 8: CLI 与端到端验收

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `process_narration_file`、`ProcessOptions`
- Produces: `panda tts [INPUT] [OUTDIR]` 可执行子命令

- [ ] **Step 1: 写失败的测试**

```rust
// src/config.rs 内的 mod tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_size_defaults_to_three() {
        assert_eq!(resolve_batch_size(None), 3);
    }

    #[test]
    fn batch_size_is_capped_at_eight() {
        assert_eq!(resolve_batch_size(Some("99")), 8);
    }

    #[test]
    fn batch_size_rejects_zero_and_garbage() {
        assert_eq!(resolve_batch_size(Some("0")), 3);
        assert_eq!(resolve_batch_size(Some("abc")), 3);
    }

    #[test]
    fn timeout_falls_back_when_below_floor() {
        assert_eq!(resolve_timeout_ms(Some("1000")), 120_000);
        assert_eq!(resolve_timeout_ms(Some("15000")), 15_000);
        assert_eq!(resolve_timeout_ms(None), 120_000);
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test config`
Expected: FAIL，函数不存在

- [ ] **Step 3: 实现 config**

```rust
// src/config.rs
pub const DEFAULT_VOICE: &str = "zh-CN-YunjianNeural";
pub const SPEED_FACTOR: f64 = 1.1;
const DEFAULT_BATCH_SIZE: usize = 3;
const BATCH_SIZE_CAP: usize = 8;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MIN_TIMEOUT_MS: u64 = 15_000;

/// 解析并发数：非法或 <1 回落默认 3，超过 8 钳制为 8。
pub fn resolve_batch_size(raw: Option<&str>) -> usize {
    match raw.map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.parse::<usize>().ok()) {
        Some(n) if n >= 1 => n.min(BATCH_SIZE_CAP),
        _ => DEFAULT_BATCH_SIZE,
    }
}

/// 解析单段超时：非法或低于下限 15000 时回落默认 120000。
pub fn resolve_timeout_ms(raw: Option<&str>) -> u64 {
    match raw.map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.parse::<u64>().ok()) {
        Some(n) if n >= MIN_TIMEOUT_MS => n,
        _ => DEFAULT_TIMEOUT_MS,
    }
}

/// `SPIDER_OUTPUT_DIR`，默认 output/spider。
pub fn spider_output_dir() -> String {
    non_empty_env("SPIDER_OUTPUT_DIR").unwrap_or_else(|| "output/spider".into())
}

/// `TTS_OUTPUT_DIR`，默认 output/tts。
pub fn tts_output_dir() -> String {
    non_empty_env("TTS_OUTPUT_DIR").unwrap_or_else(|| "output/tts".into())
}

/// `TTS_INPUT_FILE`，默认 `<SPIDER_OUTPUT_DIR>/input.txt`。
pub fn tts_input_file() -> String {
    non_empty_env("TTS_INPUT_FILE").unwrap_or_else(|| format!("{}/input.txt", spider_output_dir()))
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test config`
Expected: 4 个测试 PASS

- [ ] **Step 5: 实现 CLI**

```bash
cargo add clap --features derive
```

```rust
// src/main.rs
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

use panda::config;
use panda::tts::pipeline::{process_narration_file, ProcessOptions};

#[derive(Parser)]
#[command(name = "panda", about = "口播视频自动化引擎（Rust）")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 口播文稿 → audio.mp3 + audio.vtt
    Tts {
        /// 文稿路径，默认 $TTS_INPUT_FILE 或 $SPIDER_OUTPUT_DIR/input.txt
        input: Option<PathBuf>,
        /// 输出目录，默认 $TTS_OUTPUT_DIR 或 output/tts
        outdir: Option<PathBuf>,
        /// 音色，默认 $EDGE_TTS_VOICE 或 zh-CN-YunjianNeural
        #[arg(long)]
        voice: Option<String>,
        /// 并发段数，默认 $EDGE_TTS_BATCH_SIZE 或 3，上限 8
        #[arg(long)]
        batch_size: Option<usize>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Tts { input, outdir, voice, batch_size } => {
            let input = input.unwrap_or_else(|| PathBuf::from(config::tts_input_file()));
            let outdir = outdir.unwrap_or_else(|| PathBuf::from(config::tts_output_dir()));

            let voice_raw = voice
                .or_else(|| std::env::var("EDGE_TTS_VOICE").ok())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| config::DEFAULT_VOICE.to_string());

            let opts = ProcessOptions {
                voice: panda::tts::edge::normalize_voice_for_edge(&voice_raw),
                speed_factor: config::SPEED_FACTOR,
                batch_size: batch_size.map(|n| n.clamp(1, 8)).unwrap_or_else(|| {
                    config::resolve_batch_size(std::env::var("EDGE_TTS_BATCH_SIZE").ok().as_deref())
                }),
                timeout: Duration::from_millis(config::resolve_timeout_ms(
                    std::env::var("EDGE_TTS_TIMEOUT_MS").ok().as_deref(),
                )),
            };

            process_narration_file(&input, &outdir, &opts).await
        }
    }
}
```

在 `src/lib.rs` 加 `pub mod config;`。

- [ ] **Step 6: 端到端验收**

```bash
cargo build --release
printf '大家好，欢迎收看本期节目。\n今天我们来聊一个有意思的话题。\n希望这期内容对你有帮助，我们下期再见。\n' > /tmp/e2e.txt
./target/release/panda tts /tmp/e2e.txt /tmp/e2e-out
```

期望：
- `/tmp/e2e-out/audio.mp3` 与 `/tmp/e2e-out/audio.vtt` 均存在
- `/tmp/e2e-out/` 下**没有**残留的 `sentence*.mp3` 与 `*.concat.txt`
- `ffprobe /tmp/e2e-out/audio.mp3` 时长约 8–12 秒
- `audio.vtt` 首行为 `WEBVTT`，最后一条字幕结束时间与音频时长相差 < 0.5 秒

- [ ] **Step 7: 提交**

```bash
git add src/config.rs src/main.rs src/lib.rs
git commit -m "feat(cli): 加入 panda tts 子命令与环境变量解析"
```

---

## 完成标准

- `cargo test` 全绿
- `panda tts` 能独立完成 `pnpm tts` 的产出，中间文件清理干净
- `docs/edge-protocol.md` 记录了实测通过的协议细节

下一份计划（渲染子系统）在此计划完成后编写。
