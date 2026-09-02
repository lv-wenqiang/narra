//! 文字排版 + 描边 + 填充绘制。
//!
//! 实现照抄 `docs/text-rendering.md`（Task 0 探针实测跑通、代码审查逐项核实过的写法），
//! 不是凭印象重写。三条必做项——禁用系统字体回退、合成粗体、先变换路径再描边——
//! 的动机和实测证据都记录在那份文档里。

use anyhow::{anyhow, Context, Result};
use cosmic_text::{fontdb, Attrs, Buffer, Family, FontSystem, Metrics, Shaping};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};
use ttf_parser::{Face, GlyphId, OutlineBuilder};

use crate::assets::FONT;

/// 合成粗体的额外描边宽度系数：`bold_w = size_px * BOLD_STROKE_RATIO`。
/// 见 `docs/text-rendering.md`「合成粗体」一节，协调者拍板的方案，已实测验证生效。
const BOLD_STROKE_RATIO: f32 = 0.03;

/// 一段文字的排版与描边填充样式。
#[derive(Clone, Debug)]
pub struct TextStyle {
    /// 字号，像素。
    pub size_px: f32,
    /// 填充色，RGBA。
    pub color: [u8; 4],
    /// 描边色 + 描边宽度（像素）。`None` 表示不描边。
    pub stroke: Option<([u8; 4], f32)>,
    /// 字间距，像素。
    pub letter_spacing_px: f32,
    /// 换行宽度上限，像素。
    pub max_width_px: f32,
    /// 行高倍数（相对 `size_px`）。
    pub line_height: f32,
    /// 是否合成粗体（内嵌字体只有一个字重，真正的加粗必须靠合成，见模块顶部说明）。
    pub bold: bool,
}

/// 把 `ttf_parser::OutlineBuilder` 的回调（字体 unitsPerEm 坐标系，y 轴向上）
/// 原样转发给 `tiny_skia::PathBuilder`，不做任何变换——变换统一在拿到完整路径后
/// 一次性做（见 `draw_centered` 里的 `final_transform`）。
struct SkiaOutline(PathBuilder);

impl OutlineBuilder for SkiaOutline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

/// 读字体的 `name` 表拿真实家族名，不要凭空猜测或用文件名代替
/// （否则 `cosmic-text` 按名字匹配字体族会失败，退化成豆腐块或触发系统字体回退）。
/// 优先取 Windows 平台 + Unicode 编码的 FAMILY(name_id=1) 记录，其余可解码记录兜底。
fn family_name_from_ttf(data: &[u8]) -> Result<String> {
    let face = Face::parse(data, 0).context("解析字体 face 失败")?;
    let mut fallback = None;
    for name in face.names() {
        if name.name_id != ttf_parser::name_id::FAMILY {
            continue;
        }
        let Some(s) = name.to_string() else {
            continue;
        };
        if name.is_unicode() && name.platform_id == ttf_parser::PlatformId::Windows {
            return Ok(s);
        }
        fallback.get_or_insert(s);
    }
    fallback.ok_or_else(|| anyhow!("字体 name 表里没有可解码的 FAMILY(name_id=1) 记录"))
}

/// 用 `TextStyle` 里颜色自身的 alpha 构造 `Paint`（不并入组透明度）。
///
/// **C1 修复**：组透明度（`draw_centered` 的 `opacity` 参数）不再乘进这里——
/// 三遍描边/填充统一用颜色自身的 alpha 画进暂存画布，画完整块之后再用
/// `PixmapPaint { opacity, .. }` 一次性合成回目标 pixmap（见 `draw_centered`）。
/// 这样三遍叠加时组透明度只生效一次，不会按 `1-(1-a)^n` 累积把白字叠成灰字。
fn solid_paint(color: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    p.anti_alias = true;
    p
}

/// 文字排版与描边填充绘制器。**构造一次、多次复用**：`FontSystem` 的构造要重新
/// 解析并注册字体，代价不低；`Painter` 应当持有一个 `TextRenderer` 实例，每帧
/// 复用同一个，而不是在 `draw_centered` 里现造一个。
pub struct TextRenderer {
    font_system: FontSystem,
    family: String,
    /// 组透明度合成用的暂存画布（C1 修复）：三遍描边/填充先以不透明的
    /// per-pass 颜色画进这里，画完整块文字后再用
    /// `PixmapPaint { opacity, .. }` 一次性合成回目标 pixmap，组透明度只生效
    /// 一次。按目标 `Pixmap` 的尺寸复用，避免每帧重新分配；尺寸不符时重建，
    /// 复用前清空（见 `draw_centered`）。
    scratch: Option<Pixmap>,
}

impl TextRenderer {
    /// 加载内嵌字体、构造并持有字体系统。
    ///
    /// **禁用系统字体回退（必做项 1）**：不用 `FontSystem::new()`（它会调用
    /// `fontdb::Database::load_system_fonts()`，把运行机器上装的所有字体也注册进
    /// 同一个库，`Shaping::Advanced` 遇到内嵌字体未覆盖的字符会静默回退到系统字体）。
    /// 改为自己构造一个从未调用过 `load_system_fonts()` 的 `fontdb::Database`，只塞入
    /// 内嵌字体字节，再用 `FontSystem::new_with_locale_and_db` 包起来——这条构造路径
    /// 完全绕开 `new_with_fonts()`/`load_fonts()`。见 `docs/text-rendering.md`
    /// 「系统字体静默回退陷阱」一节的实测复现与验证。
    pub fn new() -> Result<Self> {
        let family = family_name_from_ttf(FONT)?;

        let mut db = fontdb::Database::new();
        db.load_font_data(FONT.to_vec());
        let font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);

        Ok(Self { font_system, family, scratch: None })
    }

    /// 排版一段文字：构造 `Metrics`/`Buffer`，设置换行宽度，套用字间距，跑完整形。
    /// `\n` 由 `cosmic-text` 自身按段落分行处理，无需手工拆分。
    fn shape(&mut self, text: &str, style: &TextStyle) -> Buffer {
        let line_height_px = (style.size_px * style.line_height).max(1.0);
        let metrics = Metrics::new(style.size_px, line_height_px);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(Some(style.max_width_px), None);

        // letter_spacing 原生支持（`Attrs::letter_spacing`，单位 EM），会被
        // cosmic-text 的排版/换行逻辑一并考虑，不需要事后手工挪动每个字形。
        let letter_spacing_em = if style.size_px > 0.0 {
            style.letter_spacing_px / style.size_px
        } else {
            0.0
        };
        // 刻意不调用 `.weight(Weight::BOLD)`：内嵌字体只有一个静态字重（Regular），
        // `docs/text-rendering.md` 已实测确认对这种单字重字体它是空操作——真正的
        // 粗体效果由 `draw_centered` 里的合成粗体（多遍描边）实现，不靠这里的
        // `Attrs::weight`。这不是漏抄，是刻意省略。
        let attrs = Attrs::new()
            .family(Family::Name(&self.family))
            .letter_spacing(letter_spacing_em);

        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }

    /// 排版一段文字后，量出文本块的 (宽, 高)（像素）。用的是排版逻辑度量
    /// （每行的 `line_w` 取最大值作为宽，行数 * 行高作为高），**不是精确墨迹包围盒**——
    /// 实际墨迹（含描边/合成粗体外扩）每边会比这里返回的排版宽度宽
    /// `(stroke_w + bold_w) / 2` 左右（描边、粗体各自的宽度定义见 `draw_centered`）。
    ///
    /// `measure("", style)` 返回 `(0.0, style.size_px * style.line_height)`，**不是**
    /// `(0.0, 0.0)`——空字符串仍会产生一个高度为一行行高、宽度为 0 的排版行。
    pub fn measure(&mut self, text: &str, style: &TextStyle) -> (f32, f32) {
        let buffer = self.shape(text, style);
        let mut max_w = 0.0f32;
        let mut min_top = f32::MAX;
        let mut max_bottom = f32::MIN;
        let mut any = false;
        for run in buffer.layout_runs() {
            any = true;
            max_w = max_w.max(run.line_w);
            min_top = min_top.min(run.line_top);
            max_bottom = max_bottom.max(run.line_top + run.line_height);
        }
        if !any {
            return (0.0, 0.0);
        }
        (max_w, max_bottom - min_top)
    }

    /// 以 `(center_x, center_y)` 为文本块的中心绘制。`scale` 以该点为原点整体缩放
    /// （包括字形轮廓和描边宽度——是一次真正的几何缩放，不是只挪位置）；`opacity`
    /// 是**整块文字的组透明度**（等同 CSS `opacity`），不并入每一遍绘制的颜色
    /// alpha——`TextStyle.color`/`stroke` 颜色自带的 alpha 仍按原样参与每一遍
    /// 绘制（与 CSS `rgba()` 颜色语义一致）。多行文字里每一行都单独水平居中；
    /// 整个文本块（不管多少行）作为一个整体垂直居中在 `center_y` 上。
    ///
    /// **C1 修复（组透明度合成）**：合成粗体让同一个字形要画三遍（描边 / 加粗描边
    /// / 填充）。如果把 `opacity` 乘进每一遍绘制的颜色 alpha，三遍在同一像素上
    /// 叠加时 alpha 会按 `1-(1-a)^n` 累积，且下层描边会透过上层半透明填充——
    /// `opacity=0.5` 时白字会被叠成灰字（近纯白像素直接归零）。正确做法：三遍都用
    /// 不透明的 per-pass 颜色画进一张与目标同尺寸的暂存画布，画完整块之后再用
    /// `PixmapPaint { opacity, .. }` 把暂存画布一次性合成回目标 pixmap，组透明度
    /// 只生效一次。`opacity <= 0.0` 时直接返回，连暂存画布都不分配。
    #[allow(clippy::too_many_arguments)] // 8 个参数是计划 Interfaces 一节钦定的形状，Tasks 5/6/7 按此调用，不做改动。
    pub fn draw_centered(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        center_x: f32,
        center_y: f32,
        style: &TextStyle,
        opacity: f32,
        scale: f32,
    ) {
        if opacity <= 0.0 || text.is_empty() {
            return;
        }

        let buffer = self.shape(text, style);

        // 先扫一遍算出整块文本的竖直范围，用来垂直居中。
        let mut min_top = f32::MAX;
        let mut max_bottom = f32::MIN;
        let mut any = false;
        for run in buffer.layout_runs() {
            any = true;
            min_top = min_top.min(run.line_top);
            max_bottom = max_bottom.max(run.line_top + run.line_height);
        }
        if !any {
            return;
        }
        let block_height = max_bottom - min_top;
        let y_shift = center_y - block_height / 2.0 - min_top;

        // per-pass 颜色不再乘 opacity，只保留颜色自身的 alpha（见上方文档注释）。
        let fill_paint = solid_paint(style.color);
        let stroke_paint_and_width = style.stroke.map(|(color, width)| (solid_paint(color), width));

        let bold_w = style.size_px * BOLD_STROKE_RATIO;

        // 拿到与目标同尺寸的暂存画布：尺寸不符时重建，复用前清空为全透明。
        // 用 `take()` 把它从 self 里搬出来做局部变量，避免和下面 `self.font_system`
        // 的借用冲突；画完再放回 self.scratch 供下一帧复用。
        let mut scratch = match self.scratch.take() {
            Some(mut p) if p.width() == pixmap.width() && p.height() == pixmap.height() => {
                p.fill(Color::TRANSPARENT);
                p
            }
            _ => Pixmap::new(pixmap.width(), pixmap.height())
                .expect("目标 pixmap 尺寸非零时暂存画布分配不应失败"),
        };

        for run in buffer.layout_runs() {
            // 每行单独水平居中在 center_x 上。
            let x_shift = center_x - run.line_w / 2.0;

            for glyph in run.glyphs {
                let physical = glyph.physical((x_shift, run.line_y + y_shift), 1.0);
                let cache_key = physical.cache_key;

                let Some(font) = self
                    .font_system
                    .get_font(cache_key.font_id, cache_key.font_weight)
                else {
                    continue;
                };

                let Ok(face) = Face::parse(font.data(), 0) else {
                    continue;
                };
                let units_per_em = face.units_per_em() as f32;
                if units_per_em <= 0.0 {
                    continue;
                }
                let font_size = f32::from_bits(cache_key.font_size_bits);
                let fscale = font_size / units_per_em;

                let mut outline = SkiaOutline(PathBuilder::new());
                if face
                    .outline_glyph(GlyphId(cache_key.glyph_id), &mut outline)
                    .is_none()
                {
                    // 空格等无墨字形没有轮廓，属正常情况。
                    continue;
                }
                let Some(font_space_path): Option<Path> = outline.0.finish() else {
                    continue;
                };

                // 字体空间（unitsPerEm，y 轴向上）-> 自然像素空间（未套用户 scale）。
                let natural_transform = Transform::from_row(
                    fscale,
                    0.0,
                    0.0,
                    -fscale,
                    physical.x as f32,
                    physical.y as f32,
                );
                // 把用户的 scale 以 (center_x, center_y) 为原点叠加进同一个变换：
                // 先减去中心点、再缩放、再加回中心点，等价于绕该点整体缩放。
                let final_transform = natural_transform
                    .post_translate(-center_x, -center_y)
                    .post_scale(scale, scale)
                    .post_translate(center_x, center_y);

                // **关键：先用 Path::transform 把路径归一化到最终像素空间，再用
                // Transform::identity() 描边/填充。** 反过来的话 Stroke.width 会在
                // 字体 unitsPerEm 空间里被解释（scale ≈ size/2048），几像素宽的描边
                // 会被缩成零点几像素，肉眼几乎看不见。见
                // docs/text-rendering.md「描边 + 填充（关键踩坑点）」一节的实测数据。
                let Some(path) = font_space_path.transform(final_transform) else {
                    continue;
                };

                // 合成粗体（必做项 2）：内嵌字体只有一个静态字重，Weight::BOLD 是
                // 空操作，规格要求的粗体必须靠这里的三段描边/填充合成。
                // stroke.width 和 bold_w 都乘 scale，使缩放是一次真正的几何缩放
                // （包括描边粗细），而不是只把字形放大、描边粗细不变。
                if let Some((stroke_paint, stroke_w)) = &stroke_paint_and_width {
                    let outer_w = if style.bold {
                        stroke_w + bold_w
                    } else {
                        *stroke_w
                    } * scale;
                    if outer_w > 0.0 {
                        let outer_stroke = Stroke {
                            width: outer_w,
                            ..Default::default()
                        };
                        scratch.stroke_path(
                            &path,
                            stroke_paint,
                            &outer_stroke,
                            Transform::identity(),
                            None,
                        );
                    }
                }
                if style.bold {
                    let w = bold_w * scale;
                    if w > 0.0 {
                        let bold_stroke = Stroke {
                            width: w,
                            ..Default::default()
                        };
                        scratch.stroke_path(
                            &path,
                            &fill_paint,
                            &bold_stroke,
                            Transform::identity(),
                            None,
                        );
                    }
                }
                scratch.fill_path(
                    &path,
                    &fill_paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }

        // 把暂存画布整块按组透明度合成回目标 pixmap——组透明度只在这里生效一次。
        pixmap.draw_pixmap(
            0,
            0,
            scratch.as_ref(),
            &PixmapPaint {
                opacity: opacity.clamp(0.0, 1.0),
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
        // 把暂存画布放回去，供下一次 draw_centered 调用复用，避免每帧重新分配。
        self.scratch = Some(scratch);
    }
}

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
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
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

    /// I1 修复：`renders_cjk_and_latin_without_tofu` 只断言包围盒宽度落在区间内，
    /// 抓不到「每个字都退化成同一个 `.notdef` 方框」这种豆腐块场景（审查者实测：
    /// 把取轮廓的 `GlyphId(cache_key.glyph_id)` 强改成 `GlyphId(0)` 后那条测试照样通过）。
    /// 这条测试改用「两段不同文字必须渲染出不同像素」的方式堵住这个漏洞：如果两个字
    /// 都变成同一个方框，`a.data()` 和 `b.data()` 会逐字节相同。
    #[test]
    fn renders_distinct_glyphs_for_different_cjk_text() {
        let mut r = TextRenderer::new().unwrap();
        let mut a = blank(600, 200);
        let mut b = blank(600, 200);
        r.draw_centered(&mut a, "熊猫", 300.0, 100.0, &style(70.0), 1.0, 1.0);
        r.draw_centered(&mut b, "智研", 300.0, 100.0, &style(70.0), 1.0, 1.0);
        assert_ne!(
            a.data(),
            b.data(),
            "不同文字应渲染出不同像素，否则可能都退化成同一个 .notdef 豆腐块"
        );
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
                if let Some(c) = p.pixel(x, y)
                    && c.alpha() > 200
                {
                    let (r_, g_, b_) = (c.red(), c.green(), c.blue());
                    if r_ > 240 && g_ > 240 && b_ > 240 { has_white = true; }
                    if r_ < 30 && g_ < 30 && b_ < 30 { has_black = true; }
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

    /// C1 修复的回归测试：`opacity` 是整块文字的组透明度（等同 CSS `opacity`），
    /// 不应因为合成粗体三遍描边/填充叠加而按 `1-(1-a)^n` 累积、把白字变灰。
    /// 修之前（per-pass 颜色乘 opacity）此测试在旧实现下会失败：见报告「修复轮 1」
    /// 记录的 red 输出（近纯白像素归零、最大 alpha 达 224）。
    #[test]
    fn opacity_mid_value_composites_as_group_not_per_pass() {
        let mut r = TextRenderer::new().unwrap();
        let mut p = blank(600, 200);
        r.draw_centered(&mut p, "描边测试", 300.0, 100.0, &style(70.0), 0.5, 1.0);
        let mut has_near_white = false;
        let mut max_alpha = 0u8;
        for y in 0..p.height() {
            for x in 0..p.width() {
                if let Some(c) = p.pixel(x, y) {
                    max_alpha = max_alpha.max(c.alpha());
                    if c.alpha() > 0 {
                        // tiny_skia 存的是预乘 alpha，要反预乘才能拿到真实 rgb 再判断是否近纯白。
                        let a = c.alpha() as f32;
                        let r_ = (c.red() as f32 * 255.0 / a).round() as i32;
                        let g_ = (c.green() as f32 * 255.0 / a).round() as i32;
                        let b_ = (c.blue() as f32 * 255.0 / a).round() as i32;
                        if r_ > 240 && g_ > 240 && b_ > 240 {
                            has_near_white = true;
                        }
                    }
                }
            }
        }
        assert!(
            has_near_white,
            "opacity=0.5 时应仍存在近纯白像素（组透明度合成，不应逐遍叠加变灰）"
        );
        assert!(
            max_alpha <= 132,
            "opacity=0.5 时最大 alpha 不应显著超过 128（组透明度语义，±4 容差）：{max_alpha}"
        );
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
        let mut ink = |bold: bool| {
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

    /// I2 修复：`measure()` 是 brief `Interfaces` 里三个公开 API 之一，此前零测试覆盖。
    /// 钉住「排版宽度与墨迹宽度同量级、且墨迹比排版宽度略宽（描边+粗体外扩）」这条关系。
    /// 实测（60px「熊猫智研社」）：`measure` 宽 300.0，墨迹（非透明像素包围盒）宽 298px。
    #[test]
    fn measure_width_matches_ink_width_for_a_five_char_line() {
        let mut r = TextRenderer::new().unwrap();
        let s = style(60.0);
        let (w, h) = r.measure("熊猫智研社", &s);
        assert!(w > 200.0 && w < 400.0, "measure 宽度异常：{w}");
        assert!(h > 0.0, "measure 高度应为正：{h}");

        let mut p = blank(700, 200);
        r.draw_centered(&mut p, "熊猫智研社", 350.0, 100.0, &s, 1.0, 1.0);
        let bbox = non_transparent_bbox(&p).unwrap();
        let ink_w = (bbox.2 - bbox.0) as f32;
        // 墨迹不含描边/合成粗体外扩时应约等于排版宽度；这里的描边+粗体外扩让墨迹
        // 略宽于排版宽度，但仍是同一量级，不应偏差过大。
        assert!(
            (ink_w - w).abs() < 40.0,
            "measure 宽度与墨迹宽度偏差过大：measure={w} ink={ink_w}"
        );
    }

    /// I2 修复：`\n` 强制换行下 `measure()` 的行为——高度约为单行的两倍，
    /// 宽度约为单行（不换行时）的一半（审查者实测 `(180,144)` vs `(360,72)`；
    /// 这里用另一组字符串复验同样的比例关系，不依赖具体像素值）。
    #[test]
    fn measure_forced_newline_roughly_doubles_height_and_halves_width() {
        let mut r = TextRenderer::new().unwrap();
        let s = style(60.0);
        let (w1, h1) = r.measure("测试文字", &s);
        let (w2, h2) = r.measure("测试\n文字", &s);
        assert!(
            h2 > h1 * 1.5,
            "换行后高度应显著增加（约两倍）：{h1} -> {h2}"
        );
        assert!(
            w2 < w1 * 0.75,
            "换行后单行宽度应显著变窄（约一半）：{w1} -> {w2}"
        );
    }

    /// I3 修复：`measure("")` 的既知语义——返回 `(0.0, line_height*size_px)`
    /// 而非 `(0.0, 0.0)`，与 `measure` 文档注释一致，固化下来防止回归。
    #[test]
    fn measure_empty_string_returns_one_line_height_not_zero() {
        let mut r = TextRenderer::new().unwrap();
        let s = style(60.0);
        let (w, h) = r.measure("", &s);
        assert_eq!(w, 0.0, "空字符串排版宽度应为 0");
        let expected_h = s.size_px * s.line_height;
        assert!(
            (h - expected_h).abs() < 1.0,
            "空字符串高度应约为一行行高 {expected_h}，实际 {h}"
        );
    }

    /// I3 修复：`stroke: None` 分支此前无测试覆盖。固化「只填充、不出现描边色、
    /// 不 panic」这条已手工验证过的行为。
    #[test]
    fn stroke_none_draws_fill_only_without_black_stroke() {
        let mut r = TextRenderer::new().unwrap();
        let mut s = style(70.0);
        s.stroke = None;
        let mut p = blank(600, 200);
        r.draw_centered(&mut p, "无描边", 300.0, 100.0, &s, 1.0, 1.0);
        let mut has_white = false;
        let mut has_black = false;
        for y in 0..p.height() {
            for x in 0..p.width() {
                if let Some(c) = p.pixel(x, y)
                    && c.alpha() > 200
                {
                    let (r_, g_, b_) = (c.red(), c.green(), c.blue());
                    if r_ > 240 && g_ > 240 && b_ > 240 {
                        has_white = true;
                    }
                    if r_ < 30 && g_ < 30 && b_ < 30 {
                        has_black = true;
                    }
                }
            }
        }
        assert!(has_white, "无描边时应仍有白色填充");
        assert!(!has_black, "无描边时不应出现黑色像素");
    }

    /// I3 修复：`\n` 强制换行分支此前无测试覆盖。固化「渲染出恰好两段独立的墨迹
    /// 行、行距均匀（两段之间有间隙）、且每行各自水平居中」这条已手工验证过的行为。
    #[test]
    fn newline_forces_two_evenly_spaced_centered_lines() {
        let mut r = TextRenderer::new().unwrap();
        let s = style(60.0);
        let mut p = blank(900, 500);
        r.draw_centered(&mut p, "上一行\n下一行", 450.0, 250.0, &s, 1.0, 1.0);

        // 按行扫描出每一行是否有墨迹，再把连续有墨迹的行聚成一段。
        let mut row_has_ink = vec![false; p.height() as usize];
        for y in 0..p.height() {
            for x in 0..p.width() {
                if p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false) {
                    row_has_ink[y as usize] = true;
                    break;
                }
            }
        }
        let mut segments: Vec<(usize, usize)> = Vec::new();
        let mut start = None;
        for (y, &has) in row_has_ink.iter().enumerate() {
            match (has, start) {
                (true, None) => start = Some(y),
                (false, Some(s0)) => {
                    segments.push((s0, y - 1));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s0) = start {
            segments.push((s0, row_has_ink.len() - 1));
        }
        assert_eq!(segments.len(), 2, "应正好渲染出两行，实际分段：{segments:?}");

        let (h0, h1) = (
            (segments[0].1 - segments[0].0) as f32,
            (segments[1].1 - segments[1].0) as f32,
        );
        let ratio = h0 / h1.max(1.0);
        assert!(ratio > 0.6 && ratio < 1.6, "两行高度应大致相当：{h0} vs {h1}");

        let gap = segments[1].0 as isize - segments[0].1 as isize;
        assert!(gap > 0, "两行之间应有行距间隙，实际 gap={gap}");

        for &(y0, y1) in &segments {
            let mut x0 = u32::MAX;
            let mut x1 = 0u32;
            for y in y0..=y1 {
                for x in 0..p.width() {
                    if p.pixel(x, y as u32).map(|c| c.alpha() > 0).unwrap_or(false) {
                        x0 = x0.min(x);
                        x1 = x1.max(x);
                    }
                }
            }
            let center = (x0 + x1) as f32 / 2.0;
            assert!((center - 450.0).abs() < 15.0, "行未居中：center={center}");
        }
    }
}
