//! Content 段绘制：完全透明底 + 居中字幕 + 左下角水印。
//!
//! 对应规格 §8.4「Content」小节 + §8.6 水印 `content` 预设。数值全部照抄
//! `task-5-brief.md`，不是凭印象重写。
//!
//! **修复轮 1（I1/I2）**：水印不再是 `content` 写死的一段绘制代码——`WatermarkPreset`
//! 描述「画什么、多大、什么颜色、放哪」，`layout_and_draw_watermark` 是唯一一份
//! 通用绘制逻辑，`prepare_watermark` 在 `Painter::new()` 里把每个预设渲染成一张
//! 恰好包住墨迹的小 `Pixmap` 缓存起来，每帧只需 `draw_watermark` 做一次
//! `draw_pixmap` 贴图。Task 6 的 `cover` 预设、Task 7 的整体淡出都复用这套结构，
//! 不需要改 `draw_content`、不需要复制绘制逻辑（细节见 `WatermarkAnchor` 与
//! `PreparedWatermark` 的文档注释）。

use anyhow::Context;
use tiny_skia::{IntSize, Pixmap, PixmapPaint, Transform};

use crate::assets::github_mark_rgba;
use crate::render::anim::{interpolate, spring};
use crate::render::text::{TextRenderer, TextStyle};
use crate::vtt::Caption;

/// 画布参数（规格 §8.1）。
const CANVAS_W: f32 = 1280.0;
const CANVAS_H: f32 = 720.0;
const FPS: f64 = 30.0;

/// 字幕居中于画面正中。
const CAPTION_CENTER_X: f32 = CANVAS_W / 2.0;
const CAPTION_CENTER_Y: f32 = CANVAS_H / 2.0;
/// 字幕最大宽度：画面 80%。
const CAPTION_MAX_WIDTH_PX: f32 = CANVAS_W * 0.8;
/// 长字幕判定阈值：去除空白后的字符数超过此值即用小字号。
const CAPTION_LONG_CHAR_THRESHOLD: usize = 50;
const CAPTION_FONT_SIZE_LONG: f32 = 52.0;
const CAPTION_FONT_SIZE_SHORT: f32 = 80.0;
const CAPTION_STROKE_WIDTH_PX: f32 = 6.0;
/// 默认行高倍数（相对字号）。字幕与水印当前都用它——M2 改名前叫
/// `CAPTION_LINE_HEIGHT`，水印那处复用名不副实；Tasks 6/7 的 cover/outro
/// 水印预设也会用到它。
const DEFAULT_LINE_HEIGHT: f32 = 1.2;

/// 入场动画（规格 §8.4）：`min(500ms, 字幕时长 * 0.3)`，spring 驱动的
/// scale/opacity/translate_x/letter_spacing 四条曲线。
const ENTRANCE_MAX_MS: f64 = 500.0;
const ENTRANCE_DURATION_RATIO: f64 = 0.3;
const ENTRANCE_SCALE_RANGE: [f64; 2] = [1.2, 1.0];
const ENTRANCE_OPACITY_RANGE: [f64; 2] = [0.0, 1.0];
const ENTRANCE_TRANSLATE_X_RANGE: [f64; 2] = [100.0, 0.0];
const ENTRANCE_LETTER_SPACING_RANGE: [f64; 2] = [8.0, 0.0];

/// 左下角水印（`content` 预设，规格 §8.6）：无中文后缀，`bold: false` +
/// `stroke: None`（brief 明确指出：合成粗体会让半透明水印在三遍重叠处叠加变浓）。
const WATERMARK_TEXT: &str = "Panda Video Generator";
const WATERMARK_MARGIN_LEFT_PX: f32 = 40.0;
const WATERMARK_MARGIN_BOTTOM_PX: f32 = 40.0;
const WATERMARK_FONT_SIZE_PX: f32 = 24.0;
/// `rgba(255, 255, 255, 0.27)`：alpha 由颜色自身携带（`0.27 * 255 ≈ 69`），
/// `draw_centered` 的 `opacity` 参数固定传 `1.0`——两者是两件事（brief 提醒 1）。
const WATERMARK_COLOR: [u8; 4] = [255, 255, 255, 69];
const WATERMARK_LETTER_SPACING_EM: f32 = 0.01;
const WATERMARK_ICON_SIZE_PX: u32 = 28;
const WATERMARK_ICON_GAP_PX: f32 = 10.0;
/// 水印只有一行，换行宽度给一个远大于画布宽度的值以避免意外换行。
const WATERMARK_MAX_WIDTH_PX: f32 = 2000.0;

/// 把 GitHub 图标的原始光栅化位图（`assets::github_mark_rgba` 返回的单色形状、
/// 填充色不重要）按给定纯色重新着色，返回预乘 alpha 的 `Pixmap`，可直接用
/// `Pixmap::draw_pixmap` 合成到目标画布上。已按尺寸/颜色两个维度参数化——
/// Task 6 的 32px 深色图标直接复用，不需要新写一份。
fn tint_icon(raw_rgba: &[u8], width: u32, height: u32, color: [u8; 4]) -> Pixmap {
    let mut out = vec![0u8; raw_rgba.len()];
    for (src, dst) in raw_rgba.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        // 只借用原图 alpha 通道作为形状覆盖率，颜色由 `color` 决定。
        let mask = u32::from(src[3]);
        let a = mask * u32::from(color[3]) / 255;
        dst[0] = (u32::from(color[0]) * a / 255) as u8;
        dst[1] = (u32::from(color[1]) * a / 255) as u8;
        dst[2] = (u32::from(color[2]) * a / 255) as u8;
        dst[3] = a as u8;
    }
    Pixmap::from_vec(out, IntSize::from_wh(width, height).expect("图标尺寸非零"))
        .expect("图标像素数据长度应与 width*height*4 一致")
}

/// 扫描整幅画布，返回非透明像素的包围盒 `(x0,y0,x1,y1)`（`alpha>0` 才算数，
/// 含边界）。`None` 表示整幅画布全透明。`prepare_watermark` 用它裁剪出恰好
/// 包住水印墨迹的小图；`mod tests` 里的断言也直接复用同一份逻辑。
fn non_transparent_bbox(p: &Pixmap) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..p.height() {
        for x in 0..p.width() {
            if p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 != u32::MAX).then_some((x0, y0, x1, y1))
}

/// M1 修复：取用字幕文本前先整体 trim（掐头去尾）+ 每行行尾 trim。
/// 带尾随空格的字幕会因 `cosmic-text` 的 `run.line_w` 把尾随空白的 advance
/// 算进行宽，而 `draw_centered` 按 `line_w` 居中——实测可整体偏出 300+px。
fn trim_caption_text(text: &str) -> String {
    text.trim().lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
}

/// 水印锚点：目前只有 `content` 用到的「左下角，给定左/下边距」。
///
/// **I1 修复的扩展点**：Task 6 的 `cover` 预设需要「水平居中 + 指定垂直中心
/// （y=576，见协调者裁定）」——届时只需在这个枚举上加一个
/// `Centered { center_y_px: f32 }` 变体、在 `layout_and_draw_watermark` 的
/// `match` 里加一条对应分支（用 `CANVAS_W / 2.0 - total_width / 2.0` 算
/// `row_left`，`total_width = icon_size + gap + text_w` 已经是函数里现成的量），
/// 不需要改 `draw_content`、不需要复制这份布局/绘制逻辑本身。现在不预先添加
/// 这个变体，是因为一个从未被构造过的枚举变体会被 `cargo clippy` 标为
/// `dead_code`——与"零警告"的验收口径冲突；扩展成本仍然只是"加一个变体 + 一条
/// 匹配分支"，不是重写。
#[derive(Clone, Copy)]
enum WatermarkAnchor {
    BottomLeft { margin_left_px: f32, margin_bottom_px: f32 },
}

/// 一份水印预设（规格 §8.6）：完全描述「画什么、多大、什么颜色、放哪」，
/// 不含任何 `content` 专属的写死逻辑。
///
/// **I1 修复的验收口径**：Task 6 加一份 `cover_watermark_preset()`（新的
/// `WatermarkPreset` 值，含 32px 图标、`[23,23,23,102]` 深色、`·` 分段
/// 0.75 倍率、`Centered` 锚点），在 `Painter::new()` 里多调用一次
/// `prepare_watermark(&mut renderer, &cover_watermark_preset())?` 存进
/// `Painter` 的新字段，就能画出 cover 水印——不需要改 `draw_content`，
/// 也不需要复制 `layout_and_draw_watermark`/`draw_watermark` 里的任何绘制代码。
struct WatermarkPreset {
    icon_size_px: u32,
    icon_gap_px: f32,
    font_size_px: f32,
    color: [u8; 4],
    /// 字距，像素（`content` = `24 * 0.01em = 0.24px`，由调用方从 em 换算好再填入）。
    letter_spacing_px: f32,
    /// 文字分段：`(文本, 该段颜色 alpha 的倍率)`。`content` 只有一段、倍率 1.0；
    /// `cover` 会用它给中间的 `·` 单独一段 0.75 倍率。
    segments: Vec<(String, f32)>,
    anchor: WatermarkAnchor,
}

fn content_watermark_preset() -> WatermarkPreset {
    WatermarkPreset {
        icon_size_px: WATERMARK_ICON_SIZE_PX,
        icon_gap_px: WATERMARK_ICON_GAP_PX,
        font_size_px: WATERMARK_FONT_SIZE_PX,
        color: WATERMARK_COLOR,
        letter_spacing_px: WATERMARK_FONT_SIZE_PX * WATERMARK_LETTER_SPACING_EM,
        segments: vec![(WATERMARK_TEXT.to_string(), 1.0)],
        anchor: WatermarkAnchor::BottomLeft {
            margin_left_px: WATERMARK_MARGIN_LEFT_PX,
            margin_bottom_px: WATERMARK_MARGIN_BOTTOM_PX,
        },
    }
}

/// 按预设把水印（图标 + 分段文字）画到 `pixmap` 上。**唯一一份水印绘制逻辑**：
/// `content`/`cover`/`outro` 的区别只在传入的 `WatermarkPreset` 与 `icon`
/// （尺寸、颜色都由调用方按预设准备好），本函数不认得任何具体预设的名字。
fn layout_and_draw_watermark(
    renderer: &mut TextRenderer,
    pixmap: &mut Pixmap,
    icon: &Pixmap,
    preset: &WatermarkPreset,
) {
    let style = TextStyle {
        size_px: preset.font_size_px,
        color: preset.color,
        stroke: None,
        letter_spacing_px: preset.letter_spacing_px,
        max_width_px: WATERMARK_MAX_WIDTH_PX,
        line_height: DEFAULT_LINE_HEIGHT,
        bold: false,
    };

    let seg_widths: Vec<f32> =
        preset.segments.iter().map(|(text, _)| renderer.measure(text, &style).0).collect();
    // 同一行内所有分段共享字号/行高，高度只取决于 style，与内容无关
    // （见 `TextRenderer::measure` 文档），量第一段即可。
    let text_h = preset
        .segments
        .first()
        .map(|(text, _)| renderer.measure(text, &style).1)
        .unwrap_or(0.0);

    let icon_size = preset.icon_size_px as f32;
    let row_height = text_h.max(icon_size);

    let (row_left, row_center_y) = match preset.anchor {
        WatermarkAnchor::BottomLeft { margin_left_px, margin_bottom_px } => {
            let row_bottom = CANVAS_H - margin_bottom_px;
            (margin_left_px, row_bottom - row_height / 2.0)
        }
    };

    let icon_left = row_left;
    let icon_top = row_center_y - icon_size / 2.0;
    pixmap.draw_pixmap(
        icon_left.round() as i32,
        icon_top.round() as i32,
        icon.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );

    let mut cursor_x = row_left + icon_size + preset.icon_gap_px;
    for (seg, &w) in preset.segments.iter().zip(seg_widths.iter()) {
        let (text, opacity_mul) = seg;
        if !text.is_empty() {
            let mut seg_style = style.clone();
            seg_style.color[3] =
                (f32::from(style.color[3]) * *opacity_mul).round().clamp(0.0, 255.0) as u8;
            let center_x = cursor_x + w / 2.0;
            renderer.draw_centered(pixmap, text, center_x, row_center_y, &seg_style, 1.0, 1.0);
        }
        cursor_x += w;
    }
}

/// 某个 `WatermarkPreset` 预渲染出的一张小图（连图标带文字，已裁剪到恰好包住
/// 墨迹 + 4px 安全边距）与它在画布上的贴图坐标（整数像素，`draw_pixmap` 要求）。
///
/// **I2 修复**：静态水印此前每帧都要重新排版（`measure` 一次、`draw_centered`
/// 内部再 shape 一次）+ 分配并清空一张 1280×720 暂存画布 + 一次全画布合成；
/// 实测只画水印的帧要 3.54ms。现在 `Painter::new()` 只做一次
/// `prepare_watermark`，每帧只是一次 `draw_pixmap`（整体 `opacity` 用
/// `PixmapPaint` 施加，供 Task 7 的片尾淡出复用）。
struct PreparedWatermark {
    pixmap: Pixmap,
    origin_x: i32,
    origin_y: i32,
}

/// 把预设渲染到一张与画布同尺寸的临时透明画布上（`layout_and_draw_watermark`
/// 用的坐标系本就是画布坐标系，不需要额外换算），裁剪出恰好包住墨迹的最小矩形
/// （四边各留 4px 安全边距，避免抗锯齿边缘被裁掉），返回可直接贴图的小 `Pixmap`
/// + 贴图坐标。**这一步只在 `Painter::new()` 里跑一次。**
///
/// 因为绘制坐标系与裁剪前完全一致（都是画布坐标），裁剪只是"扣掉四周的透明像素"，
/// 不改变任何有墨迹像素的颜色/alpha 值——贴图后的像素结果与"每帧直接在画布上画一遍"
/// 逐字节相同（I2 要求的等价性，报告里有实测比对）。
fn prepare_watermark(
    renderer: &mut TextRenderer,
    preset: &WatermarkPreset,
) -> anyhow::Result<PreparedWatermark> {
    let (icon_raw, iw, ih) = github_mark_rgba(preset.icon_size_px)?;
    let icon = tint_icon(&icon_raw, iw, ih, preset.color);

    let mut scratch = Pixmap::new(CANVAS_W as u32, CANVAS_H as u32)
        .context("水印预渲染暂存画布分配失败")?;
    layout_and_draw_watermark(renderer, &mut scratch, &icon, preset);

    let Some((x0, y0, x1, y1)) = non_transparent_bbox(&scratch) else {
        // 预设没有画出任何东西（理论上不会发生，防御性兜底）：1x1 透明占位，
        // 贴图时等于什么都不画。
        let empty = Pixmap::new(1, 1).context("占位画布分配失败")?;
        return Ok(PreparedWatermark { pixmap: empty, origin_x: 0, origin_y: 0 });
    };

    const PAD: i32 = 4;
    let cw = scratch.width() as i32;
    let ch = scratch.height() as i32;
    let cx0 = (x0 as i32 - PAD).max(0);
    let cy0 = (y0 as i32 - PAD).max(0);
    let cx1 = (x1 as i32 + PAD).min(cw - 1);
    let cy1 = (y1 as i32 + PAD).min(ch - 1);
    let crop_w = (cx1 - cx0 + 1) as u32;
    let crop_h = (cy1 - cy0 + 1) as u32;

    let mut cropped = Pixmap::new(crop_w, crop_h).context("水印裁剪画布分配失败")?;
    cropped.draw_pixmap(
        -cx0,
        -cy0,
        scratch.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );

    Ok(PreparedWatermark { pixmap: cropped, origin_x: cx0, origin_y: cy0 })
}

/// 把预渲染好的水印贴到目标画布上。`opacity` 是整体淡出用的组透明度
/// （Task 5 恒传 1.0；Task 7 的片尾淡出会传 <1.0 的值）。不依赖 `Painter`
/// 的任何字段，写成自由函数而非 `impl Painter` 方法；如需保留方法形态，
/// 加回 `&self` 首参数即可，不影响调用点。
fn draw_watermark(pixmap: &mut Pixmap, watermark: &PreparedWatermark, opacity: f32) {
    pixmap.draw_pixmap(
        watermark.origin_x,
        watermark.origin_y,
        watermark.pixmap.as_ref(),
        &PixmapPaint { opacity: opacity.clamp(0.0, 1.0), ..Default::default() },
        Transform::identity(),
        None,
    );
}

/// Content 段绘制器：字幕层 + 左下角水印。
///
/// `TextRenderer::new()`（加载 5.6MB 内嵌字体）与水印的预渲染（含 GitHub 图标
/// 的 SVG 光栅化）都有可观开销，因此构造一次、每帧复用，不在 `draw_content`
/// 里现造。
pub struct Painter {
    renderer: TextRenderer,
    /// `content` 预设的水印，`new()` 里预渲染一次（I1/I2 修复）。
    content_watermark: PreparedWatermark,
}

impl Painter {
    pub fn new() -> anyhow::Result<Self> {
        let mut renderer = TextRenderer::new()?;
        let content_watermark = prepare_watermark(&mut renderer, &content_watermark_preset())?;
        Ok(Self { renderer, content_watermark })
    }

    /// 绘制 Content 段一帧：完全透明底 + 当前字幕（若有）+ 左下角水印。
    /// `local_frame` 是该段内的相对帧号（规格 §8.2 的段内帧语义），
    /// `pixmap` 由调用方创建/清空，本函数不画任何背景。
    pub fn draw_content(&mut self, pixmap: &mut Pixmap, local_frame: u32, captions: &[Caption]) {
        let t_ms = local_frame as f64 / FPS * 1000.0;
        let current = captions
            .iter()
            .find(|c| (c.start_ms as f64) <= t_ms && t_ms < c.end_ms as f64);
        if let Some(caption) = current {
            self.draw_caption(pixmap, local_frame, caption);
        }
        draw_watermark(pixmap, &self.content_watermark, 1.0);
    }

    /// 绘制当前字幕，含入场动画（scale / opacity / translate_x / letter_spacing）。
    fn draw_caption(&mut self, pixmap: &mut Pixmap, local_frame: u32, caption: &Caption) {
        let start_frame = caption.start_ms as f64 * FPS / 1000.0;
        let elapsed_frames = local_frame as f64 - start_frame;

        let duration_ms = (caption.end_ms - caption.start_ms) as f64;
        let entrance_ms = (duration_ms * ENTRANCE_DURATION_RATIO).min(ENTRANCE_MAX_MS);
        let entrance_frames = entrance_ms * FPS / 1000.0;

        let p = spring(elapsed_frames, FPS, entrance_frames, 0.0);
        let scale = interpolate(p, [0.0, 1.0], ENTRANCE_SCALE_RANGE) as f32;
        let opacity = interpolate(p, [0.0, 1.0], ENTRANCE_OPACITY_RANGE) as f32;
        let translate_x = interpolate(p, [0.0, 1.0], ENTRANCE_TRANSLATE_X_RANGE) as f32;
        let letter_spacing = interpolate(p, [0.0, 1.0], ENTRANCE_LETTER_SPACING_RANGE) as f32;

        // M1 修复：先 trim 再决定字号/排版/绘制——见 `trim_caption_text` 文档注释。
        let text = trim_caption_text(&caption.text);

        let non_whitespace_chars = text.chars().filter(|c| !c.is_whitespace()).count();
        let size_px = if non_whitespace_chars > CAPTION_LONG_CHAR_THRESHOLD {
            CAPTION_FONT_SIZE_LONG
        } else {
            CAPTION_FONT_SIZE_SHORT
        };

        let style = TextStyle {
            size_px,
            color: [255, 255, 255, 255],
            stroke: Some(([0, 0, 0, 255], CAPTION_STROKE_WIDTH_PX)),
            letter_spacing_px: letter_spacing,
            max_width_px: CAPTION_MAX_WIDTH_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: true,
        };

        self.renderer.draw_centered(
            pixmap,
            &text,
            CAPTION_CENTER_X + translate_x,
            CAPTION_CENTER_Y,
            &style,
            opacity,
            scale,
        );
    }
}

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

    // ------------------------------------------------------------------
    // 修复轮 1（I3）：以下测试原有 6 条一字未改，全部是新追加的。
    // 目的：把"assert_ne! 就算过"换成有区分度的断言，锁死 Tasks 6/7 会复制的
    // 通道形状（scale/translate_x/opacity/letter_spacing 各自的方向与量级、
    // 字号阈值按去空白计数、描边未被去掉、字幕选取的半开区间边界、水印的精确
    // 几何与 alpha）。共用 helper 见下方 `bbox_excluding_watermark` /
    // `max_alpha_excluding_watermark`（避开水印区域，只看字幕能出现的那块画布，
    // 写法参照 `src/render/text.rs` 的 `non_transparent_bbox`）。
    // ------------------------------------------------------------------

    /// 字幕不可能画到 y>=600（多行也在此之内），水印固定在 y>=636 附近；
    /// 用这条线隔开两者，避免水印的固定墨迹干扰字幕相关的断言。
    const CAPTION_SCAN_Y_MAX: u32 = 600;

    fn bbox_excluding_watermark(p: &Pixmap, y_max: u32) -> Option<(u32, u32, u32, u32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..y_max.min(p.height()) {
            for x in 0..p.width() {
                if p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0 != u32::MAX).then_some((x0, y0, x1, y1))
    }

    fn max_alpha_excluding_watermark(p: &Pixmap, y_max: u32) -> u8 {
        let mut max_alpha = 0u8;
        for y in 0..y_max.min(p.height()) {
            for x in 0..p.width() {
                if let Some(c) = p.pixel(x, y) {
                    max_alpha = max_alpha.max(c.alpha());
                }
            }
        }
        max_alpha
    }

    fn caps_varied_length() -> Vec<Caption> {
        vec![
            Caption { text: "短。".into(), start_ms: 0, end_ms: 2000 },
            Caption {
                text: "这是一条长得多的字幕文本用于对比宽度。".into(),
                start_ms: 2000,
                end_ms: 5000,
            },
        ]
    }

    #[test]
    fn translate_x_shifts_caption_center_by_about_70px_from_frame_1_to_15() {
        let mut painter = Painter::new().unwrap();
        let mut f1 = Pixmap::new(1280, 720).unwrap();
        let mut f15 = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut f1, 1, &caps());
        painter.draw_content(&mut f15, 15, &caps());
        let b1 = bbox_excluding_watermark(&f1, CAPTION_SCAN_Y_MAX).expect("f=1 应有墨迹");
        let b15 = bbox_excluding_watermark(&f15, CAPTION_SCAN_Y_MAX).expect("f=15 应有墨迹");
        let cx1 = (b1.0 + b1.2) as f32 / 2.0;
        let cx15 = (b15.0 + b15.2) as f32 / 2.0;
        let delta = cx1 - cx15;
        assert!((delta - 70.0).abs() <= 8.0, "translate_x 引起的中心位移应 ≈70px±8，实得 {delta}");
    }

    #[test]
    fn scale_shrinks_caption_height_ratio_by_about_1_14_from_frame_1_to_15() {
        let mut painter = Painter::new().unwrap();
        let mut f1 = Pixmap::new(1280, 720).unwrap();
        let mut f15 = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut f1, 1, &caps());
        painter.draw_content(&mut f15, 15, &caps());
        let b1 = bbox_excluding_watermark(&f1, CAPTION_SCAN_Y_MAX).expect("f=1 应有墨迹");
        let b15 = bbox_excluding_watermark(&f15, CAPTION_SCAN_Y_MAX).expect("f=15 应有墨迹");
        let h1 = (b1.3 - b1.1) as f32;
        let h15 = (b15.3 - b15.1) as f32;
        let ratio = h1 / h15;
        assert!((ratio - 1.14).abs() <= 0.03, "scale 引起的高度比应 ≈1.14±0.03，实得 {ratio}");
    }

    /// **实测记录（步长 1 时的原始数据，见报告）**：u8 量化会在临近饱和处让相邻帧打平
    /// ——frame 13/14 的 maxAlpha 都四舍五入到 254（连续值分别是 0.994837/0.997870，
    /// 差值 < 1/255）。这不是实现 bug，是 8-bit alpha 精度的物理极限；换成隔帧
    /// （步长 2）比较后整条曲线在全程 1..15 上严格递增，仍然足以钉住"曲线在爬升、
    /// 不是恒定值/提前封顶/整体反向"这条要害。
    #[test]
    fn opacity_increases_and_saturates_at_255_by_frame_15() {
        let mut painter = Painter::new().unwrap();
        let mut alphas = [0u8; 15];
        let mut prev = 0u8;
        for f in 1..=15u32 {
            let mut p = Pixmap::new(1280, 720).unwrap();
            painter.draw_content(&mut p, f, &caps());
            let alpha = max_alpha_excluding_watermark(&p, CAPTION_SCAN_Y_MAX);
            assert!(alpha >= prev, "frame {f} 的 maxAlpha 不应比上一帧小：{prev} -> {alpha}");
            alphas[(f - 1) as usize] = alpha;
            prev = alpha;
        }
        assert_eq!(prev, 255, "frame 15 应达到 maxAlpha=255");

        let get = |f: u32| alphas[(f - 1) as usize];
        for f in (1..=13u32).step_by(2) {
            assert!(
                get(f) < get(f + 2),
                "frame {f} 与 frame {} 应严格递增：{} -> {}",
                f + 2,
                get(f),
                get(f + 2)
            );
        }
    }

    /// 按"该帧自身 maxAlpha 的一半"取包围盒，而不是绝对 `alpha>0`——
    /// 度量出的墨迹宽度因此不受组透明度（`opacity` 通道）整体缩放 alpha 的影响。
    /// **这不是可有可无的严谨性**：`letter_spacing` 测试要在 f=2（opacity≈0.51）
    /// 与 f=14（opacity≈0.998）之间比较宽度；用绝对阈值 `alpha>0` 时，opacity
    /// 越低、抗锯齿边缘像素越容易被判成"无墨"而被排除在包围盒外，这个和
    /// letter_spacing 无关的效应恰好与 letter_spacing 收窄的方向相同，
    /// 实测会掩盖掉"letter_spacing 被强制改成 0"这个真实的破坏
    /// （变异验证记录见报告：换成绝对阈值后那次破坏测试仍然通过）。
    fn bbox_at_relative_alpha(p: &Pixmap, y_max: u32, frac: f32) -> Option<(u32, u32, u32, u32)> {
        let y_max = y_max.min(p.height());
        let mut max_alpha = 0u8;
        for y in 0..y_max {
            for x in 0..p.width() {
                if let Some(c) = p.pixel(x, y) {
                    max_alpha = max_alpha.max(c.alpha());
                }
            }
        }
        if max_alpha == 0 {
            return None;
        }
        let threshold = ((max_alpha as f32) * frac).round() as u8;
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..y_max {
            for x in 0..p.width() {
                if p.pixel(x, y).map(|c| c.alpha() >= threshold).unwrap_or(false) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0 != u32::MAX).then_some((x0, y0, x1, y1))
    }

    #[test]
    fn letter_spacing_narrows_ink_width_after_normalizing_out_scale() {
        let mut painter = Painter::new().unwrap();
        let mut f2 = Pixmap::new(1280, 720).unwrap();
        let mut f14 = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut f2, 2, &caps());
        painter.draw_content(&mut f14, 14, &caps());
        let b2 = bbox_at_relative_alpha(&f2, CAPTION_SCAN_Y_MAX, 0.5).expect("f=2 应有墨迹");
        let b14 = bbox_at_relative_alpha(&f14, CAPTION_SCAN_Y_MAX, 0.5).expect("f=14 应有墨迹");
        let w2 = (b2.2 - b2.0) as f32;
        let w14 = (b14.2 - b14.0) as f32;

        // 用 anim::spring/interpolate 独立算出各自的 scale，把它的影响除掉，
        // 剩下的宽度变化只能来自 letter_spacing。
        let p2 = spring(2.0, FPS, 15.0, 0.0);
        let p14 = spring(14.0, FPS, 15.0, 0.0);
        let scale2 = interpolate(p2, [0.0, 1.0], ENTRANCE_SCALE_RANGE) as f32;
        let scale14 = interpolate(p14, [0.0, 1.0], ENTRANCE_SCALE_RANGE) as f32;
        let norm2 = w2 / scale2;
        let norm14 = w14 / scale14;
        // 用一个像素级门槛（而不是裸的 `>`）：实测真实实现的差值 ≈19.5px
        // （norm2≈454.3 norm14≈434.8），而"letter_spacing 恒 0"的破坏版本残留差值
        // 只有 ≈0.4px（bbox 量化 + scale 不精确复原的噪声）。裸 `>` 会被这点噪声
        // 蒙混过关（变异验证记录见报告），门槛设在 5px，远高于噪声、远低于真实信号。
        const MIN_NARROWING_PX: f32 = 5.0;
        assert!(
            norm2 - norm14 > MIN_NARROWING_PX,
            "去除 scale 影响后墨宽应随帧显著收窄（字距变小 >{MIN_NARROWING_PX}px）：\
             norm(f=2)={norm2} norm(f=14)={norm14}"
        );
    }

    /// 取"第一行墨迹的行高"而不是整块 bbox 高度：50/51 字在各自字号下都会因为
    /// `max_width_px=1024` 换行成好几行（80px 下 50 字 ≈5 行、52px 下 51 字 ≈3 行），
    /// 整块高度比会被行数差异污染（实测 ≈0.386，不是字号比）。单行墨迹高度只取决于
    /// `size_px`（`TextRenderer::measure` 文档：行高由 `Metrics` 直接给定，与字形/
    /// 字符数无关），才是干净的字号信号。
    fn first_ink_row_height(p: &Pixmap, y_max: u32) -> Option<u32> {
        let y_max = y_max.min(p.height());
        let mut row_has_ink = vec![false; y_max as usize];
        for y in 0..y_max {
            for x in 0..p.width() {
                if p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false) {
                    row_has_ink[y as usize] = true;
                    break;
                }
            }
        }
        let mut start = None;
        for (y, &has) in row_has_ink.iter().enumerate() {
            match (has, start) {
                (true, None) => start = Some(y as u32),
                (false, Some(s0)) => return Some(y as u32 - 1 - s0 + 1),
                _ => {}
            }
        }
        start.map(|s0| y_max - 1 - s0 + 1)
    }

    #[test]
    fn long_caption_font_height_ratio_matches_52_over_80() {
        let mut painter = Painter::new().unwrap();
        // 10 字在 80px 下单行即可容纳（10*80=800px < 1024px），干净的单行基准。
        let short_text = "长".repeat(10);
        // 51 非空白字符：超过 50 阈值 → 52px，会换行，取第一行做同样干净的基准。
        let long_text = "长".repeat(51);
        let short = vec![Caption { text: short_text, start_ms: 0, end_ms: 5000 }];
        let long = vec![Caption { text: long_text, start_ms: 0, end_ms: 5000 }];
        let mut a = Pixmap::new(1280, 720).unwrap();
        let mut b = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut a, 20, &short);
        painter.draw_content(&mut b, 20, &long);
        let ha = first_ink_row_height(&a, CAPTION_SCAN_Y_MAX).expect("80px 基准应有墨迹") as f32;
        let hb = first_ink_row_height(&b, CAPTION_SCAN_Y_MAX).expect("52px 基准应有墨迹") as f32;
        let ratio = hb / ha;
        assert!(
            (ratio - 52.0 / 80.0).abs() <= 0.05,
            "单行墨高比应 ≈52/80±0.05，实得 {ratio}（80px 行高={ha} 52px 行高={hb}）"
        );
    }

    #[test]
    fn font_size_threshold_ignores_whitespace_padding() {
        // 空格放在文本中间而不是纯末尾：M1 修复后末尾空白会被 `trim_caption_text`
        // 整个删掉，"3 字 + 60 个尾随空格" 在 trim 之后就等于"3 字"本身，
        // 测不出"字号阈值是否按去空白计数"这件事——trim 已经把差异抹平了，
        // 这条断言即便字号阈值改成按原始长度计数也照样通过（实测验证过）。
        // 中间空格不受 `trim`（只掐头去尾）影响，trim 之后仍保留 3 个非空白 + 60
        // 个空白共 63 字符，才是对字号阈值的干净测试。
        //
        // 用"长"而不是"三"：CJK 字形不保证每行像素都连续——"三"字本身是三条
        // 有间隙的横线，同一行内会被误判成好几个"墨迹段"，第一段量出来的只是
        // 顶上那一横的高度，不是整行高度（实测：84 vs 18，见变异验证记录）。
        // "长"已经在 `long_caption_font_height_ratio_matches_52_over_80` 里验证过
        // 单行内不会有这种内部断层。
        let mut painter = Painter::new().unwrap();
        let base = vec![Caption { text: "长长长".into(), start_ms: 0, end_ms: 5000 }];
        let padded_text = format!("长{}长长", " ".repeat(60));
        let padded = vec![Caption { text: padded_text, start_ms: 0, end_ms: 5000 }];
        let mut a = Pixmap::new(1280, 720).unwrap();
        let mut b = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut a, 20, &base);
        painter.draw_content(&mut b, 20, &padded);
        // 中间 60 个空格会在 80px 下把行撑到远超 max_width（换行），两边整块
        // bbox 高度比不干净（行数不同）；改用"第一行墨迹行高"，这个值只取决于
        // 字号本身，见 `first_ink_row_height` 文档。
        let ha = first_ink_row_height(&a, CAPTION_SCAN_Y_MAX).expect("基准应有墨迹");
        let hb = first_ink_row_height(&b, CAPTION_SCAN_Y_MAX).expect("带中间空白应有墨迹");
        assert_eq!(
            ha, hb,
            "3 个非空白字符 + 60 个空格（共 63 字符）：字号阈值应按去空白后的字符数判定\
             （3 <= 50 → 80px），行高应与「长长长」基准一致：{ha} vs {hb}"
        );
    }

    #[test]
    fn caption_shows_both_stroke_and_fill_colors() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut p, 20, &caps());
        let (mut has_white, mut has_black) = (false, false);
        for y in 0..CAPTION_SCAN_Y_MAX {
            for x in 0..p.width() {
                if let Some(c) = p.pixel(x, y)
                    && c.alpha() > 200
                {
                    let (r, g, b) = (c.red(), c.green(), c.blue());
                    if r > 240 && g > 240 && b > 240 {
                        has_white = true;
                    }
                    if r < 30 && g < 30 && b < 30 {
                        has_black = true;
                    }
                }
            }
        }
        assert!(has_white, "字幕区应有白色填充");
        assert!(has_black, "字幕区应有黑色描边（6px 黑描边不应被去掉）");
    }

    #[test]
    fn caption_start_end_boundary_is_half_open() {
        let mut painter = Painter::new().unwrap();
        let caps = caps_varied_length();
        let mut f59 = Pixmap::new(1280, 720).unwrap();
        let mut f60 = Pixmap::new(1280, 720).unwrap();
        let mut f61 = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut f59, 59, &caps);
        painter.draw_content(&mut f60, 60, &caps);
        painter.draw_content(&mut f61, 61, &caps);

        let b59 = bbox_excluding_watermark(&f59, CAPTION_SCAN_Y_MAX);
        let b61 = bbox_excluding_watermark(&f61, CAPTION_SCAN_Y_MAX);
        assert!(b59.is_some(), "f=59（第一条字幕内）应有墨迹");
        assert!(b61.is_some(), "f=61（第二条字幕内）应有墨迹");
        let w59 = b59.unwrap().2 - b59.unwrap().0;
        let w61 = b61.unwrap().2 - b61.unwrap().0;
        assert_ne!(w59, w61, "两条长度不同的字幕，墨宽应不同：f=59 宽={w59} f=61 宽={w61}");

        assert!(
            bbox_excluding_watermark(&f60, CAPTION_SCAN_Y_MAX).is_none(),
            "f=60（t=2000ms 恰好是边界）：第一条已按 t<end 结束，第二条按 t>=start 刚开始尚无 \
             墨迹（spring(0)=0），字幕区不应有任何墨迹"
        );
    }

    #[test]
    fn watermark_ink_geometry_and_alpha_are_exact() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut p, 300, &caps()); // 无字幕，只剩水印
        let (x0, _y0, _x1, y1) = non_transparent_bbox(&p).expect("水印应有墨迹");
        assert_eq!(x0, 40, "水印左边缘应精确贴 x=40");
        assert!((678..=680).contains(&y1), "水印底边缘应在 y∈[678,680]，实得 {y1}");

        let mut max_alpha = 0u8;
        for y in 0..p.height() {
            for x in 0..p.width() {
                if let Some(c) = p.pixel(x, y) {
                    max_alpha = max_alpha.max(c.alpha());
                }
            }
        }
        assert_eq!(max_alpha, 69, "水印 maxAlpha 应精确等于 69（未被叠厚）");

        let icon_has_ink = (40..67)
            .any(|x| (652..679).any(|y| p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false)));
        assert!(icon_has_ink, "(40,652)-(67,679) 图标框内应有墨迹（GitHub 图标真的画了）");
    }

    #[test]
    fn trailing_spaces_in_caption_do_not_shift_rendering() {
        let mut painter = Painter::new().unwrap();
        let base = vec![Caption { text: "文字".into(), start_ms: 0, end_ms: 5000 }];
        let padded = vec![Caption { text: "文字   ".into(), start_ms: 0, end_ms: 5000 }];
        let mut a = Pixmap::new(1280, 720).unwrap();
        let mut b = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut a, 20, &base);
        painter.draw_content(&mut b, 20, &padded);
        assert_eq!(a.data(), b.data(), "「文字」与「文字   」渲染结果应当一致（M1：尾随空格不应影响居中）");
    }
}
