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
            // 整数等式而非「先乘 100 再整除再比对 225」：后者会截断，例如
            // 1920x1081 的像素量整除后同样落在 225，也会侥幸通过。
            // `px * 4 == base_px * 9` 是同一个 2.25 倍关系的精确整数形式。
            assert_eq!(
                px * 4,
                base_px * 9,
                "{}x{} 的像素量应恰为 BASE 的 2.25 倍",
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
}
