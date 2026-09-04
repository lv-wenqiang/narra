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
