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
use tiny_skia::{
    Color, FillRule, FilterQuality, IntSize, Paint, PathBuilder, Pixmap, PixmapPaint, Transform,
};

use std::path::Path;

use crate::assets::logo_rgba;
use crate::config::Branding;
use crate::render::anim::{interpolate, interpolate3, spring};
use crate::render::canvas::Canvas;
use crate::render::text::{TextRenderer, TextStyle};
use crate::vtt::Caption;

/// 画布参数（规格 §8.1）。
const CANVAS_W: f32 = 1280.0;
const CANVAS_H: f32 = 720.0;
const FPS: f64 = 30.0;

/// 字幕居中于画面正中。
const CAPTION_CENTER_X: f32 = CANVAS_W / 2.0;
const CAPTION_CENTER_Y: f32 = CANVAS_H / 2.0;
/// 字幕最大宽度：画面 80% 的容器再减去左右各 40px 的 padding = 944px。
/// TS 的 `Content.tsx` 在**同一个** div 上同时写了 `width:'80%'` 与
/// `padding:'20px 40px'`，而 tailwindcss v4 的 preflight 给所有元素设了
/// `box-sizing: border-box`，所以内容宽度是 `1024 - 80 = 944`，不是 1024。
/// 与 `COVER_TITLE_MAX_WIDTH_PX` / `INTRO_TITLE_MAX_WIDTH_PX` 是同一套换算。
const CAPTION_PADDING_X_PX: f32 = 40.0;
const CAPTION_MAX_WIDTH_PX: f32 = CANVAS_W * 0.8 - CAPTION_PADDING_X_PX * 2.0;
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
const WATERMARK_MARGIN_LEFT_PX: f32 = 40.0;
const WATERMARK_MARGIN_BOTTOM_PX: f32 = 40.0;
const WATERMARK_FONT_SIZE_PX: f32 = 24.0;
/// `rgba(255, 255, 255, 0.27)`：alpha 由颜色自身携带（`0.27 * 255 ≈ 69`），
/// `draw_centered` 的 `opacity` 参数固定传 `1.0`——两者是两件事（brief 提醒 1）。
const WATERMARK_COLOR: [u8; 4] = [255, 255, 255, 69];
const WATERMARK_LETTER_SPACING_EM: f32 = 0.01;
/// 水印只有一行，换行宽度给一个远大于画布宽度的值以避免意外换行。
const WATERMARK_MAX_WIDTH_PX: f32 = 2000.0;
/// 正文水印图标的尺寸与它到文字的间距（规格 §8.6）。
const WATERMARK_ICON_SIZE_PX: u32 = 28;
const WATERMARK_ICON_GAP_PX: f32 = 10.0;

/// `cover` 段水印（规格 §8.6，Task 6）：`rgba(23,23,23,0.4)`，垂直中心见
/// `cover_watermark_preset` 文档注释（`576`，不是规格字面的 `432`）。
const COVER_WATERMARK_CENTER_Y_PX: f32 = 576.0;
const COVER_WATERMARK_FONT_SIZE_PX: f32 = 28.0;
const COVER_WATERMARK_COLOR: [u8; 4] = [23, 23, 23, 102];
/// 封面/片尾水印图标的尺寸与它到文字的间距（规格 §8.6）。
const COVER_WATERMARK_ICON_SIZE_PX: u32 = 32;
const COVER_WATERMARK_ICON_GAP_PX: f32 = 12.0;
/// 文案里的 `·` 分隔符单独降到 0.75 倍不透明度。
///
/// 这是**排版规则而非写死的文案**：对任何含 `·` 的水印文案都成立，见
/// [`split_on_separator`]。
const WATERMARK_SEP_OPACITY_MUL: f32 = 0.75;
/// 水印文案的分隔符字面量。
const WATERMARK_SEP: &str = "·";

/// Cover 居中容器（规格 §8.4「Cover」小节 + 协调者交接的精确排版）：
/// 宽度 80% = 1024px，水平居中，整个容器（上排 + 主标题）垂直居中于画布中心。
const COVER_CONTAINER_WIDTH_PX: f32 = CANVAS_W * 0.8;
const COVER_CONTAINER_LEFT_PX: f32 = (CANVAS_W - COVER_CONTAINER_WIDTH_PX) / 2.0;

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

/// 上排（logo + 品牌名）：左对齐（不是居中），左偏移 40px，
/// 整体不透明度 0.30。
const COVER_ROW_MARGIN_LEFT_PX: f32 = 40.0;
const COVER_ROW_LEFT_PX: f32 = COVER_CONTAINER_LEFT_PX + COVER_ROW_MARGIN_LEFT_PX;
const COVER_LOGO_SIZE_PX: u32 = 36;
const COVER_LOGO_MARGIN_PX: f32 = 8.0;
/// 上排行高：logo 尺寸 + 四周 margin（`36 + 8*2 = 52`）。
const COVER_ROW_HEIGHT_PX: f32 = COVER_LOGO_SIZE_PX as f32 + COVER_LOGO_MARGIN_PX * 2.0;
/// 品牌名左边缘：`row_left + logo_margin + logo_size + logo_margin`。
const COVER_ROW_TEXT_LEFT_PX: f32 =
    COVER_ROW_LEFT_PX + COVER_LOGO_MARGIN_PX + COVER_LOGO_SIZE_PX as f32 + COVER_LOGO_MARGIN_PX;
const COVER_ROW_TEXT_FONT_SIZE_PX: f32 = 38.0;
const COVER_ROW_OPACITY: f32 = 0.30;
/// 上排文字不会换行，给一个远大于画布宽度的值以避免意外换行。
const COVER_ROW_TEXT_MAX_WIDTH_PX: f32 = 2000.0;

/// 主标题：100px 粗体，左右 padding 40px（`max_width = 1024 - 80 = 944`），
/// 水平居中于 x=640。
const COVER_TITLE_FONT_SIZE_PX: f32 = 100.0;
const COVER_TITLE_PADDING_PX: f32 = 40.0;
const COVER_TITLE_MAX_WIDTH_PX: f32 = COVER_CONTAINER_WIDTH_PX - COVER_TITLE_PADDING_PX * 2.0;
const COVER_TITLE_CENTER_X: f32 = CANVAS_W / 2.0;

/// Cover 主标题、Cover 上排品牌名、Intro 标题一律用黑色、无描边
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

/// Outro（规格 §8.4「Outro」小节）：同心圆环 + logo + 固定标题 + `cover` 预设
/// 水印，整体淡出。
///
/// 同心圆环：5 个白色实心圆（`i = 0..OUTRO_RING_COUNT`，半径
/// `OUTRO_RING_RADIUS_STEP_PX * i`），倒序绘制（大的先画），整体
/// `scale = 1 / (1 - out_progress)` 以画布中心为原点——半径本身乘 `scale`
/// 即可，不需要真去构造几何变换。白底上的白色实心圆视觉上不可见（TS 原版靠
/// 一层本移植不做的 `box-shadow` 才看得出边缘），但仍照原样画出（协调者裁定：
/// 忠实移植，不因「看不见」而省略，也不擅自改色/加阴影）。
const OUTRO_RING_COUNT: u32 = 5;
/// `720 * 0.3`：`720` 是规格字面值（`CANVAS_H`），不是 `min(1280,720)`
/// （那是 logo 尺寸的算法，圆环半径规格另有其字面公式，两者数值恰好相同
/// 纯属巧合，不应共用同一个常量表达不同的语义）。
const OUTRO_RING_RADIUS_STEP_PX: f32 = CANVAS_H * 0.3;
/// 圆环淡出弹簧：时长 0.5s、延迟 1s（规格字面值）。
const OUTRO_RING_OUT_DURATION_SECONDS: f64 = 0.5;
const OUTRO_RING_OUT_DELAY_SECONDS: f64 = 1.0;
const OUTRO_RING_OUT_DURATION_FRAMES: f64 = OUTRO_RING_OUT_DURATION_SECONDS * FPS;
const OUTRO_RING_OUT_DELAY_FRAMES: f64 = OUTRO_RING_OUT_DELAY_SECONDS * FPS;
/// `out_progress` 的钳制上限（协调者裁定的两个等价方案之一）：`spring` 在
/// `elapsed >= duration_frames`（即 `local_frame >= 30+15 = 45`）时**精确**
/// 返回 `1.0`（见 `anim::spring` 文档），若不钳制，`1.0/(1.0-1.0)` = `inf`，
/// 喂给 `tiny_skia::PathBuilder::from_circle` 会产生非有限半径。钳到 `0.99`
/// 对应 scale 上限 100，仍然发散得足够快、不影响观感（120 帧全程测试见
/// `mod tests`）。
const OUTRO_RING_OUT_PROGRESS_MAX: f64 = 0.99;

/// Outro logo：`min(1280,720) * 0.3 = 216`（`min` 在当前画布尺寸下就是
/// `CANVAS_H`，与 `OUTRO_RING_RADIUS_STEP_PX` 数值相同但语义无关，见上）。
const OUTRO_LOGO_SIZE_PX: u32 = (CANVAS_H * 0.3) as u32;
/// logo 自身中心缩放：`0.2 -> 1.0`，首 0.8s 内完成（`[0, 24]` 帧，规格字面值）。
const OUTRO_LOGO_SCALE_IN_FRAMES: [f64; 2] = [0.0, 24.0];
const OUTRO_LOGO_SCALE_RANGE: [f64; 2] = [0.2, 1.0];

/// 品牌名（`Painter::brand`，默认「墨风」）：70px 粗体黑色，紧随 logo 淡入之后（`[24, 39]` 帧）
/// 淡入 + 上移 50px 归位。TS 原版 `whiteSpace: nowrap`（不换行）——沿用既有
/// 代码里表达「不换行」的惯例，给一个远大于画布宽度的 `max_width_px`
/// （见 `WATERMARK_MAX_WIDTH_PX`/`COVER_ROW_TEXT_MAX_WIDTH_PX` 的同款注释）。
const OUTRO_TITLE_FONT_SIZE_PX: f32 = 70.0;
const OUTRO_TITLE_MAX_WIDTH_PX: f32 = 2000.0;
/// logo（未缩放的原生 216px 布局盒）与标题之间的纵向间距（TS `marginTop:40px`）。
/// **CSS `transform: scale()` 不改变布局盒尺寸**：logo 的缩放动画只在其自身
/// 中心原地放大/缩小，纵向组的布局高度按 logo 的 216px 原尺寸计算，标题位置
/// 不随 logo 缩放动画上下移动。
const OUTRO_TITLE_GAP_PX: f32 = 40.0;
const OUTRO_TITLE_FADE_IN_FRAMES: [f64; 2] = [24.0, 39.0];
const OUTRO_TITLE_OPACITY_RANGE: [f64; 2] = [0.0, 1.0];
const OUTRO_TITLE_TRANSLATE_Y_RANGE: [f64; 2] = [-50.0, 0.0];

/// 整体淡出（`[105, 119]` 帧，规格字面值）：作用于圆环、logo、标题、水印
/// 四者，白底不受影响（与 Cover/Intro 的白底恒定不透明一致）。
const OUTRO_FADE_OUT_FRAMES: [f64; 2] = [105.0, 119.0];
const OUTRO_FADE_OUT_RANGE: [f64; 2] = [1.0, 0.0];

/// 把用户自备的图标文件加载成目标尺寸的预乘 `Pixmap`，可直接 `draw_pixmap`。
///
/// `assets::load_icon` 返回 straight-alpha，`Pixmap::from_vec` 要的是预乘，
/// 中间必须过一次 [`premultiply_in_place`]——跳过它会让半透明边缘的颜色偏亮。
fn load_scaled_icon(path: &Path, size: u32) -> anyhow::Result<Pixmap> {
    let (mut rgba, w, h) = crate::assets::load_icon(path, size)?;
    premultiply_in_place(&mut rgba);
    Pixmap::from_vec(rgba, IntSize::from_wh(w, h).context("图标尺寸非零")?)
        .context("图标像素数据长度应与 width*height*4 一致")
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
/// 解码好、已预乘的 logo 源位图，供多次缩放复用。
///
/// **解一次、缩两次**（销 `docs/follow-ups.md`「帧渲染 · 值得做」的 logo
/// 条目）：`Painter::new` 要 36px 与 216px 两个尺寸，此前每次都重新解码一遍
/// 2048² 的 PNG，实测单次 18ms、两次 36ms，占 `new()` 总耗时 132ms 的四分之
/// 一还多。
struct LogoSource {
    rgba: Vec<u8>,
    w: u32,
    h: u32,
}

/// 加载 logo 源：给了路径就用用户那份，没给就用内嵌的。
///
/// 用户那份走 [`crate::assets::load_icon`] 的同一条分流（`.svg` → resvg，
/// `.png` → image）。**按最大的目标尺寸光栅化**：两个目标里 Outro 的 216px
/// 更大，先出 216px 再缩到 36px，比反过来清晰。
fn load_logo_source(path: Option<&Path>) -> anyhow::Result<LogoSource> {
    let (mut rgba, w, h) = match path {
        Some(p) => crate::assets::load_icon(p, OUTRO_LOGO_SIZE_PX)?,
        None => logo_rgba()?,
    };
    premultiply_in_place(&mut rgba);
    Ok(LogoSource { rgba, w, h })
}

/// 把 logo 源缩到目标尺寸。
fn scaled_logo(src: &LogoSource, size_px: u32) -> anyhow::Result<Pixmap> {
    let img = image::RgbaImage::from_raw(src.w, src.h, src.rgba.clone())
        .context("logo 像素数据长度与声明的宽高不匹配")?;
    let resized = image::imageops::resize(
        &img,
        size_px,
        size_px,
        image::imageops::FilterType::Lanczos3,
    );
    let size = IntSize::from_wh(size_px, size_px).context("logo 目标尺寸非零")?;
    Pixmap::from_vec(resized.into_raw(), size)
        .context("logo 缩放后 Pixmap 构造失败：像素数据长度与声明尺寸不匹配")
}

/// Outro 同心圆环的整体缩放：`1 / (1 - out_progress)`，`out_progress` 钳制在
/// `OUTRO_RING_OUT_PROGRESS_MAX` 以内（见该常量文档：不钳制会在
/// `local_frame >= 45` 时除零得到 `inf`）。抽成独立的纯函数，好让
/// `mod tests` 直接对着它断言有限性/单调性，而不是只能靠「画一整帧不 panic」
/// 这种鉴别力很弱的间接判据。
fn outro_ring_scale(local_frame: f64) -> f64 {
    let out_progress = spring(
        local_frame,
        FPS,
        OUTRO_RING_OUT_DURATION_FRAMES,
        OUTRO_RING_OUT_DELAY_FRAMES,
    )
    .min(OUTRO_RING_OUT_PROGRESS_MAX);
    1.0 / (1.0 - out_progress)
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
    text.trim()
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 水印锚点：`content` 用「左下角，给定左/下边距」；`cover`（Task 6）用
/// 「水平居中 + 指定垂直中心」——`center_y_px` 语义见 `cover_watermark_preset`
/// 文档注释（不是 TS `marginTop` 那个字面值，是协调者换算过的居中点）。
#[derive(Clone, Copy)]
enum WatermarkAnchor {
    BottomLeft {
        margin_left_px: f32,
        margin_bottom_px: f32,
    },
    Centered {
        center_y_px: f32,
    },
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
    /// 图标的目标边长与它到文字的间距。**图标画不画由 `Painter` 那边的
    /// `Option<Pixmap>` 决定**，不由这两个数决定——预设只描述「若有图标，
    /// 它多大、离文字多远」。
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

/// 把水印文案切成绘制分段：`·` 单独成段并降到
/// [`WATERMARK_SEP_OPACITY_MUL`]，其余原样。
///
/// 文案不含 `·` 时返回单段，与「整段一个颜色」完全等价。文案**是**一个
/// `·` 时返回一段分隔符——不特判，因为那正是用户配的内容。
fn split_on_separator(text: &str) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(WATERMARK_SEP) {
        if i > 0 {
            out.push((rest[..i].to_string(), 1.0));
        }
        out.push((WATERMARK_SEP.to_string(), WATERMARK_SEP_OPACITY_MUL));
        rest = &rest[i + WATERMARK_SEP.len()..];
    }
    if !rest.is_empty() {
        out.push((rest.to_string(), 1.0));
    }
    out
}

fn content_watermark_preset(text: &str) -> WatermarkPreset {
    WatermarkPreset {
        icon_size_px: WATERMARK_ICON_SIZE_PX,
        icon_gap_px: WATERMARK_ICON_GAP_PX,
        font_size_px: WATERMARK_FONT_SIZE_PX,
        color: WATERMARK_COLOR,
        letter_spacing_px: WATERMARK_FONT_SIZE_PX * WATERMARK_LETTER_SPACING_EM,
        segments: split_on_separator(text),
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
fn cover_watermark_preset(text: &str) -> WatermarkPreset {
    WatermarkPreset {
        icon_size_px: COVER_WATERMARK_ICON_SIZE_PX,
        icon_gap_px: COVER_WATERMARK_ICON_GAP_PX,
        font_size_px: COVER_WATERMARK_FONT_SIZE_PX,
        color: COVER_WATERMARK_COLOR,
        letter_spacing_px: 0.0,
        segments: split_on_separator(text),
        anchor: WatermarkAnchor::Centered {
            center_y_px: COVER_WATERMARK_CENTER_Y_PX,
        },
    }
}

/// 按预设把水印（可选图标 + 分段文字）画到 `pixmap` 上。**唯一一份水印绘制
/// 逻辑**：`content`/`cover`/`outro` 的区别只在传入的 `WatermarkPreset`
/// （字号、颜色、锚点、图标尺寸都由调用方按预设准备好），本函数不认得任何
/// 具体预设的名字。
///
/// `icon` 为 `None` 时整行只有文字，行首就是文字起点——不留图标的空位。
///
/// **图标不重新着色**：原样画上去，只把预设颜色的 alpha 当作它的整体不透明
/// 度（`PixmapPaint::opacity`），这样彩色 logo 保住自己的颜色，而浓淡仍与
/// 同一行的文字一致。
fn layout_and_draw_watermark(
    renderer: &mut TextRenderer,
    pixmap: &mut Pixmap,
    icon: Option<&Pixmap>,
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

    let seg_widths: Vec<f32> = preset
        .segments
        .iter()
        .map(|(text, _)| renderer.measure(text, &style).0)
        .collect();
    // 同一行内所有分段共享字号/行高，高度只取决于 style，与内容无关
    // （见 `TextRenderer::measure` 文档），量第一段即可。
    let text_h = preset
        .segments
        .first()
        .map(|(text, _)| renderer.measure(text, &style).1)
        .unwrap_or(0.0);

    // 有图标时行高取「文字与图标的较大者」，让两者在行内垂直居中对齐。
    let icon_extent = icon.map_or(0.0, |_| preset.icon_size_px as f32);
    let icon_advance = icon.map_or(0.0, |_| preset.icon_size_px as f32 + preset.icon_gap_px);
    let row_height = text_h.max(icon_extent);

    let (row_left, row_center_y) = match preset.anchor {
        WatermarkAnchor::BottomLeft {
            margin_left_px,
            margin_bottom_px,
        } => {
            let row_bottom = CANVAS_H - margin_bottom_px;
            (margin_left_px, row_bottom - row_height / 2.0)
        }
        WatermarkAnchor::Centered { center_y_px } => {
            let total_width = icon_advance + seg_widths.iter().sum::<f32>();
            (CANVAS_W / 2.0 - total_width / 2.0, center_y_px)
        }
    };

    if let Some(icon) = icon {
        let icon_top = row_center_y - icon_extent / 2.0;
        pixmap.draw_pixmap(
            row_left.round() as i32,
            icon_top.round() as i32,
            icon.as_ref(),
            &PixmapPaint {
                opacity: f32::from(preset.color[3]) / 255.0,
                ..PixmapPaint::default()
            },
            Transform::identity(),
            None,
        );
    }

    let mut cursor_x = row_left + icon_advance;
    for (seg, &w) in preset.segments.iter().zip(seg_widths.iter()) {
        let (text, opacity_mul) = seg;
        if !text.is_empty() {
            let mut seg_style = style.clone();
            seg_style.color[3] = (f32::from(style.color[3]) * *opacity_mul)
                .round()
                .clamp(0.0, 255.0) as u8;
            let center_x = cursor_x + w / 2.0;
            renderer.draw_centered(pixmap, text, center_x, row_center_y, &seg_style, 1.0, 1.0);
        }
        cursor_x += w;
    }
}

/// 某个 `WatermarkPreset` 预渲染出的一张小图（已裁剪到恰好包住
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
    icon: Option<&Pixmap>,
    preset: &WatermarkPreset,
) -> anyhow::Result<PreparedWatermark> {
    let mut scratch =
        Pixmap::new(CANVAS_W as u32, CANVAS_H as u32).context("水印预渲染暂存画布分配失败")?;
    layout_and_draw_watermark(renderer, &mut scratch, icon, preset);

    let Some((x0, y0, x1, y1)) = non_transparent_bbox(&scratch) else {
        // 预设没有画出任何东西（理论上不会发生，防御性兜底）：1x1 透明占位，
        // 贴图时等于什么都不画。
        let empty = Pixmap::new(1, 1).context("占位画布分配失败")?;
        return Ok(PreparedWatermark {
            pixmap: empty,
            origin_x: 0,
            origin_y: 0,
        });
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

    Ok(PreparedWatermark {
        pixmap: cropped,
        origin_x: cx0,
        origin_y: cy0,
    })
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
        &PixmapPaint {
            opacity: opacity.clamp(0.0, 1.0),
            ..Default::default()
        },
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
    /// 品牌名，画在 Cover 上排与 Outro 大字上。整片恒定，故存在这里而不是
    /// 逐帧当参数传——`draw_content` 用不到它，当参数传会让三个 `draw_*`
    /// 的签名各不相同。
    brand: String,
    /// `content` 预设的水印，`new()` 里预渲染一次（I1/I2 修复）。
    /// `None` = 未配置 `--watermark`/`$WATERMARK`，整段不画。
    content_watermark: Option<PreparedWatermark>,
    /// `cover` 预设的水印，`new()` 里预渲染一次（Task 6）。
    /// `None` = 未配置 `--watermark-cover`/`$WATERMARK_COVER`，Cover 与
    /// Outro 都不画。
    cover_watermark: Option<PreparedWatermark>,
    /// Cover 上排用的 36px logo，`new()` 里用 Lanczos3 缩好一次缓存起来
    /// （Task 6；Task 7 的 outro 216px 版本是并列的另一个字段，见 `logo_216`）。
    logo_36: Pixmap,
    /// Outro 用的 216px logo（Task 7），同样在 `new()` 里缩好一次缓存起来。
    logo_216: Pixmap,
}

impl Painter {
    /// 未配置的水印**不预渲染**：`prepare_watermark` 要分配一张 1280×720 的
    /// 暂存画布、排版一次再裁剪，为一段空文案付这笔开销没有意义，而且
    /// `Some(空白预设)` 与 `None` 在成片上都是「什么都不画」——留两条等价
    /// 路径只会让「到底画没画」多一种说法。
    pub fn new(branding: &Branding) -> anyhow::Result<Self> {
        let mut renderer = TextRenderer::new()?;
        // 图标按两处各自的目标尺寸分别加载一次。**共用一个配置项、但不是
        // 共用一张位图**：正文 28px、封面/片尾 32px，各自按目标尺寸光栅化
        // （SVG 走矢量渲染、PNG 走 Lanczos3 缩放）比缩一张再二次缩放清晰。
        let icon_path = branding.watermark_icon.as_deref().map(Path::new);
        let content_icon = icon_path
            .map(|p| load_scaled_icon(p, WATERMARK_ICON_SIZE_PX))
            .transpose()?;
        let cover_icon = icon_path
            .map(|p| load_scaled_icon(p, COVER_WATERMARK_ICON_SIZE_PX))
            .transpose()?;
        let content_watermark = branding
            .watermark
            .as_deref()
            .map(|t| {
                prepare_watermark(
                    &mut renderer,
                    content_icon.as_ref(),
                    &content_watermark_preset(t),
                )
            })
            .transpose()?;
        let cover_watermark = branding
            .watermark_cover
            .as_deref()
            .map(|t| {
                prepare_watermark(
                    &mut renderer,
                    cover_icon.as_ref(),
                    &cover_watermark_preset(t),
                )
            })
            .transpose()?;
        let logo_src = load_logo_source(branding.logo.as_deref().map(Path::new))?;
        let logo_36 = scaled_logo(&logo_src, COVER_LOGO_SIZE_PX)?;
        let logo_216 = scaled_logo(&logo_src, OUTRO_LOGO_SIZE_PX)?;
        Ok(Self {
            renderer,
            brand: branding.brand.clone(),
            content_watermark,
            cover_watermark,
            logo_36,
            logo_216,
        })
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
        if let Some(wm) = &self.content_watermark {
            draw_watermark(pixmap, wm, 1.0);
        }
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
    /// （logo + 品牌名，整体 0.30 透明度，左对齐）+ 主标题（100px
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
        let container_top = cover_container_center_y(Canvas::BASE) - container_h / 2.0;
        let row_center_y = container_top + COVER_ROW_HEIGHT_PX / 2.0;
        let title_center_y = container_top + COVER_ROW_HEIGHT_PX + title_h / 2.0;

        // 上排：logo（左对齐，四周 margin 8px，整体 0.30 透明度）。
        let logo_left = COVER_ROW_LEFT_PX + COVER_LOGO_MARGIN_PX;
        let logo_top = row_center_y - COVER_LOGO_SIZE_PX as f32 / 2.0;
        pixmap.draw_pixmap(
            logo_left.round() as i32,
            logo_top.round() as i32,
            self.logo_36.as_ref(),
            &PixmapPaint {
                opacity: COVER_ROW_OPACITY,
                ..Default::default()
            },
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
        let (row_text_w, _) = self.renderer.measure(&self.brand, &row_text_style);
        let row_text_center_x = COVER_ROW_TEXT_LEFT_PX + row_text_w / 2.0;
        self.renderer.draw_centered(
            pixmap,
            &self.brand,
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

        if let Some(wm) = &self.cover_watermark {
            draw_watermark(pixmap, wm, 1.0);
        }
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

    /// 绘制 Outro 段一帧（规格 §8.4「Outro」小节）：不透明白底 + 同心圆环
    /// （不可见，忠实移植）+ logo（自身中心缩放）+ 固定标题「熊猫智研社」
    /// （紧随 logo 淡入 + 上移归位）+ `cover` 预设水印，末尾整体淡出。
    pub fn draw_outro(&mut self, pixmap: &mut Pixmap, local_frame: u32) {
        pixmap.fill(Color::from_rgba8(255, 255, 255, 255));

        let frame = local_frame as f64;
        let fade_opacity = interpolate(frame, OUTRO_FADE_OUT_FRAMES, OUTRO_FADE_OUT_RANGE) as f32;
        if fade_opacity <= 0.0 {
            return;
        }

        // 同心圆环：5 个白色实心圆，倒序绘制（大的先画）。白底上的白色实心圆
        // 视觉不可见，但仍照规格忠实画出（协调者裁定，见 `OUTRO_RING_COUNT`
        // 文档）。`out_progress`/`scale` 的钳制见 `outro_ring_scale`。
        let ring_scale = outro_ring_scale(frame);
        let ring_alpha = (255.0 * fade_opacity).round().clamp(0.0, 255.0) as u8;
        if ring_alpha > 0 {
            let mut ring_paint = Paint::default();
            ring_paint.set_color_rgba8(255, 255, 255, ring_alpha);
            ring_paint.anti_alias = true;
            for i in (0..OUTRO_RING_COUNT).rev() {
                let radius = OUTRO_RING_RADIUS_STEP_PX as f64 * f64::from(i) * ring_scale;
                if radius <= 0.0 {
                    continue; // i=0：半径 0，`from_circle` 对非正半径返回 None
                }
                if let Some(path) =
                    PathBuilder::from_circle(CANVAS_W / 2.0, CANVAS_H / 2.0, radius as f32)
                {
                    pixmap.fill_path(
                        &path,
                        &ring_paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }

        // logo + 标题的纵向组：整体垂直居中于画布中心，`justify-center
        // items-center` + `flexDirection: column` 语义。布局盒尺寸按 logo 的
        // 216px **原尺寸**计算（`transform: scale()` 不改变布局盒尺寸），
        // 标题位置因此不随 logo 的缩放动画上下移动。
        let title_style = TextStyle {
            size_px: OUTRO_TITLE_FONT_SIZE_PX,
            color: TITLE_COLOR_BLACK,
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: OUTRO_TITLE_MAX_WIDTH_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: true,
        };
        let (_, title_h) = self.renderer.measure(&self.brand, &title_style);
        let logo_size = OUTRO_LOGO_SIZE_PX as f32;
        let group_h = logo_size + OUTRO_TITLE_GAP_PX + title_h;
        let group_top = CANVAS_H / 2.0 - group_h / 2.0;
        let logo_center_x = CANVAS_W / 2.0;
        let logo_center_y = group_top + logo_size / 2.0;
        let title_top = group_top + logo_size + OUTRO_TITLE_GAP_PX;

        // Logo：以自身中心缩放，`scale` 0.2 -> 1.0。
        let logo_scale =
            interpolate(frame, OUTRO_LOGO_SCALE_IN_FRAMES, OUTRO_LOGO_SCALE_RANGE) as f32;
        let half = logo_size / 2.0;
        let logo_transform = Transform::from_translate(-half, -half)
            .post_scale(logo_scale, logo_scale)
            .post_translate(logo_center_x, logo_center_y);
        // **修复轮 1（I2）**：`PixmapPaint::default()` 的 `quality` 是
        // `FilterQuality::Nearest`——这里的 transform 带缩放（frame 0..23
        // 把 216px logo 最近邻缩到 43..216px），最近邻在圆形边缘会产生肉眼
        // 可见的锯齿/爬行。Cover 的 36px logo 不构成先例：那里的 transform
        // 是 `Transform::identity()`（不缩放），不经过这条重采样路径。这是
        // Outro 唯一带缩放动画的位图绘制，改成 `Bilinear`。
        pixmap.draw_pixmap(
            0,
            0,
            self.logo_216.as_ref(),
            &PixmapPaint {
                opacity: fade_opacity,
                quality: FilterQuality::Bilinear,
                ..Default::default()
            },
            logo_transform,
            None,
        );

        // 标题：紧随 logo 淡入之后（[24,39] 帧）淡入 + 从上方 50px 处归位；
        // `translate_y` 只作用于标题自身、不影响 logo（两者是纵向组里独立的
        // 两个元素，各自的入场动画互不耦合）。
        let title_fade_in =
            interpolate(frame, OUTRO_TITLE_FADE_IN_FRAMES, OUTRO_TITLE_OPACITY_RANGE) as f32;
        let title_opacity = title_fade_in * fade_opacity;
        let translate_y = interpolate(
            frame,
            OUTRO_TITLE_FADE_IN_FRAMES,
            OUTRO_TITLE_TRANSLATE_Y_RANGE,
        ) as f32;
        let title_center_y = title_top + title_h / 2.0 + translate_y;
        self.renderer.draw_centered(
            pixmap,
            &self.brand,
            CANVAS_W / 2.0,
            title_center_y,
            &title_style,
            title_opacity,
            1.0,
        );

        if let Some(wm) = &self.cover_watermark {
            draw_watermark(pixmap, wm, fade_opacity);
        }
    }
}

#[cfg(test)]
#[path = "draw_tests.rs"]
mod tests;
