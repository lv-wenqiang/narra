// 探针：验证「cosmic-text 排版 + ttf-parser 取字形轮廓 + tiny-skia 描边/填充」
// 这条路径在 Rust 里能否跑通，尤其是 CJK 文字是否能正确显示（而非豆腐块）。
//
// 全部 API 均已对照实际安装版本的源码核实（见下方各步骤注释里的版本号与源码路径），
// 不是照抄 brief 里"仅为说明意图"的伪代码。
//
// crate 版本（见 Cargo.lock）：cosmic-text 0.19.0、ttf-parser 0.25.1、
// tiny-skia 0.12.0、image 0.25.10。

use anyhow::{Context, Result, anyhow};
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight, fontdb};
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
    bold: bool,
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

            // 合成粗体（协调者拍板的方案，见 docs/text-rendering.md 的"合成粗体"一节）：
            // 按 size_px * 0.03 算出 bold_w，画三遍：
            //   1) 描边色描边，宽度 stroke_w+bold_w（bold=false 时就是原始 stroke_w）
            //   2) 仅 bold=true 时，再用**填充色**描边一次，宽度 bold_w——
            //      这一步把白色内芯向外挤宽 bold_w/2，产生"加粗"的视觉效果，
            //      同时黑色描边留下的外圈宽度仍是原始 stroke_w，完整包住加粗后的字形。
            //   3) 填充色填充路径内部（处理笔画本身较粗、两侧描边圈不了满的区域）。
            let bold_w = font_size * 0.03;
            let outer_stroke_width = if bold {
                stroke.width + bold_w
            } else {
                stroke.width
            };
            let outer_stroke = Stroke {
                width: outer_stroke_width,
                ..stroke.clone()
            };
            pixmap.stroke_path(
                &path,
                &black_paint,
                &outer_stroke,
                Transform::identity(),
                None,
            );
            if bold {
                let bold_stroke = Stroke {
                    width: bold_w,
                    ..stroke.clone()
                };
                pixmap.stroke_path(
                    &path,
                    &white_paint,
                    &bold_stroke,
                    Transform::identity(),
                    None,
                );
            }
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

/// 用给定的 family 排版一段文本，报告每个字形最终落在哪个字体、哪个 glyph_id 上。
/// 专门用来复现/验证"cosmic-text 静默回退到系统字体"这个问题
/// （见 docs/text-rendering.md"系统字体静默回退"一节）。
///
/// 返回 `(glyph_id, font_id, 该字体的 post_script_name)`。`glyph_id == 0` 就是
/// `.notdef`（字体自己也没有这个字符的占位符）。
fn resolve_glyphs(
    font_system: &mut FontSystem,
    family: &str,
    text: &str,
) -> Vec<(u16, fontdb::ID, Option<String>)> {
    let metrics = Metrics::new(70.0, 84.0);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(Some(400.0), Some(200.0));
    let attrs = Attrs::new().family(Family::Name(family));
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    let mut out = Vec::new();
    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            let physical = glyph.physical((0.0, 0.0), 1.0);
            let ck = physical.cache_key;
            let post_script_name = font_system
                .db()
                .face(ck.font_id)
                .map(|info| info.post_script_name.clone());
            out.push((ck.glyph_id, ck.font_id, post_script_name));
        }
    }
    out
}

/// 构造一个**从未调用过 `fontdb::Database::load_system_fonts()`** 的 `FontSystem`：
/// 自己 new 一个空 `fontdb::Database`，只塞入内嵌字体字节，再用
/// `FontSystem::new_with_locale_and_db` 包起来（这条构造路径完全跳过
/// `FontSystem::new()`/`new_with_fonts()` 里对 `load_fonts()` 的调用，
/// 见 cosmic-text-0.19.0/src/font/system.rs:191-208 与 :276）。
///
/// locale 传字面量 `"en-US"` 即可：locale 只用于给"缺字时按脚本/语言挑哪个系统
/// 回退字体"排序（`Fallbacks::new`），而这里我们压根没有系统字体可回退，locale
/// 的取值不影响任何行为。
fn font_system_without_system_fonts(font_bytes: &[u8]) -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(font_bytes.to_vec());
    FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}

fn main() -> Result<()> {
    let family = family_name_from_ttf(FONT_BYTES)?;
    println!("字体内部家族名（name 表 FAMILY 记录）：{family:?}");

    let mut font_system = FontSystem::new();
    font_system.db_mut().load_font_data(FONT_BYTES.to_vec());

    // 探针要求的主文本：70px 粗体，6px 黑色描边 + 白色填充，画在半透明灰底上。
    let main_text = "熊猫智研社 Test 123";
    println!("渲染主探针文本：{main_text:?}");
    let pixmap = render_text(&mut font_system, &family, main_text, 900, 220, true)?;
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
    let pixmap_a = render_text(&mut font_system, &family, "熊猫", 300, 220, false)?;
    let pixmap_b = render_text(&mut font_system, &family, "智研", 300, 220, false)?;
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
        println!(
            "  [PASS] 两张图像素分布不同，且都有黑/白像素——CJK 字形取到了各自正确的轮廓，不是豆腐块"
        );
    }

    // ------------------------------------------------------------------
    // 系统字体静默回退：复现问题 + 验证规避方案
    // （审查者在代码审查里指出的坑，见 docs/text-rendering.md 对应一节）
    // ------------------------------------------------------------------
    // 内嵌字体（CJK 字形为主）大概率不覆盖阿拉伯字母，用 U+0627 (ا) 探测。
    let uncovered = "\u{0627}";
    println!("系统字体回退核对：用内嵌字体大概率不覆盖的字符 {uncovered:?} (U+0627) 探测……");

    println!("  1) FontSystem::new()（会 load_system_fonts）+ load_font_data：");
    let leaky = resolve_glyphs(&mut font_system, &family, uncovered);
    for (glyph_id, font_id, psname) in &leaky {
        println!("     glyph_id={glyph_id}, font_id={font_id:?}, post_script_name={psname:?}");
    }
    let embedded_face = Face::parse(FONT_BYTES, 0).context("解析内嵌字体失败")?;
    let embedded_psname = family_name_from_ttf(FONT_BYTES).ok();
    let leaked_to_other_font = leaky
        .iter()
        .any(|(_, _, psname)| psname.as_deref() != embedded_psname.as_deref());
    if leaked_to_other_font {
        println!(
            "     [复现成功] 落到了内嵌字体（family={embedded_psname:?}）以外的字体上——\
             cosmic-text 静默回退到了系统字体，而不是内嵌字体自己的 .notdef。"
        );
    } else {
        println!(
            "     [未复现] 本机环境下没有触发系统字体回退（可能本机没有覆盖该字符的系统字体，\
             或内嵌字体本身意外覆盖了它）——不代表这个坑不存在，换一台机器/换一个字符仍可能触发。"
        );
    }
    let _ = embedded_face; // 仅用于确认能正常解析，避免 unused 警告

    println!("  2) font_system_without_system_fonts()（跳过 load_system_fonts）+ 同一个字符：");
    let mut isolated_font_system = font_system_without_system_fonts(FONT_BYTES);
    let isolated = resolve_glyphs(&mut isolated_font_system, &family, uncovered);
    for (glyph_id, font_id, psname) in &isolated {
        println!("     glyph_id={glyph_id}, font_id={font_id:?}, post_script_name={psname:?}");
    }
    let all_notdef_on_own_font = isolated.iter().all(|(glyph_id, _, psname)| {
        *glyph_id == 0 && psname.as_deref() == embedded_psname.as_deref()
    });
    if all_notdef_on_own_font {
        println!(
            "     [PASS] 落回了内嵌字体自己的 .notdef（glyph_id=0），没有回退到别的字体——\
             规避方案有效。"
        );
    } else {
        println!("     [FAIL] 仍然没有落在内嵌字体自己的 .notdef 上，规避方案未生效");
    }

    // ------------------------------------------------------------------
    // 合成粗体：对比 bold=true / bold=false 的墨迹量（alpha > 128 的像素数）
    // ------------------------------------------------------------------
    println!("合成粗体核对：对比同一段文字 bold=true / bold=false 的墨迹量……");
    let bold_text = "熊猫智研社";
    let pixmap_thin = render_text(&mut font_system, &family, bold_text, 500, 220, false)?;
    let pixmap_bold = render_text(&mut font_system, &family, bold_text, 500, 220, true)?;
    // 注意：不能直接数 `alpha > 128` 的像素——画布背景本身是半透明灰底
    // （alpha=180），会把整张画布都算成"墨迹"，bold/非 bold 两版因此测出同一个数字
    // （已实测踩到：改前两者都等于画布总像素数 500*220=110000）。
    // 复用 `analyze()` 的近纯白+近纯黑统计（要求 alpha==255，天然排除半透明背景）。
    let ink = |p: &Pixmap| -> usize {
        let s = analyze(p);
        s.near_white + s.near_black
    };
    let ink_thin = ink(&pixmap_thin);
    let ink_bold = ink(&pixmap_bold);
    let ratio = ink_bold as f64 / ink_thin as f64;
    println!("  bold=false 墨迹像素数：{ink_thin}");
    println!("  bold=true  墨迹像素数：{ink_bold}");
    println!("  比例：{ratio:.3}");
    if ink_bold <= ink_thin {
        println!("  [FAIL] 加粗版墨迹量没有比非加粗版多，合成粗体没有生效");
    } else if ratio > 2.5 {
        println!("  [FAIL] 墨迹量比例 {ratio:.3} 超过 2.5 倍，加粗过头，检查 bold_w 参数");
    } else {
        println!("  [PASS] 加粗版墨迹明显更多（{ratio:.3} 倍），且未超过 2.5 倍上限");
    }

    Ok(())
}
