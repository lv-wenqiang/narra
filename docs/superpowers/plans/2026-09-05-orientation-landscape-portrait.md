# 计划 B：横版 1920×1080 与竖版 1080×1920

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 加一个 `--orientation` 选项，产出 1920×1080（默认）与 1080×1920 两种成片。

**Architecture:** 计划 A 已经把画布尺寸变成运行期参数——`Canvas` 值类型、`Metrics` 按画布算好全部 44 个版式量、`Painter`/`FrameSource`/`RenderInputs` 一路携带画布。本计划只做三件事：给 `Canvas` 加两个具名尺寸、把 CLI 接上去、实测吞吐。**渲染层不需要任何改动**——这正是计划 A 要换来的东西。

**Tech Stack:** Rust edition 2024、tiny-skia、cosmic-text、ffmpeg。无新增依赖。

**Spec:** `docs/superpowers/specs/2026-09-04-orientation-and-resolution-design.md`（§1 目标、§5 CLI、§7 性能、§8 附带影响）

## Global Constraints

- **`Canvas::BASE`（1280×720）不是产品尺寸**，它是版式常量的调优基准与 `tests/canvas_baseline.rs` 的测试基线。**本计划不得把它加进 `--orientation` 的可选值，也不得改动它或它的快照。**
- **逐字节门禁必须始终绿**：`cargo test --test canvas_baseline`。它渲染在 `Canvas::BASE` 上，与本计划引入的两个新尺寸无关；它变红意味着改坏了共用的渲染路径。
- **`scale = canvas.w / 1280`，以宽度为基准。** LANDSCAPE 得 1.5，PORTRAIT 得 0.84375。
- 三级兜底沿用既有范式：`--flag` > 环境变量 > 默认值，每一级都要求非空白。
- 本仓库惯例：TDD（先红后绿）、每条新测试写完做变异验证、`cargo clippy --all-targets` 与 `cargo rustdoc --lib` 零告警、`cargo fmt --check` 一致。
- 提交信息用中文，说明「为什么」而不只是「做了什么」。
- **实测的数字一律写实测值**，不得把外推值写成实测。规格 §7 的「约 63fps」是按像素量线性外推的，Task 3 要用真实数字替换它。

## 计划 A 落地的接口（本计划的地基，逐字准确）

```rust
// src/render/canvas.rs
pub struct Canvas { pub w: u32, pub h: u32 }
impl Canvas {
    pub const BASE: Canvas = Canvas { w: 1280, h: 720 };
    pub fn scale(&self) -> f32;      // w / 1280.0
    pub fn w_f32(&self) -> f32;
    pub fn h_f32(&self) -> f32;
}

// src/render/metrics.rs
pub struct Metrics { /* 44 个 pub 字段 */ }
impl Metrics { pub fn for_canvas(canvas: Canvas) -> Self; }

// src/render/draw.rs
impl Painter { pub fn new(branding: &Branding, canvas: Canvas) -> anyhow::Result<Self>; }

// src/render/frame.rs
impl FrameSource {
    pub fn new(vtt_text: &str, title: String, branding: &Branding, canvas: Canvas) -> Result<Self>;
    pub fn canvas(&self) -> Canvas;
}

// src/ffmpeg.rs
pub struct RenderInputs<'a> { /* ... */ pub canvas: Canvas }
```

**画布只在两处进入生产路径**，两处现在都写死 `Canvas::BASE`，本计划把它们换掉：
- `src/main.rs:149` — `run_debug_frames` 里的 `FrameSource::new(..., Canvas::BASE)`
- `src/main.rs:466` — `compose_video_with_runner` 里的 `FrameSource::new(..., Canvas::BASE)`

## 非目标（明确不做，避免执行者犹豫）

- **不做 `--size WxH`**：任意宽高比会让版式失去可调优的基准，测试也从「两组确定基准」退化成「任意尺寸下的不变量」。
- **不做 `--orientation both`**：跑两次 `panda render` 复用同一份 TTS 产物即可；Task 2 会在 `justfile` 加一条配方。
- **不做竖版的背景模糊填充**：横版素材进竖版画布按 cover 语义中心裁切（只保留中间约 31% 宽）是规格 §8 明确保留的行为。要好的竖版取景就给竖版素材，`--bg` 本来就支持。
- **不做以下两条计划 A 留下的清理**（记在此处，免得执行者以为是遗漏）：`tests/canvas_baseline.rs` 的失败信息只给字节差异总数不指示画面位置；`layout_and_draw_watermark` 可通过把锚点算进 `WatermarkAnchor` 甩掉 `canvas` 参数。两者都不影响本计划。

---

## File Structure

| 文件 | 职责 | 本计划的改动 |
|---|---|---|
| `src/render/canvas.rs` | 画布值类型 | Modify：加 `LANDSCAPE`/`PORTRAIT` 两个常量与比例不变量测试 |
| `src/config.rs` | 环境变量层与三级兜底 | Modify：加 `Orientation` 枚举与 `orientation()` |
| `src/main.rs` | CLI 装配 | Modify：两个子命令加 `--orientation`，两处 `Canvas::BASE` 换成解析结果 |
| `src/ffmpeg.rs` | 滤镜与命令行 | 不改（早已跟着 `i.canvas` 走） |
| `src/render/**` | 渲染层 | **不改** |
| `tests/config_env.rs` | 环境变量层测试 | Modify：加 `ORIENTATION` 的三级链测试 |
| `justfile` | 任务编排 | Modify：`make` 配方透传 `--orientation`；加一条两档都出的配方 |
| `docs/ffmpeg-pipeline.md` | 实测记录 | Modify：新增两档吞吐与产物规格的实测小节 |
| `docs/superpowers/specs/2026-09-01-panda-video-rs-design.md` | 设计规格 | Modify：§6 CLI 契约补 `--orientation` |
| `README.md` | 用户文档 | Modify：功能与配置表补 `--orientation` |
| `docs/follow-ups.md` | 欠账本 | Modify：销掉 F-4 那条 park |

---

## Task 1：`Canvas` 加两个具名尺寸 + 比例不变量测试

**Files:**
- Modify: `src/render/canvas.rs`

**Interfaces:**
- Consumes: 现有 `Canvas::{BASE, scale, w_f32, h_f32}`
- Produces: `Canvas::LANDSCAPE`（1920×1080）、`Canvas::PORTRAIT`（1080×1920）。Task 2 消费它们。

**为什么先做**：它是纯值 + 纯测试，不碰任何调用点，能独立验证；Task 2 建在它上面。

- [ ] **Step 1: 写失败测试**

在 `src/render/canvas.rs` 的 `mod tests` 里追加：

```rust
    /// 两个产品尺寸的字面值，以及它们与 BASE 的关系。
    ///
    /// **BASE 不在其中**：它是版式常量的调优基准与逐字节测试基线，不是可
    /// 选的输出尺寸（规格 §1）。这条测试同时把「两档都是 BASE 的 2.25 倍
    /// 像素量」这个性能相关的事实钉住——它是 `docs/ffmpeg-pipeline.md` 里
    /// 吞吐结论的前提。
    #[test]
    fn landscape_and_portrait_have_the_documented_sizes() {
        assert_eq!((Canvas::LANDSCAPE.w, Canvas::LANDSCAPE.h), (1920, 1080));
        assert_eq!((Canvas::PORTRAIT.w, Canvas::PORTRAIT.h), (1080, 1920));

        let base_px = Canvas::BASE.w as u64 * Canvas::BASE.h as u64;
        for c in [Canvas::LANDSCAPE, Canvas::PORTRAIT] {
            let px = c.w as u64 * c.h as u64;
            assert_eq!(
                px * 100 / base_px,
                225,
                "{}x{} 的像素量应为 BASE 的 2.25 倍",
                c.w,
                c.h
            );
        }
    }

    /// `scale` 是按**宽度**推导的，所以两档的缩放因子由各自的宽度决定，
    /// 与高度无关——竖版比横版**小**，尽管它高得多。
    ///
    /// 这条防的是一类很自然的误解：「竖版是 1920 高，所以它该被放大」。
    /// 按高缩放会让竖版的字号涨到画布宽的 11%（规格 §2 算过），完全不可读。
    #[test]
    fn scale_follows_width_so_portrait_is_smaller_than_landscape() {
        assert_eq!(Canvas::LANDSCAPE.scale(), 1.5);
        assert_eq!(Canvas::PORTRAIT.scale(), 0.84375);
        assert!(
            Canvas::PORTRAIT.scale() < Canvas::LANDSCAPE.scale(),
            "竖版更窄，缩放因子必须更小——即使它更高"
        );
    }

    /// **占宽比在三档之间恒定**：这是「同一套版式按宽度重新比例」这条策略
    /// 的可检验形式（规格 §2）。取两个有代表性的量纲——字幕字号（长度类，
    /// `× scale`）与 Outro logo（`px()` 取整后的位图尺寸）——断言它们占画布
    /// 宽度的比例在 BASE / LANDSCAPE / PORTRAIT 下相同。
    ///
    /// 容差 1e-3：logo 走 `px()` 的 `round()`，整数舍入除以浮点宽度后有
    /// 真实误差（计划 A 实测最大 2.3e-4）；而按高推导的错误会造成约 0.36
    /// 的偏差，量级差三个数量级，既不漏也不 flaky。
    #[test]
    fn element_to_width_ratios_are_constant_across_all_three_canvases() {
        use crate::render::metrics::Metrics;

        let base = Metrics::for_canvas(Canvas::BASE);
        let want_caption = base.caption_font_size_short / Canvas::BASE.w_f32();
        let want_logo = base.outro_logo_size as f32 / Canvas::BASE.w_f32();

        let mut failures: Vec<String> = Vec::new();
        for c in [Canvas::LANDSCAPE, Canvas::PORTRAIT] {
            let m = Metrics::for_canvas(c);
            let caption = m.caption_font_size_short / c.w_f32();
            let logo = m.outro_logo_size as f32 / c.w_f32();
            if (caption - want_caption).abs() > 1e-3 {
                failures.push(format!(
                    "{}x{}: 字幕字号占宽比 {caption} 应等于 BASE 的 {want_caption}",
                    c.w, c.h
                ));
            }
            if (logo - want_logo).abs() > 1e-3 {
                failures.push(format!(
                    "{}x{}: Outro logo 占宽比 {logo} 应等于 BASE 的 {want_logo}",
                    c.w, c.h
                ));
            }
        }
        assert!(failures.is_empty(), "占宽比在各档之间不一致：{failures:?}");
    }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test --lib render::canvas::`
Expected: FAIL，`no associated item named LANDSCAPE found for struct Canvas`。

- [ ] **Step 3: 写实现**

在 `src/render/canvas.rs` 的 `impl Canvas` 里，紧接 `BASE` 之后加：

```rust
    /// 横版产品尺寸（默认）。
    ///
    /// 选 1920×1080 而非沿用 BASE 的 1280×720，顺带白捡一次质量提升：
    /// `public/video/0.mp4` 本身就是 1920×1080，此前被降采样到 1280×720
    /// 白白丢掉细节，现在是 1:1 映射。
    pub const LANDSCAPE: Canvas = Canvas { w: 1920, h: 1080 };

    /// 竖版产品尺寸：抖音 / 视频号 / Reels 的原生尺寸。
    ///
    /// **注意它比 BASE 还窄**（1080 < 1280），所以 `scale()` 小于 1——
    /// 版式按宽度重新比例，元素占画布宽的比例与横版一致，代价是上下留白
    /// 较多（规格 §2 的既定取舍）。
    pub const PORTRAIT: Canvas = Canvas { w: 1080, h: 1920 };
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test --lib render::canvas::`
Expected: PASS，5 条（原有 3 条 + 新增 2 条…实际按上面写了 3 条新测试，共 6 条）。

- [ ] **Step 5: 变异验证**

把 `PORTRAIT` 改成 `{ w: 1920, h: 1080 }`（与 LANDSCAPE 相同）。
Expected: `landscape_and_portrait_have_the_documented_sizes` FAIL。改回后 PASS。

再把 `scale()` 改成按高度（`self.h as f32 / Self::BASE.h as f32`）。
Expected: `scale_follows_width_so_portrait_is_smaller_than_landscape` 与
`element_to_width_ratios_are_constant_across_all_three_canvases` **两条都** FAIL。改回后 PASS。报告实际变红的测试名。

- [ ] **Step 6: 门禁与全量**

Run: `cargo test && cargo test --test canvas_baseline`
Expected: 全绿；门禁 PASS（本任务只加常量与测试，不碰渲染路径）。

- [ ] **Step 7: 提交**

```bash
git add src/render/canvas.rs
git commit -m "feat(canvas): 加入 LANDSCAPE 1920x1080 与 PORTRAIT 1080x1920 两个产品尺寸"
```

---

## Task 2：`--orientation` 接进 CLI，默认横版

**Files:**
- Modify: `src/config.rs`
- Modify: `src/main.rs`
- Modify: `tests/config_env.rs`
- Modify: `justfile`

**Interfaces:**
- Consumes: `Canvas::{LANDSCAPE, PORTRAIT}`（Task 1）
- Produces: `config::Orientation`（`Landscape` / `Portrait`），`Orientation::resolve(cli: Option<String>) -> Result<Orientation>`，`Orientation::canvas(&self) -> Canvas`

**这是本计划唯一改变用户可见行为的任务**：默认输出从 1280×720 变成 1920×1080。

- [ ] **Step 1: 写失败测试（config 层）**

在 `tests/config_env.rs` 末尾追加：

```rust
/// `--orientation` 的三级兜底：命令行 > `$ORIENTATION` > 横版。
///
/// 与 `Branding::resolve` 同一套写法与同一个理由——装配逻辑跟着它组合的
/// 那些函数走，才测得到；放在 `main.rs`（bin crate）里，本文件的环境变量
/// 夹具够不着它。
#[test]
fn orientation_resolve_prefers_cli_over_env_over_landscape() {
    let _guard = ENV_LOCK.lock().unwrap();
    use panda::config::Orientation;
    use panda::render::canvas::Canvas;
    // SAFETY: 持有 ENV_LOCK，本文件内串行。
    let clear = || unsafe { std::env::remove_var("ORIENTATION") };

    clear();
    assert_eq!(
        Orientation::resolve(None).unwrap(),
        Orientation::Landscape,
        "都没给时应是横版"
    );
    assert_eq!(
        Orientation::resolve(None).unwrap().canvas(),
        Canvas::LANDSCAPE
    );

    unsafe { std::env::set_var("ORIENTATION", "portrait") };
    assert_eq!(Orientation::resolve(None).unwrap(), Orientation::Portrait);
    assert_eq!(
        Orientation::resolve(None).unwrap().canvas(),
        Canvas::PORTRAIT
    );

    // 命令行优先于环境变量。
    assert_eq!(
        Orientation::resolve(Some("landscape".into())).unwrap(),
        Orientation::Landscape,
        "命令行应压过环境变量"
    );

    // 大小写与首尾空白都容忍——这几个值是人手打的。
    for raw in ["PORTRAIT", " portrait ", "Portrait"] {
        assert_eq!(
            Orientation::resolve(Some(raw.into())).unwrap(),
            Orientation::Portrait,
            "「{raw}」应被接受"
        );
    }

    // 全空白视同没给，继续往下兜底到环境变量（此刻是 portrait）。
    assert_eq!(
        Orientation::resolve(Some("   ".into())).unwrap(),
        Orientation::Portrait,
        "全空白的命令行参数应视同没给"
    );

    // 认不出来的值必须**报错**，不能默默回落——默默回落会让打错一个字母
    // 的用户拿到一支横版成片却以为是竖版，而中间没有任何提示。
    let err = Orientation::resolve(Some("vertical".into()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("vertical"), "报错应点名那个值：{err}");
    assert!(
        err.contains("landscape") && err.contains("portrait"),
        "报错应列出可选值：{err}"
    );

    clear();
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test --test config_env orientation`
Expected: FAIL，`cannot find type Orientation in module panda::config`。

- [ ] **Step 3: 实现 config 层**

在 `src/config.rs` 里，紧跟 `Branding` 的 `impl` 之后加：

```rust
/// 成片的画幅方向。
///
/// **是枚举而不是一对宽高数字**：两档各自的版式常量都是在确定的宽度上调优
/// 过的，接受任意尺寸等于放弃这个基准（规格 §5）。枚举也让「认不出来的值」
/// 成为一个可以报错的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Landscape,
    Portrait,
}

impl Orientation {
    /// 三级兜底：`--orientation` > `$ORIENTATION` > 横版。
    ///
    /// 每一级都要求非空白（沿用本模块 [`non_blank`] / [`non_empty_env`] 的
    /// 规矩）。**认不出来的值报错而非回落**：默默回落会让打错一个字母的人
    /// 拿到一支横版成片却以为是竖版，整条链上没有任何提示。
    pub fn resolve(cli: Option<String>) -> anyhow::Result<Self> {
        let raw = non_blank(cli).or_else(|| non_empty_env("ORIENTATION"));
        match raw {
            None => Ok(Self::Landscape),
            Some(s) => match s.trim().to_ascii_lowercase().as_str() {
                "landscape" => Ok(Self::Landscape),
                "portrait" => Ok(Self::Portrait),
                other => anyhow::bail!(
                    "认不出的画幅方向「{other}」，可选值：landscape（横版 1920x1080）、portrait（竖版 1080x1920）"
                ),
            },
        }
    }

    /// 本方向对应的画布尺寸。
    pub fn canvas(&self) -> crate::render::canvas::Canvas {
        match self {
            Self::Landscape => crate::render::canvas::Canvas::LANDSCAPE,
            Self::Portrait => crate::render::canvas::Canvas::PORTRAIT,
        }
    }
}
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test --test config_env orientation`
Expected: PASS。

- [ ] **Step 5: 接进两个子命令**

`src/main.rs`：

1. `Commands::DebugFrames` 与 `Commands::Render` 各加一个参数（放在 `--logo` 之后）：

```rust
        /// 画幅方向：landscape（1920x1080，默认）或 portrait（1080x1920）；不给则取 $ORIENTATION
        #[arg(long)]
        orientation: Option<String>,
```

2. 两个分支的解构各加 `orientation,`。

3. `DebugFrames` 分支体改为把解析结果传下去：

```rust
        } => run_debug_frames(
            vtt,
            title,
            Branding::resolve(brand, watermark, watermark_cover, watermark_icon, logo),
            Orientation::resolve(orientation)?.canvas(),
            out,
            frames,
        ),
```

4. `run_debug_frames` 签名加一个参数（放在 `branding` 之后）：

```rust
fn run_debug_frames(
    vtt: PathBuf,
    title: Option<String>,
    branding: Branding,
    canvas: Canvas,
    out: PathBuf,
    frames: Option<String>,
) -> Result<()> {
```

其内部的 `FrameSource::new(&vtt_text, title, &branding, Canvas::BASE)?` 改为 `..., canvas)?`。

5. `ComposeVideoInputs` 加一个字段 `canvas: Canvas`，`compose_inputs` 多收一个 `canvas: Canvas` 参数并填入；`compose_video_with_runner` 的解构加上它，把
`FrameSource::new(&vtt_text, resolved_title.clone(), branding, Canvas::BASE)?`
改为 `..., canvas)?`。

6. `Commands::Render` 与 `Commands::Make`（若仍存在）的分支体在调用 `compose_inputs` 时传入 `Orientation::resolve(orientation)?.canvas()`。

7. 在 `main.rs` 顶部的 `use` 区加 `use panda::config::Orientation;`。

- [ ] **Step 6: 打印里带上尺寸**

`compose_video_with_runner` 的那行 `println!` 改为同时报出画幅，让人一眼能确认跑的是哪一档：

```rust
    println!(
        "标题「{resolved_title}」，画幅 {}x{}，音频 {:.2}s，共 {} 帧（{:.2}s），输出 {}",
        source.canvas().w,
        source.canvas().h,
        source.audio_secs(),
        source.total_frames(),
        source.total_frames() as f64 / FPS as f64,
        out.display()
    );
```

- [ ] **Step 7: 写「默认已变成横版」的测试**

在 `src/main.rs` 的 `mod tests` 里追加：

```rust
    /// **默认画幅是 1920×1080，不再是 1280×720。**
    ///
    /// 这是本计划唯一改变用户可见行为的地方，值得单独钉住：`Canvas::BASE`
    /// 退化成了调优基准与测试基线，不再是任何一条生产路径的输出尺寸。
    #[test]
    fn default_orientation_renders_at_landscape_not_base() {
        let canvas = Orientation::resolve(None).unwrap().canvas();
        assert_eq!(canvas, Canvas::LANDSCAPE);
        assert_ne!(
            canvas,
            Canvas::BASE,
            "BASE 是调优基准与测试基线，不该再是任何生产路径的输出尺寸"
        );
    }
```

- [ ] **Step 8: 运行全量与门禁**

Run: `cargo test && cargo test --test canvas_baseline`
Expected: 全绿；**门禁仍 PASS**（它显式渲染在 `Canvas::BASE` 上，与默认值改动无关）。若门禁红了说明有人把门禁也改成了跟默认走——停下报告。

- [ ] **Step 9: 变异验证**

把 `Orientation::resolve` 的 `None => Ok(Self::Landscape)` 改成 `None => Ok(Self::Portrait)`。
Expected: `orientation_resolve_prefers_cli_over_env_over_landscape` 与 `default_orientation_renders_at_landscape_not_base` 两条 FAIL。改回后 PASS。

把 `canvas()` 的两个分支对调。
Expected: `orientation_resolve_prefers_cli_over_env_over_landscape` FAIL。改回后 PASS。

- [ ] **Step 10: justfile 透传与双档配方**

`justfile` 的 `make` 配方本就把额外参数透传给 `panda render`，所以 `just make 文稿.txt --orientation portrait` 已经能用——**确认一次**，并在配方注释的示例里加一行。

再加一条同时出两档的配方：

```just
# 一份文稿出两档成片（横版 + 竖版），复用同一次 TTS
make-both input="" *render_args="":
    #!/usr/bin/env bash
    set -euo pipefail
    just make "{{ input }}" --orientation landscape -o output/video/landscape.mp4 {{ render_args }}
    # 第二次跳过 TTS：产物已在 {{ tts_outdir }}，直接渲染
    cargo run --release --quiet -- render \
        --audio "{{ tts_outdir }}/audio.mp3" \
        --vtt "{{ tts_outdir }}/audio.vtt" \
        --bg "{{ bg }}" --bgm "{{ bgm }}" \
        --orientation portrait -o output/video/portrait.mp4 {{ render_args }}
```

- [ ] **Step 11: 实拍验收（两档各出一支）**

```bash
cargo run --release -- render --audio output/tts/audio.mp3 --vtt output/tts/audio.vtt \
    --title "计划 B 横版验收" -o /tmp/planB-landscape.mp4
cargo run --release -- render --audio output/tts/audio.mp3 --vtt output/tts/audio.vtt \
    --title "计划 B 竖版验收" --orientation portrait -o /tmp/planB-portrait.mp4
ffprobe -v error -show_entries stream=width,height -of csv=p=0 /tmp/planB-landscape.mp4
ffprobe -v error -show_entries stream=width,height -of csv=p=0 /tmp/planB-portrait.mp4
```
Expected: 分别是 `1920,1080` 与 `1080,1920`，两支都能播。把这两个路径写进报告——它们是人工验收的对象。

- [ ] **Step 12: 提交**

```bash
git add -A
git commit -m "feat(cli): 加入 --orientation，默认输出改为横版 1920x1080"
```

---

## Task 3：实测两档吞吐与产物规格，写回文档

**Files:**
- Modify: `docs/ffmpeg-pipeline.md`

**Interfaces:** 无新接口。

**为什么必须实测**：规格 §7 的「约 63fps」是按像素量线性外推的。编码耗时未必随像素线性（x264 的运动估计、去块滤波都有非线性成分），而余量从 4.7× 掉到 2.1× 之后，判断「是否仍在实时线以上」不能靠推算。

- [ ] **Step 1: 测三档的端到端吞吐**

对 BASE / LANDSCAPE / PORTRAIT 各跑一次完整合成，用同一份 TTS 产物、同一份素材，记录 wall-clock：

```bash
for o in landscape portrait; do
  /usr/bin/time -f "$o: %e s" cargo run --release --quiet -- render \
      --audio output/tts/audio.mp3 --vtt output/tts/audio.vtt \
      --orientation $o --title "吞吐实测" -o /tmp/bench-$o.mp4
done
```

BASE 无法经 CLI 产出（它不是可选值），所以取 `docs/ffmpeg-pipeline.md` 已有的记录作对照即可，不必另测。

对每档记录：总耗时、总帧数、`帧数 / 耗时` 得到的 fps、以及 `fps / 30` 得到的实时倍率。

- [ ] **Step 2: 记录两档的产物规格**

```bash
for o in landscape portrait; do
  echo "=== $o ==="
  ffprobe -v error -show_entries stream=codec_name,width,height,pix_fmt,r_frame_rate,nb_frames \
      -show_entries stream=codec_name,sample_rate,channels,channel_layout \
      -show_entries format=duration,size -of default=nw=1 /tmp/bench-$o.mp4
done
```

**音频各项必须与横版完全一致**（采样率 48000、立体声、时长）——音频链路与画幅无关，若两档的音频读数不同，说明有东西串了，停下报告。

- [ ] **Step 3: 记录竖版的背景裁切实况**

竖版把 1920×1080 的横版素材按 cover 语义塞进 1080×1920，会裁掉左右大部分。算出并记录实际保留的宽度比例：

```
缩放后宽度 = 1920 * (1920 / 1080) = 3413px（按高度撑满）
保留比例 = 1080 / 3413 ≈ 31.6%
```

用一帧实际画面佐证：从竖版成片抽一帧、从横版成片抽同一时刻一帧，肉眼可辨竖版只保留了中间一条。记录抽帧命令与结论。

- [ ] **Step 4: 写回 `docs/ffmpeg-pipeline.md`**

新增一节（编号接在现有最后一节之后），包含：

- 三档的像素量、帧数、实测耗时、实测 fps、实时倍率对照表
- **与规格 §7 外推值的对照**：外推 63fps，实测多少，差多少，若差得多则分析原因（编码非线性？IO？）
- 两档的产物规格 `ffprobe` 读数
- 竖版背景裁切的比例与佐证
- 一句结论：两档是否仍在 30fps 实时线以上；若不在，给出建议（降 CRF / 调 preset / 降默认档）

**数字一律写实测值。** 若某项没测到，写「未测」而不是填一个推算值。

- [ ] **Step 5: 提交**

```bash
git add docs/ffmpeg-pipeline.md
git commit -m "docs(ffmpeg): 补两档画幅的吞吐与产物规格实测，替换规格里的外推值"
```

---

## Task 4：销掉 F-4——用源码文本钉住 FPS 的推导关系

**Files:**
- Create: `tests/fps_single_source.rs`
- Modify: `docs/follow-ups.md`

**Interfaces:** 无新接口。

**背景**：计划 A 把 `draw::FPS` 改成了 `crate::render::timeline::FPS as f64`，并加了一条断言两者相等的测试。但那条测试**按构造是同义反复**——把 `draw::FPS` 改回字面量 `30.0` 零条测试变红（因为 `30.0 == 30 as f64`）。它瞄准的靶子（两者取值分歧）是对的，但守不住「有人把推导改回硬编码」这一步。当时按流程 park 了，并记下更强的写法。

**修法**：照 `tests/justfile_defaults.rs` 的既有手法——`include_str!` 源码文本、断言推导表达式在场。Rust 无法在类型层表达「这个常量必须派生自那个常量」，源码文本比对是唯一诚实的办法。

- [ ] **Step 1: 写失败测试**

`tests/fps_single_source.rs`：

```rust
//! `draw::FPS` 必须**派生自** `timeline::FPS`，而不只是碰巧相等。
//!
//! **为什么要比对源码文本**：`src/render/draw.rs` 里 `FPS` 现在写作
//! `crate::render::timeline::FPS as f64`，而既有的那条运行期断言
//! （`ffmpeg::tests::draw_fps_derives_from_the_timeline_single_source_of_truth`）
//! 只能检查两者**取值**相等——把它改回字面量 `30.0` 之后那条断言照样通过，
//! 因为 `30.0 == 30 as f64`。它守住了「取值分歧」这个有害状态，却守不住
//! 「推导被改回硬编码」这一步，而后者正是双真相源重新长出来的方式。
//!
//! Rust 没有办法在类型层表达「这个常量必须派生自那个常量」，所以只能比对
//! 源码文本。同一手法在 `tests/justfile_defaults.rs` 用过（justfile 是
//! shell，读不到 `config::*`，只能把默认值抄一份、再用测试把两边钉住）。
//!
//! **只做子串匹配，不解析 Rust**：这里要防的是「有人把推导换成字面量」，
//! 不是「语法正确」——后者由编译器负责。

const DRAW_RS: &str = include_str!("../src/render/draw.rs");

/// `draw.rs` 的 `FPS` 必须以 `timeline::FPS` 的形式写出来。
#[test]
fn draw_fps_is_written_as_a_derivation_not_a_literal() {
    let want = "const FPS: f64 = crate::render::timeline::FPS as f64;";
    assert!(
        DRAW_RS.contains(want),
        "src/render/draw.rs 里的 FPS 必须写成 `{want}`——\n\
         写成字面量会让它与 timeline::FPS 重新变成两个真相源：\n\
         改 timeline::FPS 之后 layout()/ffmpeg -r/段落起点都会跟着走，\n\
         而 draw.rs 里每个动画窗口仍按旧帧率算，动画变速且测试全绿。"
    );
}

/// `draw.rs` 里不得再出现把 30 写死成帧率的形态。
///
/// 与上一条互补：上一条保证「正确的写法在场」，这一条保证「错误的写法不在场」
/// ——两者都需要，因为有人可能在保留正确定义的同时，另加一个写死的副本。
#[test]
fn draw_rs_has_no_hardcoded_frame_rate() {
    for bad in ["const FPS: f64 = 30.0;", "const FPS: f64 = 30f64;"] {
        assert!(
            !DRAW_RS.contains(bad),
            "src/render/draw.rs 里不该出现写死帧率的 `{bad}`"
        );
    }
}
```

- [ ] **Step 2: 运行，确认通过（这是表征测试，写完即绿）**

Run: `cargo test --test fps_single_source`
Expected: PASS，2 条。

> 这两条是**表征测试**——它们描述当前已经正确的状态，所以写完就是绿的。
> 它们的价值全在变异验证那一步：必须证明「把推导改回字面量」会让它们变红。

- [ ] **Step 3: 变异验证（本任务的关键一步）**

把 `src/render/draw.rs` 的
`const FPS: f64 = crate::render::timeline::FPS as f64;`
改成
`const FPS: f64 = 30.0;`

Run: `cargo test --test fps_single_source`
Expected: **两条都 FAIL**。

对照：同一变异下跑 `cargo test --lib ffmpeg::tests::draw_fps_derives_from_the_timeline_single_source_of_truth`
Expected: **PASS**——这正是 F-4 记录的那个盲区，也是本任务存在的理由。把这个对照结果写进报告。

改回后两条 PASS。

- [ ] **Step 4: 销账**

`docs/follow-ups.md` 里 F-4 那条 park（关于 FPS 一致性测试按构造是同义反复），移进「已销账」，写明：新增 `tests/fps_single_source.rs` 以源码文本钉住推导关系；并记下实测的对照——同一变异下源码文本测试变红、而原先的运行期断言仍绿。

- [ ] **Step 5: 全量与门禁**

Run: `cargo test && cargo test --test canvas_baseline && cargo clippy --all-targets && cargo fmt --check`
Expected: 全绿、零告警。

- [ ] **Step 6: 提交**

```bash
git add tests/fps_single_source.rs docs/follow-ups.md
git commit -m "test(fps): 以源码文本钉住 draw::FPS 的推导关系，销掉 F-4 的 park"
```

---

## Task 5：文档收尾

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/specs/2026-09-01-panda-video-rs-design.md`

**Interfaces:** 无。

- [ ] **Step 1: README 的功能一节**

在「成片合成（`panda render`）」小节里补一段说明两档画幅：默认 1920×1080，`--orientation portrait` 出 1080×1920；两档共用一套版式，尺寸类的量按画布宽度重新比例，所以元素占画布宽的比例在两档之间一致；竖版上下留白较多是这一策略的既定结果。

并在配置表里加一行：

| `--orientation` | `ORIENTATION` | `landscape`（1920×1080） |

- [ ] **Step 2: README 的状态一节**

把测试计数更新为实际值（跑 `cargo test` 数出来，不要沿用旧数字）。若 Task 3 实测的吞吐值得一提（例如竖版余量偏紧），在状态一节加一句。

- [ ] **Step 3: 规格 §6 的 CLI 契约**

`docs/superpowers/specs/2026-09-01-panda-video-rs-design.md` 的 §6：在 `panda render` 与 `panda debug-frames` 的用法行里加上 `[--orientation landscape|portrait]`，并在其下的三级兜底说明里补一句：认不出的值报错而非回落，理由是打错一个字母的人会拿到一支错画幅的成片而全程无提示。

- [ ] **Step 4: 最终验证**

```bash
cargo test
cargo test --test canvas_baseline
cargo test --test render_e2e -- --ignored
cargo clippy --all-targets
cargo rustdoc --lib
cargo fmt --check
just --list
```
Expected: 全绿、零告警、`just --list` 能列出 `make` 与 `make-both`。

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "docs: README 与规格补上 --orientation 两档画幅"
```

---

## 完成标准

- [ ] `panda render` 默认产出 1920×1080；`--orientation portrait` 产出 1080×1920；两支都能播放
- [ ] `cargo test` 全绿，`cargo test --test render_e2e -- --ignored` 通过
- [ ] `cargo test --test canvas_baseline` 通过——**BASE 上的渲染仍逐字节不变**
- [ ] `cargo clippy --all-targets` 与 `cargo rustdoc --lib` 零告警，`cargo fmt --check` 一致
- [ ] 认不出的 `--orientation` 值报错并列出可选值
- [ ] `docs/ffmpeg-pipeline.md` 有两档的**实测**吞吐与产物规格，规格 §7 的外推值已被替换
- [ ] `docs/follow-ups.md` 的 F-4 已销账，`tests/fps_single_source.rs` 的变异验证记录在案
- [ ] README 与规格 §6 都写了 `--orientation`

## 人工验收（不由子代理代做）

两支成片要由人从头看一遍：

- `/tmp/planB-landscape.mp4` —— 1920×1080，确认画面比 720p 更清晰（背景素材现在是 1:1 映射，不再降采样）
- `/tmp/planB-portrait.mp4` —— 1080×1920，重点看两件事：**上下留白是否可接受**（这是「同一套版式」的既定代价，规格 §2 已写明），以及**背景只保留中间约 31% 宽之后画面是否还成立**（若不成立，结论是「竖版要给竖版素材」，而不是改代码）

这两条只有人能判断，子代理没有播放器也没有眼睛。
