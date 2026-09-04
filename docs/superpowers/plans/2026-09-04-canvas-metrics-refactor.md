# 计划 A：画布参数化重构（Canvas / Metrics）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把渲染层从「写死 1280×720」重构成「由一个 `Canvas` 值驱动」，输出在 `Canvas::BASE` 上逐字节不变，为计划 B 引入 1920×1080 / 1080×1920 打好地基。

**Architecture:** 收敛 `timeline` 与 `draw` 两份画布常量为一份 `Canvas`；把 `draw.rs` 的 78 个常量按量纲逐条分类，长度量纲的乘 `scale`，全部收进 `Painter::new()` 里一次算好的 `Metrics` 表；顺带修掉三个「换尺寸才炸」的陷阱。**本计划不引入任何新尺寸**——所有产出仍是 1280×720，且必须逐字节等于重构前。

**Tech Stack:** Rust edition 2024、tiny-skia、cosmic-text。无新增依赖。

**Spec:** `docs/superpowers/specs/2026-09-04-orientation-and-resolution-design.md`

## Global Constraints

- **逐字节门禁**：重构全程，`Canvas::BASE`（1280×720）上渲染任意帧必须与重构前**逐字节相同**。Task 1 建立基线快照，此后每个任务都要跑它。
- **常量总数校验和 78**：`src/render/draw.rs` 现有 78 个常量。分类必须穷尽且无重叠，八类之和等于 78。
- **`scale = canvas.w as f32 / 1280.0`**，以**宽度**为基准。BASE 下 `scale == 1.0`，因此所有长度量纲常量在 BASE 上的值与现在完全一致。
- **不改任何现有测试的断言数值**。测试只改「在哪个画布上渲染」，不改「断言什么数」。
- 本仓库惯例：TDD（先红后绿）、每条新测试写完做变异验证、`cargo clippy --all-targets` 与 `cargo rustdoc --lib` 零告警、`cargo fmt` 一致。
- 提交信息用中文，说明「为什么」而不只是「做了什么」。

## 常量分类表（Task 3–6 的依据）

八类，合计 78，已用脚本核对穷尽且无重叠。

| 类 | 数量 | 处理 |
|---|---|---|
| **C** 画布自身 | 3 | 由 `Canvas` 取代 |
| **L** 长度量纲 | 25 | `× scale` 进 `Metrics` |
| **W** 宽度派生 | 11 | 已正确，随 `canvas.w` 走 |
| **H** 高度派生（已正确） | 2 | 已正确，随 `canvas.h` 走 |
| **T** 陷阱 | 6 | 见规格 §3，逐条修 |
| **V** 垂直锚点 | 1 | 改成 `0.8 × h` |
| **F** 帧数派生 | 4 | 随 `FPS`，与画布无关 |
| **N** 无量纲 | 26 | 不动 |

**C（3）**：`CANVAS_W` `CANVAS_H` `FPS`

**L（25）**：`CAPTION_PADDING_X_PX` `CAPTION_FONT_SIZE_LONG` `CAPTION_FONT_SIZE_SHORT` `CAPTION_STROKE_WIDTH_PX` `ENTRANCE_TRANSLATE_X_RANGE` `ENTRANCE_LETTER_SPACING_RANGE` `WATERMARK_MARGIN_LEFT_PX` `WATERMARK_MARGIN_BOTTOM_PX` `WATERMARK_FONT_SIZE_PX` `WATERMARK_ICON_SIZE_PX` `WATERMARK_ICON_GAP_PX` `COVER_WATERMARK_FONT_SIZE_PX` `COVER_WATERMARK_ICON_SIZE_PX` `COVER_WATERMARK_ICON_GAP_PX` `COVER_ROW_MARGIN_LEFT_PX` `COVER_LOGO_SIZE_PX` `COVER_LOGO_MARGIN_PX` `COVER_ROW_TEXT_FONT_SIZE_PX` `COVER_TITLE_FONT_SIZE_PX` `COVER_TITLE_PADDING_PX` `INTRO_TITLE_FONT_SIZE_PX` `INTRO_CURSOR_GAP_PX` `OUTRO_TITLE_FONT_SIZE_PX` `OUTRO_TITLE_GAP_PX` `OUTRO_TITLE_TRANSLATE_Y_RANGE`

> **`ENTRANCE_TRANSLATE_X_RANGE` / `ENTRANCE_LETTER_SPACING_RANGE` / `OUTRO_TITLE_TRANSLATE_Y_RANGE` 写在「动画」小节里，量纲却是像素**，最容易漏。

**W（11）**：`CAPTION_CENTER_X` `CAPTION_MAX_WIDTH_PX` `COVER_CONTAINER_WIDTH_PX` `COVER_CONTAINER_LEFT_PX` `COVER_ROW_LEFT_PX` `COVER_ROW_HEIGHT_PX` `COVER_ROW_TEXT_LEFT_PX` `COVER_TITLE_MAX_WIDTH_PX` `COVER_TITLE_CENTER_X` `INTRO_TITLE_CENTER_X` `INTRO_TITLE_MAX_WIDTH_PX`

**H（2）**：`CAPTION_CENTER_Y` `INTRO_TITLE_CENTER_Y`

**T（6）**：`COVER_CONTAINER_CENTER_Y`（陷阱 1）、`OUTRO_RING_RADIUS_STEP_PX` `OUTRO_LOGO_SIZE_PX`（陷阱 2）、`WATERMARK_MAX_WIDTH_PX` `COVER_ROW_TEXT_MAX_WIDTH_PX` `OUTRO_TITLE_MAX_WIDTH_PX`（陷阱 3）

**V（1）**：`COVER_WATERMARK_CENTER_Y_PX`

**F（4）**：`INTRO_TYPEWRITER_FRAMES` `INTRO_CURSOR_BLINK_PERIOD_FRAMES` `OUTRO_RING_OUT_DURATION_FRAMES` `OUTRO_RING_OUT_DELAY_FRAMES`

**N（26）**：`CAPTION_LONG_CHAR_THRESHOLD` `DEFAULT_LINE_HEIGHT` `ENTRANCE_MAX_MS` `ENTRANCE_DURATION_RATIO` `ENTRANCE_SCALE_RANGE` `ENTRANCE_OPACITY_RANGE` `WATERMARK_COLOR` `WATERMARK_LETTER_SPACING_EM` `COVER_WATERMARK_COLOR` `WATERMARK_SEP_OPACITY_MUL` `WATERMARK_SEP` `COVER_ROW_OPACITY` `TITLE_COLOR_BLACK` `INTRO_TYPEWRITER_SECONDS` `INTRO_CURSOR_TEXT` `INTRO_FADE_OUT_RANGE` `OUTRO_RING_COUNT` `OUTRO_RING_OUT_DURATION_SECONDS` `OUTRO_RING_OUT_DELAY_SECONDS` `OUTRO_RING_OUT_PROGRESS_MAX` `OUTRO_LOGO_SCALE_IN_FRAMES` `OUTRO_LOGO_SCALE_RANGE` `OUTRO_TITLE_FADE_IN_FRAMES` `OUTRO_TITLE_OPACITY_RANGE` `OUTRO_FADE_OUT_FRAMES` `OUTRO_FADE_OUT_RANGE`

---

## File Structure

| 文件 | 职责 | 本计划的改动 |
|---|---|---|
| `src/render/canvas.rs` | **新建**。`Canvas` 值类型与三个具名尺寸；`scale()`。 | Create |
| `src/render/metrics.rs` | **新建**。`Metrics`：按 `Canvas` 一次算好的全部版式量。 | Create |
| `src/render/timeline.rs` | 时间轴。`WIDTH`/`HEIGHT` 移出，改为 re-export `Canvas`。 | Modify |
| `src/render/draw.rs` | 四段绘制。常量表迁入 `metrics.rs`，`Painter` 持 `Canvas` + `Metrics`。 | Modify |
| `src/render/draw_tests.rs` | 绘制测试。全部显式渲染在 `Canvas::BASE`，断言数值不动。 | Modify |
| `src/render/frame.rs` | 逐帧渲染。`Pixmap` 尺寸改由 `Canvas` 决定。 | Modify |
| `src/ffmpeg.rs` | 滤镜与命令行。`WIDTH`/`HEIGHT` 改从 `Canvas` 取。 | Modify |
| `src/render/mod.rs` | 模块声明。 | Modify |
| `tests/canvas_baseline.rs` | **新建**。BASE 逐字节基线快照与门禁。 | Create |

---

## Task 1：建立 BASE 逐字节基线

**Files:**
- Create: `tests/canvas_baseline.rs`
- Create: `tests/baseline/` （快照目录，随测试首次运行生成后提交）

**Interfaces:**
- Consumes: 现有 `panda::render::frame::FrameSource`、`panda::config::Branding`
- Produces: `tests/baseline/frame_<n>.rgba` 四个快照文件；后续每个任务都跑 `cargo test --test canvas_baseline` 作为门禁

**为什么第一个做**：这是整个计划唯一的「行为未变」证明。先有基线，后面每一步才有据可依。

- [ ] **Step 1: 写基线测试**

`tests/canvas_baseline.rs`：

```rust
//! **BASE（1280×720）逐字节基线**——计划 A 的硬门禁。
//!
//! 画布参数化是一次纯重构：78 个常量要逐条改成由 `Canvas` 推导，
//! 任何一处漏改都会让画面静默错位，而不会编译失败。唯一可靠的证明是
//! 「重构前后渲染同一帧，字节完全相同」。
//!
//! 快照存的是 `render()` 出来的**预乘 RGBA 原始字节**，不经 PNG 编码——
//! PNG 有压缩参数与元数据，比对它等于顺带比对编码器版本。
//!
//! 四帧覆盖四段：0=Cover、100=Intro（打字机进行中）、200=Content（字幕
//! 入场动画中）、550=Outro（logo 已入场、整体淡出未开始）。

use panda::config::Branding;
use panda::render::frame::FrameSource;

const VTT: &str = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:04.000\n第一条字幕。\n\n2\n00:00:04.000 --> 00:00:10.000\n第二条字幕，稍微长一点。\n";
const FRAMES: [u32; 4] = [0, 100, 200, 550];

fn baseline_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/baseline")
}

/// 固定的品牌装备：两处水印都配上，让水印绘制路径也进基线。
fn fixture() -> Branding {
    Branding {
        brand: "基线品牌".into(),
        watermark: Some("正文水印".into()),
        watermark_cover: Some("封面水印 · 副标题".into()),
        watermark_icon: None,
        logo: None,
    }
}

#[test]
fn base_canvas_rendering_is_byte_identical_to_the_baseline() {
    let dir = baseline_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut fs = FrameSource::new(VTT, "基线标题".into(), &fixture()).unwrap();

    let mut regenerated = Vec::new();
    for f in FRAMES {
        let pixmap = fs.render(f).unwrap();
        let got = pixmap.data().to_vec();
        let path = dir.join(format!("frame_{f}.rgba"));

        if !path.exists() {
            std::fs::write(&path, &got).unwrap();
            regenerated.push(f);
            continue;
        }
        let want = std::fs::read(&path).unwrap();
        assert_eq!(
            got.len(),
            want.len(),
            "第 {f} 帧字节数变了：基线 {} → 现在 {}。\
             重构不该改变画布尺寸；若确实要改基线，删掉 tests/baseline/ 重新生成并在提交里说明理由。",
            want.len(),
            got.len()
        );
        let diff = got.iter().zip(want.iter()).filter(|(a, b)| a != b).count();
        assert_eq!(
            diff, 0,
            "第 {f} 帧有 {diff} 个字节与基线不同（共 {} 字节）。\
             这是计划 A 的硬门禁：BASE 上的渲染必须逐字节不变。",
            want.len()
        );
    }

    assert!(
        regenerated.is_empty(),
        "首次运行已生成基线快照 {regenerated:?}，请 git add tests/baseline/ 后重跑本测试确认门禁生效"
    );
}
```

- [ ] **Step 2: 首次运行，生成快照**

Run: `cargo test --test canvas_baseline`
Expected: FAIL，信息为「首次运行已生成基线快照 [0, 100, 200, 550]」。`tests/baseline/` 下出现四个 `.rgba` 文件，每个 `1280*720*4 = 3686400` 字节。

- [ ] **Step 3: 确认快照尺寸正确**

Run: `ls -l tests/baseline/ && du -sh tests/baseline/`
Expected: 四个文件各 3686400 字节，合计约 14MB。

- [ ] **Step 4: 让快照入库并确认门禁生效**

```bash
git add -f tests/baseline/
cargo test --test canvas_baseline
```
Expected: PASS。

> `-f` 是因为 `.gitignore` 不含它，此处显式确认要入库：14MB 的二进制进 git 是有代价的，但它是本计划唯一的正确性证明，值。计划 B 完成后可评估是否改成校验和。

- [ ] **Step 5: 验证门禁真的会红（变异）**

临时把 `src/render/draw.rs` 的 `CAPTION_FONT_SIZE_SHORT` 从 `80.0` 改成 `81.0`，跑 `cargo test --test canvas_baseline`。
Expected: FAIL，报「第 200 帧有 N 个字节与基线不同」。改回来后重跑应 PASS。

- [ ] **Step 6: 提交**

```bash
git add tests/canvas_baseline.rs tests/baseline/
git commit -m "test(canvas): 建立 BASE 逐字节基线，作为画布参数化重构的硬门禁"
```

---

## Task 2：新建 `Canvas`，收敛两份画布常量

**Files:**
- Create: `src/render/canvas.rs`
- Modify: `src/render/mod.rs`
- Modify: `src/render/timeline.rs`（`WIDTH`/`HEIGHT` 改为从 `Canvas::BASE` 推导）

**Interfaces:**
- Produces: `panda::render::canvas::Canvas { pub w: u32, pub h: u32 }`，关联常量 `Canvas::BASE`，方法 `fn scale(&self) -> f32`、`fn w_f32(&self) -> f32`、`fn h_f32(&self) -> f32`。Task 3–7 全部消费它。
- **本任务只加 `BASE`**；`LANDSCAPE`/`PORTRAIT` 属于计划 B。

- [ ] **Step 1: 写失败测试**

`src/render/canvas.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// BASE 就是重构前写死的那对数值——这是「重构不改行为」的第一道保证。
    #[test]
    fn base_matches_the_pre_refactor_hardcoded_size() {
        assert_eq!((Canvas::BASE.w, Canvas::BASE.h), (1280, 720));
    }

    /// `scale` 以**宽度**为基准：文字排版的约束是横向的（换行、字符密度），
    /// 以宽度为基准才能让「元素占画布宽的比例」跨尺寸恒定。
    ///
    /// 这条同时钉死「BASE 上 scale 恰为 1.0」——计划 A 的全部长度量纲常量
    /// 因此在 BASE 上取值不变，逐字节门禁才可能成立。
    #[test]
    fn scale_is_one_at_base_and_derives_from_width() {
        assert_eq!(Canvas::BASE.scale(), 1.0);

        // 一个非 BASE 的临时尺寸：宽度翻倍 → scale 翻倍，与高度无关。
        let wide = Canvas { w: 2560, h: 720 };
        assert_eq!(wide.scale(), 2.0, "scale 应只由宽度决定");
        let tall = Canvas { w: 1280, h: 1440 };
        assert_eq!(tall.scale(), 1.0, "高度变化不应影响 scale");
    }

    #[test]
    fn float_accessors_match_the_integer_fields() {
        let c = Canvas { w: 1080, h: 1920 };
        assert_eq!(c.w_f32(), 1080.0);
        assert_eq!(c.h_f32(), 1920.0);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test --lib canvas::`
Expected: FAIL，`cannot find type Canvas in this scope`（模块尚未创建）。

- [ ] **Step 3: 写实现**

`src/render/canvas.rs` 顶部：

```rust
//! 画布尺寸。
//!
//! **为什么是一个值类型而不是一对常量**：重构前 `timeline` 与 `draw` 各写了
//! 一份 `WIDTH`/`HEIGHT`（`CANVAS_W`/`CANVAS_H`）且互不引用——
//! `docs/follow-ups.md` 早已记账：「两边一旦不一致，结果是画面静默错位，
//! 而不是编译失败」。收敛成一个值之后，尺寸只有一个来源，且能作为参数传递。

/// 一次渲染的画布尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Canvas {
    pub w: u32,
    pub h: u32,
}

impl Canvas {
    /// 版式常量的调优基准，也是测试基线。
    ///
    /// `src/render/draw.rs` 里那批长度量纲的常量（字号、边距、图标尺寸……）
    /// 当初就是在这个尺寸上手工调优的，[`Canvas::scale`] 以它的宽度为分母。
    /// **它是基准，不必然是产品尺寸**。
    pub const BASE: Canvas = Canvas { w: 1280, h: 720 };

    /// 长度量纲常量的缩放因子，以 [`Canvas::BASE`] 的**宽度**为基准。
    ///
    /// 以宽度而非高度或对角线为基准：文字排版的约束是横向的（换行位置、
    /// 一行能放多少字），以宽度为基准可以保证「元素占画布宽度的比例」在
    /// 不同尺寸之间恒定。
    pub fn scale(&self) -> f32 {
        self.w as f32 / Self::BASE.w as f32
    }

    pub fn w_f32(&self) -> f32 {
        self.w as f32
    }

    pub fn h_f32(&self) -> f32 {
        self.h as f32
    }
}
```

`src/render/mod.rs` 加 `pub mod canvas;`（放在既有模块声明的字母序位置）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test --lib canvas::`
Expected: PASS，3 条。

- [ ] **Step 5: 让 `timeline` 从 `Canvas::BASE` 推导，消灭第一份重复**

`src/render/timeline.rs` 的

```rust
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
```

改为

```rust
/// 时间轴模块沿用的画布尺寸。**不再各写一份**：由 `Canvas::BASE` 推导，
/// 与 `render::draw` 共享唯一真相源（销 `docs/follow-ups.md` TTS 子系统
/// 第 3 条：画布尺寸此前同时存在于两个文件且互不引用）。
///
/// 计划 B 会把这两个 re-export 换成随运行期 `Canvas` 走的参数；本计划只做
/// 收敛，不改数值。
pub const WIDTH: u32 = crate::render::canvas::Canvas::BASE.w;
pub const HEIGHT: u32 = crate::render::canvas::Canvas::BASE.h;
```

- [ ] **Step 6: 全量测试 + 门禁**

Run: `cargo test && cargo test --test canvas_baseline`
Expected: 全绿；基线 PASS（尺寸未变，字节未变）。

- [ ] **Step 7: 变异验证——两份常量是否真的收敛了**

把 `Canvas::BASE` 临时改成 `{ w: 1281, h: 720 }`，跑 `cargo test`。
Expected: `timeline` 与 `draw` **两侧的测试同时变红**（若只有一侧红，说明另一侧还在读自己那份写死值，收敛没做干净）。改回后重跑应全绿。

- [ ] **Step 8: 提交**

```bash
git add src/render/canvas.rs src/render/mod.rs src/render/timeline.rs
git commit -m "feat(render): 新建 Canvas，timeline 的画布尺寸改为从 BASE 推导"
```

---

## Task 3：`Metrics` 骨架 + L 类（25 个长度量纲常量）

**Files:**
- Create: `src/render/metrics.rs`
- Modify: `src/render/mod.rs`

**Interfaces:**
- Consumes: `Canvas`（Task 2）
- Produces: `panda::render::metrics::Metrics`，构造函数 `Metrics::for_canvas(canvas: Canvas) -> Metrics`，字段全部为 `f32`/`u32`（下方逐一列出）。Task 4–6 往里加字段，Task 7 让 `Painter` 持有它。

**本任务只搬 L 类 25 个**，其余类别留在 `draw.rs` 原地，分任务迁移——这样每一步都能单独跑门禁，出问题时定位范围小。

- [ ] **Step 1: 写失败测试**

`src/render/metrics.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::canvas::Canvas;

    /// **BASE 上每一个长度量纲的值都必须等于重构前手工调优的字面量。**
    ///
    /// 这是逐字节门禁在常量层面的对应物：门禁证明「画出来一样」，这条证明
    /// 「算出来一样」。两者都红时，看这条更容易定位是哪个常量漏改了。
    #[test]
    fn base_metrics_match_the_pre_refactor_literals() {
        let m = Metrics::for_canvas(Canvas::BASE);
        assert_eq!(m.caption_padding_x, 40.0);
        assert_eq!(m.caption_font_size_long, 52.0);
        assert_eq!(m.caption_font_size_short, 80.0);
        assert_eq!(m.caption_stroke_width, 6.0);
        assert_eq!(m.entrance_translate_x_from, 100.0);
        assert_eq!(m.entrance_letter_spacing_from, 8.0);
        assert_eq!(m.watermark_margin_left, 40.0);
        assert_eq!(m.watermark_margin_bottom, 40.0);
        assert_eq!(m.watermark_font_size, 24.0);
        assert_eq!(m.watermark_icon_size, 28);
        assert_eq!(m.watermark_icon_gap, 10.0);
        assert_eq!(m.cover_watermark_font_size, 28.0);
        assert_eq!(m.cover_watermark_icon_size, 32);
        assert_eq!(m.cover_watermark_icon_gap, 12.0);
        assert_eq!(m.cover_row_margin_left, 40.0);
        assert_eq!(m.cover_logo_size, 36);
        assert_eq!(m.cover_logo_margin, 8.0);
        assert_eq!(m.cover_row_text_font_size, 38.0);
        assert_eq!(m.cover_title_font_size, 100.0);
        assert_eq!(m.cover_title_padding, 40.0);
        assert_eq!(m.intro_title_font_size, 70.0);
        assert_eq!(m.intro_cursor_gap, 4.0);
        assert_eq!(m.outro_title_font_size, 70.0);
        assert_eq!(m.outro_title_gap, 40.0);
        assert_eq!(m.outro_title_translate_y_from, -50.0);
    }

    /// 长度量纲一律随 `scale` 线性放缩，且**「动画」小节里那三个像素位移
    /// 也在内**——它们看着像动画参数，量纲却是像素，是最容易漏的一类。
    #[test]
    fn every_length_scales_linearly_including_the_animation_offsets() {
        let base = Metrics::for_canvas(Canvas::BASE);
        let double = Metrics::for_canvas(Canvas { w: 2560, h: 720 });
        assert_eq!(double.caption_font_size_short, base.caption_font_size_short * 2.0);
        assert_eq!(double.cover_title_font_size, base.cover_title_font_size * 2.0);
        assert_eq!(double.watermark_margin_left, base.watermark_margin_left * 2.0);
        assert_eq!(double.cover_logo_size, base.cover_logo_size * 2);
        assert_eq!(double.watermark_icon_size, base.watermark_icon_size * 2);
        // 三个「看着像动画、量纲是像素」的
        assert_eq!(
            double.entrance_translate_x_from,
            base.entrance_translate_x_from * 2.0,
            "入场位移是像素，必须跟着缩放"
        );
        assert_eq!(
            double.entrance_letter_spacing_from,
            base.entrance_letter_spacing_from * 2.0,
            "入场字距是像素，必须跟着缩放"
        );
        assert_eq!(
            double.outro_title_translate_y_from,
            base.outro_title_translate_y_from * 2.0,
            "Outro 标题的归位位移是像素，必须跟着缩放"
        );
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test --lib metrics::`
Expected: FAIL，`cannot find type Metrics in this scope`。

- [ ] **Step 3: 写实现**

`src/render/metrics.rs`：

```rust
//! 按画布一次算好的版式量表。
//!
//! **为什么把常量搬进一个结构体**：重构前它们是 `draw.rs` 里的 `const`，
//! 值写死在编译期；画布可变之后，长度量纲的量必须随画布缩放。若逐帧现算，
//! 热路径上会反复做同样的乘法；若散在各处算，就又回到了「同一个事实写好
//! 几遍」的老问题。这里在 `Painter::new()` 里算一次，绘制时只读字段。
//!
//! 附带好处：`Metrics::for_canvas` 是纯函数，测试可以直接对着它断言比例
//! 关系，不必渲染整帧。

use crate::render::canvas::Canvas;

/// 一个画布对应的全部版式量。字段名对应重构前的常量名（去掉 `_PX` 后缀）。
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub canvas: Canvas,

    // —— 字幕（Content 段）——
    pub caption_padding_x: f32,
    pub caption_font_size_long: f32,
    pub caption_font_size_short: f32,
    pub caption_stroke_width: f32,

    // —— 字幕入场动画中的**像素**量 ——
    pub entrance_translate_x_from: f32,
    pub entrance_letter_spacing_from: f32,

    // —— 正文水印 ——
    pub watermark_margin_left: f32,
    pub watermark_margin_bottom: f32,
    pub watermark_font_size: f32,
    pub watermark_icon_size: u32,
    pub watermark_icon_gap: f32,

    // —— 封面/片尾水印 ——
    pub cover_watermark_font_size: f32,
    pub cover_watermark_icon_size: u32,
    pub cover_watermark_icon_gap: f32,

    // —— Cover 上排与主标题 ——
    pub cover_row_margin_left: f32,
    pub cover_logo_size: u32,
    pub cover_logo_margin: f32,
    pub cover_row_text_font_size: f32,
    pub cover_title_font_size: f32,
    pub cover_title_padding: f32,

    // —— Intro ——
    pub intro_title_font_size: f32,
    pub intro_cursor_gap: f32,

    // —— Outro ——
    pub outro_title_font_size: f32,
    pub outro_title_gap: f32,
    pub outro_title_translate_y_from: f32,
}

impl Metrics {
    pub fn for_canvas(canvas: Canvas) -> Self {
        let s = canvas.scale();
        // 四舍五入到整数像素的辅助：图标与 logo 是位图，尺寸必须是整数。
        let px = |v: f32| (v * s).round() as u32;
        Self {
            canvas,
            caption_padding_x: 40.0 * s,
            caption_font_size_long: 52.0 * s,
            caption_font_size_short: 80.0 * s,
            caption_stroke_width: 6.0 * s,
            entrance_translate_x_from: 100.0 * s,
            entrance_letter_spacing_from: 8.0 * s,
            watermark_margin_left: 40.0 * s,
            watermark_margin_bottom: 40.0 * s,
            watermark_font_size: 24.0 * s,
            watermark_icon_size: px(28.0),
            watermark_icon_gap: 10.0 * s,
            cover_watermark_font_size: 28.0 * s,
            cover_watermark_icon_size: px(32.0),
            cover_watermark_icon_gap: 12.0 * s,
            cover_row_margin_left: 40.0 * s,
            cover_logo_size: px(36.0),
            cover_logo_margin: 8.0 * s,
            cover_row_text_font_size: 38.0 * s,
            cover_title_font_size: 100.0 * s,
            cover_title_padding: 40.0 * s,
            intro_title_font_size: 70.0 * s,
            intro_cursor_gap: 4.0 * s,
            outro_title_font_size: 70.0 * s,
            outro_title_gap: 40.0 * s,
            outro_title_translate_y_from: -50.0 * s,
        }
    }
}
```

`src/render/mod.rs` 加 `pub mod metrics;`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test --lib metrics::`
Expected: PASS，2 条。

- [ ] **Step 5: 变异验证——漏缩放一个会不会被抓住**

把 `entrance_translate_x_from: 100.0 * s` 改成 `100.0`（漏乘 scale），跑 `cargo test --lib metrics::`。
Expected: `every_length_scales_linearly_including_the_animation_offsets` FAIL。改回后 PASS。

再把 `scale()` 改成按高度（`self.h as f32 / Self::BASE.h as f32`），跑 `cargo test --lib canvas:: metrics::`。
Expected: `scale_is_one_at_base_and_derives_from_width` FAIL。改回后 PASS。

- [ ] **Step 6: 提交**

```bash
git add src/render/metrics.rs src/render/mod.rs
git commit -m "feat(render): 新建 Metrics，25 个长度量纲常量按 scale 一次算好"
```

> 注意：本任务**只创建 `Metrics`，尚未接入 `draw.rs`**，因此 `cargo clippy` 会报 `Metrics` 的字段未被读取。这是预期的中间态，Task 7 接入后消失。若 CI 严格禁止告警，在本任务的 `Metrics` 上临时加 `#[allow(dead_code)]`，并在 Task 7 删掉。

---

## Task 4：修陷阱 1（`COVER_CONTAINER_CENTER_Y` 写死 360）

**Files:**
- Modify: `src/render/draw.rs:96`
- Test: `src/render/draw_tests.rs`

**Interfaces:**
- Consumes: `Canvas`（Task 2）
- Produces: 无新接口，只改一个常量的推导方式

**背景**（规格 §3 陷阱 1）：语义是「垂直居中于画布」= `CANVAS_H / 2`，而 `720 / 2` **恰好等于**写死的 `360.0`。BASE 上取值不变，因此逐字节门禁不受影响；1080 下应为 540、1920 下应为 960。

- [ ] **Step 1: 写失败测试**

`src/render/draw_tests.rs` 末尾追加：

```rust
/// **陷阱 1 回归**（规格 §3）：Cover 居中容器的垂直中心必须是 `h / 2`，
/// 不能是写死的 360。
///
/// 720 / 2 恰好等于 360，所以这个错误在 BASE 上**完全看不出来**——判据必须
/// 用一个非 720 高的画布。这里不渲染，直接对推导式取值断言：渲染判据在
/// BASE 上永远绿，起不到作用。
#[test]
fn cover_container_center_y_is_half_the_canvas_height() {
    for c in [
        Canvas::BASE,
        Canvas { w: 1920, h: 1080 },
        Canvas { w: 1080, h: 1920 },
    ] {
        assert_eq!(
            cover_container_center_y(c),
            c.h_f32() / 2.0,
            "Cover 容器应垂直居中于画布；写死 360 时 {}x{} 会偏上",
            c.w,
            c.h
        );
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test --lib draw::tests::cover_container_center_y`
Expected: FAIL，`cannot find function cover_container_center_y`。

- [ ] **Step 3: 写实现**

`src/render/draw.rs` 把

```rust
const COVER_CONTAINER_CENTER_Y: f32 = 360.0;
```

替换为

```rust
/// Cover 居中容器的垂直中心：画布高度的一半。
///
/// **重构前这里写死 `360.0`**，而 `CANVAS_H / 2 = 720 / 2` 恰好等于 360——
/// 两种写法在 BASE 上给出相同的数，所以这个错误在 1280×720 下怎么测都测
/// 不出来。换尺寸才会暴露：1080 高下应为 540（写死值偏上 180px），
/// 1920 高下应为 960（偏上 600px）。旁边的 `CAPTION_CENTER_Y` 与
/// `INTRO_TITLE_CENTER_Y` 一直是 `CANVAS_H / 2.0`，只有它掉了队。
fn cover_container_center_y(canvas: Canvas) -> f32 {
    canvas.h_f32() / 2.0
}
```

并把 `draw_cover` 里所有 `COVER_CONTAINER_CENTER_Y` 的引用改成
`cover_container_center_y(self.canvas)`——本任务 `Painter` 尚未持有 `canvas`，
临时用 `cover_container_center_y(Canvas::BASE)`，Task 7 接入后改为 `self.canvas`。

在 `draw_tests.rs` 顶部 `use` 区加两行（Task 5、6 的测试也要用到 `Metrics`）：

```rust
use crate::render::canvas::Canvas;
use crate::render::metrics::Metrics;
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test --lib draw::tests::cover_container_center_y`
Expected: PASS。

- [ ] **Step 5: 门禁 + 全量**

Run: `cargo test && cargo test --test canvas_baseline`
Expected: 全绿；基线 PASS（BASE 上 `720/2 == 360`，字节未变）。

- [ ] **Step 6: 变异验证**

把实现改回 `fn cover_container_center_y(_canvas: Canvas) -> f32 { 360.0 }`。
Expected: 新测试 FAIL 且**只在 1920×1080 与 1080×1920 两档报错**，BASE 那档仍通过——这正是「陷阱在 BASE 上不可见」的证据。改回后 PASS。

- [ ] **Step 7: 提交**

```bash
git add src/render/draw.rs src/render/draw_tests.rs
git commit -m "fix(draw): Cover 容器垂直中心改为 h/2，不再写死 360（陷阱 1）"
```

---

## Task 5：修陷阱 2（Outro logo 与圆环按高度推导）

**Files:**
- Modify: `src/render/draw.rs:163,179`
- Test: `src/render/draw_tests.rs`

**Interfaces:**
- Consumes: `Canvas`、`Metrics`
- Produces: `Metrics` 新增两个字段 `outro_logo_size: u32`、`outro_ring_radius_step: f32`

**背景**（规格 §3 陷阱 2）：两者现为 `CANVAS_H * 0.3`。在**任意 16:9 画布**上 `H × 0.3` 恰等于 `W × 0.16875`，所以按高推导与按宽推导给出相同的数——**横版怎么改尺寸都测不出来**。9:16 下按高得 576（占宽 53%），按宽得 182（占宽 16.875%，与设计一致）。

- [ ] **Step 1: 写失败测试**

`src/render/draw_tests.rs` 末尾追加：

```rust
/// **陷阱 2 回归**（规格 §3）：Outro 的 logo 与圆环半径步长必须按**宽度**
/// 推导，不能按高度。
///
/// 判据是「占画布宽度的比例三档相等」。**不能只用 16:9 的画布验**：在任意
/// 16:9 上 `H×0.3` 恰等于 `W×0.16875`，两种写法同值，测试会假绿。9:16 那
/// 一档才是真正的判据。
#[test]
fn outro_logo_and_ring_scale_with_width_not_height() {
    let base = Metrics::for_canvas(Canvas::BASE);
    let base_logo_ratio = base.outro_logo_size as f32 / Canvas::BASE.w_f32();
    let base_ring_ratio = base.outro_ring_radius_step / Canvas::BASE.w_f32();
    assert!(
        (base_logo_ratio - 0.16875).abs() < 1e-6,
        "BASE 上 logo 应为画布宽的 16.875%（216/1280），实得 {base_logo_ratio}"
    );

    for c in [Canvas { w: 1920, h: 1080 }, Canvas { w: 1080, h: 1920 }] {
        let m = Metrics::for_canvas(c);
        let logo_ratio = m.outro_logo_size as f32 / c.w_f32();
        let ring_ratio = m.outro_ring_radius_step / c.w_f32();
        assert!(
            (logo_ratio - base_logo_ratio).abs() < 1e-3,
            "{}x{}: logo 占宽比应与 BASE 一致（{base_logo_ratio}），实得 {logo_ratio}；\
             按 h*0.3 推导时 9:16 会得到 0.533",
            c.w,
            c.h
        );
        assert!(
            (ring_ratio - base_ring_ratio).abs() < 1e-3,
            "{}x{}: 圆环半径步长占宽比应与 BASE 一致，实得 {ring_ratio}",
            c.w,
            c.h
        );
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test --lib draw::tests::outro_logo_and_ring`
Expected: FAIL，`no field outro_logo_size on type Metrics`。

- [ ] **Step 3: 写实现**

`src/render/metrics.rs` 的 `Metrics` 加两个字段与两处赋值：

```rust
    // —— Outro（按宽度推导，见陷阱 2）——
    pub outro_logo_size: u32,
    pub outro_ring_radius_step: f32,
```

```rust
            outro_logo_size: px(216.0),
            outro_ring_radius_step: 216.0 * s,
```

并在 `Metrics` 的模块文档下方补一段：

```rust
/// **`216.0` 这个基准值的由来**：重构前写作 `CANVAS_H * 0.3`，在 BASE 上
/// 等于 `720 * 0.3 = 216`。改成按宽度推导之后基准值就是 216（= 1280 的
/// 16.875%）。两种写法在**任意 16:9 画布**上同值——`H = W × 0.5625`，故
/// `H × 0.3 = W × 0.16875`——所以这个差别只有 9:16 才看得出来。
```

`src/render/draw.rs` 删掉

```rust
const OUTRO_RING_RADIUS_STEP_PX: f32 = CANVAS_H * 0.3;
const OUTRO_LOGO_SIZE_PX: u32 = (CANVAS_H * 0.3) as u32;
```

引用处改为 `self.m.outro_ring_radius_step` / `self.m.outro_logo_size`——本任务
`Painter` 尚未持有 `Metrics`，临时用 `Metrics::for_canvas(Canvas::BASE)` 的局部
变量，Task 7 接入后改为 `self.m`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test --lib draw::tests::outro_logo_and_ring`
Expected: PASS。

- [ ] **Step 5: 门禁 + 全量**

Run: `cargo test && cargo test --test canvas_baseline`
Expected: 全绿；基线 PASS（BASE 上两种写法同为 216，字节未变）。

- [ ] **Step 6: 变异验证——确认「16:9 假绿」这件事是真的**

把 `outro_logo_size: px(216.0)` 改成按高度：`outro_logo_size: (c.h_f32() * 0.3) as u32`（需要先把 `canvas` 绑成局部变量 `c`）。
Expected: 新测试 FAIL，且报错**只来自 1080×1920 那一档**，1920×1080 那档仍通过。这直接证明了「横版测不出这个错误」。改回后 PASS。

- [ ] **Step 7: 提交**

```bash
git add src/render/draw.rs src/render/draw_tests.rs src/render/metrics.rs
git commit -m "fix(draw): Outro logo 与圆环改按宽度推导（陷阱 2：16:9 下两种写法同值）"
```

---

## Task 6：修陷阱 3（不换行哨兵）+ V 类垂直锚点

**Files:**
- Modify: `src/render/draw.rs:71,78,112,189`
- Test: `src/render/draw_tests.rs`

**Interfaces:**
- Produces: `Metrics` 新增 `no_wrap_width: f32`、`cover_watermark_center_y: f32`

**背景**：
- 陷阱 3（规格 §3）：三个 `2000.0` 的语义是「远大于画布宽以避免意外换行」。1280 下是画布的 1.5625 倍；**1920 下只宽 4.2%**，长品牌名配 1.5 倍字号真可能换行。
- V 类：`COVER_WATERMARK_CENTER_Y_PX = 576.0` = `0.8 × 720`，是唯一的绝对垂直锚点。

- [ ] **Step 1: 写失败测试**

`src/render/draw_tests.rs` 末尾追加：

```rust
/// **陷阱 3 回归**（规格 §3）：「不换行哨兵」必须显著大于画布宽度。
///
/// 重构前是写死的 `2000.0`：1280 宽下是画布的 1.56 倍（安全），1920 宽下
/// 只比画布宽 4.2%——一个长品牌名配 1.5 倍放大的字号真的可能触发换行。
/// 语义是「远大于画布宽」，就该随画布走。
#[test]
fn no_wrap_sentinel_stays_far_wider_than_the_canvas() {
    for c in [
        Canvas::BASE,
        Canvas { w: 1920, h: 1080 },
        Canvas { w: 1080, h: 1920 },
    ] {
        let m = Metrics::for_canvas(c);
        assert!(
            m.no_wrap_width >= c.w_f32() * 1.5,
            "{}x{}: 不换行哨兵应至少为画布宽的 1.5 倍，实得 {} (画布宽 {})",
            c.w,
            c.h,
            m.no_wrap_width,
            c.w
        );
    }
}

/// Cover 水印的垂直中心是画布高度的 80%，不是写死的 576。
///
/// `576 = 0.8 × 720`，在 BASE 上两种写法同值。
#[test]
fn cover_watermark_center_y_is_eighty_percent_of_height() {
    assert_eq!(
        Metrics::for_canvas(Canvas::BASE).cover_watermark_center_y,
        576.0,
        "BASE 上应与重构前的写死值一致"
    );
    for c in [Canvas { w: 1920, h: 1080 }, Canvas { w: 1080, h: 1920 }] {
        let m = Metrics::for_canvas(c);
        assert_eq!(
            m.cover_watermark_center_y,
            c.h_f32() * 0.8,
            "{}x{}: Cover 水印中心应在 0.8h",
            c.w,
            c.h
        );
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test --lib draw::tests::no_wrap_sentinel draw::tests::cover_watermark_center_y`
Expected: FAIL，`no field no_wrap_width on type Metrics`。

- [ ] **Step 3: 写实现**

`src/render/metrics.rs` 加字段与赋值：

```rust
    /// 「不换行」的哨兵宽度：喂给排版器当最大宽度，语义是「远大于画布宽，
    /// 使单行文本不会意外换行」。**随画布走**，不是写死的字面量——重构前
    /// 三处都写 `2000.0`，那个数在 1920 宽下只比画布宽 4.2%，已经不安全。
    pub no_wrap_width: f32,
    /// Cover 水印的垂直中心 = 画布高度的 80%（重构前写死 576 = 0.8 × 720）。
    pub cover_watermark_center_y: f32,
```

```rust
            no_wrap_width: canvas.w_f32() * 2.0,
            cover_watermark_center_y: canvas.h_f32() * 0.8,
```

`src/render/draw.rs` 删掉四个常量：`WATERMARK_MAX_WIDTH_PX`、
`COVER_ROW_TEXT_MAX_WIDTH_PX`、`OUTRO_TITLE_MAX_WIDTH_PX`、
`COVER_WATERMARK_CENTER_Y_PX`，引用处分别改为 `self.m.no_wrap_width` 与
`self.m.cover_watermark_center_y`（同 Task 5，本任务先用局部 `Metrics::for_canvas(Canvas::BASE)`）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test --lib draw::tests::no_wrap_sentinel draw::tests::cover_watermark_center_y`
Expected: PASS。

- [ ] **Step 5: 门禁 —— 本任务是唯一会改变 BASE 常量数值的一处**

Run: `cargo test --test canvas_baseline`
Expected: **PASS**。

> `no_wrap_width` 在 BASE 上从 `2000.0` 变成 `2560.0`，是三个陷阱里唯一改变
> BASE 取值的。它不该改变任何字节：这个值只作为排版器的「最大宽度」上限，
> 放宽一个**不曾被触发**的上限不会改变任何一次换行决策。
>
> **门禁若在这一步红了，不要调松门禁**——那说明存在一处「本来就在 2000
> 附近换行」的文案，即发现了一个既有缺陷。当场记进 `docs/follow-ups.md`
> 并向发起人报告，由人决定是接受新行为（重新生成基线并在提交里写明）还是
> 另行修复。

- [ ] **Step 6: 全量测试**

Run: `cargo test`
Expected: 全绿。

- [ ] **Step 7: 变异验证**

把 `no_wrap_width: canvas.w_f32() * 2.0` 改回 `2000.0`。
Expected: `no_wrap_sentinel_stays_far_wider_than_the_canvas` FAIL，且只在 1920 那档报错（1280 下 `2000 >= 1920` 成立、`2000 >= 1280*1.5 = 1920` 也成立）。改回后 PASS。

把 `cover_watermark_center_y` 改成 `576.0`。
Expected: `cover_watermark_center_y_is_eighty_percent_of_height` FAIL，BASE 那条断言仍通过。改回后 PASS。

- [ ] **Step 8: 提交**

```bash
git add src/render/draw.rs src/render/draw_tests.rs src/render/metrics.rs
git commit -m "fix(draw): 不换行哨兵与 Cover 水印锚点改为随画布推导（陷阱 3 + V 类）"
```

---

## Task 7：`Painter` 持有 `Canvas` + `Metrics`，接通全部引用

**Files:**
- Modify: `src/render/draw.rs`
- Modify: `src/render/frame.rs`
- Modify: `src/render/draw_tests.rs`
- Modify: `src/ffmpeg.rs`

**Interfaces:**
- Consumes: `Canvas`（Task 2）、`Metrics`（Task 3–6）
- Produces:
  - `Painter::new(branding: &Branding, canvas: Canvas) -> anyhow::Result<Painter>`
  - `FrameSource::new(vtt_text: &str, title: String, branding: &Branding, canvas: Canvas) -> Result<FrameSource>`
  - `FrameSource::canvas(&self) -> Canvas`
  - `ffmpeg::RenderInputs` 新增字段 `canvas: Canvas`

**这是把前面几个任务的中间态收尾的任务**：Task 4–6 里那些临时的
`Metrics::for_canvas(Canvas::BASE)` 局部变量在这里全部换成 `self.m`。

- [ ] **Step 1: 写失败测试**

`src/render/draw_tests.rs` 末尾追加：

```rust
/// `Painter` 按传入的画布建立版式，而不是永远用 BASE。
///
/// 判据：换一个宽度不同的画布，同一段文字的墨迹宽度必须按比例变化。若
/// `Painter` 内部还在读 `Canvas::BASE`，两次渲染会完全一样。
#[test]
fn painter_lays_out_according_to_the_canvas_it_was_given() {
    let b = test_branding();
    let base = Canvas::BASE;
    let wide = Canvas { w: 2560, h: 1440 };

    let mut p_base = Painter::new(&b, base).unwrap();
    let mut p_wide = Painter::new(&b, wide).unwrap();

    let mut a = Pixmap::new(base.w, base.h).unwrap();
    let mut c = Pixmap::new(wide.w, wide.h).unwrap();
    p_base.draw_intro(&mut a, 60, "标题");
    p_wide.draw_intro(&mut c, 60, "标题");

    let (ax0, _, ax1, _) = ink_bbox(&a).expect("BASE 应有墨迹");
    let (cx0, _, cx1, _) = ink_bbox(&c).expect("宽画布应有墨迹");
    let base_ratio = (ax1 - ax0) as f32 / base.w_f32();
    let wide_ratio = (cx1 - cx0) as f32 / wide.w_f32();
    assert!(
        (base_ratio - wide_ratio).abs() < 0.02,
        "标题墨迹占画布宽的比例应与画布无关；BASE {base_ratio} vs 宽画布 {wide_ratio}"
    );
}

/// 白底上的墨迹包围盒（任一通道显著低于 255 即算墨迹）。
fn ink_bbox(p: &Pixmap) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..p.height() {
        for x in 0..p.width() {
            let c = p.pixel(x, y).unwrap();
            if c.red() < 240 || c.green() < 240 || c.blue() < 240 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 != u32::MAX).then_some((x0, y0, x1, y1))
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test --lib draw::tests::painter_lays_out`
Expected: FAIL，`this function takes 1 argument but 2 arguments were supplied`。

- [ ] **Step 3: 让 `Painter` 持有 `Canvas` 与 `Metrics`**

`src/render/draw.rs`：

- `Painter` 结构体加两个字段：
  ```rust
      /// 本次渲染的画布。绘制里所有「画布宽/高」都读它，不再有模块级常量。
      canvas: Canvas,
      /// 按 `canvas` 一次算好的版式量表。
      m: Metrics,
  ```
- `Painter::new` 签名改为 `pub fn new(branding: &Branding, canvas: Canvas) -> anyhow::Result<Self>`，
  开头加 `let m = Metrics::for_canvas(canvas);`，末尾构造时填入 `canvas, m`。
- 删除模块级的 `const CANVAS_W` 与 `const CANVAS_H`；把它们的全部引用改为
  `self.canvas.w_f32()` / `self.canvas.h_f32()`。
- W 类与 H 类的 11+2 个派生常量改成 `Painter` 的私有方法（如
  `fn caption_center_x(&self) -> f32 { self.canvas.w_f32() / 2.0 }`），因为它们
  现在依赖运行期的 `canvas`，不能再是 `const`。
- Task 4–6 引入的临时局部 `Metrics::for_canvas(Canvas::BASE)` 全部删除，改用
  `self.m` / `self.canvas`。
- `scaled_logo` 的两个尺寸改用 `self.m.cover_logo_size` 与 `self.m.outro_logo_size`。
- 删掉 Task 3 里临时加的 `#[allow(dead_code)]`（若加过）。

- [ ] **Step 4: 让 `FrameSource` 接受并透传 `Canvas`**

`src/render/frame.rs`：

- `FrameSource` 加字段 `canvas: Canvas`。
- `new` 签名加末位参数 `canvas: Canvas`，内部 `Painter::new(branding, canvas)?`。
- 加访问器：
  ```rust
      /// 本次渲染的画布尺寸。`ffmpeg` 侧要用它决定 `-s` 与滤镜里的尺寸。
      pub fn canvas(&self) -> Canvas {
          self.canvas
      }
  ```
- `render()` 里建 `Pixmap` 的尺寸从 `WIDTH`/`HEIGHT` 改为 `self.canvas.w`/`self.canvas.h`。
- `write_rgba_frames` 里计算每帧字节数的地方同样改用 `self.canvas`。

- [ ] **Step 5: 让 `ffmpeg` 侧从 `RenderInputs` 取尺寸**

`src/ffmpeg.rs`：

- `RenderInputs` 加字段 `pub canvas: Canvas`。
- `build_render_args` 里 `format!("{WIDTH}x{HEIGHT}")` 改成
  `format!("{}x{}", i.canvas.w, i.canvas.h)`；视频滤镜里的
  `scale={WIDTH}:{HEIGHT}` / `crop={WIDTH}:{HEIGHT}` 同样改为从 `i.canvas` 取。
- 删掉 `use crate::render::timeline::{... HEIGHT ... WIDTH}` 里的这两个导入。
- `src/main.rs` 的 `build_render_inputs` 填入 `canvas: source.canvas()`。

- [ ] **Step 6: 让全部调用点显式传 `Canvas::BASE`**

所有 `Painter::new(&b)` → `Painter::new(&b, Canvas::BASE)`；
所有 `FrameSource::new(v, t, &b)` → `FrameSource::new(v, t, &b, Canvas::BASE)`。
涉及 `src/render/draw_tests.rs`、`src/render/frame.rs`、`src/ffmpeg.rs`、
`src/main.rs`、`tests/render_e2e.rs`、`tests/ffmpeg_missing.rs`、
`tests/canvas_baseline.rs`。

`draw_tests.rs` 里 102 处 `Pixmap::new(1280, 720)` 改成
`Pixmap::new(Canvas::BASE.w, Canvas::BASE.h)`——**断言数值一个都不改**。

`src/ffmpeg.rs` 测试里硬编码的 `"1280x720"`、`scale=1280:720`、`crop=1280:720`
改成由 `Canvas::BASE` 拼出，避免再写死一份。

- [ ] **Step 7: 运行，确认通过**

Run: `cargo test --lib draw::tests::painter_lays_out`
Expected: PASS。

- [ ] **Step 8: 门禁 + 全量 + 静态检查**

```bash
cargo test
cargo test --test canvas_baseline
cargo test --test render_e2e -- --ignored
cargo clippy --all-targets
cargo rustdoc --lib
cargo fmt --check
```
Expected: 全绿；基线 PASS；clippy 与 rustdoc 零告警。

- [ ] **Step 9: 变异验证——`Painter` 是否真的用了传入的画布**

把 `Painter::new` 里的 `let m = Metrics::for_canvas(canvas);` 改成
`Metrics::for_canvas(Canvas::BASE)`。
Expected: `painter_lays_out_according_to_the_canvas_it_was_given` FAIL。改回后 PASS。

- [ ] **Step 10: 提交**

```bash
git add -A
git commit -m "refactor(render): Painter 持有 Canvas 与 Metrics，画布尺寸全部改为运行期参数"
```

---

## Task 8：非 16:9 尺寸的兜底测试 + 文档收尾

**Files:**
- Modify: `src/render/draw_tests.rs`
- Modify: `docs/follow-ups.md`
- Modify: `docs/superpowers/specs/2026-09-01-panda-video-rs-design.md`

**Interfaces:**
- Consumes: 前面全部
- Produces: 无新接口

**为什么需要**（规格 §10）：三个陷阱是人工清点出来的，可能还有第四个。「在一个
**非 16:9、非 BASE** 的尺寸下渲染不 panic、不越界」能撞出那些「在 16:9 下碰巧
同值」的漏网之鱼。

- [ ] **Step 1: 写测试**

`src/render/draw_tests.rs` 末尾追加：

```rust
/// **漏网陷阱的兜底网**：在一组**刻意不是 16:9、也不是 BASE** 的尺寸上，
/// 四段都要能画完、不 panic、墨迹不越出画布。
///
/// 三个已知陷阱是人工清点出来的（规格 §3），可能还有第四个。16:9 下
/// 「按高推导」与「按宽推导」恰好同值，所以只用 16:9 与 9:16 验证是不够的
/// ——这里特意取 4:3、1:1、21:9 三种比例把这类巧合拆开。
#[test]
fn all_segments_render_within_bounds_on_non_sixteen_nine_canvases() {
    let b = branding_with(Some(TEST_CONTENT_WM), Some(TEST_COVER_WM));
    for c in [
        Canvas { w: 1024, h: 768 },  // 4:3
        Canvas { w: 900, h: 900 },   // 1:1
        Canvas { w: 2560, h: 1080 }, // 21:9
    ] {
        let mut p = Painter::new(&b, c).unwrap();
        let mut pm = Pixmap::new(c.w, c.h).unwrap();

        p.draw_cover(&mut pm, "一个足够长的测试标题用来触发换行");
        assert!(
            ink_bbox(&pm).is_some(),
            "{}x{} 的 Cover 应有墨迹",
            c.w,
            c.h
        );

        for f in [0u32, 60, 104] {
            let mut pm = Pixmap::new(c.w, c.h).unwrap();
            p.draw_intro(&mut pm, f, "一个足够长的测试标题用来触发换行");
        }

        let mut pm = Pixmap::new(c.w, c.h).unwrap();
        p.draw_content(&mut pm, 30, &caps());

        for f in [0u32, 60, 119] {
            let mut pm = Pixmap::new(c.w, c.h).unwrap();
            p.draw_outro(&mut pm, f);
        }
    }
}
```

- [ ] **Step 2: 运行**

Run: `cargo test --lib draw::tests::all_segments_render_within_bounds`
Expected: PASS。**若 panic，说明清点漏了第四个陷阱**——把它记进
`docs/follow-ups.md` 并向发起人报告，不要就地改测试绕过。

- [ ] **Step 3: 销掉画布双真相源那条账**

`docs/follow-ups.md` 的「TTS 子系统 · 值得做」第 3 条里，关于「画布尺寸与帧率
同时存在于两个文件」的那一段（帧渲染子系统补充部分），移进该节的「已销账」，
写明：收敛为 `render::canvas::Canvas`，`timeline` 的 `WIDTH`/`HEIGHT` 现由
`Canvas::BASE` 推导；变异验证为「改 `Canvas::BASE` 会让两侧测试同时变红」。

- [ ] **Step 4: 订正 logo 预缩那条**

`docs/follow-ups.md` 的「帧渲染子系统 · 已销账」里 logo 那条，补一句：
「附带的『换 256px 预缩版省 1.3MB』**确定不做**——计划 B 的 1080p 横版片尾
logo 需要 324px，256px 不够；内嵌原图 2048² 在两档下都够用。」

- [ ] **Step 5: 规格 §8 改写为「BASE 值 + 比例关系」**

`docs/superpowers/specs/2026-09-01-panda-video-rs-design.md` 的 §8.4 与 §8.6：
每个绝对像素值后面补注它在 `Canvas::BASE` 上的取值与推导方式，例如
「主标题 100px（BASE 值，实际为 `100 × scale`）」、「`cover` 水印垂直中心
`0.8 × h`（BASE 上 576）」。§8.3 补一句画布尺寸由 `Canvas` 决定。

- [ ] **Step 6: 全量验证**

```bash
cargo test && cargo test --test canvas_baseline && cargo clippy --all-targets && cargo fmt --check
```
Expected: 全绿、零告警。

- [ ] **Step 7: 提交**

```bash
git add -A
git commit -m "test(render): 非 16:9 画布的兜底测试；销画布双真相源的账，规格改写为比例关系"
```

---

## 完成标准

- [ ] `cargo test` 全绿，`cargo test --test render_e2e -- --ignored` 通过
- [ ] `cargo test --test canvas_baseline` 通过——**BASE 上渲染逐字节等于重构前**
- [ ] `cargo clippy --all-targets` 与 `cargo rustdoc --lib` 零告警，`cargo fmt --check` 一致
- [ ] `src/render/draw.rs` 里不再有 `CANVAS_W` / `CANVAS_H` 模块级常量
- [ ] 78 个常量全部归入八类之一，L 类 25 个已进 `Metrics`，T 类 6 个已修，V 类 1 个已改
- [ ] 三个陷阱各有一条**在 BASE 上不会红、在其它尺寸上会红**的回归测试
- [ ] 非 16:9 兜底测试通过
- [ ] `docs/follow-ups.md` 的画布双真相源条目已销账

## 计划 B 的前置

计划 B（加 1920×1080 / 1080×1920 两档、CLI `--orientation`、比例不变量测试、
实测吞吐）**在本计划完成后另行编写**。这样安排的原因：B 的 CLI 形状要建在 A
实际落地的 `Metrics` API 上，B 的吞吐任务需要 A 完成后才能实测——现在写只能
写成猜测。
