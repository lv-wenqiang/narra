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
#[allow(dead_code)]
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
        // `f32::round()` 将 .5 舍入远离零，所以半像素恰好落在 .5 时（如 28.0 * 1.125 = 31.5）
        // 会一致地向上舍入。
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
    ///
    /// 本测试逐一断言全部 25 个字段，而非抽样检查：在 BASE 尺寸处 `scale == 1.0`，
    /// 忘记乘 `* s` 的字段会被掩盖（`12.0` 与 `12.0 * 1.0` 都给出 12.0）。
    /// 只在抽样字段上断言会让大多数字段的缩放回归无人察觉。
    #[test]
    fn every_length_scales_linearly_including_the_animation_offsets() {
        let base = Metrics::for_canvas(Canvas::BASE);
        let double = Metrics::for_canvas(Canvas { w: 2560, h: 720 });
        // f32 fields (22)
        assert_eq!(double.caption_padding_x, base.caption_padding_x * 2.0);
        assert_eq!(
            double.caption_font_size_long,
            base.caption_font_size_long * 2.0
        );
        assert_eq!(
            double.caption_font_size_short,
            base.caption_font_size_short * 2.0
        );
        assert_eq!(double.caption_stroke_width, base.caption_stroke_width * 2.0);
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
            double.watermark_margin_left,
            base.watermark_margin_left * 2.0
        );
        assert_eq!(
            double.watermark_margin_bottom,
            base.watermark_margin_bottom * 2.0
        );
        assert_eq!(double.watermark_font_size, base.watermark_font_size * 2.0);
        assert_eq!(double.watermark_icon_gap, base.watermark_icon_gap * 2.0);
        assert_eq!(
            double.cover_watermark_font_size,
            base.cover_watermark_font_size * 2.0
        );
        assert_eq!(
            double.cover_watermark_icon_gap,
            base.cover_watermark_icon_gap * 2.0
        );
        assert_eq!(
            double.cover_row_margin_left,
            base.cover_row_margin_left * 2.0
        );
        assert_eq!(double.cover_logo_margin, base.cover_logo_margin * 2.0);
        assert_eq!(
            double.cover_row_text_font_size,
            base.cover_row_text_font_size * 2.0
        );
        assert_eq!(
            double.cover_title_font_size,
            base.cover_title_font_size * 2.0
        );
        assert_eq!(double.cover_title_padding, base.cover_title_padding * 2.0);
        assert_eq!(
            double.intro_title_font_size,
            base.intro_title_font_size * 2.0
        );
        assert_eq!(double.intro_cursor_gap, base.intro_cursor_gap * 2.0);
        assert_eq!(
            double.outro_title_font_size,
            base.outro_title_font_size * 2.0
        );
        assert_eq!(double.outro_title_gap, base.outro_title_gap * 2.0);
        assert_eq!(
            double.outro_title_translate_y_from,
            base.outro_title_translate_y_from * 2.0,
            "Outro 标题的归位位移是像素，必须跟着缩放"
        );
        // u32 fields (3)
        assert_eq!(double.watermark_icon_size, base.watermark_icon_size * 2);
        assert_eq!(
            double.cover_watermark_icon_size,
            base.cover_watermark_icon_size * 2
        );
        assert_eq!(double.cover_logo_size, base.cover_logo_size * 2);
    }
}
