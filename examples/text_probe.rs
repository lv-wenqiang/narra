// 探针：验证「cosmic-text 排版 + ttf-parser 取字形轮廓 + tiny-skia 描边/填充」
// 这条路径在 Rust 里能否跑通，尤其是 CJK 文字是否能正确显示（而非豆腐块）。
//
// 全部 API 均已对照实际安装版本的源码核实（见下方各步骤注释里的版本号与源码路径），
// 不是照抄 brief 里"仅为说明意图"的伪代码。
//
// crate 版本（见 Cargo.lock）：cosmic-text 0.19.0、ttf-parser 0.25.1、
// tiny-skia 0.12.0、image 0.25.10。

use anyhow::{anyhow, Context, Result};
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};
use ttf_parser::{Face, GlyphId, OutlineBuilder};

/// 字体文件与 `assets/dingliesongtypeface.ttf` 一致（Task 0 Step 1 从
/// `../panda-video-ts/public/fonts/` 复制而来）。用 `include_bytes!` 在编译期嵌入，
/// 避免探针依赖运行目录。
const FONT_BYTES: &[u8] = include_bytes!("../assets/dingliesongtypeface.ttf");

/// 把 ttf-parser 的 `OutlineBuilder` 回调（字体 unitsPerEm 坐标系，y 轴向上）
/// 原样转发给 tiny-skia 的 `PathBuilder`。缩放、平移、y 轴翻转全部交给调用方
/// 通过 `stroke_path`/`fill_path` 的 `transform` 参数完成，这里只管累积路径命令。
///
/// 确认过 ttf_parser::OutlineBuilder::curve_to(x1,y1,x2,y2,x,y) 与
/// tiny_skia_path::PathBuilder::cubic_to(x1,y1,x2,y2,x,y) 参数顺序完全一致
/// （ttf-parser-0.25.1/src/lib.rs:577-593，tiny-skia-path-0.12.0/src/path_builder.rs:223）。
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

/// 读字体的 `name` 表拿真实家族名（ttf-parser-0.25.1/src/tables/name.rs）。
/// 这是 Step 5 排查阶梯第 1 条要求的验证："Attrs 的 family 名是否与字体内部名称匹配"。
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

/// 用 cosmic-text 排版一段文本，再用 ttf-parser 取每个字形的轮廓，
/// 转成 tiny-skia 的 Path，先 6px 黑色描边、再白色填充，画在半透明灰底上。
///
/// 关键点（对应 brief Step 2 的两条候选路线之一——路线 1）：
/// - `Buffer::layout_runs()` 给出的 `LayoutGlyph` 只有版式信息（字体 id、字形 id、
///   像素位置），不含轮廓；
/// - 轮廓必须另外通过 `FontSystem::get_font(font_id, weight)` 拿到该字形所属字体的
///   原始字节，再用 `ttf_parser::Face::parse` + `Face::outline_glyph` 取得
///   （cosmic-text-0.19.0/src/font/mod.rs 的 `Font::data()`；
///   ttf-parser-0.25.1/src/lib.rs:2125 的 `outline_glyph`）。
fn render_text(
    font_system: &mut FontSystem,
    family: &str,
    text: &str,
    canvas_w: u32,
    canvas_h: u32,
) -> Result<Pixmap> {
    let metrics = Metrics::new(70.0, 84.0);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(Some(canvas_w as f32), Some(canvas_h as f32));

    let attrs = Attrs::new()
        .family(Family::Name(family))
        .weight(Weight::BOLD);
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    let mut pixmap = Pixmap::new(canvas_w, canvas_h)
        .ok_or_else(|| anyhow!("pixmap 尺寸非法：{canvas_w}x{canvas_h}"))?;
    // 半透明灰底，验证描边/填充是在灰底之上叠加（而非替换掉整张画布）。
    pixmap.fill(Color::from_rgba8(128, 128, 128, 180));

    let mut black_paint = Paint::default();
    black_paint.set_color_rgba8(0, 0, 0, 255);
    black_paint.anti_alias = true;

    let mut white_paint = Paint::default();
    white_paint.set_color_rgba8(255, 255, 255, 255);
    white_paint.anti_alias = true;

    let stroke = Stroke {
        width: 6.0,
        ..Default::default()
    };

    let mut glyph_count = 0usize;
    let mut outline_count = 0usize;

    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            glyph_count += 1;
            // 40px 左边距；run.line_y 是该行基线相对 buffer 顶部的像素 y 坐标，
            // 再加 40px 顶部留白防止上层轮廓（如声调符号、大写字母）被裁到画布外。
            // 用法与 cosmic-text 自己的 draw()/render() 完全一致
            // （cosmic-text-0.19.0/src/buffer.rs:1621, 1641：
            // `glyph.physical((0., run.line_y), 1.0)`）——
            // 最初漏掉 run.line_y、只传常量导致所有字形挤在画布顶部（已修复）。
            let physical = glyph.physical((40.0, run.line_y + 40.0), 1.0);
            let cache_key = physical.cache_key;

            let font = font_system
                .get_font(cache_key.font_id, cache_key.font_weight)
                .ok_or_else(|| {
                    anyhow!("找不到 font_id={:?} 对应的已加载字体", cache_key.font_id)
                })?;

            let face = Face::parse(font.data(), 0).context("解析字形所属字体失败")?;
            let units_per_em = face.units_per_em() as f32;
            let font_size = f32::from_bits(cache_key.font_size_bits);
            let scale = font_size / units_per_em;

            let mut outline = SkiaOutline(PathBuilder::new());
            if face
                .outline_glyph(GlyphId(cache_key.glyph_id), &mut outline)
                .is_none()
            {
                // 空格等无墨字形没有轮廓，属正常情况，不是失败。
                continue;
            }
            let font_space_path: Path = match outline.0.finish() {
                Some(p) => p,
                None => continue,
            };
            outline_count += 1;

            // 字体空间（unitsPerEm，y 轴向上，原点在基线）-> 像素空间（y 轴向下）：
            // 缩放 + y 轴翻转 + 平移到 physical 给出的整数像素笔位置。
            // Transform::from_row(sx, ky, kx, sy, tx, ty) 对应
            // x' = sx*x + kx*y + tx；y' = ky*x + sy*y + ty
            // （tiny-skia-path-0.12.0/src/transform.rs:52）。
            let transform = Transform::from_row(
                scale,
                0.0,
                0.0,
                -scale,
                physical.x as f32,
                physical.y as f32,
            );

            // 关键坑（brief Step 5 排查阶梯第 3 条命中）：`Stroke.width` 是在路径自身的
            // 局部坐标系里定义的，`stroke_path(..., transform, ...)` 的 transform 只是把
            // "already-stroked 的结果"整体搬过去，并不会把 6.0 解释成"最终像素宽度"。
            // 如果直接把 font-unit 空间的路径连同 transform 一起传给 stroke_path，
            // scale≈70/2048≈0.034，6px 会被缩成约 0.2px，视觉上等于消失
            // （已实测复现：改之前 near_black 恒为 0）。
            // 解法：先用 `Path::transform` 把路径本身归一化到像素空间，
            // 再用 `Transform::identity()` 描边/填充，这样 `Stroke.width` 才是
            // 真正的输出像素宽度。
            let path = match font_space_path.transform(transform) {
                Some(p) => p,
                None => continue,
            };

            pixmap.stroke_path(&path, &black_paint, &stroke, Transform::identity(), None);
            pixmap.fill_path(
                &path,
                &white_paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    eprintln!("  [{text:?}] 共 {glyph_count} 个字形，其中 {outline_count} 个取到了非空轮廓");

    Ok(pixmap)
}

/// 统计 pixmap 里满足条件的像素数：近纯白（RGB 均 > 240）、近纯黑（RGB 均 < 30）、
/// 非透明像素的包围盒。tiny-skia 的 pixel 是预乘 alpha 的 RGBA，这里先反预乘再判断，
/// 这样描边/填充色本身的判断不受背景灰底透明度影响。
struct PixelStats {
    near_white: usize,
    near_black: usize,
    bbox: Option<(u32, u32, u32, u32)>, // (min_x, min_y, max_x, max_y)
}

fn analyze(pixmap: &Pixmap) -> PixelStats {
    let mut near_white = 0;
    let mut near_black = 0;
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any_opaque = false;

    for (i, pixel) in pixmap.pixels().iter().enumerate() {
        let x = (i as u32) % pixmap.width();
        let y = (i as u32) / pixmap.width();
        let a = pixel.alpha();
        if a == 0 {
            continue;
        }
        // 反预乘，得到真实的描边/填充颜色分量。
        let unmul = pixel.demultiply();
        let (r, g, b) = (unmul.red(), unmul.green(), unmul.blue());
        if r > 240 && g > 240 && b > 240 && a == 255 {
            near_white += 1;
        }
        if r < 30 && g < 30 && b < 30 && a == 255 {
            near_black += 1;
        }
        // 包围盒只统计"明显不是背景灰底"的像素（灰底是 128,128,128,180，
        // 描边/填充像素在其上叠加后 alpha 或颜色都会显著偏离）。
        if a == 255 && !(r > 100 && r < 156 && g > 100 && g < 156 && b > 100 && b < 156) {
            any_opaque = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }

    PixelStats {
        near_white,
        near_black,
        bbox: any_opaque.then_some((min_x, min_y, max_x, max_y)),
    }
}

fn main() -> Result<()> {
    let family = family_name_from_ttf(FONT_BYTES)?;
    println!("字体内部家族名（name 表 FAMILY 记录）：{family:?}");

    let mut font_system = FontSystem::new();
    font_system.db_mut().load_font_data(FONT_BYTES.to_vec());

    // 探针要求的主文本：70px 粗体，6px 黑色描边 + 白色填充，画在半透明灰底上。
    let main_text = "熊猫智研社 Test 123";
    println!("渲染主探针文本：{main_text:?}");
    let pixmap = render_text(&mut font_system, &family, main_text, 900, 220)?;
    pixmap
        .save_png("text_probe.png")
        .context("保存 text_probe.png 失败")?;
    println!("已写出 text_probe.png");

    let stats = analyze(&pixmap);
    println!(
        "  近纯白像素数：{}，近纯黑像素数：{}",
        stats.near_white, stats.near_black
    );
    match stats.bbox {
        Some((min_x, min_y, max_x, max_y)) => {
            let w = max_x - min_x + 1;
            let h = max_y - min_y + 1;
            println!("  非透明前景包围盒：({min_x},{min_y})-({max_x},{max_y})，宽 {w}px 高 {h}px");
            if stats.near_white == 0 || stats.near_black == 0 {
                println!("  [FAIL] 缺少近纯白或近纯黑像素——描边或填充未生效");
            } else if !(500..=800).contains(&w) {
                println!("  [WARN] 包围盒宽度 {w}px 超出预期的 500~800px 量级，请人工核对截图");
            } else {
                println!("  [PASS] 白/黑像素均存在，包围盒宽度在预期量级内");
            }
        }
        None => println!("  [FAIL] 没有任何非背景像素——什么都没画出来"),
    }

    // 额外核实中文没有被渲染成豆腐块：分别渲染「熊猫」和「智研」，
    // 如果两者都是豆腐块（.notdef），它们的形状、宽度往往一致，
    // 两张图会几乎逐像素相同；只要像素分布不同，就说明确实取到了各自的字形轮廓。
    println!("豆腐块核对：分别渲染「熊猫」与「智研」并比较像素……");
    let pixmap_a = render_text(&mut font_system, &family, "熊猫", 300, 220)?;
    let pixmap_b = render_text(&mut font_system, &family, "智研", 300, 220)?;
    pixmap_a.save_png("text_probe_cjk_a.png").ok();
    pixmap_b.save_png("text_probe_cjk_b.png").ok();

    let data_a = pixmap_a.data();
    let data_b = pixmap_b.data();
    let differing = data_a
        .iter()
        .zip(data_b.iter())
        .filter(|(a, b)| a != b)
        .count();
    let stats_a = analyze(&pixmap_a);
    let stats_b = analyze(&pixmap_b);
    println!(
        "  「熊猫」黑/白像素：{}/{}，「智研」黑/白像素：{}/{}，不同字节数：{differing}",
        stats_a.near_black, stats_a.near_white, stats_b.near_black, stats_b.near_white
    );
    if differing == 0 {
        println!("  [FAIL] 两张图逐字节完全相同——高度疑似豆腐块（.notdef 复用同一个方块轮廓）");
    } else if stats_a.near_black == 0 || stats_b.near_black == 0 {
        println!("  [FAIL] 至少一张图没有黑色描边像素，取轮廓可能失败了");
    } else {
        println!("  [PASS] 两张图像素分布不同，且都有黑/白像素——CJK 字形取到了各自正确的轮廓，不是豆腐块");
    }

    Ok(())
}
