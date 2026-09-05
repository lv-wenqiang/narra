//! 文字排版 + 描边 + 填充绘制。
//!
//! 实现照抄 `docs/text-rendering.md`（Task 0 探针实测跑通、代码审查逐项核实过的写法），
//! 不是凭印象重写。三条必做项——禁用系统字体回退、合成粗体、先变换路径再描边——
//! 的动机和实测证据都记录在那份文档里。

use anyhow::{Context, Result, anyhow};
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, fontdb};
use tiny_skia::{
    Color, FillRule, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform,
};
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
    /// 字形路径的复用缓冲区（见 `draw_centered`）：每次调用先收集、再绘制，
    /// 中间要用它们的并集包围盒决定暂存画布多大。跨调用复用，避免每帧重分配。
    paths: Vec<Path>,
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
        Self::from_font_data(FONT.to_vec())
    }

    /// 用一份字体字节构造。`new()` 与 [`with_optional_font`] 都收敛到这里，
    /// 「私有 db、不加载系统字体」这条性质因此只有一处实现，不会在加自定义
    /// 字体时被漏掉。
    ///
    /// [`with_optional_font`]: Self::with_optional_font
    fn from_font_data(data: Vec<u8>) -> Result<Self> {
        let family = family_name_from_ttf(&data)?;

        let mut db = fontdb::Database::new();
        db.load_font_data(data);
        let font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);

        Ok(Self {
            font_system,
            family,
            scratch: None,
            paths: Vec::new(),
        })
    }

    /// 按用户指定的字体文件构造；`None`、或该文件不可用时，回退到内嵌字体。
    ///
    /// **回退而非报错**是明确的产品选择（与 `--orientation` 的硬报错不同），
    /// 代价是「字体没生效」不会中断出片，所以警告必须自带三样东西：出错的
    /// 路径、具体原因、以及**实际生效的是哪一份**——少了第三样，看到警告的人
    /// 仍然不知道成片里的字长什么样。
    ///
    /// **按扩展名分流，不嗅探文件头**，与 `assets::load_icon` 同一套规矩：
    /// 用户给的是自己的文件，猜错格式的后果是整片文字变成另一种样子而没有
    /// 任何提示。只认 `.ttf` 与 `.otf`。
    ///
    /// **不做字形覆盖率检查**：本渲染器刻意禁用了系统字体回退（见 [`new`]），
    /// 因此一份不覆盖中文的字体会让字幕整片变成豆腐块。这是「禁用回退」的
    /// 既定代价，此处不检测、不提示。
    ///
    /// [`new`]: Self::new
    pub fn with_optional_font(path: Option<&std::path::Path>) -> Result<Self> {
        let Some(path) = path else {
            return Self::new();
        };
        match Self::try_with_font(path) {
            Ok(r) => Ok(r),
            Err(e) => {
                Self::warn_fallback(path, &format!("{e:#}"));
                Self::new()
            }
        }
    }

    /// 严格版：加载指定字体，任何一步失败都返回 `Err`，**不回退**。
    ///
    /// 与 [`with_optional_font`] 拆开是为了让「回退」成为一个可观测的事实：
    /// 合起来写时，外部只能看到一个 `Ok`，无从分辨拿到的是用户字体还是内嵌
    /// 字体，测试也就只能断言「没有崩」。拆开之后，失败的判定归这里、回退的
    /// 决策归外层，两者各自可测。
    ///
    /// [`with_optional_font`]: Self::with_optional_font
    pub fn try_with_font(path: &std::path::Path) -> Result<Self> {
        Self::from_font_data(Self::load_font_file(path)?)
    }

    /// 读取并按扩展名校验字体文件。
    fn load_font_file(path: &std::path::Path) -> Result<Vec<u8>> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if ext != "ttf" && ext != "otf" {
            anyhow::bail!("只支持 .ttf 与 .otf，收到的扩展名是「{ext}」");
        }
        std::fs::read(path).with_context(|| format!("读取字体文件失败：{}", path.display()))
    }

    /// 回退时的警告。文案里必须点明实际生效的字体，理由见
    /// [`with_optional_font`] 的文档。
    ///
    /// [`with_optional_font`]: Self::with_optional_font
    fn warn_fallback(path: &std::path::Path, reason: &str) {
        eprintln!(
            "警告：指定的字体 {} 无法使用（{reason}），本次改用内嵌字体渲染全部文字。",
            path.display()
        );
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

    /// 返回「最后一行」（`layout_runs()` 中 `line_top` 最大的那一行）的
    /// `(该行排版宽度, 该行顶部相对于文本块顶部的偏移, 该行行高)`；没有任何
    /// layout run（理论上只有极端输入才会发生）时返回 `None`。
    ///
    /// **为什么不能用「对每个字符前缀分别 `measure()`、找高度跳变点」来推断
    /// 换行位置**（Task 6 修复轮 1 的真实教训）：那种推断法默认换行只发生在
    /// 字符边界（对纯 CJK 逐字换行成立），但 `cosmic-text` 遇到英文/数字等
    /// 做 word-wrap 时，一次性挪到下一行的是**整个单词**——高度跳变点会落在
    /// 单词内部而不是行首，据此推算出的"最后一行"会把这个单词的前半截也算进
    /// 上一行，导致宽度算少、位置算错（实测：光标被画到英文单词中间，穿过
    /// 字母本身）。这个方法直接读 `shape()`/`layout_runs()` 产出的断行结果，
    /// 与真实断行算法同源，无论逐字符还是整词换行都精确；而且只需一次
    /// `shape()`，比"对每个前缀各 measure 一次"快得多（原来是 O(n²) 次
    /// `shape()`，这里是 O(1) 次）。
    ///
    /// 三个返回值的用途：`last_top_rel` 与 `last_h` 之和就是整个文本块的高度
    /// （因为"最后一行"就是 `line_top` 最大的那一行，它的下边缘天然是全文本块
    /// 的 `max_bottom`）——所以调用方不需要再额外调用一次 `measure()` 去拿
    /// 总高度；结合 `draw_centered` 的垂直居中公式可以推出：若整块文本居中于
    /// `center_y`，则最后一行自身的垂直中心就是 `center_y + last_top_rel / 2.0`
    /// （代数展开：`center_y - total_h/2 + last_top_rel + last_h/2`，代入
    /// `total_h = last_top_rel + last_h` 化简即得）。
    pub fn last_line_metrics(&mut self, text: &str, style: &TextStyle) -> Option<(f32, f32, f32)> {
        let buffer = self.shape(text, style);
        let mut min_top = f32::MAX;
        let mut max_top = f32::MIN;
        let mut last_w = 0.0f32;
        let mut last_h = 0.0f32;
        let mut any = false;
        for run in buffer.layout_runs() {
            any = true;
            min_top = min_top.min(run.line_top);
            if run.line_top >= max_top {
                max_top = run.line_top;
                last_w = run.line_w;
                last_h = run.line_height;
            }
        }
        if !any {
            return None;
        }
        Some((last_w, max_top - min_top, last_h))
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
        let stroke_paint_and_width = style
            .stroke
            .map(|(color, width)| (solid_paint(color), width));

        let bold_w = style.size_px * BOLD_STROKE_RATIO;

        // 先把这块文字的所有字形路径收集到最终像素空间，**再**决定画到哪里。
        //
        // 分两步是为了拿到整块文字的**精确包围盒**（`Path::bounds()` 的并集，
        // 外扩最外一遍描边的半宽）——包围盒决定了暂存画布要多大。用路径的真实
        // 边界而不是行宽/行高去估，是因为字形的实际墨迹会越出排版盒（overshoot、
        // 侧边距、下伸部），估错的代价是描边最外圈被裁掉，而那正是肉眼最难发现
        // 的地方。路径缓冲区跨帧复用，不每帧重新分配。
        let mut paths = std::mem::take(&mut self.paths);
        paths.clear();

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
                paths.push(path);
            }
        }

        // 最外一遍描边是**居中**在路径上的，因此向外只多出半个宽度。
        let outer_w = match &stroke_paint_and_width {
            Some((_, stroke_w)) if style.bold => (stroke_w + bold_w) * scale,
            Some((_, stroke_w)) => stroke_w * scale,
            None if style.bold => bold_w * scale,
            None => 0.0,
        };
        let margin = outer_w / 2.0;

        // **组透明度为 1 时不需要暂存画布**：`over` 满足结合律，「三遍画进透明
        // 层、整层再 over 一次」与「三遍直接 over 到目标」等价
        // （`full_opacity_drawing_is_byte_identical_to_compositing_a_transparent_layer`
        // 拿白底与透明底两种底色逐字节验过）。省掉的是清空暂存画布 + 把它合成
        // 回目标这**两趟操作**，代价与文字占多大面积无关——实测每次调用约
        // 7.2ms（1920×1080），见 `docs/ffmpeg-pipeline.md` §13。
        let (dst_origin, mut scratch) = if opacity >= 1.0 {
            ((0, 0), None)
        } else {
            let Some((x0, y0, w, h)) = ink_box(&paths, margin, pixmap.width(), pixmap.height())
            else {
                self.paths = paths;
                return;
            };
            // 暂存画布只覆盖包围盒，不再是整幅画布。**平移量必须是整数**：
            // 抗锯齿的覆盖率取决于路径相对像素网格的位置，非整数平移会让同一
            // 个字形栅格化出不同的边缘像素，与整幅画布那条参照路径就不再逐字节
            // 相同了（`partial_opacity_..._matches_a_full_canvas_reference_layer`
            // 与 `..._clipped_by_the_canvas_edge_...` 两条测试钉住这一点）。
            let reuse = match self.scratch.take() {
                Some(mut p) if p.width() == w && p.height() == h => {
                    p.fill(Color::TRANSPARENT);
                    p
                }
                _ => Pixmap::new(w, h).expect("包围盒尺寸已夹到画布内且非零"),
            };
            ((x0, y0), Some(reuse))
        };

        let shift = Transform::from_translate(-(dst_origin.0 as f32), -(dst_origin.1 as f32));
        let dst: &mut Pixmap = match scratch.as_mut() {
            Some(s) => s,
            None => pixmap,
        };
        for path in &paths {
            // 合成粗体（必做项 2）：内嵌字体只有一个静态字重，`Weight::BOLD` 是
            // 空操作，规格要求的粗体必须靠这三段描边/填充合成。
            if let Some((stroke_paint, _)) = &stroke_paint_and_width
                && outer_w > 0.0
            {
                let outer_stroke = Stroke {
                    width: outer_w,
                    ..Default::default()
                };
                dst.stroke_path(path, stroke_paint, &outer_stroke, shift, None);
            }
            if style.bold {
                let w = bold_w * scale;
                if w > 0.0 {
                    let bold_stroke = Stroke {
                        width: w,
                        ..Default::default()
                    };
                    dst.stroke_path(path, &fill_paint, &bold_stroke, shift, None);
                }
            }
            dst.fill_path(path, &fill_paint, FillRule::Winding, shift, None);
        }

        // 走了暂存画布的话，把它按组透明度合成回目标——组透明度只在这里生效
        // 一次。全不透明那条路径上没有暂存画布，也就没有这一趟。
        if let Some(scratch) = scratch {
            pixmap.draw_pixmap(
                dst_origin.0,
                dst_origin.1,
                scratch.as_ref(),
                &PixmapPaint {
                    opacity: opacity.clamp(0.0, 1.0),
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
            // 放回去供下一次调用复用。
            self.scratch = Some(scratch);
        }
        self.paths = paths;
    }
}

/// 一组路径的墨迹包围盒，外扩 `margin`，夹到 `(w, h)` 画布内。
///
/// 返回 `(x0, y0, 宽, 高)`，整数——**非整数会改变抗锯齿覆盖率**，见
/// `draw_centered` 里平移量那段注释。完全落在画布外、或压根没有路径时返回
/// `None`（没有任何可见像素，调用方直接返回即可）。
fn ink_box(paths: &[Path], margin: f32, w: u32, h: u32) -> Option<(i32, i32, u32, u32)> {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for path in paths {
        let b = path.bounds();
        min_x = min_x.min(b.left());
        min_y = min_y.min(b.top());
        max_x = max_x.max(b.right());
        max_y = max_y.max(b.bottom());
    }
    if min_x > max_x {
        return None;
    }

    // `floor`/`ceil` 而不是四舍五入：宁可多留一个像素，也不能把边缘那一行
    // 抗锯齿像素切掉。
    let x0 = (min_x - margin).floor().max(0.0) as i32;
    let y0 = (min_y - margin).floor().max(0.0) as i32;
    let x1 = (max_x + margin).ceil().min(w as f32) as i32;
    let y1 = (max_y + margin).ceil().min(h as f32) as i32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
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
        r.draw_centered(
            &mut p,
            "文字渲染器 Test 123",
            640.0,
            360.0,
            &style(70.0),
            1.0,
            1.0,
        );
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
        r.draw_centered(&mut a, "文字", 300.0, 100.0, &style(70.0), 1.0, 1.0);
        r.draw_centered(&mut b, "渲染", 300.0, 100.0, &style(70.0), 1.0, 1.0);
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
                    if r_ > 240 && g_ > 240 && b_ > 240 {
                        has_white = true;
                    }
                    if r_ < 30 && g_ < 30 && b_ < 30 {
                        has_black = true;
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
        assert!(
            non_transparent_bbox(&p).is_none(),
            "opacity=0 时不应画出任何东西"
        );
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
        r.draw_centered(
            &mut p,
            "这是一段需要换行的比较长的中文文字内容",
            640.0,
            360.0,
            &s,
            1.0,
            1.0,
        );
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
    /// 实测（60px「文字渲染器」）：`measure` 宽 300.0，墨迹（非透明像素包围盒）宽 300px。
    #[test]
    fn measure_width_matches_ink_width_for_a_five_char_line() {
        let mut r = TextRenderer::new().unwrap();
        let s = style(60.0);
        let (w, h) = r.measure("文字渲染器", &s);
        assert!(w > 200.0 && w < 400.0, "measure 宽度异常：{w}");
        assert!(h > 0.0, "measure 高度应为正：{h}");

        let mut p = blank(700, 200);
        r.draw_centered(&mut p, "文字渲染器", 350.0, 100.0, &s, 1.0, 1.0);
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
        assert_eq!(
            segments.len(),
            2,
            "应正好渲染出两行，实际分段：{segments:?}"
        );

        let (h0, h1) = (
            (segments[0].1 - segments[0].0) as f32,
            (segments[1].1 - segments[1].0) as f32,
        );
        let ratio = h0 / h1.max(1.0);
        assert!(
            ratio > 0.6 && ratio < 1.6,
            "两行高度应大致相当：{h0} vs {h1}"
        );

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

    // ------------------------------------------------------------------
    // Task 6 修复轮 1（I1）：`last_line_metrics` 的测试。纯追加方法，
    // 覆盖单行、CJK 逐字换行、英文 word-wrap、`\n` 强制换行、空串。
    // ------------------------------------------------------------------

    #[test]
    fn last_line_metrics_single_line_matches_measure() {
        let mut r = TextRenderer::new().unwrap();
        let s = style(60.0);
        let (w, h) = (r.measure("文字渲染器", &s).0, r.measure("文字渲染器", &s).1);
        let (last_w, top_rel, last_h) = r.last_line_metrics("文字渲染器", &s).unwrap();
        assert!(
            (last_w - w).abs() < 0.01,
            "单行时最后一行宽度应与 measure 的整体宽度一致：last_w={last_w} w={w}"
        );
        assert_eq!(top_rel, 0.0, "单行时最后一行顶部相对偏移应为 0");
        assert!(
            (top_rel + last_h - h).abs() < 0.01,
            "单行时 top_rel+last_h 应等于 measure 的整体高度：{} vs {h}",
            top_rel + last_h
        );
    }

    #[test]
    fn last_line_metrics_cjk_char_wrap_is_self_consistent_with_measure() {
        let mut r = TextRenderer::new().unwrap();
        let mut s = style(70.0);
        s.max_width_px = 300.0; // 强制逐字换行成多行
        let text = "这是一段需要换行的比较长的中文文字内容";
        let (_, total_h) = r.measure(text, &s);
        let (last_w, top_rel, last_h) = r.last_line_metrics(text, &s).unwrap();
        assert!(
            top_rel > 0.0,
            "多行时最后一行顶部相对偏移应 > 0，实得 {top_rel}"
        );
        assert!(
            (top_rel + last_h - total_h).abs() < 0.01,
            "top_rel+last_h 应等于 measure 的整体高度（最后一行的下边缘就是全文本块的下边缘）：{} vs {total_h}",
            top_rel + last_h
        );
        assert!(
            last_w > 0.0 && last_w < 300.0 + 24.0,
            "最后一行宽度应在合理范围：{last_w}"
        );
    }

    /// I1 的核心场景：英文 word-wrap（挪到下一行的是整个单词，不是单个字符）。
    /// 用「self-consistency」而不是猜测具体断行点来验证——这正是
    /// `last_line_metrics` 相对旧的「前缀高度跳变点」推断法的优势：不管
    /// cosmic-text 实际在哪个单词边界断行，`top_rel+last_h` 恒等于
    /// `measure()` 给出的整体高度，因为它直接读断行结果而不是猜测规则。
    #[test]
    fn last_line_metrics_english_word_wrap_is_self_consistent_with_measure() {
        let mut r = TextRenderer::new().unwrap();
        let mut s = style(40.0);
        s.max_width_px = 220.0; // 窄到必然把长单词挤到下一行
        let text = "Narra Video Generator automated engine";
        let (unwrapped_w, _) = {
            let mut wide = style(40.0);
            wide.max_width_px = 4000.0;
            r.measure(text, &wide)
        };
        let (_, total_h) = r.measure(text, &s);
        let (last_w, top_rel, last_h) = r.last_line_metrics(text, &s).unwrap();
        assert!(
            top_rel > 0.0,
            "word-wrap 应产生多行，top_rel 应 > 0，实得 {top_rel}"
        );
        assert!(
            (top_rel + last_h - total_h).abs() < 0.01,
            "top_rel+last_h 应等于 measure 的整体高度：{} vs {total_h}",
            top_rel + last_h
        );
        assert!(
            last_w < unwrapped_w,
            "换行后最后一行宽度应明显小于不换行时的整体宽度：last_w={last_w} unwrapped_w={unwrapped_w}"
        );
    }

    #[test]
    fn last_line_metrics_forced_newline_matches_second_line_alone() {
        let mut r = TextRenderer::new().unwrap();
        let s = style(60.0);
        let (last_w, top_rel, last_h) = r.last_line_metrics("上一行\n下一行", &s).unwrap();
        let (line_h_alone,) = (r.measure("下一行", &s).1,);
        assert!(
            (top_rel - line_h_alone).abs() < 1.0,
            "强制换行后最后一行顶部相对偏移应约等于单行行高：top_rel={top_rel} line_h={line_h_alone}"
        );
        assert!(
            (last_h - line_h_alone).abs() < 1.0,
            "最后一行行高应与单独渲染该行时的行高一致：last_h={last_h} line_h_alone={line_h_alone}"
        );
        let (w_alone, _) = r.measure("下一行", &s);
        assert!(
            (last_w - w_alone).abs() < 0.01,
            "最后一行宽度应与单独渲染「下一行」时的宽度一致：last_w={last_w} w_alone={w_alone}"
        );
    }

    #[test]
    fn last_line_metrics_empty_string_is_consistent_with_measure() {
        let mut r = TextRenderer::new().unwrap();
        let s = style(60.0);
        let (_, h) = r.measure("", &s);
        match r.last_line_metrics("", &s) {
            None => {
                // 允许的另一种合法实现：空文本没有任何 layout run。
            }
            Some((w, top_rel, last_h)) => {
                assert_eq!(w, 0.0, "空字符串最后一行宽度应为 0");
                assert!(
                    (top_rel + last_h - h).abs() < 0.01,
                    "空字符串时 top_rel+last_h 应等于 measure 的高度：{} vs {h}",
                    top_rel + last_h
                );
            }
        }
    }

    /// **修复轮 2（复审建议的可选补充，纯几何、零像素扫描）**：直接用
    /// `last_line_metrics` 的返回值验证一个人工构造、断行点可预知的
    /// word-wrap 场景——最后一行宽度应精确等于末尾那个完整单词自身的宽度。
    ///
    /// **这条测试能拦住什么、拦不住什么（终审实测的结论，别再高估它）**：
    ///
    /// - **拦得住**：把 `last_line_metrics` 的返回值直接改错（返回整块宽度、
    ///   返回首行宽度、把 `top_rel` 恒置 0 之类）。
    /// - **拦不住**：把算法本体换回旧的"前缀高度跳变"启发式。复审做过这个
    ///   变异实验：换回旧启发式后这条测试**照样通过**。原因在构造本身——
    ///   这里只有"两个单词、断在唯一那个空格处"的最简场景，旧启发式在这种
    ///   场景下恰好也算得精确，两者无从区分。要真的区分，得构造多次换行、
    ///   且断点落在单词内部的用例。
    ///
    /// 所以真正的保护来自 `src/render/draw.rs` 里
    /// `cursor_never_overlaps_word_wrapped_last_line_ink` 那条端到端的
    /// "光标压字"像素测试——它是这条不变式唯一的实质防线。本测试的定位是
    /// 一道近乎零成本的返回值哨兵，不是那条像素测试的替身。
    ///
    /// 构造方法：把 `max_width_px` 卡在"刚好放得下单独一个 AAAA，放不下
    /// AAAA BBBB"，逼着 cosmic-text 精确地在两个单词之间断行（不会有任何
    /// 歧义空间）。
    #[test]
    fn last_line_metrics_word_wrap_gives_exact_width_not_a_shorter_heuristic_guess() {
        let mut r = TextRenderer::new().unwrap();
        let base_style = |max_width_px: f32| TextStyle {
            size_px: 40.0,
            color: [0, 0, 0, 255],
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px,
            line_height: 1.2,
            bold: true,
        };
        let (first_word_w, _) = r.measure("AAAA", &base_style(4000.0));
        // 宽度刚好够放下单独的 "AAAA"，放不下 "AAAA BBBB" 整体：强制断成
        // "AAAA" / "BBBB" 两行，断行点唯一、可预知。
        let style = base_style(first_word_w + 8.0);
        let (last_w, top_rel, _) = r.last_line_metrics("AAAA BBBB", &style).unwrap();
        assert!(
            top_rel > 0.0,
            "本用例的构造前提是必须发生换行，实测未换行（top_rel={top_rel}）"
        );

        let (expected_w, _) = r.measure("BBBB", &style);
        assert!(
            (last_w - expected_w).abs() < 1.0,
            "word-wrap 后最后一行宽度应精确等于末尾单词「BBBB」自身的宽度：\
             last_w={last_w} expected={expected_w}（旧的\"前缀高度跳变\"启发式会得到一个明显偏小的值）"
        );
    }

    /// 一份可用的 `.ttf`：`try_with_font` 必须成功，且解析出的 family 与内嵌
    /// 字体一致（用的正是内嵌字体的字节，写到临时文件再读回来）。
    ///
    /// 这条与下面三条「失败」用例合起来，才把「加载成功」和「回退」区分开：
    /// 只测 `with_optional_font` 的话两者都返回 `Ok`，断言不到任何东西。
    #[test]
    fn try_with_font_loads_a_valid_ttf() {
        let dir = std::env::temp_dir().join(format!("narra_font_ok_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("embedded-copy.ttf");
        std::fs::write(&p, crate::assets::FONT).unwrap();

        let loaded = TextRenderer::try_with_font(&p).expect("合法 .ttf 应加载成功");
        let embedded = TextRenderer::new().unwrap();
        assert_eq!(
            loaded.family, embedded.family,
            "写出去再读回来的同一份字体，family 应当一致"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `.otf` 也在白名单里。
    ///
    /// **为什么写的是 TTF 字节**：格式判定按扩展名、不嗅探文件头（见
    /// `load_font_file`），所以这条要验证的是「otf 这个扩展名被接受」，
    /// 与文件内容是哪种轮廓格式无关。缺了这条，把白名单缩成只认 `.ttf`
    /// 的改动不会让任何测试变红——实测确认过该变异原本可以存活。
    #[test]
    fn try_with_font_accepts_otf_extension() {
        let dir = std::env::temp_dir().join(format!("narra_font_otf_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("some-font.otf");
        std::fs::write(&p, crate::assets::FONT).unwrap();
        assert!(
            TextRenderer::try_with_font(&p).is_ok(),
            ".otf 应在支持的扩展名白名单内"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 扩展名不在白名单内：报错并点名收到的扩展名。
    ///
    /// **按扩展名而非文件头判定**（与 `assets::load_icon` 同一套规矩），所以
    /// 这里刻意写入一份**内容完全合法**的字体字节、只把扩展名改成 `.png`：
    /// 若判定改成嗅探文件头，这条会变绿，正是要拦住的那种改动。
    #[test]
    fn try_with_font_rejects_unsupported_extension() {
        let dir = std::env::temp_dir().join(format!("narra_font_ext_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("actually-a-font.png");
        std::fs::write(&p, crate::assets::FONT).unwrap();

        let msg = match TextRenderer::try_with_font(&p) {
            Ok(_) => panic!("扩展名不支持时应报错"),
            Err(e) => format!("{e:#}"),
        };
        assert!(msg.contains("png"), "报错应点名收到的扩展名：{msg}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 扩展名合法但文件不存在：报错并带上路径。
    #[test]
    fn try_with_font_reports_missing_file() {
        let p = std::env::temp_dir().join("narra_font_definitely_absent_9c3f.ttf");
        std::fs::remove_file(&p).ok();
        let msg = match TextRenderer::try_with_font(&p) {
            Ok(_) => panic!("文件不存在时应报错"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            msg.contains("narra_font_definitely_absent_9c3f.ttf"),
            "报错应带上出错的路径：{msg}"
        );
    }

    /// 扩展名合法但内容不是字体：报错而不是 panic。
    #[test]
    fn try_with_font_rejects_garbage_content() {
        let dir = std::env::temp_dir().join(format!("narra_font_junk_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("not-a-font.ttf");
        std::fs::write(&p, b"this is definitely not a font file").unwrap();
        assert!(TextRenderer::try_with_font(&p).is_err(), "非字体内容应报错");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 回退契约：`with_optional_font` 对上面每一种失败都必须**返回 `Ok`**
    /// （回退到内嵌字体，不中断出片），且拿到的 family 就是内嵌字体的。
    ///
    /// 这是产品选择而非疏漏——与 `--orientation` 的硬报错不同，理由见
    /// `with_optional_font` 的文档。
    #[test]
    fn with_optional_font_falls_back_instead_of_failing() {
        let embedded = TextRenderer::new().unwrap().family;
        let dir = std::env::temp_dir().join(format!("narra_font_fb_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let junk = dir.join("junk.ttf");
        std::fs::write(&junk, b"nope").unwrap();
        let bad_ext = dir.join("x.png");
        std::fs::write(&bad_ext, crate::assets::FONT).unwrap();
        let missing = dir.join("gone.ttf");

        for p in [&junk, &bad_ext, &missing] {
            let r = TextRenderer::with_optional_font(Some(p))
                .unwrap_or_else(|e| panic!("{} 应回退而不是报错：{e:#}", p.display()));
            assert_eq!(r.family, embedded, "{} 回退后应当用内嵌字体", p.display());
        }

        // 不给路径时同样是内嵌字体。
        let none = TextRenderer::with_optional_font(None).unwrap();
        assert_eq!(none.family, embedded);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 全不透明时的绘制，必须与「画在透明层上再整层合成回来」**逐字节相同**。
    ///
    /// 这条是 `draw_centered` 跳过暂存画布那条快路径的正确性依据：Porter-Duff
    /// `over` 满足结合律，组透明度为 1 时「三遍直接画进目标」与「三遍画进透明
    /// 层、整层再 over 一次」等价。等价性是**可测的**，不必只当理论：右边那条
    /// 路径在这条测试里是用公开 API 现搭的参照实现，不是被测代码的内部结构。
    ///
    /// 底色故意用不透明白：透明底（Content 段）下等价性最容易成立，白底
    /// （Cover/Intro/Outro）才是逐步量化最可能露出差异的地方。
    #[test]
    fn full_opacity_drawing_is_byte_identical_to_compositing_a_transparent_layer() {
        let mut r = TextRenderer::new().unwrap();
        let style = TextStyle {
            size_px: 80.0,
            color: [255, 255, 255, 255],
            stroke: Some(([0, 0, 0, 255], 6.0)),
            letter_spacing_px: 2.0,
            max_width_px: 900.0,
            line_height: 1.4,
            bold: true,
        };

        for (label, bg) in [
            ("白底", Color::from_rgba8(255, 255, 255, 255)),
            ("透明底", Color::TRANSPARENT),
        ] {
            let mut direct = blank(1280, 720);
            direct.fill(bg);
            r.draw_centered(
                &mut direct,
                "等价性测试文案",
                640.0,
                360.0,
                &style,
                1.0,
                1.0,
            );

            let mut layer = blank(1280, 720);
            r.draw_centered(&mut layer, "等价性测试文案", 640.0, 360.0, &style, 1.0, 1.0);
            let mut composited = blank(1280, 720);
            composited.fill(bg);
            composited.draw_pixmap(
                0,
                0,
                layer.as_ref(),
                &PixmapPaint {
                    opacity: 1.0,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );

            let diff = direct
                .data()
                .iter()
                .zip(composited.data())
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(
                diff, 0,
                "{label}：两条路径应逐字节相同，实际有 {diff} 字节不同"
            );
        }
    }

    /// 半透明时同样要与参照实现一致（在 1 LSB 以内）。
    ///
    /// 这条护的是暂存画布**换成只覆盖文字包围盒**之后的等价性：包围盒算错一
    /// 点点，被裁掉的就是描边最外圈那几个像素，而那正是肉眼最不容易发现、
    /// 逐字节比对最容易发现的地方。参照实现同样用公开 API 现搭：整幅透明层
    /// 画一遍，再按同一组透明度合成。
    #[test]
    fn partial_opacity_drawing_matches_a_full_canvas_reference_layer() {
        let mut r = TextRenderer::new().unwrap();
        let style = TextStyle {
            size_px: 52.0,
            color: [255, 255, 255, 255],
            stroke: Some(([0, 0, 0, 255], 5.0)),
            letter_spacing_px: 0.0,
            max_width_px: 900.0,
            line_height: 1.4,
            bold: true,
        };

        for opacity in [0.25_f32, 0.5, 0.87] {
            let mut direct = blank(1280, 720);
            direct.fill(Color::from_rgba8(255, 255, 255, 255));
            r.draw_centered(
                &mut direct,
                "半透明等价性",
                640.0,
                360.0,
                &style,
                opacity,
                1.1,
            );

            let mut layer = blank(1280, 720);
            r.draw_centered(&mut layer, "半透明等价性", 640.0, 360.0, &style, 1.0, 1.1);
            let mut composited = blank(1280, 720);
            composited.fill(Color::from_rgba8(255, 255, 255, 255));
            composited.draw_pixmap(
                0,
                0,
                layer.as_ref(),
                &PixmapPaint {
                    opacity,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );

            let worst = direct
                .data()
                .iter()
                .zip(composited.data())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            assert!(
                worst <= 1,
                "opacity={opacity}：与参照实现最大差 {worst}，超过 1 LSB 的量化余量"
            );
        }
    }

    /// 半透明 + 文字压在画布边缘：包围盒会被画布裁掉一部分，裁剪口径不能改变
    /// 结果。
    ///
    /// 暂存画布从「整幅」缩到「文字包围盒」之后，这是最容易写错的一处：包围盒
    /// 越界时既要夹到画布内，又要保持绘制坐标相对像素网格不变（只能整数平移，
    /// 否则抗锯齿覆盖率会变，逐字节比对立刻不同）。
    #[test]
    fn partial_opacity_text_clipped_by_the_canvas_edge_matches_the_reference() {
        let mut r = TextRenderer::new().unwrap();
        let style = TextStyle {
            size_px: 80.0,
            color: [255, 255, 255, 255],
            stroke: Some(([0, 0, 0, 255], 8.0)),
            letter_spacing_px: 0.0,
            max_width_px: 900.0,
            line_height: 1.4,
            bold: true,
        };

        // 四个方向各压一次边：左上角外、右下角外、上边界、左边界。
        for (cx, cy) in [
            (10.0_f32, 10.0_f32),
            (1270.0, 710.0),
            (640.0, 4.0),
            (6.0, 360.0),
        ] {
            let mut direct = blank(1280, 720);
            direct.fill(Color::from_rgba8(255, 255, 255, 255));
            r.draw_centered(&mut direct, "压边文字", cx, cy, &style, 0.6, 1.0);

            let mut layer = blank(1280, 720);
            r.draw_centered(&mut layer, "压边文字", cx, cy, &style, 1.0, 1.0);
            let mut composited = blank(1280, 720);
            composited.fill(Color::from_rgba8(255, 255, 255, 255));
            composited.draw_pixmap(
                0,
                0,
                layer.as_ref(),
                &PixmapPaint {
                    opacity: 0.6,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );

            let worst = direct
                .data()
                .iter()
                .zip(composited.data())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            assert!(
                worst <= 1,
                "中心 ({cx}, {cy})：与整幅参照实现最大差 {worst}，越界裁剪口径不一致"
            );
        }
    }
}
