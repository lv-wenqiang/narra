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
use tiny_skia::{Color, IntSize, Pixmap, PixmapPaint, Transform};

use crate::assets::{github_mark_rgba, logo_rgba};
use crate::render::anim::{interpolate, interpolate3, spring};
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

/// `cover` 段水印（规格 §8.6，Task 6）：`rgba(23,23,23,0.4)`，垂直中心见
/// `cover_watermark_preset` 文档注释（`576`，不是规格字面的 `432`）。
const COVER_WATERMARK_CENTER_Y_PX: f32 = 576.0;
const COVER_WATERMARK_FONT_SIZE_PX: f32 = 28.0;
const COVER_WATERMARK_COLOR: [u8; 4] = [23, 23, 23, 102];
const COVER_WATERMARK_ICON_SIZE_PX: u32 = 32;
const COVER_WATERMARK_ICON_GAP_PX: f32 = 12.0;
const COVER_WATERMARK_TEXT_MAIN: &str = "Panda Video Generator";
const COVER_WATERMARK_TEXT_SEP: &str = " · ";
const COVER_WATERMARK_TEXT_SUFFIX: &str = "熊猫视频自动化引擎";
const COVER_WATERMARK_SEP_OPACITY_MUL: f32 = 0.75;

/// Cover 居中容器（规格 §8.4「Cover」小节 + 协调者交接的精确排版）：
/// 宽度 80% = 1024px，水平居中，整个容器（上排 + 主标题）垂直居中于 y=360。
const COVER_CONTAINER_WIDTH_PX: f32 = CANVAS_W * 0.8;
const COVER_CONTAINER_LEFT_PX: f32 = (CANVAS_W - COVER_CONTAINER_WIDTH_PX) / 2.0;
const COVER_CONTAINER_CENTER_Y: f32 = 360.0;

/// 上排（logo + 「熊猫智研社」）：左对齐（不是居中），左偏移 40px，
/// 整体不透明度 0.30。
const COVER_ROW_MARGIN_LEFT_PX: f32 = 40.0;
const COVER_ROW_LEFT_PX: f32 = COVER_CONTAINER_LEFT_PX + COVER_ROW_MARGIN_LEFT_PX;
const COVER_LOGO_SIZE_PX: u32 = 36;
const COVER_LOGO_MARGIN_PX: f32 = 8.0;
/// 上排行高：logo 尺寸 + 四周 margin（`36 + 8*2 = 52`）。
const COVER_ROW_HEIGHT_PX: f32 = COVER_LOGO_SIZE_PX as f32 + COVER_LOGO_MARGIN_PX * 2.0;
/// 「熊猫智研社」左边缘：`row_left + logo_margin + logo_size + logo_margin`。
const COVER_ROW_TEXT_LEFT_PX: f32 =
    COVER_ROW_LEFT_PX + COVER_LOGO_MARGIN_PX + COVER_LOGO_SIZE_PX as f32 + COVER_LOGO_MARGIN_PX;
const COVER_ROW_TEXT_FONT_SIZE_PX: f32 = 38.0;
const COVER_ROW_TEXT: &str = "熊猫智研社";
const COVER_ROW_OPACITY: f32 = 0.30;
/// 上排文字不会换行，给一个远大于画布宽度的值以避免意外换行。
const COVER_ROW_TEXT_MAX_WIDTH_PX: f32 = 2000.0;

/// 主标题：100px 粗体，左右 padding 40px（`max_width = 1024 - 80 = 944`），
/// 水平居中于 x=640。
const COVER_TITLE_FONT_SIZE_PX: f32 = 100.0;
const COVER_TITLE_PADDING_PX: f32 = 40.0;
const COVER_TITLE_MAX_WIDTH_PX: f32 = COVER_CONTAINER_WIDTH_PX - COVER_TITLE_PADDING_PX * 2.0;
const COVER_TITLE_CENTER_X: f32 = CANVAS_W / 2.0;

/// Cover 主标题、Cover 上排「熊猫智研社」、Intro 标题一律用黑色、无描边
/// （协调者裁定：TS 原版这三处都没指定 `color`，浏览器在白底上按默认色渲染
/// 即黑色）。
const TITLE_COLOR_BLACK: [u8; 4] = [0, 0, 0, 255];

/// Intro（打字机片头，规格 §8.4「Intro」小节）：标题 70px 粗体，居中，
/// 宽度 80%、左右 padding 40px（与 Cover 主标题同一套换算，`max_width=944`），
/// 垂直居中于画布中心。
const INTRO_TITLE_FONT_SIZE_PX: f32 = 70.0;
const INTRO_TITLE_MAX_WIDTH_PX: f32 = CANVAS_W * 0.8 - 80.0;
const INTRO_TITLE_CENTER_X: f32 = CANVAS_W / 2.0;
const INTRO_TITLE_CENTER_Y: f32 = CANVAS_H / 2.0;
/// 打字机 2 秒内打完（brief 数值，逐字照用）。
const INTRO_TYPEWRITER_SECONDS: f64 = 2.0;
/// 光标只在打字未完成时显示：`local_frame < INTRO_TYPEWRITER_SECONDS * FPS`。
/// **修复轮 1（M3）**：从 `秒数 * FPS` 推导而不是硬编码 `60`——改
/// `INTRO_TYPEWRITER_SECONDS` 或 `FPS` 时这个截止帧会自动跟着走，数值本身
/// 不变（`2.0*30.0=60.0`）。
const INTRO_TYPEWRITER_FRAMES: u32 = (INTRO_TYPEWRITER_SECONDS * FPS) as u32;
/// 光标 2 次/秒闪烁：一个完整的「亮→暗」周期是 `FPS/2` 帧。**修复轮 1
/// （M3）**：从 `FPS` 推导而不是硬编码 `15`，数值不变（`30.0/2.0=15.0`）；
/// `interpolate3` 的三个断点 `[0, 半周期, 整周期]` 在使用处按这个常量算出，
/// 不再是字面量 `[0.0, 7.5, 15.0]`。
const INTRO_CURSOR_BLINK_PERIOD_FRAMES: u32 = (FPS / 2.0) as u32;
const INTRO_CURSOR_TEXT: &str = "|";
const INTRO_CURSOR_GAP_PX: f32 = 4.0;
/// 3.0s -> 3.5s 线性淡出（brief 数值，逐字照用）。
const INTRO_FADE_OUT_RANGE: [f64; 2] = [90.0, 104.0];

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

/// 把一段 straight-alpha RGBA8 像素原地转换为 `tiny_skia::Pixmap` 要求的
/// 预乘 alpha（`Pixmap::from_vec`/`decode_png` 内部都是这么做的，见
/// `tiny_skia::Pixmap::decode_png` 源码）。
///
/// **必须在缩放之前调用，不能在之后**：`image::imageops::resize` 对
/// straight alpha 做线性插值，在半透明边缘会把「透明像素本身携带的（通常
/// 无意义的）RGB 值」按插值权重混进结果颜色里，产生色边；缩放之前先转成
/// 预乘 alpha，插值就是对预乘值做的，等价于合成语义下正确的边缘混合——
/// 转换后的结果可以直接喂给 `Pixmap::from_vec`，不需要再反预乘。
fn premultiply_in_place(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        px[0] = (u32::from(px[0]) * a / 255) as u8;
        px[1] = (u32::from(px[1]) * a / 255) as u8;
        px[2] = (u32::from(px[2]) * a / 255) as u8;
    }
}

/// 把 `assets::logo_rgba()`（实际 2048×2048）用高质量重采样（Lanczos3）缩到
/// `size_px` 正方形，返回可直接 `Pixmap::draw_pixmap` 的预乘 `Pixmap`。
///
/// **只应在 `Painter::new()` 里调用**：对 2048×2048 做 Lanczos3 降采样有
/// 实打实的开销，每帧重做不划算。Task 7 需要 216px 的 outro 版本时，对同一份
/// `logo_rgba()` 结果再调一次本函数、存进 `Painter` 的新字段即可，不需要改
/// 这个函数本身（`size_px` 已经是参数）。
fn scaled_logo(size_px: u32) -> anyhow::Result<Pixmap> {
    let (mut raw, w, h) = logo_rgba()?;
    premultiply_in_place(&mut raw);
    let img = image::RgbaImage::from_raw(w, h, raw)
        .context("logo 像素数据长度与声明的宽高不匹配")?;
    let resized =
        image::imageops::resize(&img, size_px, size_px, image::imageops::FilterType::Lanczos3);
    let size =
        IntSize::from_wh(size_px, size_px).context("logo 目标尺寸非零")?;
    Pixmap::from_vec(resized.into_raw(), size)
        .context("logo 缩放后 Pixmap 构造失败：像素数据长度与声明尺寸不匹配")
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

/// 水印锚点：`content` 用「左下角，给定左/下边距」；`cover`（Task 6）用
/// 「水平居中 + 指定垂直中心」——`center_y_px` 语义见 `cover_watermark_preset`
/// 文档注释（不是 TS `marginTop` 那个字面值，是协调者换算过的居中点）。
#[derive(Clone, Copy)]
enum WatermarkAnchor {
    BottomLeft { margin_left_px: f32, margin_bottom_px: f32 },
    Centered { center_y_px: f32 },
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

/// `cover` 预设（规格 §8.4/§8.6，Task 6）：水平居中、垂直中心 `y=576`。
///
/// **`576` 的由来**（协调者交接，不是规格字面值）：规格写「自顶部偏移
/// 432px」，那是从 TS 版 `marginTop:'432px'`（配合 `inset:0` 的居中 flex
/// 容器）原样搬来的，语义是「在 y=432 以下的剩余区域里垂直居中」，即
/// `432 + (720-432)/2 = 576`，不是把水印中心直接放在 y=432（那样会压进
/// Cover 主标题）。
fn cover_watermark_preset() -> WatermarkPreset {
    WatermarkPreset {
        icon_size_px: COVER_WATERMARK_ICON_SIZE_PX,
        icon_gap_px: COVER_WATERMARK_ICON_GAP_PX,
        font_size_px: COVER_WATERMARK_FONT_SIZE_PX,
        color: COVER_WATERMARK_COLOR,
        letter_spacing_px: 0.0,
        segments: vec![
            (COVER_WATERMARK_TEXT_MAIN.to_string(), 1.0),
            (COVER_WATERMARK_TEXT_SEP.to_string(), COVER_WATERMARK_SEP_OPACITY_MUL),
            (COVER_WATERMARK_TEXT_SUFFIX.to_string(), 1.0),
        ],
        anchor: WatermarkAnchor::Centered { center_y_px: COVER_WATERMARK_CENTER_Y_PX },
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
        WatermarkAnchor::Centered { center_y_px } => {
            let total_width = icon_size + preset.icon_gap_px + seg_widths.iter().sum::<f32>();
            (CANVAS_W / 2.0 - total_width / 2.0, center_y_px)
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
    /// `cover` 预设的水印，`new()` 里预渲染一次（Task 6）。
    cover_watermark: PreparedWatermark,
    /// Cover 上排用的 36px logo，`new()` 里用 Lanczos3 缩好一次缓存起来
    /// （Task 6；Task 7 的 outro 216px 版本会是并列的另一个字段）。
    logo_36: Pixmap,
}

impl Painter {
    pub fn new() -> anyhow::Result<Self> {
        let mut renderer = TextRenderer::new()?;
        let content_watermark = prepare_watermark(&mut renderer, &content_watermark_preset())?;
        let cover_watermark = prepare_watermark(&mut renderer, &cover_watermark_preset())?;
        let logo_36 = scaled_logo(COVER_LOGO_SIZE_PX)?;
        Ok(Self { renderer, content_watermark, cover_watermark, logo_36 })
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

    /// 绘制 Cover 段一帧（规格 §8.4「Cover」小节）：不透明白底 + 左上排
    /// （logo + 「熊猫智研社」，整体 0.30 透明度，左对齐）+ 主标题（100px
    /// 粗体，居中，支持换行）+ `cover` 预设水印。
    ///
    /// 居中容器（宽 1024px）整体垂直居中于 y=360：上排固定 52px 高
    /// （logo 36px + 上下 margin 各 8px），主标题紧随其后，容器总高
    /// = 52 + 主标题排版高度，容器顶 = 360 - 总高/2。
    pub fn draw_cover(&mut self, pixmap: &mut Pixmap, title: &str) {
        pixmap.fill(Color::from_rgba8(255, 255, 255, 255));

        let title_style = TextStyle {
            size_px: COVER_TITLE_FONT_SIZE_PX,
            color: TITLE_COLOR_BLACK,
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: COVER_TITLE_MAX_WIDTH_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: true,
        };
        let (_, title_h) = self.renderer.measure(title, &title_style);

        let container_h = COVER_ROW_HEIGHT_PX + title_h;
        let container_top = COVER_CONTAINER_CENTER_Y - container_h / 2.0;
        let row_center_y = container_top + COVER_ROW_HEIGHT_PX / 2.0;
        let title_center_y = container_top + COVER_ROW_HEIGHT_PX + title_h / 2.0;

        // 上排：logo（左对齐，四周 margin 8px，整体 0.30 透明度）。
        let logo_left = COVER_ROW_LEFT_PX + COVER_LOGO_MARGIN_PX;
        let logo_top = row_center_y - COVER_LOGO_SIZE_PX as f32 / 2.0;
        pixmap.draw_pixmap(
            logo_left.round() as i32,
            logo_top.round() as i32,
            self.logo_36.as_ref(),
            &PixmapPaint { opacity: COVER_ROW_OPACITY, ..Default::default() },
            Transform::identity(),
            None,
        );

        // 上排：「熊猫智研社」（左对齐，与 logo 在这 52px 行内垂直居中，
        // 38px 粗体，整体 0.30 透明度——与 logo 不重叠，逐元素施加等价于
        // 整体施加）。
        let row_text_style = TextStyle {
            size_px: COVER_ROW_TEXT_FONT_SIZE_PX,
            color: TITLE_COLOR_BLACK,
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: COVER_ROW_TEXT_MAX_WIDTH_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: true,
        };
        let (row_text_w, _) = self.renderer.measure(COVER_ROW_TEXT, &row_text_style);
        let row_text_center_x = COVER_ROW_TEXT_LEFT_PX + row_text_w / 2.0;
        self.renderer.draw_centered(
            pixmap,
            COVER_ROW_TEXT,
            row_text_center_x,
            row_center_y,
            &row_text_style,
            COVER_ROW_OPACITY,
            1.0,
        );

        // 主标题：100px 粗体，水平居中，支持换行，不透明。
        self.renderer.draw_centered(
            pixmap,
            title,
            COVER_TITLE_CENTER_X,
            title_center_y,
            &title_style,
            1.0,
            1.0,
        );

        draw_watermark(pixmap, &self.cover_watermark, 1.0);
    }

    /// 绘制 Intro 段一帧（规格 §8.4「Intro」小节）：不透明白底 + 打字机标题
    /// （70px 粗体，居中，支持换行）+ 光标（打字未完成时显示，2 次/秒闪烁）+
    /// 3.0s→3.5s 整体淡出。无水印。
    pub fn draw_intro(&mut self, pixmap: &mut Pixmap, local_frame: u32, title: &str) {
        pixmap.fill(Color::from_rgba8(255, 255, 255, 255));
        // 3.0s -> 3.5s 线性淡出：只作用于文字（含光标），白底始终不透明
        // （brief 提醒 5：`opacity` 挂在 TS 的 `<h1>` 上，外层 `bg-white` 不
        // 参与淡出）。
        let fade_opacity = interpolate(local_frame as f64, INTRO_FADE_OUT_RANGE, [1.0, 0.0]) as f32;
        if fade_opacity <= 0.0 {
            return;
        }

        // 打字机：`visible` 只取决于字符数与帧号，与光标 opacity 无关——
        // 这正是「光标闪烁不导致文字抖动」的关键（brief 提醒 6）：文字排版
        // 里从不包含光标本身，光标是完全独立的第二次绘制。
        let title_chars: Vec<char> = title.chars().collect();
        let total_chars = title_chars.len();
        let chars_per_sec = total_chars as f64 / INTRO_TYPEWRITER_SECONDS;
        let visible =
            ((local_frame as f64 * chars_per_sec / FPS).floor() as usize).min(total_chars);
        let display_text: String = title_chars[..visible].iter().collect();

        let style = TextStyle {
            size_px: INTRO_TITLE_FONT_SIZE_PX,
            color: TITLE_COLOR_BLACK,
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: INTRO_TITLE_MAX_WIDTH_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: true,
        };

        self.renderer.draw_centered(
            pixmap,
            &display_text,
            INTRO_TITLE_CENTER_X,
            INTRO_TITLE_CENTER_Y,
            &style,
            fade_opacity,
            1.0,
        );

        if local_frame < INTRO_TYPEWRITER_FRAMES {
            let period = INTRO_CURSOR_BLINK_PERIOD_FRAMES as f64;
            let blink_opacity = interpolate3(
                (local_frame % INTRO_CURSOR_BLINK_PERIOD_FRAMES) as f64,
                [0.0, period / 2.0, period],
                [1.0, 1.0, 0.0],
            ) as f32;
            let cursor_opacity = fade_opacity * blink_opacity;
            if cursor_opacity > 0.0 {
                let (right_edge_x, last_line_center_y) =
                    self.intro_last_line_anchor(&display_text, &style);
                let (cursor_w, _) = self.renderer.measure(INTRO_CURSOR_TEXT, &style);
                let cursor_center_x = right_edge_x + INTRO_CURSOR_GAP_PX + cursor_w / 2.0;
                self.renderer.draw_centered(
                    pixmap,
                    INTRO_CURSOR_TEXT,
                    cursor_center_x,
                    last_line_center_y,
                    &style,
                    cursor_opacity,
                    1.0,
                );
            }
        }
    }

    /// 定位「当前已显示文字」（`display_text`）最后一行的右边缘 x 与垂直
    /// 中心 y，用于放置打字机光标（brief 提醒 6）。
    ///
    /// **修复轮 1（I1）**：原实现用「对每个字符前缀分别 `measure()`、找高度
    /// 跳变点」推断换行位置，默认换行只发生在字符边界——对纯 CJK 逐字换行
    /// 成立，但英文/数字等 word-wrap 一次性挪到下一行的是整个单词，跳变点会
    /// 落在单词内部，导致最后一行宽度算少、光标被画到单词中间（审查实测：
    /// 混排标题 20/60 帧、纯英文 35/60 帧光标压字，见报告"修复轮 1"的
    /// before/after 对照）。现在改用 `TextRenderer::last_line_metrics`——
    /// 直接读 `shape()`/`layout_runs()` 产出的断行结果，与真实断行算法同源，
    /// 逐字符/整词换行都精确，且只需一次 `shape()`（原来是 O(n²) 次）。
    ///
    /// 垂直位置公式 `center_y + last_top_rel / 2.0` 的推导见
    /// `last_line_metrics` 的文档注释。
    fn intro_last_line_anchor(&mut self, display_text: &str, style: &TextStyle) -> (f32, f32) {
        let Some((last_w, last_top_rel, _last_h)) =
            self.renderer.last_line_metrics(display_text, style)
        else {
            // 没有任何 layout run（理论上只有空文本才会走到这里）：视为一行
            // 零宽度，光标落在标题中心正右侧。
            return (INTRO_TITLE_CENTER_X, INTRO_TITLE_CENTER_Y);
        };
        let right_edge_x = INTRO_TITLE_CENTER_X + last_w / 2.0;
        let last_line_center_y = INTRO_TITLE_CENTER_Y + last_top_rel / 2.0;
        (right_edge_x, last_line_center_y)
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

    /// I3 修复轮 2（开放项 1）：`caps()`/`caps_varied_length()` 里的区间都不重叠，
    /// first-match（`.iter().find(...)`）与 last-match（`.iter().rev().find(...)`）
    /// 在这些既有 fixture 上无法区分——变异验证证实：把 `draw_content` 的选取
    /// 逻辑改成 `.iter().rev().find(...)` 后，round 1 的全部 16 条测试仍然全绿。
    /// 规格 §8.4 明确要求"满足条件的第一条"，这是选取逻辑本身的正确性；VTT 解析
    /// 一旦在边界产生哪怕一帧的重叠，first/last 的选择就会显示错的字幕。
    /// 这里构造一组真正重叠的区间（`[0,3000)` 与 `[1000,2000)`，文本长度不同），
    /// 取 t 落在重叠区（frame=45 → 1500ms）的一帧，断言"两条都在列表里"时的渲染
    /// 结果与"列表里只有第一条"时逐字节相同——这只在选取逻辑真的取第一条匹配时成立。
    fn overlapping_caps() -> Vec<Caption> {
        vec![
            Caption { text: "短句。".into(), start_ms: 0, end_ms: 3000 },
            Caption {
                text: "这是第二条更长一些的重叠字幕文本。".into(),
                start_ms: 1000,
                end_ms: 2000,
            },
        ]
    }

    #[test]
    fn overlapping_captions_pick_the_first_match_in_the_list() {
        let mut painter = Painter::new().unwrap();
        let caps = overlapping_caps();
        let only_first = vec![caps[0].clone()];

        // frame 45 -> 1500ms：同时落在两条区间 [0,3000) 与 [1000,2000) 内。
        let mut with_both = Pixmap::new(1280, 720).unwrap();
        let mut with_first_only = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut with_both, 45, &caps);
        painter.draw_content(&mut with_first_only, 45, &only_first);

        assert_eq!(
            with_both.data(),
            with_first_only.data(),
            "重叠区间内应选取列表中第一条匹配的字幕（规格 §8.4：满足 start<=t<end 的第一条），             渲染结果应与「列表里只有第一条」时逐字节相同"
        );
    }

    #[test]
    fn watermark_ink_geometry_and_alpha_are_exact() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_content(&mut p, 300, &caps()); // 无字幕，只剩水印
        let (x0, _y0, x1, y1) = non_transparent_bbox(&p).expect("水印应有墨迹");
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

        assert!((305..=325).contains(&x1), "水印右边缘应在 x∈[305,325]，实得 {x1}");

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

    // ------------------------------------------------------------------
    // Task 6：Cover / Intro。以下 5 条测试是 brief Step 1 原样照抄，一字未改。
    // ------------------------------------------------------------------

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
        painter.draw_intro(&mut done, 62, title); // 2 秒 = 60 帧后打完

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
        painter.draw_intro(&mut before, 89, title); // 淡出开始前
        painter.draw_intro(&mut last, 104, title); // 淡出终点
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

    // ------------------------------------------------------------------
    // Task 6 追加的鉴别性断言（协调者要求）：brief 那 5 条只能测出很粗的形状，
    // 这里锁死协调者交接里点名的精确数值与几何关系。每条都做过变异验证，
    // 记录见报告。
    //
    // **关键陷阱记录**：Cover/Intro 的画布从头到尾都是不透明白底铺满
    // （`alpha` 恒为 255），不像 Content 段那样以透明为「无墨迹」的信号——
    // 用 `c.alpha() > 0` 判断「有没有墨迹」在这里永远为真，是一个会让断言
    // 静默失去区分度的陷阱（红色阶段的运行记录：好几条新断言用这个判据时
    // 全部因为"整张图都算有墨迹"而给出荒谬的数值，被红色阶段当场抓出）。
    // 这里统一改用 `darkness()`——离纯白的距离（`255 - min(r,g,b)`）——
    // 作为"有没有墨迹/墨迹有多深"的判据，白底恒为 0，合成后的黑字/水印
    // 越深该值越大。
    // ------------------------------------------------------------------

    fn darkness(c: tiny_skia::PremultipliedColorU8) -> u8 {
        255 - c.red().min(c.green()).min(c.blue())
    }

    fn max_darkness_in_rect(p: &Pixmap, x0: u32, x1: u32, y0: u32, y1: u32) -> u8 {
        let mut m = 0u8;
        for y in y0..y1.min(p.height()) {
            for x in x0..x1.min(p.width()) {
                if let Some(c) = p.pixel(x, y) {
                    m = m.max(darkness(c));
                }
            }
        }
        m
    }

    /// 按 `darkness>0`（非纯白）判定的包围盒，`y` 范围可限定，避免扫到无关区域。
    fn ink_bbox_in_y_range(p: &Pixmap, y0: u32, y1: u32) -> Option<(u32, u32, u32, u32)> {
        let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in y0..y1.min(p.height()) {
            for x in 0..p.width() {
                if p.pixel(x, y).map(|c| darkness(c) > 0).unwrap_or(false) {
                    bx0 = bx0.min(x);
                    by0 = by0.min(y);
                    bx1 = bx1.max(x);
                    by1 = by1.max(y);
                }
            }
        }
        (bx0 != u32::MAX).then_some((bx0, by0, bx1, by1))
    }

    /// **修复轮 1（M2）**：从容器几何动态推导上排 / 主标题各自的 y 窗口，
    /// 而不是像修复前那样手算死的像素窗口（`0..335`、`top+1..500`）。
    ///
    /// 审查记录的问题：那种手算窗口只对"当前这套精确排版数值"成立，字号一变
    /// （变异 N2：主标题 100→70）或 logo 尺寸一变（变异 N8：36→48）窗口就
    /// 与实际渲染错位，导致失败信息指向错误的测试——字号变异让上排/主标题
    /// 两条带"串位"，结果是 `cover_top_row_opacity_…` 报错，而不是真正
    /// 应该报错的 `cover_title_font_size_matches_100px`（后者反而因为窗口
    /// 恰好还能凑出一个看似合理的比值而通过）。
    ///
    /// 这里改用与 `draw_cover` 完全相同的公式（同一套常量 + `measure()`）
    /// 重新推导 `container_top`/`row_bottom`/`title_bottom`：任何改动这些
    /// 常量的变异都会被两条窗口"感知到"，继续对齐到正确的区域，而不是死守
    /// 一份过时的像素窗口。窗口本身在几何边界外各留 8px 安全余量，容纳抗
    /// 锯齿边缘。
    fn cover_dynamic_windows(painter: &mut Painter, title: &str) -> (u32, u32, u32, u32) {
        let title_style = TextStyle {
            size_px: COVER_TITLE_FONT_SIZE_PX,
            color: TITLE_COLOR_BLACK,
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: COVER_TITLE_MAX_WIDTH_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: true,
        };
        let (_, title_h) = painter.renderer.measure(title, &title_style);
        let container_h = COVER_ROW_HEIGHT_PX + title_h;
        let container_top = COVER_CONTAINER_CENTER_Y - container_h / 2.0;
        let row_bottom = container_top + COVER_ROW_HEIGHT_PX;
        let title_bottom = row_bottom + title_h;

        const MARGIN: f32 = 8.0;
        (
            (container_top - MARGIN).max(0.0).floor() as u32,
            (row_bottom + MARGIN).ceil() as u32,
            (row_bottom + MARGIN).ceil() as u32,
            (title_bottom + MARGIN).ceil().min(CANVAS_H) as u32,
        )
    }

    /// Cover 上排「熊猫智研社」的整体不透明度应精确为 0.30
    /// （黑字合成到白底：`darkness ≈ round(255*0.30) = 76`），而不是 255
    /// （忘了施加整体透明度）。只扫文字所在的 x 范围（`COVER_ROW_TEXT_LEFT_PX`
    /// 起，用常量而非字面量——避开 logo 颜色未知会污染这个精确数值，同时
    /// logo 尺寸变化时这个常量本身也会跟着动，不会读到过时的边界）。
    #[test]
    fn cover_top_row_opacity_is_about_76_not_opaque() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_cover(&mut p, "标题");
        let (row_y0, row_y1, _, _) = cover_dynamic_windows(&mut painter, "标题");
        let max_darkness =
            max_darkness_in_rect(&p, COVER_ROW_TEXT_LEFT_PX as u32, 1000, row_y0, row_y1);
        assert!(
            (max_darkness as i32 - 76).abs() <= 8,
            "上排文字 darkness 应约为 76（0.30 组透明度合成到白底），实得 {max_darkness}"
        );
        assert_ne!(max_darkness, 255, "上排文字不应是纯黑（未施加整体透明度）");
    }

    /// Cover 应该画出 logo：logo 占据的 36x36 区域内应有非白像素。
    #[test]
    fn cover_draws_a_logo_in_the_top_row() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_cover(&mut p, "标题");
        // logo 左边缘 x=176，尺寸 36px；容器顶 <= 274（见上一条推导），
        // 故 logo 顶 <= 274+8=282，给足够宽的窗口。
        let has_logo_ink = (176..212).any(|x| (0..320).any(|y| p.pixel(x, y).map(|c| darkness(c) > 0).unwrap_or(false)));
        assert!(has_logo_ink, "logo 所在的 36x36 区域内应有非白像素");
    }

    /// **修复轮 1（I2）**：logo 自身的 0.30 组透明度此前没有任何断言
    /// （审查变异 N19：logo 改成 `PixmapPaint::default()`，即不施加 0.30，
    /// 31 条测试一条都不响——`cover_top_row_opacity_…` 的扫描窗口刻意避开了
    /// logo，`cover_draws_a_logo_in_the_top_row` 只判断"非白"、不判断具体
    /// 深浅）。这里对 logo 自身的像素区域做同样精确的 darkness 断言。
    #[test]
    fn cover_logo_opacity_is_about_76_not_opaque() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_cover(&mut p, "标题");
        let (row_y0, row_y1, _, _) = cover_dynamic_windows(&mut painter, "标题");
        let logo_x0 = COVER_ROW_LEFT_PX as u32 + COVER_LOGO_MARGIN_PX as u32;
        let logo_x1 = logo_x0 + COVER_LOGO_SIZE_PX;
        let max_darkness = max_darkness_in_rect(&p, logo_x0, logo_x1, row_y0, row_y1);
        assert!(
            (max_darkness as i32 - 76).abs() <= 10,
            "logo darkness 应约为 76（0.30 组透明度合成到白底），实得 {max_darkness}"
        );
        assert_ne!(max_darkness, 255, "logo 不应是不透明的纯色（未施加整体透明度）");
    }

    /// **修复轮 1（I3）**：上排"左对齐、行左边缘 x=176"此前没有任何断言
    /// （审查变异 N3：去掉 `marginLeft 40`；N9：上排改成水平居中——双双存活）。
    /// 上排（logo）是这一带最左侧的元素，直接断言该窗口内最左侧墨迹的 x 坐标。
    #[test]
    fn cover_top_row_left_edge_is_176() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_cover(&mut p, "标题");
        let (row_y0, row_y1, _, _) = cover_dynamic_windows(&mut painter, "标题");
        let (x0, _, _, _) = ink_bbox_in_y_range(&p, row_y0, row_y1).expect("上排应有墨迹");
        // 故意用字面量 176（而不是 `COVER_ROW_LEFT_PX + COVER_LOGO_MARGIN_PX`）
        // 做期望值——审查实测就是 176。若改用这两个常量相加，当
        // `COVER_ROW_MARGIN_LEFT_PX`（marginLeft 40）被错误改掉时，`expected`
        // 会跟着"一起错"，测试变成永远自证成立、测不出任何东西（这正是
        // 变异验证时抓到的真实教训：用同一个被改动的常量算期望值，等于没测）。
        assert!((x0 as i32 - 176).abs() <= 2, "上排（logo）左边缘应≈176，实得 {x0}");
    }

    /// Cover 主标题应是 100px 量级：单行短标题的墨高与上排 38px 文字墨高的比值
    /// 应约为 100/38（±10%）。
    #[test]
    fn cover_title_font_size_matches_100px() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        let title = "短标题"; // 短标题，单行，不换行
        painter.draw_cover(&mut p, title);
        let (top_h, title_h) = cover_row_and_title_ink_heights(&mut painter, title, &p);
        let ratio = title_h / top_h;
        let expected = 100.0 / 38.0;
        assert!(
            (ratio - expected).abs() / expected <= 0.10,
            "主标题/上排墨高比应约为 {expected:.3}±10%，实得 {ratio:.3}（top_h={top_h} title_h={title_h}）"
        );
    }

    /// 扫描出上排（logo 所在带）与主标题各自的整体墨迹纵向跨度（最上一行有
    /// 墨迹到最下一行有墨迹）。
    ///
    /// **不用"第一段连续墨迹"**：某些 CJK 字形内部存在完全空白的行（笔画间的
    /// 间隙，例如既有测试 `font_size_threshold_ignores_whitespace_padding`
    /// 文档记录的"三"字——三条横线，行扫描会在笔画间的空白处误判"这一段墨迹
    /// 结束了"），用"从第一行有墨迹到最后一行有墨迹"的整体跨度不受这个陷阱
    /// 影响。上排与主标题各自在互不重叠的 y 窗口内查找，两个窗口本身不会
    /// 互相污染——**修复轮 1（M2）**：这两个窗口现在由 `cover_dynamic_windows`
    /// 动态推导，不再是手算死的像素值。
    fn cover_row_and_title_ink_heights(painter: &mut Painter, title: &str, p: &Pixmap) -> (f32, f32) {
        let row_has_ink =
            |y: u32| (0..p.width()).any(|x| p.pixel(x, y).map(|c| darkness(c) > 0).unwrap_or(false));
        let ink_extent = |y_start: u32, y_end: u32| -> Option<(u32, u32)> {
            let mut first = None;
            let mut last = None;
            for y in y_start..y_end {
                if row_has_ink(y) {
                    first.get_or_insert(y);
                    last = Some(y);
                }
            }
            first.zip(last)
        };
        let (row_y0, row_y1, title_y0, title_y1) = cover_dynamic_windows(painter, title);
        let top_extent = ink_extent(row_y0, row_y1).expect("上排应有墨迹");
        let title_extent = ink_extent(title_y0, title_y1).expect("主标题应有墨迹");
        (
            (top_extent.1 - top_extent.0 + 1) as f32,
            (title_extent.1 - title_extent.0 + 1) as f32,
        )
    }

    /// Cover 水印：直接检查 `prepare_watermark` 预渲染出的水印小图（透明底上
    /// 画出来的，未与 Cover 白底合成，因此可以像 `content` 预设的既有测试
    /// 一样直接读 `alpha` 精确核对数值，不受"合成到不透明白底后 alpha 恒为
    /// 255、只能靠颜色深浅反推"这件事的影响）：墨迹（含 origin 换算回画布
    /// 坐标）水平/垂直中心分别 ≈640/≈576（±4），全区 maxAlpha 精确等于 102
    /// （`rgba(23,23,23,0.4)`），且含中文后缀（墨宽显著大于 content 预设的
    /// 275px，给下界 450px）；并确认 `draw_cover` 真的把它画了出来（不只是
    /// prepare 了但没调用）。
    #[test]
    fn cover_watermark_is_centered_at_640_576_with_alpha_102_and_chinese_suffix() {
        let mut renderer = TextRenderer::new().unwrap();
        let prepared = prepare_watermark(&mut renderer, &cover_watermark_preset()).unwrap();
        let (x0, y0, x1, y1) = non_transparent_bbox(&prepared.pixmap).expect("cover 水印应有墨迹");
        let canvas_x0 = prepared.origin_x + x0 as i32;
        let canvas_x1 = prepared.origin_x + x1 as i32;
        let canvas_y0 = prepared.origin_y + y0 as i32;
        let canvas_y1 = prepared.origin_y + y1 as i32;
        let cx = (canvas_x0 + canvas_x1) as f32 / 2.0;
        let cy = (canvas_y0 + canvas_y1) as f32 / 2.0;
        assert!((cx - 640.0).abs() <= 4.0, "水印水平中心应≈640，实得 {cx}");
        assert!((cy - 576.0).abs() <= 4.0, "水印垂直中心应≈576，实得 {cy}");

        let mut max_alpha = 0u8;
        for y in 0..prepared.pixmap.height() {
            for x in 0..prepared.pixmap.width() {
                if let Some(c) = prepared.pixmap.pixel(x, y) {
                    max_alpha = max_alpha.max(c.alpha());
                }
            }
        }
        assert_eq!(max_alpha, 102, "cover 水印 maxAlpha 应精确等于 102");

        // **修复轮 1（M1）**：把过松的下界 `>450` 换成窄区间——实测 601。
        // 渲染全程确定性（同一份字体 + 同一套矢量排版，没有任何随机性来源），
        // 容差只需要覆盖裁剪/取整的量级，给 ±3（而不是审查建议的 ±10——
        // 实测 ±10 的容差盖不住 N15 那种 8px 量级的偏移，会让变异存活，见
        // 报告"修复轮 1"变异验证记录）。这一条覆盖三个此前零覆盖的精确参数
        // （审查变异 N13/N14/N15：水印图标 32→28、字号 28→24、图标间距
        // 12→4，全部会显著改变这个墨宽，之前 `>450` 的松散下界测不出这些
        // 变化）。
        let width = canvas_x1 - canvas_x0;
        assert!(
            (width - 601).abs() <= 3,
            "带中文后缀的水印墨宽应精确约为 601±3px，实得 {width}"
        );

        // 确认 draw_cover 真的调用了它，不只是 Painter::new() 里预渲染了但没贴图。
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_cover(&mut p, "标题");
        assert!(
            ink_bbox_in_y_range(&p, 550, 720).is_some(),
            "draw_cover 应该把 cover 水印实际画到画布上"
        );
    }

    /// **修复轮 1（M1）**：`·` 分隔点的 0.75 倍率此前零覆盖（审查变异 N6：
    /// 倍率 0.75→1.0，存活）。
    ///
    /// **第一版实现有缺陷，被自己的变异验证抓到**：最初的写法是"扫全图找
    /// 有没有 alpha≈77 的像素"——但字形抗锯齿边缘本身就会产生从 0 到
    /// 102 连续过渡的 alpha 值，边缘上几乎必然会经过 77 附近，导致这条断言
    /// 无论倍率是不是 0.75 都成立（变异 N6 验证时这条测试纹丝不动地通过）。
    /// 改成只在分隔点自身的 x 范围内求 **maxAlpha**：分隔点内部（非边缘）的
    /// 像素在正确实现下应该封顶在 ≈77，被错误改成 1.0 倍率后会封顶在 102——
    /// 这才是能被变异翻转的判据。分隔点的 x 范围与
    /// `layout_and_draw_watermark` 内部算法同源重新推导（整行居中、图标+gap+
    /// 各分段依次左对齐），再换算成 `prepared.pixmap` 的局部坐标。
    #[test]
    fn cover_watermark_separator_dot_has_reduced_opacity() {
        let mut renderer = TextRenderer::new().unwrap();
        let preset = cover_watermark_preset();
        let prepared = prepare_watermark(&mut renderer, &preset).unwrap();

        let style = TextStyle {
            size_px: preset.font_size_px,
            color: preset.color,
            stroke: None,
            letter_spacing_px: preset.letter_spacing_px,
            max_width_px: WATERMARK_MAX_WIDTH_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: false,
        };
        let (main_w, _) = renderer.measure(COVER_WATERMARK_TEXT_MAIN, &style);
        let (sep_w, _) = renderer.measure(COVER_WATERMARK_TEXT_SEP, &style);
        let (suffix_w, _) = renderer.measure(COVER_WATERMARK_TEXT_SUFFIX, &style);
        let icon_size = preset.icon_size_px as f32;
        let total_width = icon_size + preset.icon_gap_px + main_w + sep_w + suffix_w;
        let row_left = CANVAS_W / 2.0 - total_width / 2.0;
        let text_start = row_left + icon_size + preset.icon_gap_px;
        let sep_x0 = text_start + main_w;
        let sep_x1 = sep_x0 + sep_w;
        // 只取分隔段 advance 宽度的中间 50%：`" · "` 前后各有一个空格，
        // 字形本身的墨迹（那个点）不会贴着 advance box 的边界，而相邻分段
        // 的字形又可能有轻微的字宽外溢（overhang）越过自己的 advance 边界——
        // 直接用整个 `[sep_x0, sep_x1)` 扫描会把相邻満倍率分段的溢出像素也
        // 扫进来，把 maxAlpha 误判成 102（实测过：不收窄时基线场景就会出现
        // 这个假阳性）。中间 50% 足够远离两侧边界，同时仍完整覆盖点号本身
        // （点号在等宽的 `" · "` 里天然居中）。
        let sep_margin = sep_w * 0.25;
        let local_x0 = ((sep_x0 + sep_margin) - prepared.origin_x as f32).max(0.0) as u32;
        let local_x1 = (((sep_x1 - sep_margin) - prepared.origin_x as f32).max(0.0) as u32)
            .min(prepared.pixmap.width());

        let mut sep_max_alpha = 0u8;
        for y in 0..prepared.pixmap.height() {
            for x in local_x0..local_x1 {
                if let Some(c) = prepared.pixmap.pixel(x, y) {
                    sep_max_alpha = sep_max_alpha.max(c.alpha());
                }
            }
        }
        assert!(
            (sep_max_alpha as i32 - 77).abs() <= 3,
            "分隔点「·」自身范围内的 maxAlpha 应≈77（102×0.75），实得 {sep_max_alpha}"
        );
    }

    /// **修复轮 1（M1）**：光标间距（advance 口径 4px）此前零覆盖（审查变异
    /// N5：间距 4→40，存活）。断言"光标墨迹左边缘 − 最后一行墨迹右边缘"落在
    /// `[4,25]`——4px 加在 advance 口径上，`|` 字形自身还有 side bearing，
    /// 审查实测视觉间隙约 12px，故给一个覆盖两者的合理区间而不是精确值。
    #[test]
    fn cursor_gap_from_last_line_is_within_expected_range() {
        let mut painter = Painter::new().unwrap();
        let title = "标题文字"; // 4 字，chars_per_sec=2.0，纯 CJK 不涉及 word-wrap
        let (cursor_bbox, last_line_bbox) = cursor_and_last_line_bboxes(&mut painter, 15, title);
        let cursor = cursor_bbox.expect("frame 15 应有光标（f%15=0，最亮）");
        let last_line = last_line_bbox.expect("frame 15 应有文字墨迹");
        let gap = cursor.0 as i32 - last_line.2 as i32;
        assert!(
            (4..=25).contains(&gap),
            "光标左边缘与最后一行右边缘的间隙应落在 [4,25]px，实得 {gap}"
        );
    }

    /// **修复轮 1（M1）**：Cover 主标题 / Intro 标题的换行宽度上限
    /// （均为 944px）此前零覆盖（审查变异 N11：Intro `max_width` 944→1280；
    /// N12：Cover 主标题 `max_width` 944→1280——双双存活）。用一个必然换行
    /// 的长标题，断言两处的墨宽都不超过 944px（留一点描边/字距的余量）。
    #[test]
    fn cover_and_intro_titles_wrap_within_944px() {
        let mut painter = Painter::new().unwrap();
        let long_title = "长".repeat(30); // 100px/70px 字号下必然远超 944px，需要换行

        let mut cover_p = Pixmap::new(1280, 720).unwrap();
        painter.draw_cover(&mut cover_p, &long_title);
        let (_, _, title_y0, title_y1) = cover_dynamic_windows(&mut painter, &long_title);
        let (cx0, _, cx1, _) =
            ink_bbox_in_y_range(&cover_p, title_y0, title_y1).expect("Cover 主标题应有墨迹");
        let cover_w = cx1 - cx0;
        assert!(
            cover_w <= 944 + 8,
            "Cover 主标题墨宽应 ≤944px（含少量描边/字距余量），实得 {cover_w}"
        );

        let mut intro_p = Pixmap::new(1280, 720).unwrap();
        painter.draw_intro(&mut intro_p, 70, &long_title); // 70 帧：打字已完成、尚未开始淡出
        let (ix0, _, ix1, _) =
            ink_bbox_in_y_range(&intro_p, 0, 720).expect("Intro 标题应有墨迹");
        let intro_w = ix1 - ix0;
        assert!(intro_w <= 944 + 8, "Intro 标题墨宽应 ≤944px，实得 {intro_w}");
    }

    /// Intro 不应有水印：下半部（y>600，覆盖 content 水印所在的位置区域）不应有墨。
    ///
    /// **已知盲区（修复轮 1 审查复现并确认，M4）**：`content` 预设水印的
    /// 颜色是 `rgba(255,255,255,0.27)`——白色、低透明度。如果哪天有人误在
    /// `draw_intro` 里调用 `draw_watermark(pixmap, &self.content_watermark, ..)`，
    /// 把这个白色水印合成到 Intro 本就不透明的白底上，**在数学上是恒等运算**
    /// （白叠白，alpha/颜色判据都测不出任何差异，输出逐字节不变）。这不是
    /// 这条测试或任何像素判据能堵住的漏洞——同时也正因为如此，这种"回归"
    /// **没有任何用户可见后果**（输出真的没变）。审查明确建议不要为此引入
    /// 测试专用的全局状态或作弊式检测（例如给 `draw_watermark` 打桩记录调用
    /// 次数），这里保留这条测试是为了堵住"画了别的、有颜色差异的东西"这类
    /// 更常见的回归（下面的完整性断言 `intro_frame_matches_hand_composited_reference_exactly`
    /// 覆盖了同一类关注点的另一半：正向证明每一帧"不多画任何东西"）。
    #[test]
    fn intro_has_no_watermark() {
        let mut painter = Painter::new().unwrap();
        let mut p = Pixmap::new(1280, 720).unwrap();
        painter.draw_intro(&mut p, 30, "标题");
        assert!(ink_bbox_in_y_range(&p, 600, 720).is_none(), "Intro 下半部不应有水印墨迹");
    }

    /// 打字机字符数：用一个不会换行的短标题（6 字），断言 frame 5/15/30 的墨宽
    /// 阶梯上升，且打完（frame>=60）后与整串标题的墨宽一致（±4px）。
    #[test]
    fn typewriter_ink_width_steps_up_and_matches_full_title_when_done() {
        let mut painter = Painter::new().unwrap();
        let title = "六个字标题呀"; // 6 字，不会换行
        let mut f5 = Pixmap::new(1280, 720).unwrap();
        let mut f15 = Pixmap::new(1280, 720).unwrap();
        let mut f30 = Pixmap::new(1280, 720).unwrap();
        let mut f70 = Pixmap::new(1280, 720).unwrap();
        painter.draw_intro(&mut f5, 5, title);
        painter.draw_intro(&mut f15, 15, title);
        painter.draw_intro(&mut f30, 30, title);
        painter.draw_intro(&mut f70, 70, title);

        let ink_width = |p: &Pixmap| -> f32 {
            let (x0, _, x1, _) = ink_bbox_in_y_range(p, 0, 720).expect("应有墨迹");
            (x1 - x0) as f32
        };
        let w5 = ink_width(&f5);
        let w15 = ink_width(&f15);
        let w30 = ink_width(&f30);
        let w70 = ink_width(&f70);
        assert!(w5 < w15, "frame 5 应比 frame 15 窄：{w5} vs {w15}");
        assert!(w15 < w30, "frame 15 应比 frame 30 窄：{w15} vs {w30}");

        // 单独渲染整串标题（不经打字机）作为基准比较墨宽。**必须先填白底**：
        // `darkness()` 把「透明像素」（premultiplied rgb 恒为 0）误判成
        // 「纯黑」（`darkness=255`），不填白底会让 `ink_bbox_in_y_range`
        // 把整张画布都当成墨迹（红色阶段实测：这条测试当场因此失败）。
        let mut full = Pixmap::new(1280, 720).unwrap();
        full.fill(Color::from_rgba8(255, 255, 255, 255));
        let style = TextStyle {
            size_px: 70.0,
            color: [0, 0, 0, 255],
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: 944.0,
            line_height: 1.2,
            bold: true,
        };
        painter.renderer.draw_centered(&mut full, title, 640.0, 360.0, &style, 1.0, 1.0);
        let w_full = ink_width(&full);
        assert!(
            (w70 - w_full).abs() <= 4.0,
            "打完后墨宽应与整串标题一致（±4px）：frame70={w70} full={w_full}"
        );
    }

    /// 光标存在且会闪：`interpolate3((f%15) as f64,[0,7.5,15],[1,1,0])` 在
    /// `f%15=0` 时最亮（1.0），在 `f%15=14` 时最暗（≈0.133）。用「标题」
    /// （2 字，`chars_per_sec=1.0`）保证 frame 0 与 frame 14 的 `visible` 都是
    /// 0（`30/chars_per_sec=30` 帧才显示第一个字），这样两帧唯一的差异就是
    /// 光标本身的透明度——用累计 darkness（正比于合成的组透明度）而不是
    /// "是否非纯白"的布尔判据来比较亮度，因为同一光标形状不管多暗、只要非
    /// 纯白就会被布尔判据判定为"有墨迹"，量不出亮暗差异（红色阶段实测：
    /// 用 `darkness>0` 布尔计数时 bright/dim 的墨迹像素数完全相等，虽然
    /// 实际颜色深浅明显不同）。
    #[test]
    fn cursor_exists_blinks_and_disappears_once_typing_completes() {
        let mut painter = Painter::new().unwrap();
        let title = "标题"; // 2 字：chars_per_sec=1.0，frame<30 时 visible 恒为 0
        let mut bright = Pixmap::new(1280, 720).unwrap();
        let mut dim = Pixmap::new(1280, 720).unwrap();
        painter.draw_intro(&mut bright, 0, title); // visible=0，f%15=0 → blink=1.0（最亮）
        painter.draw_intro(&mut dim, 14, title); // visible=0，f%15=14 → blink≈0.133（最暗）
        let darkness_sum = |p: &Pixmap| -> u64 {
            (0..p.height())
                .flat_map(|y| (0..p.width()).map(move |x| (x, y)))
                .filter_map(|(x, y)| p.pixel(x, y))
                .map(|c| u64::from(darkness(c)))
                .sum()
        };
        let sum_bright = darkness_sum(&bright);
        let sum_dim = darkness_sum(&dim);
        assert!(sum_bright > sum_dim * 2, "光标全亮帧的累计 darkness 应显著大于全暗帧：bright={sum_bright} dim={sum_dim}");
        assert!(sum_dim > 0, "全暗帧（blink≈0.133）光标仍应残留极淡的墨迹，不应完全消失");

        // 打完（local_frame>=60）后不应有光标：与整串标题单独渲染逐字节一致。
        // 同样必须先填白底（原因见上一条测试的注释）。
        let title2 = "六个字标题呀";
        let mut done = Pixmap::new(1280, 720).unwrap();
        painter.draw_intro(&mut done, 60, title2);
        let mut full = Pixmap::new(1280, 720).unwrap();
        full.fill(Color::from_rgba8(255, 255, 255, 255));
        let style = TextStyle {
            size_px: 70.0,
            color: [0, 0, 0, 255],
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: 944.0,
            line_height: 1.2,
            bold: true,
        };
        painter.renderer.draw_centered(&mut full, title2, 640.0, 360.0, &style, 1.0, 1.0);
        assert_eq!(done.data(), full.data(), "打完后应与整串标题渲染结果逐字节一致（无光标残留）");
    }

    /// **修复轮 1（M4.1）**：把上一条测试里"打完后与整串标题逐字节一致"的
    /// 正向完全性断言，推广到若干打字中途的帧——手工按 `draw_intro` 同一套
    /// 公式合成"白底 + 当前 visible 文字 + 光标（若应显示）"参考图，与真实
    /// 输出逐字节比较。这既钉死"不多画任何东西"（含 M4 指出的"content 水印
    /// 白叠白测不出来"这个盲区之外的所有其它可能的意外墨迹来源——只要那个
    /// 来源不是"恰好也是白色"，这条测试都能抓到），也顺带验证了
    /// `intro_last_line_anchor`/`last_line_metrics` 算出的光标位置与
    /// `draw_intro` 实际使用的完全一致。
    #[test]
    fn intro_frame_matches_hand_composited_reference_exactly() {
        let mut painter = Painter::new().unwrap();
        let title = "标题文字标题文字标题"; // 10 字，chars_per_sec=5.0，覆盖多个 visible 台阶
        for f in [0u32, 5, 10, 15, 29, 45, 59, 60] {
            let mut actual = Pixmap::new(1280, 720).unwrap();
            painter.draw_intro(&mut actual, f, title);

            let mut expected = Pixmap::new(1280, 720).unwrap();
            expected.fill(Color::from_rgba8(255, 255, 255, 255));

            let title_chars: Vec<char> = title.chars().collect();
            let total = title_chars.len();
            let chars_per_sec = total as f64 / INTRO_TYPEWRITER_SECONDS;
            let visible = ((f as f64 * chars_per_sec / FPS).floor() as usize).min(total);
            let display_text: String = title_chars[..visible].iter().collect();
            let style = TextStyle {
                size_px: INTRO_TITLE_FONT_SIZE_PX,
                color: TITLE_COLOR_BLACK,
                stroke: None,
                letter_spacing_px: 0.0,
                max_width_px: INTRO_TITLE_MAX_WIDTH_PX,
                line_height: DEFAULT_LINE_HEIGHT,
                bold: true,
            };
            let fade_opacity = interpolate(f as f64, INTRO_FADE_OUT_RANGE, [1.0, 0.0]) as f32;
            if fade_opacity > 0.0 {
                painter.renderer.draw_centered(
                    &mut expected,
                    &display_text,
                    INTRO_TITLE_CENTER_X,
                    INTRO_TITLE_CENTER_Y,
                    &style,
                    fade_opacity,
                    1.0,
                );

                if f < INTRO_TYPEWRITER_FRAMES {
                    let period = INTRO_CURSOR_BLINK_PERIOD_FRAMES as f64;
                    let blink_opacity = interpolate3(
                        (f % INTRO_CURSOR_BLINK_PERIOD_FRAMES) as f64,
                        [0.0, period / 2.0, period],
                        [1.0, 1.0, 0.0],
                    ) as f32;
                    let cursor_opacity = fade_opacity * blink_opacity;
                    if cursor_opacity > 0.0 {
                        let (right_edge_x, last_line_center_y) = painter
                            .renderer
                            .last_line_metrics(&display_text, &style)
                            .map(|(w, top_rel, _)| {
                                (INTRO_TITLE_CENTER_X + w / 2.0, INTRO_TITLE_CENTER_Y + top_rel / 2.0)
                            })
                            .unwrap_or((INTRO_TITLE_CENTER_X, INTRO_TITLE_CENTER_Y));
                        let (cursor_w, _) = painter.renderer.measure(INTRO_CURSOR_TEXT, &style);
                        let cursor_center_x = right_edge_x + INTRO_CURSOR_GAP_PX + cursor_w / 2.0;
                        painter.renderer.draw_centered(
                            &mut expected,
                            INTRO_CURSOR_TEXT,
                            cursor_center_x,
                            last_line_center_y,
                            &style,
                            cursor_opacity,
                            1.0,
                        );
                    }
                }
            }

            assert_eq!(
                actual.data(),
                expected.data(),
                "frame {f}: draw_intro 的输出应与手工合成的参考图逐字节一致"
            );
        }
    }

    /// 像素包围盒 `(x0, y0, x1, y1)`（含边界）。
    type Bbox = (u32, u32, u32, u32);

    /// 求两帧差异像素的包围盒（`None` 表示完全一致）。
    fn diff_bbox(a: &Pixmap, b: &Pixmap) -> Option<Bbox> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..a.height().min(b.height()) {
            for x in 0..a.width().min(b.width()) {
                if a.pixel(x, y) != b.pixel(x, y) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0 != u32::MAX).then_some((x0, y0, x1, y1))
    }

    fn bboxes_overlap_on_x(a: Bbox, b: Bbox) -> bool {
        a.0 <= b.2 && a.2 >= b.0
    }

    /// 在给定帧里分别求出「光标自身的像素 bbox」与「最后一行文字墨迹的
    /// bbox」，用于 I1（word-wrap 场景下光标是否压字）与光标间距（M1）的
    /// 断言。光标 bbox 通过「有光标」与「无光标」两次渲染的像素 diff 求出
    /// （两者除光标外应逐字节相同——这本身就是
    /// `intro_frame_matches_hand_composited_reference_exactly` 验证过的
    /// 不变量）；最后一行墨迹 bbox 通过 `last_line_metrics` 换算出的 y 带在
    /// "无光标"那张图里扫描得到。若该帧压根没有光标（打字已完成、或淡出/
    /// 闪烁相位使 opacity 恰好为 0），光标 bbox 返回 `None`。
    fn cursor_and_last_line_bboxes(
        painter: &mut Painter,
        local_frame: u32,
        title: &str,
    ) -> (Option<Bbox>, Option<Bbox>) {
        let mut with_cursor = Pixmap::new(1280, 720).unwrap();
        painter.draw_intro(&mut with_cursor, local_frame, title);

        let title_chars: Vec<char> = title.chars().collect();
        let total = title_chars.len();
        let chars_per_sec = total as f64 / INTRO_TYPEWRITER_SECONDS;
        let visible = ((local_frame as f64 * chars_per_sec / FPS).floor() as usize).min(total);
        let display_text: String = title_chars[..visible].iter().collect();
        let style = TextStyle {
            size_px: INTRO_TITLE_FONT_SIZE_PX,
            color: TITLE_COLOR_BLACK,
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: INTRO_TITLE_MAX_WIDTH_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: true,
        };
        let fade_opacity = interpolate(local_frame as f64, INTRO_FADE_OUT_RANGE, [1.0, 0.0]) as f32;

        let mut text_only = Pixmap::new(1280, 720).unwrap();
        text_only.fill(Color::from_rgba8(255, 255, 255, 255));
        if fade_opacity > 0.0 {
            painter.renderer.draw_centered(
                &mut text_only,
                &display_text,
                INTRO_TITLE_CENTER_X,
                INTRO_TITLE_CENTER_Y,
                &style,
                fade_opacity,
                1.0,
            );
        }

        let cursor_bbox = diff_bbox(&with_cursor, &text_only);
        let last_line_bbox = painter
            .renderer
            .last_line_metrics(&display_text, &style)
            .and_then(|(_, top_rel, h)| {
                let center_y = INTRO_TITLE_CENTER_Y + top_rel / 2.0;
                let y0 = (center_y - h / 2.0).max(0.0) as u32;
                let y1 = ((center_y + h / 2.0).min(719.0) as u32) + 1;
                ink_bbox_in_y_range(&text_only, y0, y1)
            });

        (cursor_bbox, last_line_bbox)
    }

    /// 求两帧在给定 y 范围内差异像素的包围盒（`None` 表示该范围内完全一致）。
    /// **修复轮 2（性能收窄专用）**：与 `diff_bbox` 是同一件事，只是多一个
    /// y 范围裁剪——不改 `diff_bbox` 本身（它被 `cursor_gap_from_last_line_is_within_expected_range`
    /// 复用，改动会波及那条测试，本轮范围只限定在
    /// `cursor_never_overlaps_word_wrapped_last_line_ink` 这一条）。
    fn diff_bbox_in_y_range(a: &Pixmap, b: &Pixmap, y0: u32, y1: u32) -> Option<Bbox> {
        let (mut x0, mut by0, mut x1, mut by1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        let y_end = y1.min(a.height()).min(b.height());
        for y in y0..y_end {
            for x in 0..a.width().min(b.width()) {
                if a.pixel(x, y) != b.pixel(x, y) {
                    x0 = x0.min(x);
                    by0 = by0.min(y);
                    x1 = x1.max(x);
                    by1 = by1.max(y);
                }
            }
        }
        (x0 != u32::MAX).then_some((x0, by0, x1, by1))
    }

    /// 与 `cursor_and_last_line_bboxes` 逻辑相同，唯一区别是像素扫描（diff
    /// 与最后一行墨迹的查找）被限定在一个窄 y 带内，专供
    /// `cursor_never_overlaps_word_wrapped_last_line_ink` 使用（性能收窄，
    /// 修复轮 2）。**不修改 `cursor_and_last_line_bboxes` 本身**——那个函数
    /// 被另一条测试（`cursor_gap_from_last_line_is_within_expected_range`）
    /// 复用，本轮的收窄范围明确限定在这一条测试。
    ///
    /// y 带的推导**故意只用 `measure()`，不用 `last_line_metrics`**：
    /// `last_line_metrics` 正是 `cursor_never_overlaps_word_wrapped_last_line_ink`
    /// 要验证的对象，如果拿它自己的返回值来定义"该往哪扫"，一旦它本身出现
    /// 回归（比如又变回旧的"前缀高度跳变"启发式、算出一个偏小的行位置），
    /// 窗口会跟着算错并可能收窄到看不见问题的地方——这是循环论证，会让
    /// 性能收窄反而削弱了测试的有效性。`measure()` 给出的是文本块的整体
    /// 排版高度，与"最后一行具体在哪"这个问题相互独立，据此确定的窗口
    /// 不依赖被测方法本身是否正确。
    fn cursor_and_last_line_bboxes_narrow(
        painter: &mut Painter,
        local_frame: u32,
        title: &str,
    ) -> (Option<Bbox>, Option<Bbox>) {
        let mut with_cursor = Pixmap::new(1280, 720).unwrap();
        painter.draw_intro(&mut with_cursor, local_frame, title);

        let title_chars: Vec<char> = title.chars().collect();
        let total = title_chars.len();
        let chars_per_sec = total as f64 / INTRO_TYPEWRITER_SECONDS;
        let visible = ((local_frame as f64 * chars_per_sec / FPS).floor() as usize).min(total);
        let display_text: String = title_chars[..visible].iter().collect();
        let style = TextStyle {
            size_px: INTRO_TITLE_FONT_SIZE_PX,
            color: TITLE_COLOR_BLACK,
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: INTRO_TITLE_MAX_WIDTH_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: true,
        };
        let fade_opacity = interpolate(local_frame as f64, INTRO_FADE_OUT_RANGE, [1.0, 0.0]) as f32;

        // 扫描窗口：`measure()` 给出的文本块整体高度，上下各留 40px 安全
        // 边距（光标即便因为某种 bug 跑到相邻行，40px 也足够覆盖一整行
        // 70px 字号 * 1.2 行高 = 84px 的量级）。
        let (_, total_h) = painter.renderer.measure(&display_text, &style);
        const BAND_MARGIN_PX: f32 = 40.0;
        let band_y0 = (INTRO_TITLE_CENTER_Y - total_h / 2.0 - BAND_MARGIN_PX)
            .max(0.0)
            .floor() as u32;
        let band_y1 = ((INTRO_TITLE_CENTER_Y + total_h / 2.0 + BAND_MARGIN_PX)
            .min(CANVAS_H)
            .ceil() as u32)
            .max(band_y0 + 1);

        let mut text_only = Pixmap::new(1280, 720).unwrap();
        text_only.fill(Color::from_rgba8(255, 255, 255, 255));
        if fade_opacity > 0.0 {
            painter.renderer.draw_centered(
                &mut text_only,
                &display_text,
                INTRO_TITLE_CENTER_X,
                INTRO_TITLE_CENTER_Y,
                &style,
                fade_opacity,
                1.0,
            );
        }

        let cursor_bbox = diff_bbox_in_y_range(&with_cursor, &text_only, band_y0, band_y1);
        let last_line_bbox = painter
            .renderer
            .last_line_metrics(&display_text, &style)
            .and_then(|(_, top_rel, h)| {
                let center_y = INTRO_TITLE_CENTER_Y + top_rel / 2.0;
                let y0 = (center_y - h / 2.0).max(0.0) as u32;
                let y1 = ((center_y + h / 2.0).min(719.0) as u32) + 1;
                ink_bbox_in_y_range(&text_only, y0, y1)
            });

        (cursor_bbox, last_line_bbox)
    }

    /// **修复轮 1（I1，核心修复的验收测试）；修复轮 2（性能收窄）**：
    /// word-wrap 换行时光标不应被画到最后一行中间、压在字形上。
    ///
    /// 审查用「逐帧枚举 0..60、判据『光标 bbox 是否落在最后一行墨迹 bbox
    /// 之内』」实测出修复前的破相帧数：
    ///
    /// | 标题 | 修复前 | 修复后 |
    /// |---|---|---|
    /// | 英文长标题（`Panda Video Generator automated engine for long titles wrapping`） | 35/60 | 0/60 |
    /// | 中英混排（`熊猫视频自动化引擎 Panda Video Generator 全流程演示标题`） | 20/60 | 0/60 |
    ///
    /// **修复轮 2**：原来逐帧枚举 `0..60` 单条耗时约 90s（每帧两次
    /// 1280×720 全画布渲染 + diff），改成 10 个代表帧、扫描窗口收窄到文本块
    /// 高度±40px 的窄带后降到约 17s（改前/改后的具体帧数与耗时对照见报告
    /// "修复轮 2"）。这 10 帧是**代表性抽样，不是全枚举**：
    ///
    /// - `33`、`45` 是原始缺陷报告截图里点名的破相帧（光标穿过 "Video" 的
    ///   "o"、演示过修复前后对比），必须保留；
    /// - `10/15/20/25/30/38/40/50` 分布在打字机推进的不同阶段——随着
    ///   `visible` 增长，word-wrap 的换行点会跟着移动，因此不同帧下"最后
    ///   一行从哪个单词开始"并不相同，这组帧覆盖了"刚越过一次换行边界"
    ///   "下一次换行前夕""中间稳定期"等几种典型状态，不是等间隔地随便抽样。
    ///
    /// 这里只按 x 轴判断重叠（`bboxes_overlap_on_x`）：光标与文字的 y 位置
    /// 由同一个 `last_line_center_y` 公式给出，天然对齐在同一行，真正会
    /// "压字"的失败模式是水平方向上光标落进了文字的包围盒内。
    #[test]
    fn cursor_never_overlaps_word_wrapped_last_line_ink() {
        let mut painter = Painter::new().unwrap();
        let titles = [
            "Panda Video Generator automated engine for long titles wrapping",
            "熊猫视频自动化引擎 Panda Video Generator 全流程演示标题",
        ];
        const SAMPLE_FRAMES: [u32; 10] = [10, 15, 20, 25, 30, 33, 38, 40, 45, 50];
        for title in titles {
            let mut overlap_frames = 0u32;
            for f in SAMPLE_FRAMES {
                let (cursor_bbox, last_line_bbox) =
                    cursor_and_last_line_bboxes_narrow(&mut painter, f, title);
                if let (Some(c), Some(t)) = (cursor_bbox, last_line_bbox)
                    && bboxes_overlap_on_x(c, t)
                {
                    overlap_frames += 1;
                }
            }
            assert_eq!(
                overlap_frames,
                0,
                "标题 {title:?} 不应有任何代表帧光标压在最后一行文字上，实际 {overlap_frames}/{} 帧",
                SAMPLE_FRAMES.len()
            );
        }
    }

    /// 光标闪烁不导致文字抖动：取同一 `visible`（=1）下光标不同透明度的两帧
    /// （frame 15 与 frame 29，用 4 字标题使 `chars_per_sec=2.0`，每 15 帧显示
    /// 一个字符，`visible=floor(f/15)` 在 `[15,29]` 内恒为 1，`f%15` 分别是
    /// 0 与 14，正好是全亮与全暗），不预判光标的具体像素坐标（不写死"光标在
    /// x>=某值"这类耦合实现细节的断言），而是直接比较两帧的像素差异区域：
    /// 如果文字位置真的跟着光标透明度抖动，差异会扩散到整块文字的宽度
    /// （几百像素）；如果只有光标本身在变暗变亮，差异只会集中在一个字形
    /// 宽度以内的窄带。
    #[test]
    fn cursor_blinking_does_not_shift_text_pixels() {
        let mut painter = Painter::new().unwrap();
        let title = "标题文字"; // 4 字：chars_per_sec=2.0，每 15 帧显示一个字符
        let mut bright = Pixmap::new(1280, 720).unwrap();
        let mut dim = Pixmap::new(1280, 720).unwrap();
        painter.draw_intro(&mut bright, 15, title); // visible=1，f%15=0 → 光标全亮
        painter.draw_intro(&mut dim, 29, title); // visible=1，f%15=14 → 光标近乎全暗
        let mut diff_x0 = u32::MAX;
        let mut diff_x1 = 0u32;
        let mut diff_count = 0usize;
        for y in 0..720u32 {
            for x in 0..1280u32 {
                if bright.pixel(x, y) != dim.pixel(x, y) {
                    diff_x0 = diff_x0.min(x);
                    diff_x1 = diff_x1.max(x);
                    diff_count += 1;
                }
            }
        }
        assert!(diff_count > 0, "光标不同透明度应产生像素差异（否则光标根本没画出来）");
        let diff_width = diff_x1 - diff_x0;
        assert!(
            diff_width < 50,
            "像素差异应只集中在光标本身的窄带内（<50px），不应扩散到文字：diff_x=[{diff_x0},{diff_x1}] 宽度={diff_width}"
        );
    }

    /// 淡出端点：local_frame = 104 的墨迹应接近 0
    /// （`interpolate(104,[90,104],[1,0])=0`），而 local_frame=89 是满不透明
    /// （`interpolate(89,[90,104],[1,0])=1`，因为 89<=90）。
    #[test]
    fn fade_out_endpoints_match_interpolate_exactly() {
        assert_eq!(interpolate(104.0, [90.0, 104.0], [1.0, 0.0]), 0.0);
        assert_eq!(interpolate(89.0, [90.0, 104.0], [1.0, 0.0]), 1.0);

        let mut painter = Painter::new().unwrap();
        let title = "淡出端点测试";
        let mut f89 = Pixmap::new(1280, 720).unwrap();
        let mut f104 = Pixmap::new(1280, 720).unwrap();
        painter.draw_intro(&mut f89, 89, title);
        painter.draw_intro(&mut f104, 104, title);
        assert!(ink_bbox_in_y_range(&f104, 0, 720).is_none(), "frame 104 应完全无墨迹（fade=0）");
        assert!(ink_bbox_in_y_range(&f89, 0, 720).is_some(), "frame 89 应满不透明，有墨迹");
    }

}
