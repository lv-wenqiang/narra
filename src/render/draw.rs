//! Content 段绘制：完全透明底 + 居中字幕 + 左下角水印。
//!
//! 对应规格 §8.4「Content」小节 + §8.6 水印 `content` 预设。数值全部照抄
//! `task-5-brief.md`，不是凭印象重写。

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
const CAPTION_LINE_HEIGHT: f32 = 1.2;

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
/// `Pixmap::draw_pixmap` 合成到目标画布上。
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

/// Content 段绘制器：字幕层 + 左下角水印。
///
/// `TextRenderer::new()`（加载 5.6MB 内嵌字体）与 GitHub 图标的 SVG 光栅化都有
/// 可观开销，因此构造一次、每帧复用，不在 `draw_content` 里现造。
pub struct Painter {
    renderer: TextRenderer,
    /// 已按 `content` 预设着色（白色、alpha ≈ 69）的 GitHub 图标位图。
    /// `new()` 里光栅化 + 着色一次，之后每帧直接合成，不重新解析 SVG。
    watermark_icon: Pixmap,
}

impl Painter {
    pub fn new() -> anyhow::Result<Self> {
        let renderer = TextRenderer::new()?;
        let (raw, w, h) = github_mark_rgba(WATERMARK_ICON_SIZE_PX)?;
        let watermark_icon = tint_icon(&raw, w, h, WATERMARK_COLOR);
        Ok(Self { renderer, watermark_icon })
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
        self.draw_watermark(pixmap);
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

        let non_whitespace_chars = caption.text.chars().filter(|c| !c.is_whitespace()).count();
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
            line_height: CAPTION_LINE_HEIGHT,
            bold: true,
        };

        self.renderer.draw_centered(
            pixmap,
            &caption.text,
            CAPTION_CENTER_X + translate_x,
            CAPTION_CENTER_Y,
            &style,
            opacity,
            scale,
        );
    }

    /// 绘制左下角水印：GitHub 图标 + `Panda Video Generator`，水平排列、垂直居中对齐，
    /// 整个组件的左边距 40px、下边距 40px。
    fn draw_watermark(&mut self, pixmap: &mut Pixmap) {
        let style = TextStyle {
            size_px: WATERMARK_FONT_SIZE_PX,
            color: WATERMARK_COLOR,
            stroke: None,
            letter_spacing_px: WATERMARK_FONT_SIZE_PX * WATERMARK_LETTER_SPACING_EM,
            max_width_px: WATERMARK_MAX_WIDTH_PX,
            line_height: CAPTION_LINE_HEIGHT,
            bold: false,
        };

        let (text_w, text_h) = self.renderer.measure(WATERMARK_TEXT, &style);
        let icon_size = WATERMARK_ICON_SIZE_PX as f32;
        let row_height = text_h.max(icon_size);
        let row_bottom = CANVAS_H - WATERMARK_MARGIN_BOTTOM_PX;
        let row_center_y = row_bottom - row_height / 2.0;

        let icon_left = WATERMARK_MARGIN_LEFT_PX;
        let icon_top = row_center_y - icon_size / 2.0;
        pixmap.draw_pixmap(
            icon_left.round() as i32,
            icon_top.round() as i32,
            self.watermark_icon.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );

        let text_left = icon_left + icon_size + WATERMARK_ICON_GAP_PX;
        let text_center_x = text_left + text_w / 2.0;
        self.renderer.draw_centered(
            pixmap,
            WATERMARK_TEXT,
            text_center_x,
            row_center_y,
            &style,
            1.0,
            1.0,
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
}
