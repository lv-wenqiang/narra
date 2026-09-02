# panda-video-rs 帧渲染 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现把成片的**任意一帧**渲染成 1280×720 RGBA 位图的能力——封面、片头打字机、字幕层、片尾，全部按规格 §8 绘制。

**Architecture:** 纯计算 + 2D 光栅化，不碰视频编解码。动画曲线（`interpolate`/`spring`）与时间轴布局是纯函数；文字经 `cosmic-text` 排版后取字形轮廓，用 `tiny-skia` 先描边后填充；各段绘制到同一张 RGBA 画布上。交付一个调试子命令把指定帧导出 PNG，供人工核对视觉。**帧流管道、ffmpeg 滤镜图、混音与 `panda render`/`make` 是下一份计划的事。**

**Tech Stack:** Rust 2024 / tiny-skia（2D 光栅化）/ cosmic-text（文字排版）/ ttf-parser 或 swash（字形轮廓）/ resvg（GitHub 图标）/ image（PNG 编解码）/ clap。

**Spec:** `docs/superpowers/specs/2026-09-01-panda-video-rs-design.md`（§8 渲染子系统全部、§10 验证策略）

## Global Constraints

- 画布 **1280 × 720**，**30 fps**。
- 段落布局（`A` = 音频时长秒数，取 VTT 最后一条字幕的结束时间）：Cover 起始帧 **0** 长 **15**；Intro 起始帧 **15** 长 **105**；Content 起始帧 **120** 长 `ceil((A+2)*30)`；Outro 起始帧 `120+content_frames` 长 **120**。总帧数 `240 + ceil((A+2)*30)`。
- **各段内部的帧号从该段起点重新从 0 计数**（对应 Remotion 的 `Sequence` 相对帧语义）。
- Cover / Intro / Outro 画**不透明白底铺满**（alpha=255）；Content 画**完全透明底**，只画字幕和水印。
- 所有文字统一用内嵌的 `dingliesongtypeface`，**包括水印**（规格 §8.6 决策 1，不内嵌第二套字体）。
- 验收标准是**功能正确可用**，不要求与 TypeScript 原版逐像素一致。
- 目标平台仅 **Linux（WSL2）**。
- 外部依赖只允许 ffmpeg（本计划实际上一次都不调用它，测试素材除外）。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `assets/dingliesongtypeface.ttf` | 内嵌字体（5.6MB） |
| `assets/logo.png` | 内嵌 logo（1.4MB） |
| `assets/github-mark.svg` | 内嵌 GitHub 图标源码 |
| `src/assets.rs` | `include_bytes!` 内嵌资源与访问器 |
| `src/render/mod.rs` | 子模块声明 |
| `src/render/anim.rs` | `interpolate` / `spring`（纯函数） |
| `src/render/timeline.rs` | 段落布局与全局帧号→段内帧号的映射（纯函数） |
| `src/render/text.rs` | 文字排版、字形轮廓、描边+填充绘制 |
| `src/render/draw.rs` | 四个段落与水印的绘制 |
| `src/render/frame.rs` | 按全局帧号分派到对应段落，产出 RGBA 位图 |
| `src/vtt.rs`（修改） | 追加 `parse_vtt` |
| `src/main.rs`（修改） | 追加 `debug-frames` 子命令 |

---

### Task 0: 文字渲染探针（关卡）

**这是探针任务，不是 TDD 任务。** 产出是一个答案和一份 API 记录，不是要保留的生产代码。文字排版 + 字形轮廓 + 描边是本计划风险最高、也最难凭记忆写对的一环——**必须先确认这条路走得通，再往下做**。

**Files:**
- Create: `examples/text_probe.rs`
- Create: `docs/text-rendering.md`
- Create: `assets/dingliesongtypeface.ttf`（从 `../panda-video-ts/public/fonts/` 复制）

**Interfaces:**
- Produces: `docs/text-rendering.md`，记录实测通过的 crate 版本与 API 用法，供 Task 4 照抄。

- [ ] **Step 1: 复制字体并加依赖**

```bash
mkdir -p assets
cp ../panda-video-ts/public/fonts/dingliesongtypeface.ttf assets/
cargo add tiny-skia cosmic-text ttf-parser image
```

- [ ] **Step 2: 写探针**

目标：把「熊猫智研社 Test 123」以 **70px 粗体**渲染出来，**先用 6px 黑色描边、再填充白色**，画在半透明灰底上，存成 `text_probe.png`。

关键在于**必须拿到字形轮廓**（outline），而不是位图——只有轮廓才能描边。两条候选路线：

1. `cosmic-text` 负责排版（换行、字形定位），再用 `ttf-parser` 的 `outline_glyph` 拿每个字形的轮廓，转成 `tiny_skia::Path`；
2. 用 `swash` 的 `scale_outline`（`cosmic-text` 内部就用它）。

**你不必照抄任何 API 写法**——下面这些是我按印象写的，很可能与实际 crate 版本对不上：

```rust
// 仅为说明意图，API 以实际 crate 为准
let mut font_system = cosmic_text::FontSystem::new();
font_system.db_mut().load_font_data(FONT_BYTES.to_vec());
let mut buffer = cosmic_text::Buffer::new(&mut font_system, Metrics::new(70.0, 84.0));
buffer.set_text(&mut font_system, "熊猫智研社 Test 123", &Attrs::new().family(Family::Name("...")), Shaping::Advanced);
for run in buffer.layout_runs() {
    for glyph in run.glyphs {
        // 需要：glyph.glyph_id、glyph.x、glyph.y、以及从字体取轮廓的方法
        // 目标：构造 tiny_skia::Path，然后
        //   pixmap.stroke_path(&path, &black_paint, &Stroke { width: 6.0, .. }, transform, None);
        //   pixmap.fill_path(&path, &white_paint, FillRule::Winding, transform, None);
    }
}
```

请**自己查实际安装到的 crate 版本的 API**（`~/.cargo/registry/src/*/[crate]-[version]/src/` 下有源码，`cargo doc --open` 也可以），按真实 API 写。

- [ ] **Step 3: 运行并肉眼核对**

```bash
cargo run --example text_probe
```

期望：`text_probe.png` 里能看到白字黑边的「熊猫智研社 Test 123」，中文和拉丁字符都正确显示（**不是豆腐块 □□□**），描边均匀包裹字形、没有断裂或自相交伪影。

- [ ] **Step 4: 核对关键指标**

用 `python3` 读 PNG 逐像素检查（`image` crate 也行）：

- 画面里**同时存在**接近纯白（RGB 均 > 240）和接近纯黑（RGB 均 < 30）的像素——证明描边和填充都生效了
- 非透明像素的包围盒宽度合理（70px 字号、9 个字符，宽度应在 500~800px 量级）

- [ ] **Step 5: 若失败，按此顺序排查**

1. **中文显示为豆腐块** → 字体没被 `cosmic-text` 识别。检查 `load_font_data` 是否成功、`Attrs` 的 family 名是否与字体内部名称匹配（用 `ttf-parser` 读 `name` 表拿到真实家族名）。
2. **拿不到字形轮廓** → 换另一条路线（route 1 ↔ route 2）。若两条都不通，试 `ab_glyph` 或直接用 `ttf-parser` 的 `OutlineBuilder` trait 自己实现。
3. **描边有自相交伪影** → 字形轮廓的填充规则应为 `FillRule::Winding`；描边时若仍有问题，尝试先 `path.transform()` 归一化再描边。
4. **以上都不通** → **停止，返回 BLOCKED 并说明卡在哪**。备选方案是放弃描边、改用「四方向偏移描黑边再叠白字」的土办法（画 5 次），视觉上接近但边缘略糙——这个决定要由协调者做，不要自己切换。

- [ ] **Step 6: 记录实测 API**

把**实际跑通**的写法写进 `docs/text-rendering.md`：crate 名与版本、字体加载方式、字体内部家族名、排版调用、取轮廓的确切方法、构造 `tiny_skia::Path` 的方式、描边+填充的参数。Task 4 会照抄这份记录。

- [ ] **Step 7: 提交**

```bash
git add examples/text_probe.rs docs/text-rendering.md assets/ Cargo.toml Cargo.lock
git commit -m "spike: 验证 CJK 排版与字形描边的渲染路径"
```

---

### Task 1: 资源内嵌

**Files:**
- Create: `assets/logo.png`、`assets/github-mark.svg`
- Create: `src/assets.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `pub const FONT: &[u8]`
  - `pub const LOGO_PNG: &[u8]`
  - `pub const GITHUB_MARK_SVG: &[u8]`
  - `pub fn logo_rgba() -> anyhow::Result<(Vec<u8>, u32, u32)>`（解码后的 RGBA 像素、宽、高）
  - `pub fn github_mark_rgba(size: u32) -> anyhow::Result<(Vec<u8>, u32, u32)>`（按目标尺寸光栅化的 RGBA）

- [ ] **Step 1: 准备资源文件**

```bash
cp ../panda-video-ts/public/logo/logo.png assets/
```

GitHub 图标的 SVG 内容如下，原样写入 `assets/github-mark.svg`（path 数据取自 `../panda-video-ts/src/remotion/compositions/WatermarkText.tsx` 的官方 mark）：

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 98 96" width="98" height="96"><path fill="#000000" fill-rule="evenodd" clip-rule="evenodd" d="M48.854 0C21.839 0 0 22 0 49.217c0 21.756 13.993 40.172 33.405 46.69 2.427.49 3.316-1.059 3.316-2.362 0-1.141-.08-5.052-.08-9.127-13.59 2.934-16.42-5.867-16.42-5.867-2.184-5.704-5.42-7.17-5.42-7.17-4.448-3.015.324-3.015.324-3.015 4.934.326 7.523 5.052 7.523 5.052 4.367 7.496 11.404 5.378 14.235 4.074.404-3.178 1.699-5.378 3.074-6.6-10.839-1.225-22.243-5.546-22.243-24.705 0-5.378 1.94-9.778 5.014-13.2-.485-1.222-2.184-6.275.486-13.038 0 0 4.125-1.304 13.426 5.052a46.97 46.97 0 0 1 12.214-1.63c4.125 0 8.33.571 12.213 1.63 9.302-6.356 13.427-5.052 13.427-5.052 2.67 6.763.97 11.816.485 13.038 3.155 3.422 5.015 7.822 5.015 13.2 0 19.216-11.416 23.443-22.124 24.659 1.735 1.49 3.316 4.391 3.316 8.867 0 6.398-.08 11.546-.08 13.19 0 1.304.89 2.853 3.316 2.364 19.412-6.52 33.405-24.935 33.405-46.691C97.707 22 75.788 0 48.854 0z"/></svg>
```

注意：原 SVG 用 `fill="currentColor"`，这里改成 `#000000` 以便独立光栅化——绘制时会按预设颜色重新着色，所以填充色是什么不影响最终效果，只要不是透明即可。

- [ ] **Step 2: 写失败的测试**

```rust
// src/assets.rs 的 mod tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_are_non_empty() {
        assert!(FONT.len() > 1_000_000, "字体过小：{} 字节", FONT.len());
        assert!(LOGO_PNG.len() > 100_000, "logo 过小：{} 字节", LOGO_PNG.len());
        assert!(GITHUB_MARK_SVG.len() > 500, "svg 过小：{} 字节", GITHUB_MARK_SVG.len());
    }

    #[test]
    fn font_parses_as_truetype() {
        // sfnt version 应为 0x00010000（TrueType）
        assert_eq!(&FONT[..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn logo_decodes_to_rgba() {
        let (px, w, h) = logo_rgba().unwrap();
        assert!(w > 100 && h > 100, "logo 尺寸异常：{w}x{h}");
        assert_eq!(px.len(), (w * h * 4) as usize);
        // 不能是全透明
        assert!(px.chunks(4).any(|p| p[3] > 0), "logo 全透明");
    }

    #[test]
    fn github_mark_rasterizes_at_requested_size() {
        let (px, w, h) = github_mark_rgba(32).unwrap();
        assert_eq!((w, h), (32, 32));
        assert_eq!(px.len(), 32 * 32 * 4);
        assert!(px.chunks(4).any(|p| p[3] > 0), "图标全透明");
    }
}
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cargo test assets::`
Expected: FAIL，模块不存在

- [ ] **Step 4: 实现**

```bash
cargo add resvg
```

```rust
// src/assets.rs
use anyhow::{Context, Result};

pub const FONT: &[u8] = include_bytes!("../assets/dingliesongtypeface.ttf");
pub const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");
pub const GITHUB_MARK_SVG: &[u8] = include_bytes!("../assets/github-mark.svg");

/// 解码内嵌 logo 为 RGBA8，返回 (像素, 宽, 高)。
pub fn logo_rgba() -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(LOGO_PNG).context("解码内嵌 logo.png 失败")?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

/// 把内嵌的 GitHub 图标 SVG 光栅化为 size×size 的 RGBA8。
/// 图标是单色的，调用方会按水印预设重新着色，故此处填充色不重要。
pub fn github_mark_rgba(size: u32) -> Result<(Vec<u8>, u32, u32)> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(GITHUB_MARK_SVG, &opt)
        .context("解析内嵌 github-mark.svg 失败")?;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .context("创建图标画布失败")?;

    let svg_size = tree.size();
    let transform = resvg::tiny_skia::Transform::from_scale(
        size as f32 / svg_size.width(),
        size as f32 / svg_size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    Ok((pixmap.data().to_vec(), size, size))
}
```

> **⚠️ 上面这段 `resvg` 用法是按印象写的，很可能与实际安装到的版本对不上。** 这个 crate 在大版本间改动很大——`usvg::Tree::from_data` 的参数个数（有的版本要传 `fontdb`）、`resvg::render` 的签名与返回值、`Options` 的字段、以及 `usvg`/`tiny_skia` 是否从 `resvg` 重导出，都变过。
>
> **请打开实际源码确认**（`~/.cargo/registry/src/*/resvg-*/src/lib.rs`，或 `cargo doc --open`），按真实 API 写。
>
> **必须保持的语义**：输入是内嵌的 SVG 字节和目标边长，输出 `size × size` 的 RGBA8 像素，图标**拉伸铺满**画布（源 viewBox 是 `98 × 96` 非正方形，x/y 各自缩放，约 2% 形变，肉眼不可见；不要 letterbox 留白）。改了什么写进报告。
>
> 另注意：若 `resvg` 重导出的 `tiny_skia` 与本项目直接依赖的 `tiny-skia` **版本不一致**，两者的 `Pixmap` 会是不同类型、不能直接互传。真遇到就通过原始字节 `Vec<u8>` 交接（本函数的返回类型已经是字节，正是为此）。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test assets::`
Expected: 4 个测试 PASS

- [ ] **Step 6: 提交**

```bash
git add assets/ src/assets.rs src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat(assets): 内嵌字体、logo 与 GitHub 图标"
```

---

### Task 2: 动画函数

**Files:**
- Create: `src/render/mod.rs`、`src/render/anim.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `pub fn interpolate(x: f64, input_range: [f64; 2], output_range: [f64; 2]) -> f64`（两端钳制）
  - `pub fn interpolate3(x: f64, input_range: [f64; 3], output_range: [f64; 3]) -> f64`（三点分段，两端钳制）
  - `pub fn spring(frame: f64, fps: f64, duration_frames: f64, delay_frames: f64) -> f64`

- [ ] **Step 1: 写失败的测试**

```rust
// src/render/anim.rs 的 mod tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolate_hits_endpoints_exactly() {
        assert_eq!(interpolate(0.0, [0.0, 10.0], [1.2, 1.0]), 1.2);
        assert_eq!(interpolate(10.0, [0.0, 10.0], [1.2, 1.0]), 1.0);
    }

    #[test]
    fn interpolate_is_linear_in_between() {
        assert!((interpolate(5.0, [0.0, 10.0], [0.0, 100.0]) - 50.0).abs() < 1e-9);
        assert!((interpolate(2.5, [0.0, 10.0], [100.0, 0.0]) - 75.0).abs() < 1e-9);
    }

    #[test]
    fn interpolate_clamps_outside_range() {
        assert_eq!(interpolate(-5.0, [0.0, 10.0], [0.0, 1.0]), 0.0);
        assert_eq!(interpolate(99.0, [0.0, 10.0], [0.0, 1.0]), 1.0);
    }

    #[test]
    fn interpolate3_handles_the_cursor_blink_shape() {
        // 光标闪烁：[0, 7.5, 15] -> [1, 1, 0]
        assert_eq!(interpolate3(0.0, [0.0, 7.5, 15.0], [1.0, 1.0, 0.0]), 1.0);
        assert_eq!(interpolate3(7.5, [0.0, 7.5, 15.0], [1.0, 1.0, 0.0]), 1.0);
        assert!((interpolate3(11.25, [0.0, 7.5, 15.0], [1.0, 1.0, 0.0]) - 0.5).abs() < 1e-9);
        assert_eq!(interpolate3(15.0, [0.0, 7.5, 15.0], [1.0, 1.0, 0.0]), 0.0);
    }

    #[test]
    fn spring_hits_both_endpoints_exactly() {
        assert_eq!(spring(0.0, 30.0, 15.0, 0.0), 0.0);
        assert!((spring(15.0, 30.0, 15.0, 0.0) - 1.0).abs() < 1e-9);
        assert!((spring(999.0, 30.0, 15.0, 0.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn spring_is_monotonic_and_finite() {
        let mut prev = -1.0;
        for f in 0..=15 {
            let v = spring(f as f64, 30.0, 15.0, 0.0);
            assert!(v.is_finite(), "frame {f} 得到非有限值 {v}");
            assert!(v >= prev - 1e-12, "frame {f} 回退：{prev} -> {v}");
            prev = v;
        }
    }

    #[test]
    fn spring_eases_out_rather_than_moving_linearly() {
        // 过阻尼弹簧必须「先快后慢」。若误把 duration 当成时间跨度直接映射，
        // 只会取到自然曲线最前面约 22% 的一段，归一化后是近似匀速直线
        // （首末增量比约 1.07），本测试会失败。
        let v: Vec<f64> = (0..=15).map(|f| spring(f as f64, 30.0, 15.0, 0.0)).collect();
        assert!(v[7] > 0.85, "半程应已完成大部分行程，实得 {}", v[7]);
        let first = v[1] - v[0];
        let last = v[15] - v[14];
        assert!(
            first > last * 20.0,
            "首帧增量应远大于末帧（缓出特征），实得 {first} vs {last}"
        );
    }

    #[test]
    fn spring_stays_finite_over_long_durations() {
        // 自然稳定时间约 10.6 秒；若用 cosh/sinh 直接求值会在此量级溢出。
        for dur in [15.0, 120.0, 600.0] {
            for f in 0..=(dur as u32) {
                let v = spring(f as f64, 30.0, dur, 0.0);
                assert!(v.is_finite(), "duration={dur} frame={f} 得到非有限值 {v}");
                assert!((0.0..=1.0).contains(&v), "duration={dur} frame={f} 越界 {v}");
            }
        }
    }

    #[test]
    fn spring_respects_delay() {
        // delay 30 帧：之前恒为 0
        assert_eq!(spring(0.0, 30.0, 15.0, 30.0), 0.0);
        assert_eq!(spring(29.0, 30.0, 15.0, 30.0), 0.0);
        assert_eq!(spring(30.0, 30.0, 15.0, 30.0), 0.0);
        assert!(spring(38.0, 30.0, 15.0, 30.0) > 0.0);
        assert!((spring(45.0, 30.0, 15.0, 30.0) - 1.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test render::anim`
Expected: FAIL，函数不存在

- [ ] **Step 3: 实现**

```rust
// src/render/anim.rs

/// 两点线性插值，输入超出区间时钳制到端点。
/// 对应 Remotion 的 `interpolate(x, input, output, {extrapolate*: 'clamp'})`。
pub fn interpolate(x: f64, input_range: [f64; 2], output_range: [f64; 2]) -> f64 {
    let [i0, i1] = input_range;
    let [o0, o1] = output_range;
    if x <= i0 {
        return o0;
    }
    if x >= i1 {
        return o1;
    }
    let t = (x - i0) / (i1 - i0);
    o0 + (o1 - o0) * t
}

/// 三点分段线性插值，两端钳制。用于光标闪烁这类「保持后再下降」的曲线。
pub fn interpolate3(x: f64, input_range: [f64; 3], output_range: [f64; 3]) -> f64 {
    let [i0, i1, i2] = input_range;
    let [o0, o1, o2] = output_range;
    if x <= i1 {
        interpolate(x, [i0, i1], [o0, o1])
    } else {
        interpolate(x, [i1, i2], [o1, o2])
    }
}

// 规格 §8.5：mass = 1, stiffness = 100, damping = 200
const SPRING_MASS: f64 = 1.0;
const SPRING_STIFFNESS: f64 = 100.0;
const SPRING_DAMPING: f64 = 200.0;
/// Remotion 的 `durationRestThreshold` 默认值：距终值小于此即视为静止。
const SPRING_REST_THRESHOLD: f64 = 0.005;

/// 阻尼谐振子的阶跃响应，从 0 平滑趋近 1。
///
/// 规格 §8.5 的参数 `mass=1, stiffness=100, damping=200` give 阻尼比 `zeta = 10`，
/// 属**过阻尼**，因此曲线单调、无过冲。
///
/// **时间映射**：把弹簧的**自然稳定时间**（本参数下约 10.6 秒 ≈ 317 帧 @30fps）
/// 压缩进 `duration_frames`，使整条曲线连同末尾的缓出都落在给定帧数内。
/// 这与 Remotion 传 `durationInFrames` 时的重缩放语义一致。
///
/// **不要**改成「把 `duration_frames / fps` 当成时间跨度直接求值」——本参数下
/// 那样只会取到自然曲线最前面约 22% 的一段，归一化后是近似匀速直线
/// （首末增量比约 1.07），完全失去弹簧观感。`spring_eases_out_rather_than_moving_linearly`
/// 这条测试就是钉住这一点的。
pub fn spring(frame: f64, fps: f64, duration_frames: f64, delay_frames: f64) -> f64 {
    let _ = fps; // 时间映射基于自然稳定时间，与 fps 无关；保留参数以维持调用形态
    let elapsed = frame - delay_frames;
    if elapsed <= 0.0 || duration_frames <= 0.0 {
        return 0.0;
    }
    if elapsed >= duration_frames {
        return 1.0;
    }

    let w0 = (SPRING_STIFFNESS / SPRING_MASS).sqrt();
    let zeta = SPRING_DAMPING / (2.0 * (SPRING_STIFFNESS * SPRING_MASS).sqrt());
    debug_assert!(zeta > 1.0, "本实现只覆盖过阻尼情形，当前 zeta = {zeta}");

    // 过阻尼解析解写成两个**衰减**指数之和。
    // 不用 cosh/sinh：它们在自然稳定时间那个量级（t ≈ 10.6s）会直接溢出。
    let wd = w0 * (zeta * zeta - 1.0).sqrt();
    let k = zeta * w0 / wd;
    let slow = zeta * w0 - wd;
    let fast = zeta * w0 + wd;
    let ca = (1.0 + k) / 2.0;
    let cb = (1.0 - k) / 2.0;
    let step = |t: f64| 1.0 - (ca * (-slow * t).exp() + cb * (-fast * t).exp());

    // step(t) = 1 - threshold 的时刻（快极点项在此量级已可忽略）
    let settle_secs = (ca / SPRING_REST_THRESHOLD).ln() / slow;
    let denom = step(settle_secs);
    if denom.abs() < 1e-12 {
        return elapsed / duration_frames; // 极端参数下退化为线性，避免除零
    }
    (step((elapsed / duration_frames) * settle_secs) / denom).clamp(0.0, 1.0)
}
```

`src/render/mod.rs` 写 `pub mod anim;`，`src/lib.rs` 追加 `pub mod render;`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test render::anim`
Expected: 7 个测试 PASS

- [ ] **Step 5: 提交**

```bash
git add src/render/ src/lib.rs
git commit -m "feat(render): 实现 interpolate 与 spring 动画函数"
```

---

### Task 3: 时间轴布局与 VTT 解析

**Files:**
- Create: `src/render/timeline.rs`
- Modify: `src/render/mod.rs`、`src/vtt.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `pub struct Caption { pub text: String, pub start_ms: u64, pub end_ms: u64 }`（在 `src/vtt.rs`）
  - `pub fn parse_vtt(text: &str) -> Vec<Caption>`（在 `src/vtt.rs`）
  - `pub enum Segment { Cover, Intro, Content, Outro }`
  - `pub struct Layout { pub content_frames: u32, pub total_frames: u32 }`
  - `pub fn layout(audio_secs: f64) -> Layout`
  - `pub fn segment_at(layout: &Layout, global_frame: u32) -> Option<(Segment, u32)>`（返回段落与**段内**帧号）

- [ ] **Step 1: 写失败的测试**

```rust
// src/vtt.rs 的 mod tests 追加
#[test]
fn parse_vtt_reads_cues_with_text() {
    let s = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:02.500\n第一句。\n\n2\n00:00:02.500 --> 00:00:06.000\n第二句。\n";
    let caps = parse_vtt(s);
    assert_eq!(caps.len(), 2);
    assert_eq!(caps[0].start_ms, 0);
    assert_eq!(caps[0].end_ms, 2500);
    assert_eq!(caps[0].text, "第一句。");
    assert_eq!(caps[1].start_ms, 2500);
    assert_eq!(caps[1].end_ms, 6000);
}

#[test]
fn parse_vtt_roundtrips_generated_output() {
    let lines = vec!["第一段。".to_string(), "第二段。".to_string()];
    let out = generate_vtt(&lines, &[2.0, 3.0], 30);
    let caps = parse_vtt(&out);
    assert_eq!(caps.len(), 2);
    assert_eq!(caps[0].text, "第一段。");
    assert_eq!(caps[1].end_ms, 5000);
}

#[test]
fn parse_vtt_ignores_header_and_blank_lines() {
    assert!(parse_vtt("WEBVTT\n\n").is_empty());
    assert!(parse_vtt("").is_empty());
}
```

```rust
// src/render/timeline.rs 的 mod tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_spec_numbers() {
        // A = 10 秒 → content = ceil(12 * 30) = 360，总帧 = 240 + 360 = 600
        let l = layout(10.0);
        assert_eq!(l.content_frames, 360);
        assert_eq!(l.total_frames, 600);
    }

    #[test]
    fn layout_rounds_content_frames_up() {
        // A = 10.01 → ceil(12.01 * 30) = ceil(360.3) = 361
        assert_eq!(layout(10.01).content_frames, 361);
    }

    #[test]
    fn segments_tile_the_timeline_without_gaps() {
        let l = layout(10.0);
        let mut counts = [0u32; 4];
        for f in 0..l.total_frames {
            let (seg, _) = segment_at(&l, f).expect("每一帧都应属于某个段落");
            counts[seg as usize] += 1;
        }
        assert_eq!(counts, [15, 105, 360, 120]);
    }

    #[test]
    fn segment_local_frames_restart_at_zero() {
        let l = layout(10.0);
        assert_eq!(segment_at(&l, 0), Some((Segment::Cover, 0)));
        assert_eq!(segment_at(&l, 14), Some((Segment::Cover, 14)));
        assert_eq!(segment_at(&l, 15), Some((Segment::Intro, 0)));
        assert_eq!(segment_at(&l, 119), Some((Segment::Intro, 104)));
        assert_eq!(segment_at(&l, 120), Some((Segment::Content, 0)));
        assert_eq!(segment_at(&l, 479), Some((Segment::Content, 359)));
        assert_eq!(segment_at(&l, 480), Some((Segment::Outro, 0)));
        assert_eq!(segment_at(&l, 599), Some((Segment::Outro, 119)));
    }

    #[test]
    fn segment_at_returns_none_past_the_end() {
        let l = layout(10.0);
        assert_eq!(segment_at(&l, 600), None);
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test render::timeline vtt::tests::parse`
Expected: FAIL，函数不存在

- [ ] **Step 3: 实现 `parse_vtt`**

```rust
// src/vtt.rs 追加

/// 一条字幕。
#[derive(Debug, Clone, PartialEq)]
pub struct Caption {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// 解析 WebVTT。只认时间行与其后的文本行，忽略 WEBVTT 头、序号行与空行。
/// 多行文本用 `\n` 连接。无法解析的时间行整条跳过。
pub fn parse_vtt(text: &str) -> Vec<Caption> {
    let mut out = Vec::new();
    let mut pending: Option<(u64, u64)> = None;
    let mut buf: Vec<String> = Vec::new();

    let flush = |out: &mut Vec<Caption>, pending: &mut Option<(u64, u64)>, buf: &mut Vec<String>| {
        if let Some((start_ms, end_ms)) = pending.take() {
            if !buf.is_empty() {
                out.push(Caption { text: buf.join("\n"), start_ms, end_ms });
            }
        }
        buf.clear();
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line == "WEBVTT" || line.starts_with("NOTE") {
            continue;
        }
        if line.is_empty() {
            flush(&mut out, &mut pending, &mut buf);
            continue;
        }
        if let Some((a, b)) = line.split_once("-->") {
            flush(&mut out, &mut pending, &mut buf);
            if let (Some(s), Some(e)) = (parse_ts_ms(a.trim()), parse_ts_ms(b.trim())) {
                pending = Some((s, e));
            }
            continue;
        }
        // 纯数字的序号行：只有在还没开始收文本时才跳过
        if pending.is_some() && buf.is_empty() && line.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if pending.is_some() {
            buf.push(line.to_string());
        }
    }
    flush(&mut out, &mut pending, &mut buf);
    out
}

/// 解析 `HH:MM:SS.mmm`，失败返回 None。
fn parse_ts_ms(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let (sec, ms) = parts[2].split_once('.')?;
    let h: u64 = parts[0].parse().ok()?;
    let m: u64 = parts[1].parse().ok()?;
    let sec: u64 = sec.parse().ok()?;
    let ms: u64 = format!("{ms:0<3}")[..3].parse().ok()?;
    Some(h * 3_600_000 + m * 60_000 + sec * 1000 + ms)
}
```

- [ ] **Step 4: 实现时间轴布局**

```rust
// src/render/timeline.rs
pub const FPS: u32 = 30;
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;

const COVER_FRAMES: u32 = 15;   // ceil(0.5 * 30)
const INTRO_FRAMES: u32 = 105;  // ceil(3.5 * 30)
const OUTRO_FRAMES: u32 = 120;  // ceil(4.0 * 30)
const CONTENT_TAIL_SECS: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    Cover = 0,
    Intro = 1,
    Content = 2,
    Outro = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub content_frames: u32,
    pub total_frames: u32,
}

/// 由音频时长（秒）算出各段帧数。对应规格 §8.2。
pub fn layout(audio_secs: f64) -> Layout {
    let content_frames = ((audio_secs + CONTENT_TAIL_SECS) * FPS as f64).ceil().max(0.0) as u32;
    Layout {
        content_frames,
        total_frames: COVER_FRAMES + INTRO_FRAMES + content_frames + OUTRO_FRAMES,
    }
}

/// 全局帧号 → (段落, 段内帧号)。超出总时长返回 None。
pub fn segment_at(layout: &Layout, global_frame: u32) -> Option<(Segment, u32)> {
    let intro_start = COVER_FRAMES;
    let content_start = intro_start + INTRO_FRAMES;
    let outro_start = content_start + layout.content_frames;

    if global_frame < intro_start {
        Some((Segment::Cover, global_frame))
    } else if global_frame < content_start {
        Some((Segment::Intro, global_frame - intro_start))
    } else if global_frame < outro_start {
        Some((Segment::Content, global_frame - content_start))
    } else if global_frame < layout.total_frames {
        Some((Segment::Outro, global_frame - outro_start))
    } else {
        None
    }
}
```

`src/render/mod.rs` 追加 `pub mod timeline;`。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test`
Expected: 全部 PASS

- [ ] **Step 6: 提交**

```bash
git add src/render/timeline.rs src/render/mod.rs src/vtt.rs
git commit -m "feat(render): 段落时间轴布局与 WebVTT 解析"
```

---

### Task 4: 文字排版与描边绘制

**前置条件：Task 0 必须已通过，且 `docs/text-rendering.md` 已写好。**

**Files:**
- Create: `src/render/text.rs`
- Modify: `src/render/mod.rs`

**Interfaces:**
- Consumes: `crate::assets::FONT`
- Produces:
  - `pub struct TextStyle { pub size_px: f32, pub color: [u8; 4], pub stroke: Option<([u8; 4], f32)>, pub letter_spacing_px: f32, pub max_width_px: f32, pub line_height: f32, pub bold: bool }`
  - `pub struct TextRenderer`（持有字体系统，构造一次复用）
  - `pub fn TextRenderer::new() -> anyhow::Result<Self>`
  - `pub fn TextRenderer::measure(&mut self, text: &str, style: &TextStyle) -> (f32, f32)`（返回宽、高）
  - `pub fn TextRenderer::draw_centered(&mut self, pixmap: &mut tiny_skia::Pixmap, text: &str, center_x: f32, center_y: f32, style: &TextStyle, opacity: f32, scale: f32)`

- [ ] **Step 1: 写失败的测试**

这些测试断言的是**像素事实**，不依赖任何具体 crate 的 API 形状：

```rust
// src/render/text.rs 的 mod tests
#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Pixmap;

    fn blank(w: u32, h: u32) -> Pixmap {
        Pixmap::new(w, h).unwrap()
    }

    fn non_transparent_bbox(p: &Pixmap) -> Option<(u32, u32, u32, u32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..p.height() {
            for x in 0..p.width() {
                if p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false) {
                    x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y);
                }
            }
        }
        (x0 != u32::MAX).then_some((x0, y0, x1, y1))
    }

    fn style(size: f32) -> TextStyle {
        TextStyle {
            size_px: size,
            color: [255, 255, 255, 255],
            stroke: Some(([0, 0, 0, 255], 6.0)),
            letter_spacing_px: 0.0,
            max_width_px: 1024.0,
            line_height: 1.2,
            bold: true,
        }
    }

    #[test]
    fn renders_cjk_and_latin_without_tofu() {
        let mut r = TextRenderer::new().unwrap();
        let mut p = blank(1280, 720);
        r.draw_centered(&mut p, "熊猫智研社 Test 123", 640.0, 360.0, &style(70.0), 1.0, 1.0);
        let bbox = non_transparent_bbox(&p).expect("画布应有非透明像素");
        let w = bbox.2 - bbox.0;
        // 9 个字符 @70px，宽度应在合理量级；豆腐块也有宽度，故另用下面的测试排除
        assert!(w > 300 && w < 1100, "文字宽度异常：{w}px");
    }

    #[test]
    fn stroke_and_fill_both_present() {
        let mut r = TextRenderer::new().unwrap();
        let mut p = blank(600, 200);
        r.draw_centered(&mut p, "描边测试", 300.0, 100.0, &style(70.0), 1.0, 1.0);
        let mut has_white = false;
        let mut has_black = false;
        for y in 0..p.height() {
            for x in 0..p.width() {
                if let Some(c) = p.pixel(x, y) {
                    if c.alpha() > 200 {
                        let (r_, g_, b_) = (c.red(), c.green(), c.blue());
                        if r_ > 240 && g_ > 240 && b_ > 240 { has_white = true; }
                        if r_ < 30 && g_ < 30 && b_ < 30 { has_black = true; }
                    }
                }
            }
        }
        assert!(has_white, "没有白色填充");
        assert!(has_black, "没有黑色描边");
    }

    #[test]
    fn opacity_zero_draws_nothing() {
        let mut r = TextRenderer::new().unwrap();
        let mut p = blank(400, 200);
        r.draw_centered(&mut p, "隐形", 200.0, 100.0, &style(70.0), 0.0, 1.0);
        assert!(non_transparent_bbox(&p).is_none(), "opacity=0 时不应画出任何东西");
    }

    #[test]
    fn larger_font_produces_wider_bbox() {
        let mut r = TextRenderer::new().unwrap();
        let mut small = blank(1280, 400);
        let mut large = blank(1280, 400);
        r.draw_centered(&mut small, "测试文字", 640.0, 200.0, &style(40.0), 1.0, 1.0);
        r.draw_centered(&mut large, "测试文字", 640.0, 200.0, &style(80.0), 1.0, 1.0);
        let ws = non_transparent_bbox(&small).unwrap();
        let wl = non_transparent_bbox(&large).unwrap();
        assert!((wl.2 - wl.0) > (ws.2 - ws.0), "80px 应比 40px 宽");
    }

    #[test]
    fn text_is_horizontally_centered_on_the_given_point() {
        let mut r = TextRenderer::new().unwrap();
        let mut p = blank(1280, 400);
        r.draw_centered(&mut p, "居中", 640.0, 200.0, &style(70.0), 1.0, 1.0);
        let (x0, _, x1, _) = non_transparent_bbox(&p).unwrap();
        let center = (x0 + x1) as f32 / 2.0;
        assert!((center - 640.0).abs() < 12.0, "水平中心偏移过大：{center}");
    }

    #[test]
    fn long_text_wraps_within_max_width() {
        let mut r = TextRenderer::new().unwrap();
        let mut s = style(70.0);
        s.max_width_px = 400.0;
        let mut p = blank(1280, 720);
        r.draw_centered(&mut p, "这是一段需要换行的比较长的中文文字内容", 640.0, 360.0, &s, 1.0, 1.0);
        let (x0, y0, x1, y1) = non_transparent_bbox(&p).unwrap();
        assert!((x1 - x0) <= 400 + 24, "宽度超出 max_width：{}", x1 - x0);
        assert!((y1 - y0) > 80, "应该换了行，高度只有 {}", y1 - y0);
    }

    #[test]
    fn synthetic_bold_produces_more_ink_than_regular() {
        // 内嵌字体只有 Regular 一个字重，粗体必须靠合成。
        // 断言：bold=true 的墨迹量显著多于 bold=false。
        let mut r = TextRenderer::new().unwrap();
        let ink = |bold: bool| {
            let mut s = style(80.0);
            s.bold = bold;
            s.stroke = None; // 去掉描边，只比字形本身的粗细
            let mut p = blank(1280, 400);
            r_draw(&mut r, &mut p, "粗体测试", &s);
            (0..p.height())
                .flat_map(|y| (0..p.width()).map(move |x| (x, y)))
                .filter(|&(x, y)| p.pixel(x, y).map(|c| c.alpha() > 128).unwrap_or(false))
                .count()
        };
        let regular = ink(false);
        let bold = ink(true);
        assert!(bold > regular, "合成粗体应比常规更粗：{bold} vs {regular}");
        assert!(
            (bold as f64) < (regular as f64) * 2.5,
            "合成粗体过粗，可能宽度参数写错：{bold} vs {regular}"
        );
    }

    /// 测试辅助：避免闭包重复借用 renderer。
    fn r_draw(r: &mut TextRenderer, p: &mut Pixmap, text: &str, s: &TextStyle) {
        r.draw_centered(p, text, 640.0, 200.0, s, 1.0, 1.0);
    }

    #[test]
    fn scale_enlarges_around_the_center_point() {
        let mut r = TextRenderer::new().unwrap();
        let mut a = blank(1280, 720);
        let mut b = blank(1280, 720);
        r.draw_centered(&mut a, "缩放", 640.0, 360.0, &style(70.0), 1.0, 1.0);
        r.draw_centered(&mut b, "缩放", 640.0, 360.0, &style(70.0), 1.0, 1.2);
        let ba = non_transparent_bbox(&a).unwrap();
        let bb = non_transparent_bbox(&b).unwrap();
        assert!((bb.2 - bb.0) > (ba.2 - ba.0), "scale=1.2 应更宽");
        // 中心不应漂移
        let ca = (ba.0 + ba.2) as f32 / 2.0;
        let cb = (bb.0 + bb.2) as f32 / 2.0;
        assert!((ca - cb).abs() < 12.0, "缩放后中心漂移：{ca} vs {cb}");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test render::text`
Expected: FAIL，模块不存在

- [ ] **Step 3: 实现**

**照抄 `docs/text-rendering.md` 里 Task 0 实测通过的写法。** 要点：

- `TextRenderer::new()` 加载内嵌字体，构造并持有字体系统（构造一次、多次复用，不要每帧重建——那会很慢）
- `draw_centered` 的语义：以 `(center_x, center_y)` 为**文本块的中心**绘制；`scale` 以该点为原点缩放；
  `opacity` 是**整块文字的组透明度**（等同 CSS `opacity`），**不是**乘进每一遍绘制的颜色 alpha。
  合成粗体使得同一个字形要画三遍（描边 / 加粗描边 / 填充），三遍在同一像素上叠加时 alpha 按 `1-(1-a)^n` 累积，
  下层黑描边还会透过上层半透明白填充——`opacity=0.5` 实测近纯白像素直接归零、整个字变灰。
  正确做法：**用完全不透明的 per-pass 颜色画进一张暂存 `Pixmap`，再用 `PixmapPaint { opacity, .. }` 一次性合成回目标 pixmap**。
  `TextStyle.color` 自身的 alpha 仍按原样参与每一遍绘制（与 CSS 中 `rgba()` 颜色的语义一致），不并入组透明度。
- `stroke` 为 `Some((颜色, 宽度))` 时**先描边再填充**（描边在下、填充在上），`None` 时只填充
- `letter_spacing_px` 逐字形追加水平偏移
- 超过 `max_width_px` 时换行；`\n` 强制换行；行距为 `size_px * line_height`

**两条必做项（来自 Task 0 的实测发现，不做会有实质缺陷）**：

1. **必须禁用系统字体回退。** `cosmic_text::FontSystem::new()` 内部会调 `fontdb::Database::load_system_fonts()`，把运行机器上装的所有字体注册进同一个库；`Shaping::Advanced` 遇到内嵌字体未覆盖的字符时会**静默回退到系统字体**（Task 0 的审查实测：阿拉伯字母被 DejaVu Sans 接管）。这会让成片长什么样取决于运行机器装了什么字体，违背「所有文字统一用内嵌字体」这条全局约束。**改用 `FontSystem::new_with_locale_and_db(locale, db)`，自己构造一个从未调用 `load_system_fonts()` 的 `fontdb::Database`，只 `load_font_data` 内嵌字体。** 具体写法见 `docs/text-rendering.md`。

2. **必须实现合成粗体。** 内嵌字体只有 Regular 一个静态字重（Task 0 的审查用 `ttf-parser` 读 `OS/2` 表确认：`weight: Normal`、`is_variable: false`、face 数量 1），`Weight::BOLD` 对它是空操作。而规格 §8.4 四个段落的文字**全部要求粗体**。TS 原版在 Chrome 下由浏览器合成粗体，Rust 侧没有那一层，不做的话四段文字会全部偏细。

   在本渲染路线（取矢量轮廓 → 描边/填充）下的做法是**按下面的顺序画三遍**，`bold_w = size_px * 0.03`：

   1. 若 `stroke` 为 `Some((stroke_color, stroke_w))`：用 `stroke_color` 描边，宽度 `stroke_w + bold_w`
   2. 用 `color`（填充色）描边，宽度 `bold_w`  ← 这一步产生加粗效果
   3. 用 `color` 填充

   这样黑色描边仍然完整包在**加粗后**的字形外侧。`bold: false` 时跳过第 2 步、第 1 步宽度用 `stroke_w`。

> **API 以实际 crate 版本为准。** Task 0 已经趟过一遍，以那份记录为准；若实现中发现记录有误，**改代码同时改记录**，并在报告里说明。

`src/render/mod.rs` 追加 `pub mod text;`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test render::text`
Expected: 7 个测试 PASS

- [ ] **Step 5: 目视抽查**

写一个临时程序（或用 `#[ignore]` 的测试）把 `stroke_and_fill_both_present` 那张图存成 PNG，**自己打开看一眼**：字形是否正常、描边是否均匀、有没有伪影。把观察结果写进报告。看完删掉临时文件。

- [ ] **Step 6: 提交**

```bash
git add src/render/text.rs src/render/mod.rs
git commit -m "feat(render): 文字排版与描边填充绘制"
```

---

### Task 5: Content 段——字幕层

**Files:**
- Create: `src/render/draw.rs`
- Modify: `src/render/mod.rs`

**Interfaces:**
- Consumes: `TextRenderer`、`TextStyle`、`anim::{interpolate, spring}`、`vtt::Caption`
- Produces:
  - `pub struct Painter { renderer: TextRenderer, /* 缓存的 logo / 图标位图 */ }`
  - `pub fn Painter::new() -> anyhow::Result<Self>`
  - `pub fn Painter::draw_content(&mut self, pixmap: &mut Pixmap, local_frame: u32, captions: &[Caption])`

- [ ] **Step 1: 写失败的测试**

```rust
// src/render/draw.rs 的 mod tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vtt::Caption;
    use tiny_skia::Pixmap;

    fn caps() -> Vec<Caption> {
        vec![
            Caption { text: "第一条字幕。".into(), start_ms: 0, end_ms: 2000 },
            Caption { text: "第二条字幕。".into(), start_ms: 2000, end_ms: 5000 },
        ]
    }

    fn count_visible(p: &Pixmap) -> usize {
        (0..p.height())
            .flat_map(|y| (0..p.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false))
            .count()
    }

    #[test]
    fn content_background_stays_transparent() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut p, 30, &caps());
        // 四角必须仍是全透明——Content 段不能画底
        for (x, y) in [(0, 0), (1279, 0), (0, 719), (1279, 719)] {
            assert_eq!(p.pixel(x, y).unwrap().alpha(), 0, "角点 ({x},{y}) 不应被填充");
        }
    }

    #[test]
    fn picks_the_caption_covering_the_current_time() {
        let mut painter = Painter::new().unwrap();
        // frame 30 → 1000ms → 第一条；frame 105 → 3500ms → 第二条
        let mut a = Pixmap::new(1280, 720).unwrap();
        let mut b = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut a, 30, &caps());
        painter.draw_content(&mut b, 105, &caps());
        // 两条字幕文本不同，像素分布必然不同
        assert_ne!(a.data(), b.data(), "不同时刻应显示不同字幕");
    }

    #[test]
    fn draws_nothing_but_watermark_when_no_caption_covers_the_time() {
        let mut painter = Painter::new().unwrap();
        let mut with_cap = Pixmap::new(1280, 720).unwrap();
        let mut past_end = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut with_cap, 30, &caps());
        painter.draw_content(&mut past_end, 300, &caps()); // 10000ms，超出最后一条
        assert!(count_visible(&past_end) < count_visible(&with_cap),
            "字幕结束后可见像素应显著减少（只剩水印）");
        assert!(count_visible(&past_end) > 0, "水印应该还在");
    }

    #[test]
    fn watermark_sits_in_the_lower_left_corner() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut p, 300, &caps()); // 无字幕，只剩水印
        // 水印在左下：距左 40px、距下 40px 附近应有像素，右上角不应有
        let has_in = (30..400).any(|x| (620..700).any(|y|
            p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false)));
        let has_top_right = (900..1280).any(|x| (0..200).any(|y|
            p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false)));
        assert!(has_in, "左下角应有水印");
        assert!(!has_top_right, "右上角不应有内容");
    }

    #[test]
    fn caption_entrance_animation_changes_over_time() {
        let mut painter = Painter::new().unwrap();
        // 入场动画持续 min(500ms, 时长*0.3)。第一条时长 2000ms → 500ms → 15 帧
        let mut f0 = Pixmap::new(1280, 720).unwrap();
        let mut f7 = Pixmap::new(1280, 720).unwrap();
        let mut f20 = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut f0, 0, &caps());
        painter.draw_content(&mut f7, 7, &caps());
        painter.draw_content(&mut f20, 20, &caps());
        assert_ne!(f0.data(), f7.data(), "动画中途应与起点不同");
        assert_ne!(f7.data(), f20.data(), "动画结束后应与中途不同");
    }

    #[test]
    fn long_caption_uses_smaller_font() {
        let mut painter = Painter::new().unwrap();
        // 去空白后 > 50 字 → 52px；否则 80px
        let short = vec![Caption { text: "短句。".into(), start_ms: 0, end_ms: 5000 }];
        let long_text: String = "长".repeat(60) + "。";
        let long = vec![Caption { text: long_text, start_ms: 0, end_ms: 5000 }];
        let mut a = Pixmap::new(1280, 720).unwrap();
        let mut b = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut a, 20, &short);
        painter.draw_content(&mut b, 20, &long);
        // 仅断言两者渲染结果不同即可——字号差异由实现保证，这里防的是"忘了实现字号规则"
        assert_ne!(a.data(), b.data());
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test render::draw`
Expected: FAIL，模块不存在

- [ ] **Step 3: 实现**

按规格 §8.4 的 **Content** 小节：

- 当前字幕 = `captions` 中满足 `start_ms <= t < end_ms` 的**第一条**，`t = local_frame / 30 * 1000`（毫秒）
- 字号：文本去除所有空白字符后 `chars().count() > 50` → **52px**，否则 **80px**
- 粗体，`dingliesongtypeface`，居中于画面正中（640, 360），最大宽度为画面 80%（1024px），保留换行
- 描边 **6px 黑 `#000000`**，填充 **白 `#FFFFFF`**
- 入场动画持续 `min(500ms, 字幕时长 * 0.3)` 换算成帧；`p = spring(自字幕开始经过的帧, 30, 动画帧数, 0)`
  - `scale`：`interpolate(p, [0,1], [1.2, 1.0])`
  - `opacity`：`interpolate(p, [0,1], [0.0, 1.0])`
  - `translate_x`：`interpolate(p, [0,1], [100.0, 0.0])`（像素，向右偏移后归位）
  - `letter_spacing`：`interpolate(p, [0,1], [8.0, 0.0])`
- 左下角水印（`content` 预设，规格 §8.6）：距左 40px、距下 40px，24px 字号，颜色 `rgba(255,255,255,0.27)`，字距 `0.01em`，GitHub 图标 28px、与文字间距 10px，**无中文后缀**
- **不画任何背景**——Content 段必须保持透明底

`src/render/mod.rs` 追加 `pub mod draw;`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test render::draw`
Expected: 6 个测试 PASS

- [ ] **Step 5: 目视抽查**

导出 `local_frame` 为 0 / 7 / 20 / 300 的四张 PNG，自己看一眼：字幕位置、描边、入场动画的缩放与位移、水印位置。写进报告，看完删掉临时文件。

- [ ] **Step 6: 提交**

```bash
git add src/render/draw.rs src/render/mod.rs
git commit -m "feat(render): 绘制 Content 字幕层与水印"
```

---

### Task 6: Cover 与 Intro 段

**Files:**
- Modify: `src/render/draw.rs`

**Interfaces:**
- Consumes: Task 5 的 `Painter`
- Produces:
  - `pub fn Painter::draw_cover(&mut self, pixmap: &mut Pixmap, title: &str)`
  - `pub fn Painter::draw_intro(&mut self, pixmap: &mut Pixmap, local_frame: u32, title: &str)`

- [ ] **Step 1: 写失败的测试**

```rust
// 追加到 src/render/draw.rs 的 mod tests
#[test]
fn cover_paints_an_opaque_white_background() {
    let mut painter = Painter::new().unwrap();
    let mut p = Pixmap::new(1280, 720).unwrap();
    painter.draw_cover(&mut p, "测试标题");
    for (x, y) in [(0, 0), (1279, 0), (0, 719), (1279, 719)] {
        let c = p.pixel(x, y).unwrap();
        assert_eq!(c.alpha(), 255, "角点 ({x},{y}) 应不透明");
        assert!(c.red() > 240 && c.green() > 240 && c.blue() > 240, "角点应为白底");
    }
}

#[test]
fn intro_paints_an_opaque_white_background() {
    let mut painter = Painter::new().unwrap();
    let mut p = Pixmap::new(1280, 720).unwrap();
    painter.draw_intro(&mut p, 60, "测试标题");
    let c = p.pixel(0, 0).unwrap();
    assert_eq!(c.alpha(), 255);
    assert!(c.red() > 240 && c.green() > 240 && c.blue() > 240);
}

#[test]
fn intro_typewriter_reveals_more_characters_over_time() {
    let mut painter = Painter::new().unwrap();
    let title = "这是一个比较长的测试标题用来看打字机效果";
    let mut early = Pixmap::new(1280, 720).unwrap();
    let mut mid = Pixmap::new(1280, 720).unwrap();
    let mut done = Pixmap::new(1280, 720).unwrap();
    painter.draw_intro(&mut early, 5, title);
    painter.draw_intro(&mut mid, 30, title);
    painter.draw_intro(&mut done, 62, title);  // 2 秒 = 60 帧后打完

    let ink = |p: &Pixmap| (0..p.height()).flat_map(|y| (0..p.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| { let c = p.pixel(x, y).unwrap(); c.red() < 200 }).count();
    assert!(ink(&early) < ink(&mid), "第 30 帧应比第 5 帧显示更多字");
    assert!(ink(&mid) < ink(&done), "打完后应比中途更多字");
}

#[test]
fn intro_fades_out_at_the_end() {
    let mut painter = Painter::new().unwrap();
    let title = "淡出测试";
    let mut before = Pixmap::new(1280, 720).unwrap();
    let mut last = Pixmap::new(1280, 720).unwrap();
    painter.draw_intro(&mut before, 89, title);   // 淡出开始前
    painter.draw_intro(&mut last, 104, title);    // 淡出终点
    let ink = |p: &Pixmap| (0..p.height()).flat_map(|y| (0..p.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| { let c = p.pixel(x, y).unwrap(); c.red() < 200 }).count();
    assert!(ink(&last) < ink(&before), "第 104 帧应比第 89 帧淡");
}

#[test]
fn cover_shows_the_given_title() {
    let mut painter = Painter::new().unwrap();
    let mut a = Pixmap::new(1280, 720).unwrap();
    let mut b = Pixmap::new(1280, 720).unwrap();
    painter.draw_cover(&mut a, "标题甲");
    painter.draw_cover(&mut b, "标题乙完全不同");
    assert_ne!(a.data(), b.data(), "不同标题应渲染出不同画面");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test render::draw`
Expected: FAIL，方法不存在

- [ ] **Step 3: 实现 Cover**

按规格 §8.4 的 **Cover** 小节：

- 白底 `#FFFFFF` 铺满，alpha 255
- 居中容器（画面正中，宽度 80%）：
  - **上排**（整体不透明度 **0.30**，左偏移 40px，水平排列、垂直居中）：`logo.png` 缩放到 **36px**、四周 margin 8px；紧邻「熊猫智研社」**38px 粗体**、行高 1.2
  - **主标题**：**100px 粗体**，左右 padding 40px，行高 1.2，支持换行
- 水印（`cover` 预设）：画面水平居中、自顶部偏移 **432px**，28px 字号，颜色 `rgba(23,23,23,0.4)`，图标 32px、间距 12px，**带中文后缀** ` · 熊猫视频自动化引擎`（间隔点 `·` 不透明度 0.75）

- [ ] **Step 4: 实现 Intro（打字机）**

按规格 §8.4 的 **Intro** 小节：

- 白底铺满
- 标题 **70px 粗体**，居中，宽度 80%，左右 padding 40px，支持换行
- 打字机：**2 秒内打完**
  - `chars_per_sec = title.chars().count() as f64 / 2.0`
  - `visible = min(floor(local_frame as f64 * chars_per_sec / 30.0), len)`
  - 只绘制前 `visible` 个字符
- 光标 `|`：**打字未完成时**显示（`local_frame < 60`），2 次/秒闪烁，绘制在文本右侧、左边距 4px
  - `opacity = interpolate3((local_frame % 15) as f64, [0.0, 7.5, 15.0], [1.0, 1.0, 0.0])`
- **3.0s → 3.5s 线性淡出**：整体不透明度 `interpolate(local_frame as f64, [90.0, 104.0], [1.0, 0.0])`
- **无水印**

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test render::draw`
Expected: 全部 PASS

- [ ] **Step 6: 目视抽查**

导出 Cover 一帧，以及 Intro 的第 5 / 30 / 62 / 100 帧，自己看：打字机是否逐字出现、光标是否闪、末尾是否淡出、封面的 logo 与标题排布是否合理。写进报告，看完删掉临时文件。

- [ ] **Step 7: 提交**

```bash
git add src/render/draw.rs
git commit -m "feat(render): 绘制 Cover 与 Intro 打字机段落"
```

---

### Task 7: Outro 段

**Files:**
- Modify: `src/render/draw.rs`

**Interfaces:**
- Consumes: Task 5 的 `Painter`、`anim::spring`
- Produces: `pub fn Painter::draw_outro(&mut self, pixmap: &mut Pixmap, local_frame: u32)`

- [ ] **Step 1: 写失败的测试**

```rust
// 追加到 src/render/draw.rs 的 mod tests
#[test]
fn outro_paints_an_opaque_white_background() {
    let mut painter = Painter::new().unwrap();
    let mut p = Pixmap::new(1280, 720).unwrap();
    painter.draw_outro(&mut p, 0);
    let c = p.pixel(0, 0).unwrap();
    assert_eq!(c.alpha(), 255);
    assert!(c.red() > 240 && c.green() > 240 && c.blue() > 240);
}

#[test]
fn outro_logo_grows_during_the_first_08_seconds() {
    let mut painter = Painter::new().unwrap();
    let mut f0 = Pixmap::new(1280, 720).unwrap();
    let mut f24 = Pixmap::new(1280, 720).unwrap();
    painter.draw_outro(&mut f0, 0);
    painter.draw_outro(&mut f24, 24);
    assert_ne!(f0.data(), f24.data(), "logo 应从 0.2 倍放大到 1.0 倍");
}

#[test]
fn outro_title_fades_in_after_the_logo() {
    let mut painter = Painter::new().unwrap();
    let mut f24 = Pixmap::new(1280, 720).unwrap();
    let mut f39 = Pixmap::new(1280, 720).unwrap();
    painter.draw_outro(&mut f24, 24);
    painter.draw_outro(&mut f39, 39);
    assert_ne!(f24.data(), f39.data(), "标题应在第 24~39 帧淡入");
}

#[test]
fn outro_fades_out_at_the_end() {
    let mut painter = Painter::new().unwrap();
    let ink = |p: &Pixmap| (0..p.height()).flat_map(|y| (0..p.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| { let c = p.pixel(x, y).unwrap(); c.red() < 200 }).count();
    let mut f100 = Pixmap::new(1280, 720).unwrap();
    let mut f119 = Pixmap::new(1280, 720).unwrap();
    painter.draw_outro(&mut f100, 100);
    painter.draw_outro(&mut f119, 119);
    assert!(ink(&f119) < ink(&f100), "第 119 帧应比第 100 帧淡");
}

#[test]
fn outro_never_produces_non_finite_geometry() {
    // 圆环的 scale = 1/(1-out_progress)，out_progress→1 时发散，必须钳制
    let mut painter = Painter::new().unwrap();
    for f in 0..120 {
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_outro(&mut p, f); // 不 panic 即通过
        assert_eq!(p.width(), 1280);
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test render::draw::tests::outro`
Expected: FAIL，方法不存在

- [ ] **Step 3: 实现**

按规格 §8.4 的 **Outro** 小节：

- 白底铺满
- **同心圆环**：5 个白色实心圆，半径 `720 * 0.3 * i`（`i = 0..4`），**倒序绘制**（大的在下），整体 `scale = 1 / (1 - out_progress)`
  - `out_progress = spring(local_frame as f64, 30.0, 15.0, 30.0)`（时长 0.5s、延迟 1s）
  - **必须钳制**：`out_progress` 趋近 1 时 scale 发散。把 `out_progress` 钳到最大 `0.99`（对应 scale 100），或直接钳制 scale 上限。**不钳制会产生 inf/NaN 几何而 panic 或画出垃圾。**
- **Logo**：`logo.png` 缩放到 `min(1280,720) * 0.3 = 216px`，`scale = interpolate(local_frame as f64, [0.0, 24.0], [0.2, 1.0])`，居中
- **固定标题「熊猫智研社」**：**70px 粗体黑色**，位于 logo 下方 40px
  - `opacity = interpolate(local_frame as f64, [24.0, 39.0], [0.0, 1.0])`
  - `translate_y = interpolate(local_frame as f64, [24.0, 39.0], [-50.0, 0.0])`
- **整体淡出**：`interpolate(local_frame as f64, [105.0, 119.0], [1.0, 0.0])`，作用于圆环、logo、标题、水印
- 水印（`cover` 预设，同 Cover）

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test render::draw`
Expected: 全部 PASS

- [ ] **Step 5: 目视抽查**

导出 Outro 的第 0 / 24 / 39 / 60 / 100 / 119 帧，自己看：logo 放大、标题下移淡入、圆环扩散、末尾整体淡出。写进报告，看完删掉临时文件。

- [ ] **Step 6: 提交**

```bash
git add src/render/draw.rs
git commit -m "feat(render): 绘制 Outro 段落"
```

---

### Task 8: 帧分派与调试导出命令

**Files:**
- Create: `src/render/frame.rs`
- Modify: `src/render/mod.rs`、`src/main.rs`

**Interfaces:**
- Consumes: `Painter`、`timeline::{layout, segment_at, Layout, Segment}`、`vtt::{parse_vtt, Caption}`
- Produces:
  - `pub struct FrameSource { /* painter, layout, captions, title */ }`
  - `pub fn FrameSource::new(vtt_text: &str, title: String) -> anyhow::Result<Self>`
  - `pub fn FrameSource::total_frames(&self) -> u32`
  - `pub fn FrameSource::render(&mut self, global_frame: u32) -> anyhow::Result<tiny_skia::Pixmap>`
  - CLI：`panda debug-frames --vtt <路径> [--title <标题>] -o <目录> [--frames 0,15,120]`

- [ ] **Step 1: 写失败的测试**

```rust
// src/render/frame.rs 的 mod tests
#[cfg(test)]
mod tests {
    use super::*;

    const VTT: &str = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:04.000\n第一条。\n\n2\n00:00:04.000 --> 00:00:10.000\n第二条。\n";

    #[test]
    fn total_frames_follows_the_last_cue_end_time() {
        let fs = FrameSource::new(VTT, "标题".into()).unwrap();
        // A = 10 秒 → content = ceil(12*30) = 360 → 总帧 = 600
        assert_eq!(fs.total_frames(), 600);
    }

    #[test]
    fn every_frame_renders_at_the_right_size() {
        let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
        for f in [0, 14, 15, 119, 120, 479, 480, 599] {
            let p = fs.render(f).unwrap();
            assert_eq!((p.width(), p.height()), (1280, 720), "帧 {f} 尺寸错误");
        }
    }

    #[test]
    fn cover_intro_outro_are_opaque_and_content_is_transparent() {
        let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
        for f in [0, 60, 500] {
            let p = fs.render(f).unwrap();
            assert_eq!(p.pixel(0, 0).unwrap().alpha(), 255, "帧 {f} 应不透明");
        }
        let p = fs.render(200).unwrap(); // Content 段
        assert_eq!(p.pixel(0, 0).unwrap().alpha(), 0, "Content 段应透明底");
    }

    #[test]
    fn rendering_past_the_end_is_an_error() {
        let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
        assert!(fs.render(600).is_err());
    }

    #[test]
    fn renders_the_whole_timeline_without_panicking() {
        let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
        // 全量跑一遍，抓 panic 与非有限几何
        for f in 0..fs.total_frames() {
            fs.render(f).unwrap();
        }
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test render::frame`
Expected: FAIL，模块不存在

- [ ] **Step 3: 实现 FrameSource**

```rust
// src/render/frame.rs
use anyhow::{bail, Result};
use tiny_skia::Pixmap;

use crate::render::draw::Painter;
use crate::render::timeline::{layout, segment_at, Layout, Segment, HEIGHT, WIDTH};
use crate::vtt::{parse_vtt, Caption};

pub struct FrameSource {
    painter: Painter,
    layout: Layout,
    captions: Vec<Caption>,
    title: String,
}

impl FrameSource {
    /// 由 VTT 文本与标题构造。音频时长取最后一条字幕的结束时间。
    pub fn new(vtt_text: &str, title: String) -> Result<Self> {
        let captions = parse_vtt(vtt_text);
        let audio_secs = captions.iter().map(|c| c.end_ms).max().unwrap_or(0) as f64 / 1000.0;
        Ok(Self {
            painter: Painter::new()?,
            layout: layout(audio_secs),
            captions,
            title,
        })
    }

    pub fn total_frames(&self) -> u32 {
        self.layout.total_frames
    }

    /// 渲染一帧。Cover/Intro/Outro 为不透明白底，Content 为透明底。
    pub fn render(&mut self, global_frame: u32) -> Result<Pixmap> {
        let Some((seg, local)) = segment_at(&self.layout, global_frame) else {
            bail!("帧号 {global_frame} 超出总时长 {}", self.layout.total_frames);
        };
        let mut pixmap = Pixmap::new(WIDTH, HEIGHT).expect("画布尺寸应合法");
        match seg {
            Segment::Cover => self.painter.draw_cover(&mut pixmap, &self.title),
            Segment::Intro => self.painter.draw_intro(&mut pixmap, local, &self.title),
            Segment::Content => self.painter.draw_content(&mut pixmap, local, &self.captions),
            Segment::Outro => self.painter.draw_outro(&mut pixmap, local),
        }
        Ok(pixmap)
    }
}
```

`src/render/mod.rs` 追加 `pub mod frame;` 和 `pub mod draw;`（若尚未追加）。

- [ ] **Step 4: 实现调试子命令**

在 `src/main.rs` 的 `Commands` 枚举追加：

```rust
    /// 把指定帧渲染成 PNG，用于人工核对视觉
    DebugFrames {
        /// VTT 文件路径
        #[arg(long)]
        vtt: PathBuf,
        /// 标题，默认「熊猫智研社」
        #[arg(long)]
        title: Option<String>,
        /// 输出目录
        #[arg(short, long)]
        out: PathBuf,
        /// 要导出的帧号，逗号分隔；不给则每 30 帧导一张
        #[arg(long)]
        frames: Option<String>,
    },
```

处理逻辑：读 VTT → 构造 `FrameSource` → 解析 `--frames`（为空则 `(0..total).step_by(30)`）→ 逐帧渲染并用 `image` 存成 `frame_{:05}.png` → 打印总帧数与导出清单。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test`
Expected: 全部 PASS

- [ ] **Step 6: 端到端目视验收（本计划的核心验收）**

用上一个子系统真实产出的 VTT：

```bash
printf '大家好，欢迎收看本期节目。\n今天我们来聊一个有意思的话题，这段话稍微长一点，用来测试字幕换行和字号规则。\n希望这期内容对你有帮助，我们下期再见。\n' > /tmp/e2e.txt
cargo run --release -- tts /tmp/e2e.txt /tmp/e2e-out
cargo run --release -- debug-frames --vtt /tmp/e2e-out/audio.vtt --title "这是一个测试标题" -o /tmp/frames
```

然后**逐张打开看**，至少覆盖这些帧：

| 帧 | 该看什么 |
|---|---|
| 0 | Cover：白底、logo + 小标题（30% 不透明）、100px 主标题、居中水印 |
| 20 | Intro：打字机刚开始，光标可见 |
| 60 | Intro：打字机刚打完 |
| 100 | Intro：正在淡出 |
| 130 | Content：第一条字幕入场动画中（偏右、略大、半透明） |
| 150 | Content：字幕稳定，白字黑边，左下角水印 |
| 内容段中后部 | Content：长字幕是否换行、字号是否变小 |
| 倒数第 100 帧 | Outro：logo 放大中 |
| 倒数第 80 帧 | Outro：标题淡入完成 |
| 倒数第 5 帧 | Outro：整体淡出 |

**把观察结果详细写进报告**——这是本计划唯一的视觉验收手段，也是下一份计划（接 ffmpeg）的前提。有任何看起来不对的地方（字重、间距、位置、动画节奏）都要记下来，即使不确定是不是问题。

- [ ] **Step 7: 提交**

```bash
git add src/render/frame.rs src/render/mod.rs src/main.rs
git commit -m "feat(render): 帧分派与 debug-frames 导出命令"
```

---

## 完成标准

- `cargo test` 全绿，`cargo clippy --all-targets` 零警告
- `panda debug-frames` 能把整条时间轴的任意帧导成 PNG，全量跑一遍不 panic
- 目视验收：四个段落的视觉与规格 §8.4 描述相符，动画节奏合理
- `docs/text-rendering.md` 记录了实测通过的文字渲染 API

下一份计划（ffmpeg 合成与 CLI）在此计划完成后编写。
