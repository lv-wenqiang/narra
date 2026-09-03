# panda-video-rs ffmpeg 合成与 CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把已有的逐帧渲染能力接上 ffmpeg，产出可播放的成片 mp4，并交付 `panda render` 与 `panda make` 两个子命令。

**Architecture:** Rust 侧把 `FrameSource` 逐帧渲染成 RGBA 位图，反预乘后经 stdin 以 rawvideo 喂给 ffmpeg；ffmpeg 一条 `filter_complex` 同时做视频叠加（背景视频 cover 裁切 + 亮度 0.8 + overlay 帧流）与四路音频混合（TTS / BGM / 打字机音效 / 片尾音效，各自延迟到段落起点）。**命令行构造是纯函数**（可断言参数向量），**进程管理**单独一层（写帧线程 + stderr 捕获 + broken pipe 处理）。

**Tech Stack:** Rust 2024 / tiny-skia（已有，帧位图）/ std::process（ffmpeg 子进程）/ clap（CLI）/ serde_json（读 title.json）/ 外部 ffmpeg 二进制。

**Spec:** `docs/superpowers/specs/2026-09-01-panda-video-rs-design.md`（**§6 CLI 契约**、**§9 ffmpeg 调用**全部、§5 模块划分、§10 验证策略）

## Global Constraints

- 画布 **1280 × 720**，**30 fps**，成片编码 **libx264 / CRF 23 / yuv420p**。
- 段落布局（`A` = 音频时长秒数 = VTT 最后一条字幕的结束时间）：Cover 帧 **0..15**；Intro **15..120**；Content **120..120+content_frames**，`content_frames = ceil((A+2)*30)`；Outro 最后 **120** 帧。总帧数 `240 + content_frames`。
- **成片总时长** = `total_frames / 30` 秒。
- 四路音频的起点与音量（规格 §9.3，逐字）：TTS `audio.mp3` 起点 **4.0s** 音量 **1.0**；BGM 起点 **4.0s** 音量 **0.15** 并在 **`[A-2, A]`（Content 段内时间）** 线性降到 0、之后保持 0；`intro_typewriter.mp3` 起点 **0.5s** 音量 **0.6**；`intro.mp3` 起点 **Outro 起点** 音量 **0.6**。
- **`amix` 必须设 `normalize=0`**，否则自动归一化会改变各路相对音量。
- 背景视频按 `objectFit: cover` 等价处理：`scale=...:force_original_aspect_ratio=increase` 后 `crop`；亮度用 **`colorchannelmixer=rr=0.8:gg=0.8:bb=0.8`**（**乘性**），**不得用 `eq=brightness`**（那是加性的，语义不同）。
- 背景视频与 BGM **保持为外部文件**（有 `shuffle:bg-video` / `shuffle:bgm` 换素材的流程）；`intro.mp3` 与 `intro_typewriter.mp3` **内嵌**（规格 §5 的 `assets/` 清单已列明）。
- 标题三级兜底：`--title` > `--title-json` 指向文件的 `title` 字段 > `熊猫智研社`。
- 默认路径：背景视频 `public/video/0.mp4`、BGM `public/bgm/0.mp3`、标题 JSON `public/video/title.json`、成片输出 `output/video/video.mp4`。
- 外部依赖只允许 **ffmpeg**。目标平台仅 **Linux（WSL2）**。
- 验收标准是**功能正确可用**，不要求与 TypeScript 原版逐像素/逐样本一致。

## 上一份计划交接的既成事实（不要重新发现）

- `FrameSource`（`src/render/frame.rs`）已可用：`new(vtt_text, title) -> Result<Self>`、`total_frames() -> u32`、`render(global_frame) -> Result<Pixmap>`。
- **`render()` 返回的 `Pixmap` 是预乘 alpha**。`-pix_fmt rgba` 是 straight alpha。**直接喂会让 Content 段字幕入场整体发暗**——本计划 Task 2 专门处理。
- 渲染性能实测（release）：**8.79 ms/帧、113.7 fps、3.8× 实时**；2100 帧（60s 成片）纯渲染 18.5 s。渲染不是瓶颈。
- `src/ffmpeg.rs` 已有 `assert_available()`（探测 `ffmpeg -version`）、`concat_list_body()`、`merge_mp3_with_speed()`。后两个是 TTS 用的，本计划不改它们。
- `src/config.rs` 已有 TTS 的环境变量层与 `resolve_*` 纯函数；`spider_output_dir()` / `tts_output_dir()` / `tts_input_file()` 三个读 `std::env` 的函数**至今没有测试**（`docs/follow-ups.md` 第 2 条点名本计划偿还）。
- `src/main.rs` 的 `Commands` 现有 `Tts` 与 `DebugFrames` 两个变体，`main` 是 `#[tokio::main] async fn`。**CLI 层至今零单测**（同上，本计划偿还）。
- **Cover 主标题超过约 2 行会压水印**：实测约 8~9 字/行，3 行时标题墨迹底 553 与水印墨迹顶 560 只剩 7px，≥4 行必压穿。见 `docs/follow-ups.md`。
- `Cargo.toml` 已有 `[profile.dev.package."*"] opt-level = 3`（只优化依赖）。**新增测试若慢，先想是不是 profile 问题再想收窄帧集。**

## 本机实测的素材事实（不要重新探测）

| 素材 | 实测 |
|---|---|
| `../panda-video-ts/public/audio/intro.mp3` | 84K，**2.486s**，44.1kHz 立体声 |
| `../panda-video-ts/public/audio/intro_typewriter.mp3` | 40K，**3.157s**，**24kHz** 立体声 |
| `../panda-video-ts/public/bgm/0.mp3` | 3.9M，**186.5s**，48kHz，**含内嵌 mjpeg 封面图（第二条流）** |
| `../panda-video-ts/public/video/0.mp4` | 6.2M，**1920×1080**，**AV1**，30fps，**162.3s** |

本机 `ffmpeg 8.1.2`，`libx264` / `aac` / `libdav1d` 均在场。

**三个由此而来的硬性要求：**

1. **BGM 有两条流**（`0=mp3(audio)`、`1=mjpeg(video)`）。filter_complex 里**必须显式写 `[N:a]`**，不能写 `[N]`，否则会把封面图当视频流拉进来。
2. **背景视频是 AV1**，软解 1080p30 比 H.264 重。Task 0 必须实测编码吞吐。
3. 两段音效**都短于所在段落**（3.157s < Intro 3.5s，2.486s < Outro 4s），是「留白」不是「裁剪」，`adelay` + `amix` 天然处理，**不需要 `atrim`**；而 BGM 186.5s 在长文稿下可能不够，`-stream_loop -1` 是必需的。

## 计划层已做的三条裁定

**R1 — BGM 淡出用 `afade=t=out` 而非规格 §9.3 写的「`volume` 滤镜的时间表达式」。** 理由：`volume` 的表达式里含逗号（`if(lt(t,X),...)`），而逗号在 filtergraph 里是滤镜分隔符，必须靠引号保护——这是一个容易写对一次、被后人改错且**成片照样能播**的脆弱点。`afade=t=out:st=<起点>:d=2` 默认 `curve=tri`（线性），语义与「在 2 秒内线性降到 0、之后保持 0」**完全等价**，且无需任何转义。代价：若判断有误，是淡出曲线形状与 `volume` 表达式版本有细微差别——`tri` 就是线性，实际无差别。

**R2 — BGM 淡出的时间基准换算到全片绝对时间。** 规格 §9.3 写的 `[A-2, A]` 是 **Content 段内时间**（TS 的 `Content.tsx` 用的是 `currentTimeMs`，段内相对）。Content 起点是 4.0s，故绝对时间是 **`[4.0 + A - 2, 4.0 + A]`**。**漏掉这个 +4.0 会让 BGM 整体提前 4 秒开始淡出，而成片能播、不报错**——属于不容易发现的那类。Task 3 有一条测试专门钉这个。

**R4 — 线程分工与规格 §9.4 的字面描述相反：帧流写在主线程，stderr 读在子线程。** 规格写的是「Rust 侧另起线程向 stdin 写帧，主线程等待进程结束」。两种分法都能避免死锁，但**必须有一个东西被并发读走**：`stderr` 用 `Stdio::piped()` 时，ffmpeg 写满管道缓冲区就会阻塞，而我们若同时阻塞在 `wait()` 上就是死锁。规格的写法把帧流放进线程、却没说 stderr 怎么办，照字面实现容易漏掉这一点（§9.4 又明确要求「捕获 stderr 全文」）。本计划把并发那一侧给 stderr：主线程写帧、写完 `drop(stdin)`、再 `wait()`。行为等价且少一处跨线程错误传递。代价：若判断有误（例如实测发现帧流必须并发才不阻塞），Task 0 的探针会先撞上，届时按探针记录的形态实现并在 Task 5 的报告里说明。

**R5 — 帧流写入器放在既有的 `src/render/frame.rs`，不新建规格 §5 列的 `render/frames.rs`。** 规格的模块表把「逐帧生成 → 写管道」列为 `frames.rs`，但上一份计划已经建了 `frame.rs` 承载 `FrameSource`，而写管道就是它的一个方法（复用同一份 `Painter` 与 `Layout`）。同时存在 `frame.rs` 与 `frames.rs` 只会让人反复看错。代价：与规格 §5 的文件名不一致——**Task 2 完成后请顺手把规格 §5 的 `frames.rs` 改成 `frame.rs`**，别让文档继续指向一个不存在的文件。

**R3 — 帧流在喂 ffmpeg 前反预乘。** `-pix_fmt rgba` 是 straight alpha，而 `Pixmap` 持预乘。另一条路是换用 ffmpeg 能解释预乘的像素格式，但那要验证 ffmpeg 侧语义；反预乘是纯计算、可单测、可断言。代价：每帧多一次逐像素转换（Task 2 会实测其开销占比）。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `assets/intro.mp3` | 内嵌片尾音效（从 `../panda-video-ts/public/audio/` 复制） |
| `assets/intro_typewriter.mp3` | 内嵌打字机音效（同上） |
| `src/assets.rs`（修改） | 追加两段音效的 `include_bytes!` 与「落到临时文件」的访问器 |
| `src/render/frame.rs`（修改） | 追加 `audio_secs()`、反预乘、帧流写入器 |
| `src/ffmpeg.rs`（修改） | 追加 `RenderInputs`、`build_render_args()`（纯函数）、`run_render()`（进程管理） |
| `src/config.rs`（修改） | 追加素材与产物的默认路径、标题三级兜底，并补齐既有环境变量层的测试 |
| `src/main.rs`（修改） | 追加 `render` 与 `make` 子命令，并补齐 CLI 层的测试 |
| `docs/ffmpeg-pipeline.md` | Task 0 探针产出：实测通过的管道形态与吞吐数据，供 Task 3/4/5 照抄 |

---

### Task 0: ffmpeg 管道联通性探针（关卡）

**这是探针任务，不是 TDD 任务。** 产出是一个答案和一份实测记录，不是要保留的生产代码。「rawvideo 经 stdin 喂进 filter_complex 并与 AV1 背景视频叠加」是本计划风险最高的一环——**必须先确认这条路走得通，再往下做**。

**Files:**
- Create: `examples/pipe_probe.rs`
- Create: `docs/ffmpeg-pipeline.md`
- Create: `assets/intro.mp3`、`assets/intro_typewriter.mp3`（复制，Task 1 会用）

**Interfaces:**
- Produces: `docs/ffmpeg-pipeline.md`，记录实测通过的命令形态与吞吐数字，供 Task 3/4/5 照抄。

- [ ] **Step 1: 复制两段音效**

```bash
cp ../panda-video-ts/public/audio/intro.mp3 assets/intro.mp3
cp ../panda-video-ts/public/audio/intro_typewriter.mp3 assets/intro_typewriter.mp3
ls -la assets/
```

- [ ] **Step 2: 写探针**

`examples/pipe_probe.rs`。它不依赖本项目的任何渲染代码——**故意只用最小依赖**，这样管道失败时能确定问题不在渲染侧。

```rust
//! ffmpeg 管道联通性探针（Task 0 关卡，用完即弃）。
//!
//! 目的：确认「rawvideo RGBA 经 stdin 喂进 filter_complex，与 AV1 背景视频
//! overlay 后编码成 mp4」这条路走得通，并测出编码吞吐。
//!
//! 运行：cargo run --release --example pipe_probe

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Instant;

const W: usize = 1280;
const H: usize = 720;
const FPS: usize = 30;
const FRAMES: usize = 90; // 3 秒

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bg = "../panda-video-ts/public/video/0.mp4";
    let out = "/tmp/pipe_probe.mp4";

    let mut child = Command::new("ffmpeg")
        .args([
            "-y",
            "-stream_loop", "-1", "-i", bg,
            "-f", "rawvideo", "-pix_fmt", "rgba",
            "-s", "1280x720", "-r", "30", "-i", "-",
            "-filter_complex",
            "[0:v]scale=1280:720:force_original_aspect_ratio=increase,\
             crop=1280:720,colorchannelmixer=rr=0.8:gg=0.8:bb=0.8[bg];\
             [bg][1:v]overlay=shortest=0[v]",
            "-map", "[v]",
            "-t", "3",
            "-c:v", "libx264", "-crf", "23", "-pix_fmt", "yuv420p",
            out,
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("stdin 已 piped");
    let started = Instant::now();

    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        // 画一个「左半边不透明红、右半边全透明」的图案：
        // 成片里应能看到左半边被红色盖住、右半边露出背景视频。
        // 这同时验证了 overlay 是否真的按 alpha 混合。
        let mut frame = vec![0u8; W * H * 4];
        for y in 0..H {
            for x in 0..W {
                let i = (y * W + x) * 4;
                if x < W / 2 {
                    frame[i] = 255;     // R
                    frame[i + 3] = 255; // A
                }
            }
        }
        for _ in 0..FRAMES {
            stdin.write_all(&frame)?;
        }
        stdin.flush()
    });

    let status = child.wait()?;
    let elapsed = started.elapsed();
    let write_result = writer.join().expect("写帧线程不应 panic");

    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        use std::io::Read;
        e.read_to_string(&mut stderr).ok();
    }

    println!("--- 退出状态: {status}");
    println!("--- 写帧结果: {write_result:?}");
    println!("--- 耗时: {elapsed:?}（{} 帧 → {:.1} fps）",
             FRAMES, FRAMES as f64 / elapsed.as_secs_f64());
    println!("--- stderr 末尾 ---\n{}",
             stderr.lines().rev().take(15).collect::<Vec<_>>()
                   .into_iter().rev().collect::<Vec<_>>().join("\n"));
    let _ = FPS;
    Ok(())
}
```

- [ ] **Step 3: 跑探针并核对产物**

```bash
cargo run --release --example pipe_probe
ffprobe -v error -show_entries stream=codec_name,width,height,r_frame_rate,nb_frames \
        -show_entries format=duration -of default=noprint_wrappers=1 /tmp/pipe_probe.mp4
ffmpeg -y -v error -ss 1 -i /tmp/pipe_probe.mp4 -frames:v 1 /tmp/pipe_probe.png
```

**验收（四条全中才算关卡通过）：**
1. ffmpeg 退出状态为 0，写帧线程返回 `Ok(())`。
2. `ffprobe` 报告 `codec_name=h264`、`1280x720`、`30/1`、`duration≈3.0`。
3. `/tmp/pipe_probe.png` **左半边是纯红、右半边能看到背景视频内容**——这证明 overlay 真的按 alpha 混合，而不是整块盖住或整块透明。**必须自己打开这张图看**，不要只看 ffprobe。
4. 记录吞吐 fps。

- [ ] **Step 4: 测 AV1 解码的额外开销**

把上面的命令改成不带 overlay 的纯转码（`-i bg -t 3 -c:v libx264 ...`），对比耗时，隔离出「AV1 解码」与「叠加+编码」各占多少。

```bash
time ffmpeg -y -v error -stream_loop -1 -i ../panda-video-ts/public/video/0.mp4 \
     -t 3 -vf scale=1280:720 -c:v libx264 -crf 23 -pix_fmt yuv420p /tmp/av1_only.mp4
```

- [ ] **Step 5: 写 `docs/ffmpeg-pipeline.md`**

必须记录（Task 3/4/5 会照抄，写错比不写更糟）：
- 实测通过的**完整命令行**（逐个参数，含转义形态）。
- **`-stream_loop -1` 必须写在 `-i bg` 之前**（它是输入选项）——若你发现顺序有讲究，写清楚。
- overlay 的 alpha 行为：确认 `-pix_fmt rgba` 被当作 **straight alpha**（这决定了 Task 2 必须反预乘）。**如果你的探针图案显示 ffmpeg 其实按预乘解释，立刻停下来报告**——那会推翻计划的 R3 裁定。
- 吞吐数字：叠加+编码的 fps、纯 AV1 解码的 fps、由此外推一份成片的预计耗时——
  注意「60 秒成片」本身有歧义，请分别给出「60 秒总视频输出」（30fps×60s=1800 帧）
  和「`A=60` 秒音频驱动的完整成片」（Content 段落 `ceil((A+2)*30)=1860` 帧，
  加上 Cover/Intro/Outro 固定 240 帧，总帧数 2100 帧，对应 70 秒视频）这两种
  口径各自的预计耗时，别把两者混为一谈。
- 写帧线程与 `child.wait()` 的**正确配合顺序**（先 take stdin、后 spawn 线程、再 wait；若你踩到死锁，把正确顺序写下来）。
- stderr 用 `Stdio::piped()` 时**必须在 `wait()` 之后或并发读取**，否则管道缓冲区满会死锁——记录你实测的行为。

- [ ] **Step 6: 清理与提交**

```bash
rm -f /tmp/pipe_probe.mp4 /tmp/pipe_probe.png /tmp/av1_only.mp4
git add assets/intro.mp3 assets/intro_typewriter.mp3 examples/pipe_probe.rs docs/ffmpeg-pipeline.md
git commit -m "spike: 验证 rawvideo 经 stdin 喂 ffmpeg 的合成管道"
```

---

### Task 1: 内嵌两段音效

**Files:**
- Modify: `src/assets.rs`

**Interfaces:**
- Consumes: Task 0 复制到 `assets/` 的两个 mp3。
- Produces:
  - `pub const INTRO_MP3: &[u8]`
  - `pub const INTRO_TYPEWRITER_MP3: &[u8]`
  - `pub fn write_embedded_audio(dir: &std::path::Path) -> anyhow::Result<(std::path::PathBuf, std::path::PathBuf)>` — 把两段音效写进 `dir`，返回 `(intro_path, typewriter_path)`

- [ ] **Step 1: 写失败的测试**

```rust
// 追加到 src/assets.rs 的 mod tests
#[test]
fn embedded_audio_is_non_empty_mp3() {
    // ID3v2 头是 "ID3"，裸 MPEG 帧头是 0xFF 0xFB/0xF3/0xF2。两者都算合法 mp3 开头。
    for (name, bytes) in [("intro", INTRO_MP3), ("typewriter", INTRO_TYPEWRITER_MP3)] {
        assert!(bytes.len() > 10_000, "{name} 太小，可能没复制成功：{}", bytes.len());
        let ok = bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0);
        assert!(ok, "{name} 开头不像 mp3：{:02X?}", &bytes[..4]);
    }
}

#[test]
fn write_embedded_audio_produces_two_readable_files() {
    let dir = std::env::temp_dir().join(format!("panda_assets_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (intro, typewriter) = write_embedded_audio(&dir).unwrap();

    assert_eq!(std::fs::read(&intro).unwrap(), INTRO_MP3, "写出的内容应与内嵌字节一致");
    assert_eq!(std::fs::read(&typewriter).unwrap(), INTRO_TYPEWRITER_MP3);
    assert_ne!(intro, typewriter, "两个文件不能是同一个路径");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn write_embedded_audio_is_idempotent() {
    // render 与 make 可能在同一个目录下先后调用，重复写不应报错。
    let dir = std::env::temp_dir().join(format!("panda_assets_idem_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let first = write_embedded_audio(&dir).unwrap();
    let second = write_embedded_audio(&dir).unwrap();
    assert_eq!(first, second);
    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test assets::`
Expected: FAIL，常量与函数不存在

- [ ] **Step 3: 实现**

```rust
// 追加到 src/assets.rs 顶部的常量区
pub const INTRO_MP3: &[u8] = include_bytes!("../assets/intro.mp3");
pub const INTRO_TYPEWRITER_MP3: &[u8] = include_bytes!("../assets/intro_typewriter.mp3");

/// 把两段内嵌音效写进 `dir`，返回 `(intro.mp3, intro_typewriter.mp3)` 的路径。
///
/// ffmpeg 的四路音频里有两路是内嵌资源，而 stdin 已经被帧流占用，无法再从
/// 管道喂第二、第三份数据；所以运行时落成真实文件是最简单可靠的做法。
/// 调用方负责选一个临时目录并在结束后清理。
pub fn write_embedded_audio(
    dir: &std::path::Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let intro = dir.join("intro.mp3");
    let typewriter = dir.join("intro_typewriter.mp3");
    std::fs::write(&intro, INTRO_MP3)
        .with_context(|| format!("写入内嵌片尾音效失败：{}", intro.display()))?;
    std::fs::write(&typewriter, INTRO_TYPEWRITER_MP3)
        .with_context(|| format!("写入内嵌打字机音效失败：{}", typewriter.display()))?;
    Ok((intro, typewriter))
}
```

`src/assets.rs` 顶部已有 `use anyhow::{Context, Result};`（若没有则补上）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test assets::`
Expected: 全部 PASS

- [ ] **Step 5: 提交**

```bash
git add src/assets.rs
git commit -m "feat(assets): 内嵌片尾与打字机音效"
```

---

### Task 2: 反预乘与帧流写入器

**Files:**
- Modify: `src/render/frame.rs`

**Interfaces:**
- Consumes: 既有 `FrameSource::{new, total_frames, render}`、`render::timeline::{WIDTH, HEIGHT, FPS}`
- Produces:
  - `pub fn FrameSource::audio_secs(&self) -> f64`
  - `pub fn FrameSource::content_frames(&self) -> u32`
  - `pub fn FrameSource::write_rgba_frames<W: std::io::Write>(&mut self, out: &mut W) -> anyhow::Result<u32>` — 逐帧渲染并写出 straight-alpha RGBA8，返回写出的帧数
  - `fn unpremultiply_into(pixmap: &tiny_skia::Pixmap, buf: &mut Vec<u8>)`（私有，但要被测试直接调用）

- [ ] **Step 1: 写失败的测试**

```rust
// 追加到 src/render/frame.rs 的 mod tests
use tiny_skia::{Paint, Pixmap, Rect, Transform};

/// 造一张含半透明像素的画布：左半边 alpha=128 的纯红，右半边全透明。
fn half_transparent_red() -> Pixmap {
    let mut p = Pixmap::new(4, 1).unwrap();
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 0, 0, 128);
    p.fill_rect(Rect::from_xywh(0.0, 0.0, 2.0, 1.0).unwrap(),
                &paint, Transform::identity(), None);
    p
}

#[test]
fn unpremultiply_restores_full_intensity_red_for_half_alpha_pixels() {
    let p = half_transparent_red();
    // 预乘态下红通道已经被 alpha 乘过：128/255*255 ≈ 128，而不是 255。
    assert!(p.data()[0] < 200, "前提检查：Pixmap 应是预乘的，实得 R={}", p.data()[0]);

    let mut buf = Vec::new();
    unpremultiply_into(&p, &mut buf);

    assert_eq!(buf.len(), 4 * 1 * 4, "输出应是 w*h*4 字节");
    // 反预乘后红通道应回到接近 255（整数除法允许 ±2 误差）。
    assert!(buf[0] >= 253, "反预乘后 R 应接近 255，实得 {}", buf[0]);
    assert_eq!(buf[3], 128, "alpha 通道不应被改动");
    // 全透明像素：alpha=0 时 RGB 无意义，但必须是 0 而不是垃圾值。
    assert_eq!(&buf[8..12], &[0, 0, 0, 0], "全透明像素应输出全 0");
}

#[test]
fn unpremultiply_leaves_opaque_pixels_byte_identical() {
    // alpha=255 时反预乘是恒等运算——这条保证 Cover/Intro/Outro 三段不受影响。
    let mut p = Pixmap::new(2, 1).unwrap();
    p.fill(tiny_skia::Color::from_rgba8(200, 100, 50, 255));
    let mut buf = Vec::new();
    unpremultiply_into(&p, &mut buf);
    assert_eq!(buf, vec![200, 100, 50, 255, 200, 100, 50, 255]);
}

#[test]
fn unpremultiply_reuses_the_buffer_without_growing_it() {
    // 写帧是热路径，缓冲区必须复用而不是每帧重新分配。
    let p = half_transparent_red();
    let mut buf = Vec::new();
    unpremultiply_into(&p, &mut buf);
    let cap = buf.capacity();
    for _ in 0..10 {
        unpremultiply_into(&p, &mut buf);
        assert_eq!(buf.len(), 16);
    }
    assert_eq!(buf.capacity(), cap, "重复调用不应导致重新分配");
}

const VTT: &str = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:04.000\n第一条。\n\n2\n00:00:04.000 --> 00:00:10.000\n第二条。\n";

#[test]
fn audio_secs_and_content_frames_follow_the_layout() {
    let fs = FrameSource::new(VTT, "标题".into()).unwrap();
    // A = 10 秒 → content = ceil(12*30) = 360 → 总帧 600
    assert!((fs.audio_secs() - 10.0).abs() < 1e-9, "实得 {}", fs.audio_secs());
    assert_eq!(fs.content_frames(), 360);
    assert_eq!(fs.total_frames(), 600);
}

/// 只统计字节数、不保存内容。整条时间轴是 600 帧 × 3.5MB ≈ 2.1GB，
/// 攒进 `Vec<u8>` 是不可接受的；写帧本来就是流式的，测试也该是流式的。
struct CountingWriter {
    bytes: usize,
}
impl std::io::Write for CountingWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.bytes += b.len();
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

/// 只记住指定帧的**首像素**，其余字节丢弃。用来在字节流里验证段落语义
/// 而不占内存。依赖 `write_rgba_frames` 每帧恰好一次 `write_all`。
struct FirstPixelPicker {
    frame_bytes: usize,
    seen: usize,
    picked: std::collections::HashMap<usize, [u8; 4]>,
}
impl std::io::Write for FirstPixelPicker {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        assert_eq!(b.len(), self.frame_bytes, "每帧应恰好一次 write_all 整帧");
        self.picked.insert(self.seen, [b[0], b[1], b[2], b[3]]);
        self.seen += 1;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

#[test]
fn write_rgba_frames_emits_exactly_one_frame_worth_of_bytes_per_frame() {
    let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
    let mut w = CountingWriter { bytes: 0 };
    let n = fs.write_rgba_frames(&mut w).unwrap();
    assert_eq!(n, 600);
    assert_eq!(w.bytes, 600 * 1280 * 720 * 4, "字节数必须精确等于 帧数×W×H×4");
}

#[test]
fn write_rgba_frames_emits_opaque_cover_and_transparent_content() {
    // 在字节流里验证段落语义：帧 0（Cover）左上角必须不透明，
    // 帧 200（Content）左上角必须全透明。这条同时钉住「反预乘没有破坏 alpha」。
    let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
    let mut w = FirstPixelPicker {
        frame_bytes: 1280 * 720 * 4,
        seen: 0,
        picked: std::collections::HashMap::new(),
    };
    fs.write_rgba_frames(&mut w).unwrap();

    let cover = w.picked[&0];
    let content = w.picked[&200];
    assert_eq!(cover[3], 255, "Cover 帧左上角应不透明");
    assert!(cover[0] > 240, "Cover 帧左上角应接近白色，实得 {}", cover[0]);
    assert_eq!(content[3], 0, "Content 帧左上角应全透明");
}

#[test]
fn write_rgba_frames_propagates_writer_errors_instead_of_panicking() {
    // ffmpeg 提前退出时写端会遇到 broken pipe，必须变成 Err 而不是 panic。
    struct FailAfter(usize);
    impl std::io::Write for FailAfter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "管道已关闭"));
            }
            self.0 -= 1;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
    let err = fs.write_rgba_frames(&mut FailAfter(3)).unwrap_err();
    assert!(format!("{err:#}").contains("管道"), "错误应透出底层原因：{err:#}");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test render::frame`
Expected: FAIL，`unpremultiply_into` / `audio_secs` / `write_rgba_frames` 不存在

- [ ] **Step 3: 实现**

```rust
// src/render/frame.rs 顶部补 use
use anyhow::Context;
use std::io::Write;

/// 把 `Pixmap` 的**预乘** RGBA8 转成 ffmpeg `-pix_fmt rgba` 要求的
/// **straight** alpha，写进 `buf`（会先 `clear()`，容量复用）。
///
/// 为什么必须做这一步：`tiny_skia::Pixmap` 内部存的是预乘值（R 已经乘过
/// alpha），而 ffmpeg 的 `rgba` 是 straight。直接喂过去，每个半透明像素会
/// 被再乘一次 alpha——Content 段字幕的入场动画（opacity 0→1）与 Outro 的
/// 整体淡出会整体发暗，而**成片能播、不报错**，属于不容易发现的那类。
///
/// `alpha == 0` 时 RGB 没有定义，统一输出 0，避免把预乘残留的垃圾值喂出去。
fn unpremultiply_into(pixmap: &Pixmap, buf: &mut Vec<u8>) {
    buf.clear();
    buf.reserve(pixmap.width() as usize * pixmap.height() as usize * 4);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        buf.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
}

impl FrameSource {
    /// 音频时长 `A`（秒）：VTT 最后一条字幕的结束时间。
    /// Content 段长 `ceil((A+2)*30)` 帧，BGM 的淡出区间由它推出。
    pub fn audio_secs(&self) -> f64 {
        self.captions.iter().map(|c| c.end_ms).max().unwrap_or(0) as f64 / 1000.0
    }

    /// Content 段的帧数。Outro 起点 = `120 + content_frames`。
    pub fn content_frames(&self) -> u32 {
        self.layout.content_frames
    }

    /// 逐帧渲染整条时间轴并写出 straight-alpha RGBA8，返回写出的帧数。
    ///
    /// 缓冲区跨帧复用（一帧 3.5MB，每帧重新分配是纯浪费）。写失败立即返回
    /// `Err` 并停止渲染——ffmpeg 提前退出时这里会收到 broken pipe，属于正常
    /// 的失败路径，不是 panic。
    pub fn write_rgba_frames<W: Write>(&mut self, out: &mut W) -> Result<u32> {
        let total = self.total_frames();
        let mut buf: Vec<u8> = Vec::with_capacity(WIDTH as usize * HEIGHT as usize * 4);
        for f in 0..total {
            let pixmap = self.render(f)?;
            unpremultiply_into(&pixmap, &mut buf);
            out.write_all(&buf)
                .with_context(|| format!("写第 {f} 帧到管道失败"))?;
        }
        out.flush().context("刷新帧流管道失败")?;
        Ok(total)
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test render::frame`
Expected: 全部 PASS

两条「写满整条时间轴」的测试各自要渲染 600 帧。**在报告里给出它们的实测耗时**；若单条超过 15 秒，参照本仓库既有做法收窄到代表帧（但**先确认不是 `[profile.dev.package."*"]` 没生效**——`Cargo.toml` 里已有这个设置，渲染测试本应快约 25×）。

`FirstPixelPicker` 断言了「每帧恰好一次 `write_all` 整帧」。若你的实现分多次写（例如按行写），这条会失败——**那说明实现该改成整帧一次写**，而不是把断言放宽：整帧一次写既少一次系统调用，也让下游的管道语义更简单。

- [ ] **Step 5: 实测反预乘的开销占比**

```bash
cargo run --release --example pipe_probe  # 若已删除，跳过
```

写一个临时的 `examples/_unpremul_bench.rs`，对同一张 1280×720 Pixmap 调 `unpremultiply_into` 1000 次，报告单次耗时；与既有的 8.79 ms/帧渲染成本对比，算出占比。写进报告后删掉该文件。

- [ ] **Step 6: 提交**

```bash
git add src/render/frame.rs
git commit -m "feat(render): 反预乘与帧流写入器"
```

---

### Task 3: 音频滤镜图构造（纯函数）

**Files:**
- Modify: `src/ffmpeg.rs`

**Interfaces:**
- Consumes: 无（纯字符串构造）
- Produces: `pub fn audio_filter_graph(audio_secs: f64, outro_start_secs: f64) -> String`

输入编号约定（Task 4 会按同一约定排列 `-i`）：`[2:a]` = TTS、`[3:a]` = BGM、`[4:a]` = 打字机、`[5:a]` = 片尾音效。

- [ ] **Step 1: 写失败的测试**

```rust
// 追加到 src/ffmpeg.rs 的 mod tests
#[test]
fn audio_graph_uses_explicit_audio_stream_selectors() {
    // BGM 文件带内嵌 mjpeg 封面图（第二条流）。写 [3] 会把封面图当视频流
    // 拉进来；必须写 [3:a]。四路一律显式选音频流。
    let g = audio_filter_graph(10.0, 16.0);
    for label in ["[2:a]", "[3:a]", "[4:a]", "[5:a]"] {
        assert!(g.contains(label), "缺少显式音频流选择器 {label}：{g}");
    }
    for bare in ["[2]", "[3]", "[4]", "[5]"] {
        assert!(!g.contains(bare), "不得使用裸流选择器 {bare}：{g}");
    }
}

#[test]
fn audio_graph_delays_each_source_to_its_segment_start() {
    // TTS 与 BGM 起点 4.0s；打字机 0.5s；片尾音效 = outro_start。
    let g = audio_filter_graph(10.0, 16.0);
    assert!(g.contains("adelay=4000:all=1"), "TTS/BGM 应延迟 4000ms：{g}");
    assert!(g.contains("adelay=500:all=1"), "打字机应延迟 500ms：{g}");
    assert!(g.contains("adelay=16000:all=1"), "片尾音效应延迟到 Outro 起点 16000ms：{g}");
}

#[test]
fn bgm_fade_starts_two_seconds_before_audio_end_in_absolute_time() {
    // 这是本计划最容易写错的一条（裁定 R2）：
    // 规格 §9.3 的 [A-2, A] 是 Content 段内时间，Content 起点是 4.0s，
    // 所以绝对时间是 [4+A-2, 4+A]。A=10 → 淡出从 12.0s 开始，持续 2s。
    // 漏掉 +4.0 会得到 8.0——成片照样能播，但 BGM 提前 4 秒淡出。
    let g = audio_filter_graph(10.0, 16.0);
    assert!(g.contains("afade=t=out:st=12:d=2"), "淡出应从绝对时间 12s 开始：{g}");
    assert!(!g.contains("st=8"), "st=8 说明漏掉了 Content 起点的 +4.0 偏移：{g}");
}

#[test]
fn bgm_fade_start_tracks_audio_length() {
    // 换一个 A 值，确认 12 不是写死的。A=30 → 4+30-2 = 32。
    let g = audio_filter_graph(30.0, 36.0);
    assert!(g.contains("afade=t=out:st=32:d=2"), "A=30 时淡出应从 32s 开始：{g}");
}

#[test]
fn bgm_is_ducked_to_zero_point_one_five_before_fading() {
    let g = audio_filter_graph(10.0, 16.0);
    assert!(g.contains("volume=0.15"), "BGM 基准音量应为 0.15：{g}");
}

#[test]
fn sound_effects_use_zero_point_six_and_tts_is_unattenuated() {
    let g = audio_filter_graph(10.0, 16.0);
    assert_eq!(g.matches("volume=0.6").count(), 2, "两段音效都应是 0.6：{g}");
    assert!(g.contains("volume=1"), "TTS 不衰减：{g}");
}

#[test]
fn amix_disables_normalization_and_mixes_four_inputs() {
    // normalize=1（默认）会按输入数自动缩放，把各路相对音量全改掉。
    let g = audio_filter_graph(10.0, 16.0);
    assert!(g.contains("amix=inputs=4"), "应混合四路：{g}");
    assert!(g.contains("normalize=0"), "必须关闭自动归一化：{g}");
}

#[test]
fn audio_graph_has_no_bare_commas_inside_filter_arguments() {
    // 逗号在 filtergraph 里是滤镜分隔符。裁定 R1 选 afade 而非 volume 表达式
    // 正是为了避免 if(lt(t,X),...) 这种带逗号的参数。这条测试钉住这个选择：
    // 如果有人把 afade 换回 volume 表达式，圆括号里就会出现逗号。
    let g = audio_filter_graph(10.0, 16.0);
    let mut depth = 0i32;
    for ch in g.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth > 0 => panic!("滤镜参数的括号内出现逗号，会被当成滤镜分隔符：{g}"),
            _ => {}
        }
    }
    assert_eq!(depth, 0, "括号不配对：{g}");
}

#[test]
fn audio_graph_ends_with_the_mixed_output_label() {
    let g = audio_filter_graph(10.0, 16.0);
    assert!(g.trim_end().ends_with("[a]"), "输出标签应为 [a]：{g}");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test ffmpeg::tests::audio`
Expected: FAIL，函数不存在

- [ ] **Step 3: 实现**

```rust
/// Content 段起点（秒）：Cover 15 帧 + Intro 105 帧 = 120 帧 = 4.0s。
const CONTENT_START_SECS: f64 = 4.0;
/// Intro 段起点（秒）：Cover 15 帧 = 0.5s。
const INTRO_START_SECS: f64 = 0.5;
/// BGM 淡出时长（秒），规格 §9.3。
const BGM_FADE_SECS: f64 = 2.0;
/// BGM 在 TTS 之下的基准音量，规格 §9.3。
const BGM_VOLUME: f64 = 0.15;
/// 两段音效的音量，规格 §9.3。
const SFX_VOLUME: f64 = 0.6;

/// 构造四路音频的 filter_complex 片段（不含视频部分）。
///
/// `audio_secs` 是 `A`（VTT 末条字幕的结束时间）；`outro_start_secs` 是
/// Outro 段起点的绝对时间 = `(120 + content_frames) / 30`。
///
/// **BGM 淡出的时间基准**：规格 §9.3 写的 `[A-2, A]` 是 **Content 段内**
/// 时间（TS 原版用的是段内相对时钟）。这里必须换算成全片绝对时间
/// `[CONTENT_START + A - 2, CONTENT_START + A]`。漏掉这个 +4.0 会让 BGM
/// 提前 4 秒开始淡出，而成片照样能播、不报错。
///
/// **为什么用 `afade` 而非规格写的 `volume` 时间表达式**：`volume` 的表达式
/// 形如 `if(lt(t,X),...)`，其中的逗号在 filtergraph 里是滤镜分隔符，必须靠
/// 引号保护——写对一次容易，被后人改错也容易，且失败形式是成片音量不对而
/// 非报错。`afade=t=out` 默认 `curve=tri`（线性），与「2 秒内线性降到 0、
/// 之后保持 0」语义完全等价，且不含逗号。
///
/// **`[N:a]` 而非 `[N]`**：BGM 文件带内嵌 mjpeg 封面图，是第二条流；不显式
/// 选音频流会把封面图当视频流拉进来。
pub fn audio_filter_graph(audio_secs: f64, outro_start_secs: f64) -> String {
    let content_ms = (CONTENT_START_SECS * 1000.0).round() as i64;
    let intro_ms = (INTRO_START_SECS * 1000.0).round() as i64;
    let outro_ms = (outro_start_secs * 1000.0).round() as i64;
    let fade_start = CONTENT_START_SECS + audio_secs - BGM_FADE_SECS;

    format!(
        "[2:a]adelay={content_ms}:all=1,volume=1[a_tts];\
         [3:a]adelay={content_ms}:all=1,volume={BGM_VOLUME},\
         afade=t=out:st={fade_start}:d={BGM_FADE_SECS}[a_bgm];\
         [4:a]adelay={intro_ms}:all=1,volume={SFX_VOLUME}[a_type];\
         [5:a]adelay={outro_ms}:all=1,volume={SFX_VOLUME}[a_intro];\
         [a_tts][a_bgm][a_type][a_intro]amix=inputs=4:normalize=0:duration=longest[a]"
    )
}
```

**注意 `{fade_start}` 的格式化**：`f64` 的 `Display` 对 `12.0` 输出 `12`，对 `12.5` 输出 `12.5`——都是 ffmpeg 能接受的形式，测试里断言的 `st=12` 与此一致。若你发现某个 `A` 值产生了科学计数法或过长的小数（例如 `12.000000000000002`），改成 `format!("{fade_start:.3}")` 并**同步更新测试断言**，在报告里说明。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test ffmpeg::tests::`
Expected: 全部 PASS

- [ ] **Step 5: 提交**

```bash
git add src/ffmpeg.rs
git commit -m "feat(ffmpeg): 四路音频滤镜图构造"
```

---

### Task 4: 视频滤镜图与完整命令行构造（纯函数）

**Files:**
- Modify: `src/ffmpeg.rs`

**Interfaces:**
- Consumes: Task 3 的 `audio_filter_graph`
- Produces:
  - `pub struct RenderInputs<'a> { pub bg: &'a Path, pub tts_audio: &'a Path, pub bgm: &'a Path, pub typewriter: &'a Path, pub intro: &'a Path, pub out: &'a Path, pub total_frames: u32, pub audio_secs: f64, pub content_frames: u32 }`
  - `pub fn build_render_args(i: &RenderInputs) -> Vec<String>`

- [ ] **Step 1: 写失败的测试**

```rust
// 追加到 src/ffmpeg.rs 的 mod tests
fn sample_inputs() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf,
                       std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    (
        "/m/bg.mp4".into(), "/m/audio.mp3".into(), "/m/bgm.mp3".into(),
        "/m/typewriter.mp3".into(), "/m/intro.mp3".into(), "/m/out.mp4".into(),
    )
}

fn sample_args() -> Vec<String> {
    let (bg, tts, bgm, tw, intro, out) = sample_inputs();
    build_render_args(&RenderInputs {
        bg: &bg, tts_audio: &tts, bgm: &bgm, typewriter: &tw, intro: &intro, out: &out,
        total_frames: 600, audio_secs: 10.0, content_frames: 360,
    })
}

/// 取 `args` 里 `flag` 后面紧跟的那个值。
fn value_after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

#[test]
fn inputs_appear_in_the_order_the_filter_graph_assumes() {
    // filter_complex 用 [0:v] 背景、[1:v] 帧流、[2:a] TTS、[3:a] BGM、
    // [4:a] 打字机、[5:a] 片尾音效。-i 的顺序就是编号，错位会静默画错。
    let args = sample_args();
    let inputs: Vec<&String> = args.iter().zip(args.iter().skip(1))
        .filter(|(f, _)| *f == "-i").map(|(_, v)| v).collect();
    assert_eq!(inputs.len(), 6, "应有 6 个输入：{args:?}");
    assert_eq!(inputs[0], "/m/bg.mp4");
    assert_eq!(inputs[1], "-", "第二个输入必须是 stdin 帧流");
    assert_eq!(inputs[2], "/m/audio.mp3");
    assert_eq!(inputs[3], "/m/bgm.mp3");
    assert_eq!(inputs[4], "/m/typewriter.mp3");
    assert_eq!(inputs[5], "/m/intro.mp3");
}

#[test]
fn stream_loop_precedes_the_inputs_it_applies_to() {
    // -stream_loop 是输入选项，必须紧接在它要循环的那个 -i 之前。
    // 放错位置会静默失效：背景视频与 BGM 播完就断，而 ffmpeg 不报错。
    let args = sample_args();
    let loops: Vec<usize> = args.iter().enumerate()
        .filter(|(_, a)| *a == "-stream_loop").map(|(i, _)| i).collect();
    assert_eq!(loops.len(), 2, "背景视频与 BGM 都需要循环：{args:?}");
    for i in loops {
        assert_eq!(args[i + 1], "-1", "应是无限循环");
        assert_eq!(args[i + 2], "-i", "-stream_loop 必须紧贴它的 -i：{args:?}");
    }
    // 且这两个 -i 分别是背景视频与 BGM
    let looped: Vec<&String> = args.iter().enumerate()
        .filter(|(i, a)| *a == "-stream_loop" && args.get(i + 3).is_some())
        .map(|(i, _)| &args[i + 3]).collect();
    assert!(looped.contains(&&"/m/bg.mp4".to_string()), "背景视频应被循环：{looped:?}");
    assert!(looped.contains(&&"/m/bgm.mp3".to_string()), "BGM 应被循环：{looped:?}");
}

#[test]
fn raw_frame_stream_declares_size_rate_and_pixel_format() {
    let args = sample_args();
    assert_eq!(value_after(&args, "-f").as_deref(), Some("rawvideo"));
    assert_eq!(value_after(&args, "-pix_fmt").as_deref(), Some("rgba"),
               "帧流是 straight-alpha RGBA8");
    assert_eq!(value_after(&args, "-s").as_deref(), Some("1280x720"));
    assert_eq!(value_after(&args, "-r").as_deref(), Some("30"));
}

#[test]
fn background_uses_cover_scaling_and_multiplicative_brightness() {
    let args = sample_args();
    let g = value_after(&args, "-filter_complex").expect("应有 filter_complex");
    assert!(g.contains("scale=1280:720:force_original_aspect_ratio=increase"),
            "objectFit:cover 的等价是 increase + crop：{g}");
    assert!(g.contains("crop=1280:720"), "{g}");
    assert!(g.contains("colorchannelmixer=rr=0.8:gg=0.8:bb=0.8"),
            "CSS brightness(0.8) 是乘性的：{g}");
    assert!(!g.contains("eq=brightness"),
            "eq=brightness 是加性的，语义不同，不得使用：{g}");
}

#[test]
fn frame_stream_is_overlaid_on_the_background() {
    let args = sample_args();
    let g = value_after(&args, "-filter_complex").unwrap();
    assert!(g.contains("[bg][1:v]overlay=shortest=0[v]"),
            "帧流应叠在处理后的背景之上：{g}");
}

#[test]
fn filter_complex_embeds_the_audio_graph() {
    // 视频与音频合成一条 filter_complex；音频部分由 Task 3 的函数产出。
    let args = sample_args();
    let g = value_after(&args, "-filter_complex").unwrap();
    assert!(g.contains(&audio_filter_graph(10.0, 16.0)),
            "应内嵌 audio_filter_graph 的产物：{g}");
}

#[test]
fn outro_start_is_derived_from_content_frames_not_hardcoded() {
    // Outro 起点 = (120 + content_frames) / 30。content_frames=360 → 16.0s。
    // 换一组数验证不是写死的：content_frames=90 → (120+90)/30 = 7.0s。
    let (bg, tts, bgm, tw, intro, out) = sample_inputs();
    let args = build_render_args(&RenderInputs {
        bg: &bg, tts_audio: &tts, bgm: &bgm, typewriter: &tw, intro: &intro, out: &out,
        total_frames: 330, audio_secs: 1.0, content_frames: 90,
    });
    let g = value_after(&args, "-filter_complex").unwrap();
    assert!(g.contains("adelay=7000:all=1"), "Outro 起点应为 7000ms：{g}");
}

#[test]
fn duration_comes_from_total_frames_at_thirty_fps() {
    let args = sample_args();
    assert_eq!(value_after(&args, "-t").as_deref(), Some("20"),
               "600 帧 / 30fps = 20 秒");
}

#[test]
fn output_encoding_matches_the_spec() {
    let args = sample_args();
    assert!(args.windows(2).any(|w| w[0] == "-c:v" && w[1] == "libx264"));
    assert!(args.windows(2).any(|w| w[0] == "-crf" && w[1] == "23"));
    assert!(args.windows(2).any(|w| w[0] == "-c:a" && w[1] == "aac"),
            "mp4 容器需要 aac 音频：{args:?}");
    // 输出的像素格式是 yuv420p（与输入帧流的 rgba 是两回事）
    let pix: Vec<&String> = args.iter().zip(args.iter().skip(1))
        .filter(|(f, _)| *f == "-pix_fmt").map(|(_, v)| v).collect();
    assert_eq!(pix, vec!["rgba", "yuv420p"], "输入 rgba、输出 yuv420p：{pix:?}");
    assert_eq!(args.last().map(String::as_str), Some("/m/out.mp4"),
               "输出路径必须在最后：{args:?}");
}

#[test]
fn maps_only_the_composed_video_and_mixed_audio() {
    let args = sample_args();
    let maps: Vec<&String> = args.iter().zip(args.iter().skip(1))
        .filter(|(f, _)| *f == "-map").map(|(_, v)| v).collect();
    assert_eq!(maps, vec!["[v]", "[a]"],
               "只映射合成后的视频与混合后的音频，不得带上任何原始流：{maps:?}");
}

#[test]
fn overwrites_without_prompting() {
    // 没有 -y 时 ffmpeg 会在目标已存在时交互式询问，管道场景下会挂死。
    assert!(sample_args().contains(&"-y".to_string()));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test ffmpeg::tests::`
Expected: FAIL，`RenderInputs` / `build_render_args` 不存在

- [ ] **Step 3: 实现**

```rust
use crate::render::timeline::{FPS, HEIGHT, WIDTH};

/// 一次成片合成所需的全部输入与参数。
pub struct RenderInputs<'a> {
    pub bg: &'a Path,
    pub tts_audio: &'a Path,
    pub bgm: &'a Path,
    pub typewriter: &'a Path,
    pub intro: &'a Path,
    pub out: &'a Path,
    pub total_frames: u32,
    pub audio_secs: f64,
    pub content_frames: u32,
}

/// 构造完整的 ffmpeg 参数向量（不含程序名）。
///
/// 纯函数，不碰文件系统也不起进程——规格 §5 要求 `ffmpeg` 模块「只构造参数
/// （可断言命令行）」，进程管理是 `run_render` 的事。
///
/// **输入顺序即 filter_complex 里的编号**：0 背景视频、1 stdin 帧流、
/// 2 TTS、3 BGM、4 打字机、5 片尾音效。改动顺序必须同步改滤镜图。
///
/// **`-stream_loop -1` 是输入选项**，必须紧贴它要循环的那个 `-i`。放错位置
/// 会静默失效（素材播完即止，ffmpeg 不报错）。
pub fn build_render_args(i: &RenderInputs) -> Vec<String> {
    let outro_start_secs = (120 + i.content_frames) as f64 / FPS as f64;
    let total_secs = i.total_frames as f64 / FPS as f64;

    let filter = format!(
        "[0:v]scale={WIDTH}:{HEIGHT}:force_original_aspect_ratio=increase,\
         crop={WIDTH}:{HEIGHT},colorchannelmixer=rr=0.8:gg=0.8:bb=0.8[bg];\
         [bg][1:v]overlay=shortest=0[v];{}",
        audio_filter_graph(i.audio_secs, outro_start_secs)
    );

    let s = |p: &Path| p.to_string_lossy().into_owned();
    vec![
        "-y".into(),
        // 0: 背景视频（循环）
        "-stream_loop".into(), "-1".into(), "-i".into(), s(i.bg),
        // 1: 帧流（stdin）
        "-f".into(), "rawvideo".into(),
        "-pix_fmt".into(), "rgba".into(),
        "-s".into(), format!("{WIDTH}x{HEIGHT}"),
        "-r".into(), FPS.to_string(),
        "-i".into(), "-".into(),
        // 2: TTS
        "-i".into(), s(i.tts_audio),
        // 3: BGM（循环）
        "-stream_loop".into(), "-1".into(), "-i".into(), s(i.bgm),
        // 4: 打字机音效
        "-i".into(), s(i.typewriter),
        // 5: 片尾音效
        "-i".into(), s(i.intro),
        "-filter_complex".into(), filter,
        "-map".into(), "[v]".into(),
        "-map".into(), "[a]".into(),
        "-t".into(), format!("{total_secs}"),
        "-c:v".into(), "libx264".into(),
        "-crf".into(), "23".into(),
        "-pix_fmt".into(), "yuv420p".into(),
        "-c:a".into(), "aac".into(),
        "-b:a".into(), "192k".into(),
        s(i.out),
    ]
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test ffmpeg::tests::`
Expected: 全部 PASS

- [ ] **Step 5: 提交**

```bash
git add src/ffmpeg.rs
git commit -m "feat(ffmpeg): 视频滤镜图与完整命令行构造"
```

---

### Task 5: 进程管理与执行

**Files:**
- Modify: `src/ffmpeg.rs`

**Interfaces:**
- Consumes: Task 2 的 `FrameSource::write_rgba_frames`、Task 4 的 `build_render_args`
- Produces: `pub fn run_render(source: &mut crate::render::frame::FrameSource, inputs: &RenderInputs) -> anyhow::Result<()>`

- [ ] **Step 1: 写失败的测试**

这一层要起真实进程，纯单测覆盖不到全部。按「能单测的单测、要真跑的用 `#[ignore]` 集成测试」拆开。

```rust
// 追加到 src/ffmpeg.rs 的 mod tests

#[test]
fn run_render_reports_ffmpeg_stderr_verbatim_when_it_exits_nonzero() {
    // 用一个必然失败的参数组合（输出到不存在的目录）触发 ffmpeg 非零退出，
    // 确认错误信息把 ffmpeg 自己的话原样透出，而不是吞掉或改写。
    let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n短。\n";
    let mut fs = crate::render::frame::FrameSource::new(vtt, "标题".into()).unwrap();
    let missing = std::path::Path::new("/nonexistent-dir-xyz/out.mp4");
    let bg = std::path::Path::new("/nonexistent-bg-xyz.mp4");
    let a = std::path::Path::new("/nonexistent-a-xyz.mp3");
    // 三个数值必须在 &mut fs 之前算好：否则 &mut fs 与 &fs 同时活着，借用检查不过。
    let total_frames = fs.total_frames();
    let audio_secs = fs.audio_secs();
    let content_frames = fs.content_frames();
    let err = run_render(&mut fs, &RenderInputs {
        bg, tts_audio: a, bgm: a, typewriter: a, intro: a, out: missing,
        total_frames, audio_secs, content_frames,
    }).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("ffmpeg"), "错误应指明是 ffmpeg 失败：{msg}");
    // ffmpeg 找不到输入时会说 "No such file or directory"
    assert!(msg.contains("No such file") || msg.contains("Invalid"),
            "应原样透出 ffmpeg 的 stderr：{msg}");
}

#[test]
fn run_render_does_not_panic_when_ffmpeg_exits_early() {
    // ffmpeg 因参数错误立刻退出时，写帧线程会遇到 broken pipe。
    // 这条测试的全部要求就是：返回 Err，不 panic，不挂死。
    let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:20.000\n够长的一条，保证帧数多到写端会撞上已关闭的管道。\n";
    let mut fs = crate::render::frame::FrameSource::new(vtt, "标题".into()).unwrap();
    let bad = std::path::Path::new("/nonexistent-xyz.mp4");
    let total = fs.total_frames();
    let audio_secs = fs.audio_secs();
    let content_frames = fs.content_frames();
    let result = run_render(&mut fs, &RenderInputs {
        bg: bad, tts_audio: bad, bgm: bad, typewriter: bad, intro: bad,
        out: std::path::Path::new("/tmp/panda_early_exit_test.mp4"),
        total_frames: total, audio_secs, content_frames,
    });
    assert!(result.is_err(), "应返回 Err");
    std::fs::remove_file("/tmp/panda_early_exit_test.mp4").ok();
}
```

再建一个真跑的集成测试：

```rust
// tests/render_e2e.rs
//! 端到端合成测试。需要 ffmpeg 与真实素材，默认 #[ignore]。
//! 手动运行：cargo test --test render_e2e -- --ignored --nocapture

use std::path::Path;

#[test]
#[ignore = "需要 ffmpeg 与 ../panda-video-ts/public 下的素材，且耗时约 30 秒"]
fn produces_a_playable_mp4_with_video_and_audio_streams() {
    let bg = Path::new("../panda-video-ts/public/video/0.mp4");
    let bgm = Path::new("../panda-video-ts/public/bgm/0.mp3");
    if !bg.exists() || !bgm.exists() {
        eprintln!("跳过：素材不存在");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("panda_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let (intro, typewriter) = panda::assets::write_embedded_audio(&tmp).unwrap();

    // 用打字机音效充当 TTS 音轨——本测试只验证管道通、流齐、时长对，
    // 不验证语音内容。
    let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:03.000\n第一条字幕。\n\n\
               2\n00:00:03.000 --> 00:00:06.000\n第二条字幕，稍微长一点点。\n";
    let mut fs = panda::render::frame::FrameSource::new(vtt, "端到端测试标题".into()).unwrap();
    let out = tmp.join("out.mp4");

    let total = fs.total_frames();
    let audio_secs = fs.audio_secs();
    let content_frames = fs.content_frames();
    panda::ffmpeg::run_render(&mut fs, &panda::ffmpeg::RenderInputs {
        bg, tts_audio: &typewriter, bgm, typewriter: &typewriter, intro: &intro,
        out: &out, total_frames: total, audio_secs, content_frames,
    }).unwrap();

    assert!(out.exists(), "成片应存在");
    let meta = std::fs::metadata(&out).unwrap();
    assert!(meta.len() > 50_000, "成片太小，可能是空壳：{} 字节", meta.len());

    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name",
               "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1"])
        .arg(&out).output().unwrap();
    let info = String::from_utf8_lossy(&probe.stdout);
    assert!(info.contains("codec_type=video"), "应有视频流：{info}");
    assert!(info.contains("codec_type=audio"), "应有音频流：{info}");
    assert!(info.contains("codec_name=h264"), "视频应是 h264：{info}");
    assert!(info.contains("codec_name=aac"), "音频应是 aac：{info}");

    // 总帧数 = 240 + ceil((6+2)*30) = 240 + 240 = 480 帧 = 16.0 秒
    let dur: f64 = info.lines().find_map(|l| l.strip_prefix("duration="))
        .and_then(|v| v.parse().ok()).expect("应有 duration");
    assert!((dur - 16.0).abs() < 0.5, "时长应约 16 秒，实得 {dur}");

    std::fs::remove_dir_all(&tmp).ok();
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test ffmpeg::tests::run_render`
Expected: FAIL，`run_render` 不存在

- [ ] **Step 3: 实现**

照抄 Task 0 的 `docs/ffmpeg-pipeline.md` 里记录的正确配合顺序。

```rust
use std::io::Read;
use std::process::{Command, Stdio};

/// 起 ffmpeg 子进程，另开线程把帧流写进它的 stdin，主线程等待结束。
///
/// **顺序很要紧**（Task 0 探针实测确认）：先 `spawn`，再 `take()` 走 stdin，
/// 再起写帧线程，然后**并发读 stderr**，最后 `wait()`。stderr 用管道时若不
/// 并发读取，缓冲区写满会让 ffmpeg 阻塞、而我们在 `wait()` 上阻塞——死锁。
///
/// **ffmpeg 提前退出**（参数错误、素材缺失）时写帧线程会遇到 broken pipe。
/// 那是正常的失败路径：线程返回 `Err`，我们优先报告 ffmpeg 自己的 stderr，
/// 因为它说的才是根因；写端的 broken pipe 只是后果。
pub fn run_render(
    source: &mut crate::render::frame::FrameSource,
    inputs: &RenderInputs,
) -> Result<()> {
    assert_available()?;

    if let Some(parent) = inputs.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建输出目录失败：{}", parent.display()))?;
        }
    }

    let args = build_render_args(inputs);
    let mut child = Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("启动 ffmpeg 失败")?;

    let mut stdin = child.stdin.take().expect("stdin 已声明为 piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr 已声明为 piped");

    // stderr 必须并发读，否则管道缓冲区满会死锁。
    let stderr_reader = std::thread::spawn(move || {
        let mut s = String::new();
        stderr_pipe.read_to_string(&mut s).ok();
        s
    });

    let write_result = source.write_rgba_frames(&mut stdin);
    // 显式 drop 关闭管道，让 ffmpeg 知道输入结束；否则它会一直等。
    drop(stdin);

    let status = child.wait().context("等待 ffmpeg 结束失败")?;
    let stderr = stderr_reader.join().unwrap_or_default();

    if !status.success() {
        // ffmpeg 的 stderr 说的才是根因，原样透出，不做吞噬或改写。
        bail!("ffmpeg 退出码 {status}，原始输出：\n{stderr}");
    }
    // ffmpeg 成功了但写帧失败：说明帧数对不上，属于我们这侧的错。
    write_result.map(|_| ())
}
```

`src/ffmpeg.rs` 顶部已有 `use anyhow::{bail, Context, Result};` 与 `use std::process::Command;`——补上 `Read` 与 `Stdio` 即可，不要重复 import。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test ffmpeg::` 然后 `cargo test --test render_e2e -- --ignored --nocapture`
Expected: 全部 PASS

- [ ] **Step 5: 目视验收**

端到端测试跑出的 mp4 **自己打开看**（或抽几帧）：

```bash
ffmpeg -y -v error -i /tmp/panda_e2e_*/out.mp4 -vf "select='eq(n\,0)+eq(n\,60)+eq(n\,200)+eq(n\,470)'" -vsync 0 /tmp/e2e_%02d.png
```

确认：帧 0 是 Cover（白底、居中标题）、帧 60 是 Intro 打字机、帧 200 是 Content（**背景视频可见、字幕叠在上面**）、帧 470 是 Outro。**特别确认 Content 帧的字幕没有发暗**——那是反预乘是否生效的直接证据。看完写进报告并删掉临时文件。

- [ ] **Step 6: 提交**

```bash
git add src/ffmpeg.rs tests/render_e2e.rs
git commit -m "feat(ffmpeg): 帧流管道与进程管理"
```

---

### Task 6: 素材路径、标题兜底与环境变量层

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `pub const DEFAULT_TITLE: &str = "熊猫智研社";`
  - `pub fn bg_video_path() -> String` / `pub fn bgm_path() -> String` / `pub fn title_json_path() -> String` / `pub fn video_output_path() -> String`
  - `pub fn resolve_title(cli: Option<&str>, json_text: Option<&str>) -> String`

本任务同时偿还 `docs/follow-ups.md` 「值得做」第 2 条：既有的 `spider_output_dir` / `tts_output_dir` / `tts_input_file` 三个读 `std::env` 的函数至今零测试。

- [ ] **Step 1: 写失败的测试**

```rust
// 追加到 src/config.rs 的 mod tests

#[test]
fn material_paths_have_the_documented_defaults() {
    assert_eq!(bg_video_path(), "public/video/0.mp4");
    assert_eq!(bgm_path(), "public/bgm/0.mp3");
    assert_eq!(title_json_path(), "public/video/title.json");
    assert_eq!(video_output_path(), "output/video/video.mp4");
}

#[test]
fn title_prefers_cli_over_json_over_default() {
    let json = r#"{"title": "来自 JSON 的标题"}"#;
    assert_eq!(resolve_title(Some("来自命令行"), Some(json)), "来自命令行");
    assert_eq!(resolve_title(None, Some(json)), "来自 JSON 的标题");
    assert_eq!(resolve_title(None, None), DEFAULT_TITLE);
}

#[test]
fn title_falls_through_blank_and_malformed_json() {
    // 空字符串不算「给了标题」，应继续往下兜底。
    assert_eq!(resolve_title(Some("   "), None), DEFAULT_TITLE);
    // JSON 解析失败、缺 title 字段、title 为空，都应回落默认值而不是报错。
    assert_eq!(resolve_title(None, Some("不是 JSON")), DEFAULT_TITLE);
    assert_eq!(resolve_title(None, Some(r#"{"other": 1}"#)), DEFAULT_TITLE);
    assert_eq!(resolve_title(None, Some(r#"{"title": ""}"#)), DEFAULT_TITLE);
    assert_eq!(resolve_title(None, Some(r#"{"title": "  "}"#)), DEFAULT_TITLE);
}

#[test]
fn title_from_json_is_trimmed() {
    assert_eq!(resolve_title(None, Some(r#"{"title": "  带空格  "}"#)), "带空格");
}

/// 偿还 follow-ups「值得做」第 2 条：三个读 std::env 的函数此前零测试。
/// 环境变量是进程级全局状态，这些断言必须在同一个测试里串行做，
/// 否则并行测试之间会互相干扰。
#[test]
fn env_backed_paths_read_the_environment_and_fall_back() {
    // SAFETY: 单线程内串行设置与清除，且本文件没有其它测试读这几个变量。
    unsafe {
        std::env::remove_var("SPIDER_OUTPUT_DIR");
        std::env::remove_var("TTS_OUTPUT_DIR");
        std::env::remove_var("TTS_INPUT_FILE");
    }
    assert_eq!(spider_output_dir(), "output/spider");
    assert_eq!(tts_output_dir(), "output/tts");
    assert_eq!(tts_input_file(), "output/spider/input.txt", "应基于 SPIDER_OUTPUT_DIR 推导");

    unsafe { std::env::set_var("SPIDER_OUTPUT_DIR", "/tmp/spider"); }
    assert_eq!(spider_output_dir(), "/tmp/spider");
    assert_eq!(tts_input_file(), "/tmp/spider/input.txt", "推导应跟随 SPIDER_OUTPUT_DIR");

    // 空白值等同于未设置
    unsafe { std::env::set_var("TTS_OUTPUT_DIR", "   "); }
    assert_eq!(tts_output_dir(), "output/tts", "全空白应回落默认值");

    unsafe { std::env::set_var("TTS_INPUT_FILE", "/tmp/x.txt"); }
    assert_eq!(tts_input_file(), "/tmp/x.txt", "显式设置应优先于推导");

    unsafe {
        std::env::remove_var("SPIDER_OUTPUT_DIR");
        std::env::remove_var("TTS_OUTPUT_DIR");
        std::env::remove_var("TTS_INPUT_FILE");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test config::`
Expected: FAIL，新函数不存在

- [ ] **Step 3: 实现**

```rust
/// 标题的最终兜底值（规格 §6 的三级兜底最后一级）。
pub const DEFAULT_TITLE: &str = "熊猫智研社";

/// 背景视频，默认 `public/video/0.mp4`（`--bg` 覆盖）。
pub fn bg_video_path() -> String {
    non_empty_env("BG_VIDEO").unwrap_or_else(|| "public/video/0.mp4".into())
}

/// BGM，默认 `public/bgm/0.mp3`（`--bgm` 覆盖）。
pub fn bgm_path() -> String {
    non_empty_env("BGM_FILE").unwrap_or_else(|| "public/bgm/0.mp3".into())
}

/// 标题 JSON，默认 `public/video/title.json`（`--title-json` 覆盖）。
pub fn title_json_path() -> String {
    non_empty_env("TITLE_JSON").unwrap_or_else(|| "public/video/title.json".into())
}

/// 成片输出，默认 `output/video/video.mp4`（`-o` 覆盖）。
pub fn video_output_path() -> String {
    non_empty_env("VIDEO_OUTPUT").unwrap_or_else(|| "output/video/video.mp4".into())
}

/// 标题三级兜底（规格 §6）：`--title` > `title.json` 的 `title` 字段 > 默认值。
///
/// 每一级都要求「非空白」才算数：`--title "  "` 与 `{"title": ""}` 都继续往下
/// 兜底，而不是产出一个空标题的封面。JSON 解析失败也回落而非报错——标题文件
/// 是可选素材，缺失或损坏不应让整条合成挂掉。
pub fn resolve_title(cli: Option<&str>, json_text: Option<&str>) -> String {
    if let Some(t) = cli.map(str::trim).filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    if let Some(text) = json_text {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            if let Some(t) = v.get("title").and_then(|t| t.as_str())
                .map(str::trim).filter(|s| !s.is_empty())
            {
                return t.to_string();
            }
        }
    }
    DEFAULT_TITLE.to_string()
}
```

`serde_json` 已在 `Cargo.toml` 里，不需要新增依赖。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test config::`
Expected: 全部 PASS

若 `env_backed_paths_...` 与其它测试并行时不稳定，**不要**加 `--test-threads=1`（会拖慢全套件）；改用一个进程内互斥锁把所有读写环境变量的测试串起来，并在报告里说明。

- [ ] **Step 5: 提交**

```bash
git add src/config.rs
git commit -m "feat(config): 素材路径、标题三级兜底与环境变量层测试"
```

---

### Task 7: `panda render` 子命令

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: Task 1 的 `write_embedded_audio`、Task 2 的 `FrameSource` 三个访问器、Task 4/5 的 `RenderInputs` 与 `run_render`、Task 6 的 config 函数
- Produces:
  - `Commands::Render` 变体
  - `fn read_title(cli: Option<&str>, json_path: &Path) -> String`（`main.rs` 内，可测）

- [ ] **Step 1: 写失败的测试**

`main.rs` 至今没有 `mod tests`（`docs/follow-ups.md` 记账项）。本任务建立它。

```rust
// 追加到 src/main.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_title_uses_cli_when_given() {
        let missing = std::path::Path::new("/nonexistent-title-xyz.json");
        assert_eq!(read_title(Some("命令行标题"), missing), "命令行标题");
    }

    #[test]
    fn read_title_falls_back_to_default_when_json_is_missing() {
        // 标题 JSON 是可选素材：文件不存在不应报错，应静默回落默认值。
        let missing = std::path::Path::new("/nonexistent-title-xyz.json");
        assert_eq!(read_title(None, missing), panda::config::DEFAULT_TITLE);
    }

    #[test]
    fn read_title_reads_the_json_file_when_it_exists() {
        let p = std::env::temp_dir().join(format!("panda_title_{}.json", std::process::id()));
        std::fs::write(&p, r#"{"title": "文件里的标题"}"#).unwrap();
        assert_eq!(read_title(None, &p), "文件里的标题");
        assert_eq!(read_title(Some("覆盖"), &p), "覆盖", "CLI 优先级最高");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn parse_frame_list_splits_trims_and_rejects_garbage() {
        // 偿还 follow-ups 记账项：CLI 层此前零单测。
        assert_eq!(parse_frame_list(Some("0,15,120"), 600).unwrap(), vec![0, 15, 120]);
        assert_eq!(parse_frame_list(Some(" 3 , 4 "), 600).unwrap(), vec![3, 4]);
        assert!(parse_frame_list(Some("1,x"), 600).is_err());
    }

    #[test]
    fn parse_frame_list_default_sweep_includes_the_last_frame() {
        // 末帧是 Outro 淡出终点，是目视验收最该看的一帧。
        let ids = parse_frame_list(None, 937).unwrap();
        assert_eq!(ids.first(), Some(&0));
        assert_eq!(ids.last(), Some(&936), "默认帧集必须含末帧：{ids:?}");
        let mut dedup = ids.clone();
        dedup.dedup();
        assert_eq!(dedup, ids, "不得有重复帧号");
    }
}
```

`parse_frame_list` 的当前签名是 `fn parse_frame_list(frames: Option<&str>, total_frames: u32) -> Result<Vec<u32>>`（已核实，`src/main.rs:55`）。上面的测试按此签名书写；**不要为迁就测试去改它的实现**。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --bin panda`
Expected: FAIL，`read_title` 不存在

- [ ] **Step 3: 实现子命令与辅助函数**

在 `Commands` 枚举追加：

```rust
    /// 音频 + 字幕 + 素材 → 成片 mp4
    Render {
        /// TTS 产出的 mp3
        #[arg(long)]
        audio: PathBuf,
        /// TTS 产出的 vtt
        #[arg(long)]
        vtt: PathBuf,
        /// 标题，优先级最高
        #[arg(long)]
        title: Option<String>,
        /// 标题 JSON，默认 public/video/title.json
        #[arg(long)]
        title_json: Option<PathBuf>,
        /// 背景视频，默认 public/video/0.mp4
        #[arg(long)]
        bg: Option<PathBuf>,
        /// 背景音乐，默认 public/bgm/0.mp3
        #[arg(long)]
        bgm: Option<PathBuf>,
        /// 成片输出，默认 output/video/video.mp4
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
```

辅助函数：

```rust
/// 标题三级兜底的 IO 外壳：读文件（缺失或不可读视作「没有 JSON」），
/// 纯粹的优先级逻辑交给 `config::resolve_title`。
fn read_title(cli: Option<&str>, json_path: &Path) -> String {
    let json_text = std::fs::read_to_string(json_path).ok();
    panda::config::resolve_title(cli, json_text.as_deref())
}
```

分支处理：

```rust
        Commands::Render { audio, vtt, title, title_json, bg, bgm, out } => {
            let title_json = title_json
                .unwrap_or_else(|| PathBuf::from(config::title_json_path()));
            let bg = bg.unwrap_or_else(|| PathBuf::from(config::bg_video_path()));
            let bgm = bgm.unwrap_or_else(|| PathBuf::from(config::bgm_path()));
            let out = out.unwrap_or_else(|| PathBuf::from(config::video_output_path()));

            for (label, p) in [("音频", &audio), ("字幕", &vtt), ("背景视频", &bg), ("背景音乐", &bgm)] {
                if !p.exists() {
                    anyhow::bail!("{label}文件不存在：{}", p.display());
                }
            }

            let resolved_title = read_title(title.as_deref(), &title_json);
            let vtt_text = std::fs::read_to_string(&vtt)
                .with_context(|| format!("读取字幕失败：{}", vtt.display()))?;

            // 两段内嵌音效落到临时目录，供 ffmpeg 作为输入文件读取。
            let tmp = std::env::temp_dir().join(format!("panda_render_{}", std::process::id()));
            std::fs::create_dir_all(&tmp)?;
            let (intro, typewriter) = panda::assets::write_embedded_audio(&tmp)?;

            let mut source = panda::render::frame::FrameSource::new(&vtt_text, resolved_title.clone())?;
            let total_frames = source.total_frames();
            let audio_secs = source.audio_secs();
            let content_frames = source.content_frames();

            println!(
                "标题「{resolved_title}」，音频 {audio_secs:.2}s，共 {total_frames} 帧（{:.2}s），输出 {}",
                total_frames as f64 / 30.0,
                out.display()
            );

            let result = panda::ffmpeg::run_render(&mut source, &panda::ffmpeg::RenderInputs {
                bg: &bg, tts_audio: &audio, bgm: &bgm,
                typewriter: &typewriter, intro: &intro, out: &out,
                total_frames, audio_secs, content_frames,
            });
            // 无论成败都清理临时音效，但清理失败不掩盖真正的错误。
            std::fs::remove_dir_all(&tmp).ok();
            result?;

            println!("成片已写入 {}", out.display());
        }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test` 与 `cargo clippy --all-targets`
Expected: 测试全绿、clippy 零警告

- [ ] **Step 5: 手动跑一次**

```bash
cargo run --release -- render \
  --audio ../panda-video-ts/public/audio/intro_typewriter.mp3 \
  --vtt /tmp/hand.vtt --title "手动验收标题" \
  --bg ../panda-video-ts/public/video/0.mp4 \
  --bgm ../panda-video-ts/public/bgm/0.mp3 \
  -o /tmp/manual.mp4
```

（`/tmp/hand.vtt` 自己写一份，含一条 >50 字与一条短字幕，总时长 ≥15s。）
确认命令输出了标题、帧数、时长三项信息，成片可播放。写进报告后删掉临时文件。

- [ ] **Step 6: 提交**

```bash
git add src/main.rs
git commit -m "feat(cli): 加入 panda render 子命令"
```

---

### Task 8: `panda make` 一条龙与端到端验收

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: 既有 `tts::pipeline::process_narration_file`、Task 7 的 `Commands::Render` 处理逻辑
- Produces: `Commands::Make` 变体

- [ ] **Step 1: 抽出共用的合成逻辑**

Task 7 的 `Commands::Render` 分支与 `Make` 分支要做同一件事（只是输入来源不同）。**先把它抽成一个函数再加 `Make`**，不要复制粘贴：

```rust
/// 一次成片合成。`render` 与 `make` 共用这条路径。
fn compose_video(
    audio: &Path, vtt: &Path, title: Option<&str>, title_json: &Path,
    bg: &Path, bgm: &Path, out: &Path,
) -> Result<()> {
    // Task 7 里 Commands::Render 分支的函数体原样搬进来
}
```

然后 `Commands::Render` 分支只剩参数兜底 + 调用 `compose_video`。

- [ ] **Step 2: 写失败的测试**

```rust
// 追加到 src/main.rs 的 mod tests
#[test]
fn make_defaults_the_tts_output_paths_under_the_output_dir() {
    // make 把 TTS 的产物喂给合成，两者的路径约定必须一致：
    // audio.mp3 与 audio.vtt 都在 TTS 输出目录下。
    let dir = std::path::Path::new("/tmp/some-tts-out");
    let (a, v) = tts_artifact_paths(dir);
    assert_eq!(a, std::path::Path::new("/tmp/some-tts-out/audio.mp3"));
    assert_eq!(v, std::path::Path::new("/tmp/some-tts-out/audio.vtt"));
}
```

- [ ] **Step 3: 实现**

```rust
/// TTS 流水线的两个产物在输出目录下的固定文件名。
/// `make` 靠这个约定把 TTS 的输出接到合成的输入上。
fn tts_artifact_paths(outdir: &Path) -> (PathBuf, PathBuf) {
    (outdir.join("audio.mp3"), outdir.join("audio.vtt"))
}
```

这两个文件名**已核实**与 `tts::pipeline` 实际写出的一致（`src/tts/pipeline.rs:189` 的 `output_dir.join("audio.mp3")` 与 `:204` 的 `output_dir.join("audio.vtt")`），直接用即可，不必再查。

`Commands` 枚举追加：

```rust
    /// 文稿 → TTS → 成片，一条龙
    Make {
        /// 文稿路径，默认与 panda tts 相同
        input: Option<PathBuf>,
        /// 标题，优先级最高
        #[arg(long)]
        title: Option<String>,
        /// 标题 JSON
        #[arg(long)]
        title_json: Option<PathBuf>,
        /// 背景视频
        #[arg(long)]
        bg: Option<PathBuf>,
        /// 背景音乐
        #[arg(long)]
        bgm: Option<PathBuf>,
        /// 成片输出
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
```

分支：先按 `Commands::Tts` 分支的既有写法跑一遍 TTS（复用同样的 `ProcessOptions` 构造与环境变量兜底逻辑），拿到 `outdir`，再用 `tts_artifact_paths(&outdir)` 得到 audio/vtt，最后调 `compose_video`。**TTS 部分不要重写，把 `Commands::Tts` 分支里那段也抽成函数共用。**

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test` 与 `cargo clippy --all-targets`
Expected: 测试全绿、clippy 零警告

- [ ] **Step 5: 端到端验收（本计划的核心验收）**

```bash
printf '大家好，欢迎收看本期节目。\n今天我们来聊一个有意思的话题，这段话稍微长一点，用来测试字幕换行和字号规则，也顺便验证背景音乐的淡出时机。\n希望这期内容对你有帮助，我们下期再见。\n' > /tmp/e2e.txt

cargo run --release -- make /tmp/e2e.txt \
  --title "端到端验收标题" \
  --bg ../panda-video-ts/public/video/0.mp4 \
  --bgm ../panda-video-ts/public/bgm/0.mp3 \
  -o /tmp/final.mp4
```

**用播放器完整看一遍**（不是抽帧），逐项确认并把观察详细写进报告：

| 时间点 | 该确认什么 |
|---|---|
| 0.0–0.5s | Cover：白底、logo + 小标题、主标题、居中水印 |
| 0.5s | 打字机音效**响起** |
| 0.5–4.0s | Intro：逐字打出、光标闪烁、末尾淡出 |
| 4.0s | TTS 人声**开始**，BGM **同时**进来且明显在人声之下 |
| 4.0s 起 | Content：**背景视频可见并在动**、亮度偏暗、字幕白字黑边叠在上面 |
| 长字幕处 | 字号变小、换行正常、不溢出 |
| `4+A-2` 到 `4+A` | BGM **线性淡出到无声**（这是裁定 R2 的现场验收，注意别把它听成提前 4 秒） |
| Outro 起点 | 片尾音效响起，logo 放大、标题淡入、圆环扩散 |
| 末尾 | 整体淡出，视频干净结束、无黑帧闪烁 |

**任何看起来或听起来不对的地方都要记下来，即使不确定是不是问题。** 这是本计划唯一的整体验收手段。

- [ ] **Step 6: 提交**

```bash
git add src/main.rs
git commit -m "feat(cli): 加入 panda make 一条龙命令"
```

---

## 完成标准

- `cargo test` 全绿，`cargo clippy --all-targets` **零警告**
- `cargo test --test render_e2e -- --ignored` 通过（真实素材合成出可播放 mp4）
- `panda render` 与 `panda make` 均可跑通，产出可播放的 1280×720 / 30fps / h264 + aac 成片
- 端到端目视 + 试听验收：四段视觉、四路音频的起止与相对音量均符合规格 §8.4 与 §9.3
- `docs/ffmpeg-pipeline.md` 记录了实测通过的管道形态与吞吐数据

## 已知的、本计划**不**处理的事

- `docs/follow-ups.md` 里除「CLI 层零单测」与「环境变量层零测试」外的其余欠账（`draw.rs` 拆分、`draw_centered` 全画布合成的 ~3× 优化、logo 解码两遍、常量两处真相源等）——本计划完全不碰 `src/render/draw.rs`。
- Cover 主标题超过约 2 行会压水印。本计划**不做**限长或水印下沉，只在端到端验收时用短标题避开。是否要在 `panda render` 层加警告，留给下一份计划裁定。
- 竖版 1080×1920、封面图导出、Windows 交叉编译（规格 §12 明确划在本期外）。
