use super::*;
use crate::config::Branding;
use crate::render::canvas::Canvas;
use crate::render::metrics::Metrics;
use crate::vtt::Caption;
use tiny_skia::Pixmap;

/// 测试用品牌名。**刻意不等于生产默认值「墨风」**——若这里写「墨风」，把
/// `self.brand` 换成字面量 `"墨风"` 的变异就检不出来了：测试要断言的是
/// 「画的是传进去的那个品牌名」，而不是「画的字符串恰好等于默认值」。
const TEST_BRAND: &str = "测试品牌";

/// 测试用封面/片尾水印文案。含 `·` 是为了顺带覆盖分隔符那条排版规则。
const TEST_COVER_WM: &str = "测试水印 · 副标题";

/// 「{TEST_COVER_WM}」在 28px 下的墨宽实测值，见
/// `cover_watermark_is_centered_at_640_576_with_alpha_102` 的文档。
const COVER_WM_INK_WIDTH_PX: i32 = 229;

/// 正文水印墨迹的实测底边 y 与墨宽，见
/// `watermark_ink_geometry_and_alpha_are_exact`。
const CONTENT_WM_INK_BOTTOM_Y: i32 = 675;
const CONTENT_WM_INK_WIDTH_PX: i32 = 92;

/// 测试用正文水印文案，**与 [`TEST_COVER_WM`] 不同**——两处配不同的文案，
/// 「把两处读反」的变异才检得出来。
const TEST_CONTENT_WM: &str = "正文水印";

/// 默认装备：只有品牌名，两处水印都不画（= 用户什么都没配的形态）。
fn test_branding() -> Branding {
    Branding::plain(TEST_BRAND)
}

/// 按需配水印的装备。两处分别可为 `None`，正是为了让「把两处读反」「本该
/// 不画却画了」这两类变异可检出。
fn branding_with(content: Option<&str>, cover: Option<&str>) -> Branding {
    Branding {
        brand: TEST_BRAND.to_string(),
        watermark: content.map(str::to_string),
        watermark_cover: cover.map(str::to_string),
        watermark_icon: None,
        logo: None,
    }
}

/// 整幅 `Pixmap` 的 maxAlpha。
fn max_alpha_of(p: &Pixmap) -> u8 {
    let mut m = 0u8;
    for y in 0..p.height() {
        for x in 0..p.width() {
            if let Some(c) = p.pixel(x, y) {
                m = m.max(c.alpha());
            }
        }
    }
    m
}

fn caps() -> Vec<Caption> {
    vec![
        Caption {
            text: "第一条字幕。".into(),
            start_ms: 0,
            end_ms: 2000,
        },
        Caption {
            text: "第二条字幕。".into(),
            start_ms: 2000,
            end_ms: 5000,
        },
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut p, 30, &caps());
    // 四角必须仍是全透明——Content 段不能画底
    for (x, y) in [(0, 0), (1279, 0), (0, 719), (1279, 719)] {
        assert_eq!(
            p.pixel(x, y).unwrap().alpha(),
            0,
            "角点 ({x},{y}) 不应被填充"
        );
    }
}

#[test]
fn picks_the_caption_covering_the_current_time() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    // frame 30 → 1000ms → 第一条；frame 105 → 3500ms → 第二条
    let mut a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut a, 30, &caps());
    painter.draw_content(&mut b, 105, &caps());
    // 两条字幕文本不同，像素分布必然不同
    assert_ne!(a.data(), b.data(), "不同时刻应显示不同字幕");
}

/// 未配置水印时，字幕结束后的 Content 帧**完全空白**。
///
/// 这是「默认不画水印」的主判据，也是本次改动最容易被悄悄改回去的地方：
/// 只要有人给 `content_watermark` 塞一个非 `None` 的兜底，这条就会变红。
#[test]
fn draws_nothing_when_no_caption_covers_the_time_and_no_watermark_is_configured() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut past_end = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut past_end, 300, &caps()); // 10000ms，超出最后一条
    assert_eq!(
        count_visible(&past_end),
        0,
        "未配置水印时，无字幕的 Content 帧应一个可见像素都没有"
    );
}

/// 配置了正文水印时，字幕结束后只剩水印。
#[test]
fn draws_nothing_but_watermark_when_no_caption_covers_the_time() {
    let mut painter =
        Painter::new(&branding_with(Some(TEST_CONTENT_WM), None), Canvas::BASE).unwrap();
    let mut with_cap = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut past_end = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut with_cap, 30, &caps());
    painter.draw_content(&mut past_end, 300, &caps()); // 10000ms，超出最后一条
    assert!(
        count_visible(&past_end) < count_visible(&with_cap),
        "字幕结束后可见像素应显著减少（只剩水印）"
    );
    assert!(count_visible(&past_end) > 0, "水印应该还在");
}

#[test]
fn watermark_sits_in_the_lower_left_corner() {
    let mut painter =
        Painter::new(&branding_with(Some(TEST_CONTENT_WM), None), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut p, 300, &caps()); // 无字幕，只剩水印
    // 水印在左下：距左 40px、距下 40px 附近应有像素，右上角不应有
    let has_in = (30..400)
        .any(|x| (620..700).any(|y| p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false)));
    let has_top_right = (900..1280)
        .any(|x| (0..200).any(|y| p.pixel(x, y).map(|c| c.alpha() > 0).unwrap_or(false)));
    assert!(has_in, "左下角应有水印");
    assert!(!has_top_right, "右上角不应有内容");
}

#[test]
fn caption_entrance_animation_changes_over_time() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    // 入场动画持续 min(500ms, 时长*0.3)。第一条时长 2000ms → 500ms → 15 帧
    let mut f0 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f7 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f20 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut f0, 0, &caps());
    painter.draw_content(&mut f7, 7, &caps());
    painter.draw_content(&mut f20, 20, &caps());
    assert_ne!(f0.data(), f7.data(), "动画中途应与起点不同");
    assert_ne!(f7.data(), f20.data(), "动画结束后应与中途不同");
}

#[test]
fn long_caption_uses_smaller_font() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    // 去空白后 > 50 字 → 52px；否则 80px
    let short = vec![Caption {
        text: "短句。".into(),
        start_ms: 0,
        end_ms: 5000,
    }];
    let long_text: String = "长".repeat(60) + "。";
    let long = vec![Caption {
        text: long_text,
        start_ms: 0,
        end_ms: 5000,
    }];
    let mut a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
        Caption {
            text: "短。".into(),
            start_ms: 0,
            end_ms: 2000,
        },
        Caption {
            text: "这是一条长得多的字幕文本用于对比宽度。".into(),
            start_ms: 2000,
            end_ms: 5000,
        },
    ]
}

/// **终审修复波次（第 2 项的固化断言，复审裁定必须加）**：`Metrics::caption_max_width`
/// 在 BASE 上是 `1024 - 2*40 = 944`，不是 `CANVAS_W * 0.8 = 1024`——TS 的
/// `Content.tsx` 在同一个 div 上同时写了 `width:'80%'` 与 `padding:'20px 40px'`，
/// tailwindcss v4 preflight 的 `box-sizing: border-box` 下内容宽度是 944。
///
/// 加这条断言的理由不是"怕值算错"，而是**没有测试守着的正确值会被改回错值**：
/// `画布宽度 * 0.8` 这个看起来更"自然"的写法就在 `Metrics::intro_title_max_width`
/// 旁边，回退成本极低；实测把常量改回 1024 跑全量套件是 134 passed / 0 failed，
/// 944 与 1024 对改动前的套件**完全等价**。
///
/// 上下限是**推导**出来的、不是把当前输出抄成期望值（所以不是变更探测器）：
/// 墨迹宽 = 排版宽度上限 + 描边外扩。上限 `950 = 944 + 6`（`Metrics::caption_stroke_width`
/// 在 BASE 上是 6px，左右各外扩半个描边宽）；下限 `920` 只是要求这段文本确实把 944 用满，
/// 否则"墨迹 <= 950"这个约束会因为文本太短而自动成立、失去鉴别力。
#[test]
fn long_caption_wraps_inside_the_944px_content_box_not_the_1024px_container() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let text = "这是一段专门用来触发换行的长字幕文本总共超过五十个字符会走五十二像素的小字号分支并且必然需要折成好几行来显示效果";
    let caps = vec![Caption {
        text: text.into(),
        start_ms: 0,
        end_ms: 5000,
    }];
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut p, 90, &caps);
    let (x0, _, x1, _) = bbox_excluding_watermark(&p, CAPTION_SCAN_Y_MAX).expect("长字幕应有墨迹");
    let ink_w = x1 - x0 + 1;
    assert!(
        ink_w <= 950,
        "字幕换行宽度受 944px 内容盒约束（1024 容器 - 2x40 padding），墨迹再加 6px 描边外扩，上限 950；实得 {ink_w}"
    );
    assert!(
        ink_w >= 920,
        "这段文本必须把 944px 用满，否则本测试对上限的约束失去意义；实得 {ink_w}"
    );
}

#[test]
fn translate_x_shifts_caption_center_by_about_70px_from_frame_1_to_15() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut f1 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f15 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut f1, 1, &caps());
    painter.draw_content(&mut f15, 15, &caps());
    let b1 = bbox_excluding_watermark(&f1, CAPTION_SCAN_Y_MAX).expect("f=1 应有墨迹");
    let b15 = bbox_excluding_watermark(&f15, CAPTION_SCAN_Y_MAX).expect("f=15 应有墨迹");
    let cx1 = (b1.0 + b1.2) as f32 / 2.0;
    let cx15 = (b15.0 + b15.2) as f32 / 2.0;
    let delta = cx1 - cx15;
    assert!(
        (delta - 70.0).abs() <= 8.0,
        "translate_x 引起的中心位移应 ≈70px±8，实得 {delta}"
    );
}

#[test]
fn scale_shrinks_caption_height_ratio_by_about_1_14_from_frame_1_to_15() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut f1 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f15 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut f1, 1, &caps());
    painter.draw_content(&mut f15, 15, &caps());
    let b1 = bbox_excluding_watermark(&f1, CAPTION_SCAN_Y_MAX).expect("f=1 应有墨迹");
    let b15 = bbox_excluding_watermark(&f15, CAPTION_SCAN_Y_MAX).expect("f=15 应有墨迹");
    let h1 = (b1.3 - b1.1) as f32;
    let h15 = (b15.3 - b15.1) as f32;
    let ratio = h1 / h15;
    assert!(
        (ratio - 1.14).abs() <= 0.03,
        "scale 引起的高度比应 ≈1.14±0.03，实得 {ratio}"
    );
}

/// **实测记录（步长 1 时的原始数据，见报告）**：u8 量化会在临近饱和处让相邻帧打平
/// ——frame 13/14 的 maxAlpha 都四舍五入到 254（连续值分别是 0.994837/0.997870，
/// 差值 < 1/255）。这不是实现 bug，是 8-bit alpha 精度的物理极限；换成隔帧
/// （步长 2）比较后整条曲线在全程 1..15 上严格递增，仍然足以钉住"曲线在爬升、
/// 不是恒定值/提前封顶/整体反向"这条要害。
#[test]
fn opacity_increases_and_saturates_at_255_by_frame_15() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut alphas = [0u8; 15];
    let mut prev = 0u8;
    for f in 1..=15u32 {
        let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
        painter.draw_content(&mut p, f, &caps());
        let alpha = max_alpha_excluding_watermark(&p, CAPTION_SCAN_Y_MAX);
        assert!(
            alpha >= prev,
            "frame {f} 的 maxAlpha 不应比上一帧小：{prev} -> {alpha}"
        );
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
            if p.pixel(x, y)
                .map(|c| c.alpha() >= threshold)
                .unwrap_or(false)
            {
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut f2 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f14 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    // 10 字在 80px 下单行即可容纳（10*80=800px < 1024px），干净的单行基准。
    let short_text = "长".repeat(10);
    // 51 非空白字符：超过 50 阈值 → 52px，会换行，取第一行做同样干净的基准。
    let long_text = "长".repeat(51);
    let short = vec![Caption {
        text: short_text,
        start_ms: 0,
        end_ms: 5000,
    }];
    let long = vec![Caption {
        text: long_text,
        start_ms: 0,
        end_ms: 5000,
    }];
    let mut a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let base = vec![Caption {
        text: "长长长".into(),
        start_ms: 0,
        end_ms: 5000,
    }];
    let padded_text = format!("长{}长长", " ".repeat(60));
    let padded = vec![Caption {
        text: padded_text,
        start_ms: 0,
        end_ms: 5000,
    }];
    let mut a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let caps = caps_varied_length();
    let mut f59 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f60 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f61 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut f59, 59, &caps);
    painter.draw_content(&mut f60, 60, &caps);
    painter.draw_content(&mut f61, 61, &caps);

    let b59 = bbox_excluding_watermark(&f59, CAPTION_SCAN_Y_MAX);
    let b61 = bbox_excluding_watermark(&f61, CAPTION_SCAN_Y_MAX);
    assert!(b59.is_some(), "f=59（第一条字幕内）应有墨迹");
    assert!(b61.is_some(), "f=61（第二条字幕内）应有墨迹");
    let w59 = b59.unwrap().2 - b59.unwrap().0;
    let w61 = b61.unwrap().2 - b61.unwrap().0;
    assert_ne!(
        w59, w61,
        "两条长度不同的字幕，墨宽应不同：f=59 宽={w59} f=61 宽={w61}"
    );

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
        Caption {
            text: "短句。".into(),
            start_ms: 0,
            end_ms: 3000,
        },
        Caption {
            text: "这是第二条更长一些的重叠字幕文本。".into(),
            start_ms: 1000,
            end_ms: 2000,
        },
    ]
}

#[test]
fn overlapping_captions_pick_the_first_match_in_the_list() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let caps = overlapping_caps();
    let only_first = vec![caps[0].clone()];

    // frame 45 -> 1500ms：同时落在两条区间 [0,3000) 与 [1000,2000) 内。
    let mut with_both = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut with_first_only = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut with_both, 45, &caps);
    painter.draw_content(&mut with_first_only, 45, &only_first);

    assert_eq!(
        with_both.data(),
        with_first_only.data(),
        "重叠区间内应选取列表中第一条匹配的字幕（规格 §8.4：满足 start<=t<end 的第一条），             渲染结果应与「列表里只有第一条」时逐字节相同"
    );
}

/// 正文水印的墨迹几何与 alpha 精确值。
///
/// 图标移除后文字直接从 `Metrics::watermark_margin_left` 起排，BASE 上是
/// 40，所以左边缘就是 40；`maxAlpha` 精确 69（`rgba(255,255,255,0.27)`，
/// 且未被合成粗体或描边叠厚——`content` 预设特意用 `bold: false` +
/// `stroke: None`，三遍重叠会让半透明水印在交叠处变浓）。
///
/// **墨宽区间钉的是「文案 + 字号」而不是某个写死的字符串**：文案由
/// `TEST_CONTENT_WM` 给，改字号（`Metrics::watermark_font_size` BASE 上
/// 24→其它）会让它落到区间外。
#[test]
fn watermark_ink_geometry_and_alpha_are_exact() {
    let mut painter =
        Painter::new(&branding_with(Some(TEST_CONTENT_WM), None), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut p, 300, &caps()); // 无字幕，只剩水印
    let (x0, _y0, x1, y1) = non_transparent_bbox(&p).expect("水印应有墨迹");
    // 行首在 x=40（`Metrics::watermark_margin_left`，BASE 上是 40），墨迹起点还要加上首字形的
    // 左边距（left side bearing），所以是「贴着 40 但不等于 40」。图标时代
    // 这里能精确取 40，是因为图标位图铺满自己的框、没有边距。
    assert!(
        (40..=44).contains(&x0),
        "水印左边缘应紧贴 x=40（含首字形左边距），实得 {x0}"
    );
    assert!(
        (CONTENT_WM_INK_BOTTOM_Y - y1 as i32).abs() <= 2,
        "水印底边缘应在 y≈{CONTENT_WM_INK_BOTTOM_Y}，实得 {y1}"
    );
    assert_eq!(
        max_alpha_of(&p),
        69,
        "水印 maxAlpha 应精确等于 69（未被叠厚）"
    );
    let width = x1 as i32 - x0 as i32;
    assert!(
        (width - CONTENT_WM_INK_WIDTH_PX).abs() <= 3,
        "「{TEST_CONTENT_WM}」的墨宽应约为 {CONTENT_WM_INK_WIDTH_PX}±3px，实得 {width}"
    );
}

#[test]
fn trailing_spaces_in_caption_do_not_shift_rendering() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let base = vec![Caption {
        text: "文字".into(),
        start_ms: 0,
        end_ms: 5000,
    }];
    let padded = vec![Caption {
        text: "文字   ".into(),
        start_ms: 0,
        end_ms: 5000,
    }];
    let mut a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut a, 20, &base);
    painter.draw_content(&mut b, 20, &padded);
    assert_eq!(
        a.data(),
        b.data(),
        "「文字」与「文字   」渲染结果应当一致（M1：尾随空格不应影响居中）"
    );
}

// ------------------------------------------------------------------
// Task 6：Cover / Intro。以下 5 条测试是 brief Step 1 原样照抄，一字未改。
// ------------------------------------------------------------------

#[test]
fn cover_paints_an_opaque_white_background() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut p, "测试标题");
    for (x, y) in [(0, 0), (1279, 0), (0, 719), (1279, 719)] {
        let c = p.pixel(x, y).unwrap();
        assert_eq!(c.alpha(), 255, "角点 ({x},{y}) 应不透明");
        assert!(
            c.red() > 240 && c.green() > 240 && c.blue() > 240,
            "角点应为白底"
        );
    }
}

#[test]
fn intro_paints_an_opaque_white_background() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut p, 60, "测试标题");
    let c = p.pixel(0, 0).unwrap();
    assert_eq!(c.alpha(), 255);
    assert!(c.red() > 240 && c.green() > 240 && c.blue() > 240);
}

#[test]
fn intro_typewriter_reveals_more_characters_over_time() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let title = "这是一个比较长的测试标题用来看打字机效果";
    let mut early = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut mid = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut done = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut early, 5, title);
    painter.draw_intro(&mut mid, 30, title);
    painter.draw_intro(&mut done, 62, title); // 2 秒 = 60 帧后打完

    let ink = |p: &Pixmap| {
        (0..p.height())
            .flat_map(|y| (0..p.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let c = p.pixel(x, y).unwrap();
                c.red() < 200
            })
            .count()
    };
    assert!(ink(&early) < ink(&mid), "第 30 帧应比第 5 帧显示更多字");
    assert!(ink(&mid) < ink(&done), "打完后应比中途更多字");
}

#[test]
fn intro_fades_out_at_the_end() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let title = "淡出测试";
    let mut before = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut last = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut before, 89, title); // 淡出开始前
    painter.draw_intro(&mut last, 104, title); // 淡出终点
    let ink = |p: &Pixmap| {
        (0..p.height())
            .flat_map(|y| (0..p.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let c = p.pixel(x, y).unwrap();
                c.red() < 200
            })
            .count()
    };
    assert!(ink(&last) < ink(&before), "第 104 帧应比第 89 帧淡");
}

#[test]
fn cover_shows_the_given_title() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
        size_px: painter.m.cover_title_font_size,
        color: TITLE_COLOR_BLACK,
        stroke: None,
        letter_spacing_px: 0.0,
        max_width_px: painter.m.cover_title_max_width,
        line_height: DEFAULT_LINE_HEIGHT,
        bold: true,
    };
    let (_, title_h) = painter.renderer.measure(title, &title_style);
    let row_height = painter.m.cover_row_height;
    let container_h = row_height + title_h;
    let container_top = painter.m.cover_container_center_y - container_h / 2.0;
    let row_bottom = container_top + row_height;
    let title_bottom = row_bottom + title_h;
    let canvas_h = painter.m.canvas.h_f32();

    const MARGIN: f32 = 8.0;
    (
        (container_top - MARGIN).max(0.0).floor() as u32,
        (row_bottom + MARGIN).ceil() as u32,
        (row_bottom + MARGIN).ceil() as u32,
        (title_bottom + MARGIN).ceil().min(canvas_h) as u32,
    )
}

/// Cover 上排品牌名的整体不透明度应精确为 0.30
/// （黑字合成到白底：`darkness ≈ round(255*0.30) = 76`），而不是 255
/// （忘了施加整体透明度）。只扫文字所在的 x 范围（`Metrics::cover_row_text_left`
/// 起，用字段而非字面量——避开 logo 颜色未知会污染这个精确数值，同时
/// logo 尺寸变化时这个字段本身也会跟着动，不会读到过时的边界）。
#[test]
fn cover_top_row_opacity_is_about_76_not_opaque() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut p, "标题");
    let (row_y0, row_y1, _, _) = cover_dynamic_windows(&mut painter, "标题");
    let row_text_left = painter.m.cover_row_text_left as u32;
    let max_darkness = max_darkness_in_rect(&p, row_text_left, 1000, row_y0, row_y1);
    assert!(
        (max_darkness as i32 - 76).abs() <= 8,
        "上排文字 darkness 应约为 76（0.30 组透明度合成到白底），实得 {max_darkness}"
    );
    assert_ne!(max_darkness, 255, "上排文字不应是纯黑（未施加整体透明度）");
}

/// Cover 应该画出 logo：logo 占据的 36x36 区域内应有非白像素。
#[test]
fn cover_draws_a_logo_in_the_top_row() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut p, "标题");
    // logo 左边缘 x=176，尺寸 36px；容器顶 <= 274（见上一条推导），
    // 故 logo 顶 <= 274+8=282，给足够宽的窗口。
    let has_logo_ink = (176..212)
        .any(|x| (0..320).any(|y| p.pixel(x, y).map(|c| darkness(c) > 0).unwrap_or(false)));
    assert!(has_logo_ink, "logo 所在的 36x36 区域内应有非白像素");
}

/// **修复轮 1（I2）**：logo 自身的 0.30 组透明度此前没有任何断言
/// （审查变异 N19：logo 改成 `PixmapPaint::default()`，即不施加 0.30，
/// 31 条测试一条都不响——`cover_top_row_opacity_…` 的扫描窗口刻意避开了
/// logo，`cover_draws_a_logo_in_the_top_row` 只判断"非白"、不判断具体
/// 深浅）。这里对 logo 自身的像素区域做同样精确的 darkness 断言。
#[test]
fn cover_logo_opacity_is_about_76_not_opaque() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut p, "标题");
    let (row_y0, row_y1, _, _) = cover_dynamic_windows(&mut painter, "标题");
    let logo_x0 = painter.m.cover_row_left as u32 + painter.m.cover_logo_margin as u32;
    let logo_x1 = logo_x0 + painter.m.cover_logo_size;
    let max_darkness = max_darkness_in_rect(&p, logo_x0, logo_x1, row_y0, row_y1);
    assert!(
        (max_darkness as i32 - 76).abs() <= 10,
        "logo darkness 应约为 76（0.30 组透明度合成到白底），实得 {max_darkness}"
    );
    assert_ne!(
        max_darkness, 255,
        "logo 不应是不透明的纯色（未施加整体透明度）"
    );
}

/// **修复轮 1（I3）**：上排"左对齐、行左边缘 x=176"此前没有任何断言
/// （审查变异 N3：去掉 `marginLeft 40`；N9：上排改成水平居中——双双存活）。
/// 上排（logo）是这一带最左侧的元素，直接断言该窗口内最左侧墨迹的 x 坐标。
#[test]
fn cover_top_row_left_edge_is_176() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut p, "标题");
    let (row_y0, row_y1, _, _) = cover_dynamic_windows(&mut painter, "标题");
    let (x0, _, _, _) = ink_bbox_in_y_range(&p, row_y0, row_y1).expect("上排应有墨迹");
    // 故意用字面量 176（而不是 `Metrics::cover_row_left + cover_logo_margin`）
    // 做期望值——审查实测就是 176。若改用这两个字段相加，当
    // `Metrics::cover_row_margin_left`（marginLeft 40）被错误改掉时，
    // `expected` 会跟着"一起错"，测试变成永远自证成立、测不出任何东西
    // （这正是变异验证时抓到的真实教训：用同一个被改动的量算期望值，
    // 等于没测）。
    assert!(
        (x0 as i32 - 176).abs() <= 2,
        "上排（logo）左边缘应≈176，实得 {x0}"
    );
}

/// Cover 主标题应是 100px 量级：单行短标题的墨高与上排 38px 文字墨高的比值
/// 应约为 100/38（±10%）。
#[test]
fn cover_title_font_size_matches_100px() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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

/// Cover 水印：直接检查 `prepare_watermark` 预渲染出的水印小图（透明底上画出
/// 来的，未与 Cover 白底合成，因此可以直接读 `alpha` 精确核对数值，不受「合成
/// 到不透明白底后 alpha 恒为 255、只能靠颜色深浅反推」的影响）：墨迹（含
/// origin 换算回画布坐标）水平/垂直中心分别 ≈640/≈576（±4），全区 maxAlpha
/// 精确等于 102（`rgba(23,23,23,0.4)`）；并确认 `draw_cover` 真的把它画了
/// 出来（不只是 prepare 了但没调用）。
///
/// **墨宽的窄区间**沿用原修复轮 1（M1）的口径，数值随「文案可配置 + 图标已
/// 移除」重新实测。它覆盖字号这个此前零覆盖的精确参数：把
/// `Metrics::cover_watermark_font_size`（BASE 上 28）改成 24 会显著改变墨宽。
/// 渲染全程确定性（同一份字体 + 同一套矢量排版，无随机性来源），容差只需
/// 覆盖裁剪/取整的量级，给 ±3。
#[test]
fn cover_watermark_is_centered_at_640_576_with_alpha_102() {
    let mut renderer = TextRenderer::new().unwrap();
    let m = Metrics::for_canvas(Canvas::BASE);
    let prepared = prepare_watermark(
        &mut renderer,
        None,
        &cover_watermark_preset(TEST_COVER_WM, &m),
        m.no_wrap_width,
        Canvas::BASE,
    )
    .unwrap();
    let (x0, y0, x1, y1) = non_transparent_bbox(&prepared.pixmap).expect("cover 水印应有墨迹");
    let canvas_x0 = prepared.origin_x + x0 as i32;
    let canvas_x1 = prepared.origin_x + x1 as i32;
    let canvas_y0 = prepared.origin_y + y0 as i32;
    let canvas_y1 = prepared.origin_y + y1 as i32;
    let cx = (canvas_x0 + canvas_x1) as f32 / 2.0;
    let cy = (canvas_y0 + canvas_y1) as f32 / 2.0;
    assert!((cx - 640.0).abs() <= 4.0, "水印水平中心应≈640，实得 {cx}");
    assert!((cy - 576.0).abs() <= 4.0, "水印垂直中心应≈576，实得 {cy}");

    assert_eq!(
        max_alpha_of(&prepared.pixmap),
        102,
        "cover 水印 maxAlpha 应精确等于 102"
    );

    let width = canvas_x1 - canvas_x0;
    assert!(
        (width - COVER_WM_INK_WIDTH_PX).abs() <= 3,
        "「{TEST_COVER_WM}」的墨宽应约为 {COVER_WM_INK_WIDTH_PX}±3px，实得 {width}"
    );

    // 确认 draw_cover 真的调用了它，不只是 Painter::new() 里预渲染了但没贴图。
    let mut painter =
        Painter::new(&branding_with(None, Some(TEST_COVER_WM)), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut p, "标题");
    assert!(
        ink_bbox_in_y_range(&p, 550, 720).is_some(),
        "draw_cover 应该把 cover 水印实际画到画布上"
    );
}

/// 分隔点 `·` 的 0.75 倍率（原修复轮 1/M1，审查变异 N6：倍率 0.75→1.0 曾存活）。
///
/// **判据为什么换成「整幅图的 maxAlpha」**：早先的写法要在混排文案里重新推导
/// 分隔点的 x 范围再局部扫描，既依赖 `layout_and_draw_watermark` 的内部排版
/// 算法（改一处排版就要同步改测试），又要跟相邻分段的字宽外溢（overhang）
/// 斗争。文案可配置之后有更干净的办法：**让文案只有一个 `·`**——没有相邻
/// 分段，整幅图的 maxAlpha 就是分隔点自己的封顶值，一个数把倍率钉死。
///
/// 与 `non_separator_text_is_drawn_at_full_opacity` **成对存在**：单独看
/// 「≈77」不足以说明倍率被施加了（把颜色 alpha 本身改成 77 也会得 77），
/// 两条一起才把「满倍率 102 / 分隔符 0.75 倍 → 77」这组关系钉住。
#[test]
fn separator_segment_is_drawn_at_reduced_opacity() {
    let mut renderer = TextRenderer::new().unwrap();
    let m = Metrics::for_canvas(Canvas::BASE);
    let prepared = prepare_watermark(
        &mut renderer,
        None,
        &cover_watermark_preset(WATERMARK_SEP, &m),
        m.no_wrap_width,
        Canvas::BASE,
    )
    .unwrap();
    let max_alpha = max_alpha_of(&prepared.pixmap);
    assert!(
        (max_alpha as i32 - 77).abs() <= 3,
        "只含「·」的水印，maxAlpha 应≈77（102×0.75），实得 {max_alpha}"
    );
}

/// 非分隔符的文字按满倍率画（`cover` 预设 `rgba(23,23,23,0.4)` → 102）。
/// 与上面那条成对，见其文档。
#[test]
fn non_separator_text_is_drawn_at_full_opacity() {
    let mut renderer = TextRenderer::new().unwrap();
    let m = Metrics::for_canvas(Canvas::BASE);
    let prepared = prepare_watermark(
        &mut renderer,
        None,
        &cover_watermark_preset("测试", &m),
        m.no_wrap_width,
        Canvas::BASE,
    )
    .unwrap();
    assert_eq!(
        max_alpha_of(&prepared.pixmap),
        102,
        "不含分隔符的水印 maxAlpha 应精确等于 102"
    );
}

/// `split_on_separator` 的纯函数行为：`·` 单独成段并降倍率，其余原样。
#[test]
fn separator_splitting_isolates_each_middot() {
    assert_eq!(
        split_on_separator("测试水印 · 副标题"),
        vec![
            ("测试水印 ".to_string(), 1.0),
            ("·".to_string(), WATERMARK_SEP_OPACITY_MUL),
            (" 副标题".to_string(), 1.0),
        ]
    );
    // 不含分隔符：单段满倍率，与「整段一个颜色」完全等价。
    assert_eq!(
        split_on_separator("没有分隔符"),
        vec![("没有分隔符".to_string(), 1.0)]
    );
    // 文案本身就是一个分隔符：不特判，那正是用户配的内容。
    assert_eq!(
        split_on_separator("·"),
        vec![("·".to_string(), WATERMARK_SEP_OPACITY_MUL)]
    );
    // 多个分隔符各自成段。
    assert_eq!(
        split_on_separator("a·b·c"),
        vec![
            ("a".to_string(), 1.0),
            ("·".to_string(), WATERMARK_SEP_OPACITY_MUL),
            ("b".to_string(), 1.0),
            ("·".to_string(), WATERMARK_SEP_OPACITY_MUL),
            ("c".to_string(), 1.0),
        ]
    );
}

/// 配了 `--watermark-icon` 时图标真的画到了水印上，且**保留自己的颜色**。
///
/// 判据用一个纯蓝方块图标配一个纯黑文字的 `cover` 预设：图标区若被按水印色
/// （`rgb(23,23,23)` 近黑）重新着色，蓝色通道就不会显著高于红色通道。这条
/// 同时钉住「图标画了」与「没被 tint」两件事——只断言「多了墨迹」的话，
/// 恢复 `tint_icon` 的变异会存活。
#[test]
fn watermark_icon_is_drawn_and_keeps_its_own_colors() {
    let dir = std::env::temp_dir().join(format!("panda_wm_icon_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("mark.png");
    // 纯蓝不透明方块。
    image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0xff, 0xff]))
        .save(&png)
        .unwrap();

    let branding = Branding {
        brand: TEST_BRAND.to_string(),
        watermark: None,
        watermark_cover: Some(TEST_COVER_WM.to_string()),
        watermark_icon: Some(png.to_string_lossy().into_owned()),
        logo: None,
    };
    let mut painter = Painter::new(&branding, Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut p, "标题");

    // 水印所在的 y 带里，找蓝色显著强于红色的像素——图标原色的证据。
    let has_blue = (0..1280).any(|x| {
        (550..620).any(|y| {
            p.pixel(x, y)
                .map(|c| c.blue() as i32 - c.red() as i32 > 20)
                .unwrap_or(false)
        })
    });
    assert!(
        has_blue,
        "图标应原样画出（蓝色保留）；若被按水印色重新着色，这里会是近灰的"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// 没配图标时，水印整行只有文字——不给图标留空位。
///
/// 判据是「同一段文案，配图标的墨迹比不配图标的更宽」，且不配时墨迹左边缘
/// 就是文字起点。只断言「不配时没有蓝色像素」是不够的：那对「留了空位但没
/// 画东西」（整行右移、文字位置错了）完全无感。
#[test]
fn watermark_without_an_icon_starts_at_the_text() {
    let dir = std::env::temp_dir().join(format!("panda_wm_noicon_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("mark.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0xff, 0xff]))
        .save(&png)
        .unwrap();

    let mut renderer = TextRenderer::new().unwrap();
    let m = Metrics::for_canvas(Canvas::BASE);
    let preset = cover_watermark_preset(TEST_COVER_WM, &m);
    let icon = load_scaled_icon(&png, preset.icon_size_px).unwrap();

    let bare =
        prepare_watermark(&mut renderer, None, &preset, m.no_wrap_width, Canvas::BASE).unwrap();
    let with_icon = prepare_watermark(
        &mut renderer,
        Some(&icon),
        &preset,
        m.no_wrap_width,
        Canvas::BASE,
    )
    .unwrap();

    // 图标虽是不透明的纯蓝，进水印后必须吃到预设的整体不透明度（cover =
    // 102/255），与同一行的文字浓淡一致。少了这条，「图标按原样 100% 不透明
    // 画上去」的变异检不出来——颜色仍是蓝的，只是浓得突兀。
    assert_eq!(
        max_alpha_of(&with_icon.pixmap),
        102,
        "图标应吃到水印预设的整体不透明度，而不是保持自身的 100% 不透明"
    );

    let bare_w = bare.pixmap.width();
    let icon_w = with_icon.pixmap.width();
    let expected_extra = preset.icon_size_px as f32 + preset.icon_gap_px;
    assert!(
        (icon_w as f32 - bare_w as f32 - expected_extra).abs() <= 3.0,
        "配图标后整行应正好宽出「图标 + 间距」= {expected_extra}px，实得 {bare_w} → {icon_w}"
    );

    // 两者都水平居中，所以配了图标之后整行左边缘应当左移约一半的增量。
    let shift = bare.origin_x - with_icon.origin_x;
    assert!(
        (shift as f32 - expected_extra / 2.0).abs() <= 3.0,
        "整行仍应水平居中：左边缘应左移约 {}px，实得 {shift}",
        expected_extra / 2.0
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **修复轮 1（M1）**：光标间距（advance 口径 4px）此前零覆盖（审查变异
/// N5：间距 4→40，存活）。断言"光标墨迹左边缘 − 最后一行墨迹右边缘"落在
/// `[4,25]`——4px 加在 advance 口径上，`|` 字形自身还有 side bearing，
/// 审查实测视觉间隙约 12px，故给一个覆盖两者的合理区间而不是精确值。
#[test]
fn cursor_gap_from_last_line_is_within_expected_range() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let long_title = "长".repeat(30); // 100px/70px 字号下必然远超 944px，需要换行

    let mut cover_p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut cover_p, &long_title);
    let (_, _, title_y0, title_y1) = cover_dynamic_windows(&mut painter, &long_title);
    let (cx0, _, cx1, _) =
        ink_bbox_in_y_range(&cover_p, title_y0, title_y1).expect("Cover 主标题应有墨迹");
    let cover_w = cx1 - cx0;
    assert!(
        cover_w <= 944 + 8,
        "Cover 主标题墨宽应 ≤944px（含少量描边/字距余量），实得 {cover_w}"
    );

    let mut intro_p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut intro_p, 70, &long_title); // 70 帧：打字已完成、尚未开始淡出
    let (ix0, _, ix1, _) = ink_bbox_in_y_range(&intro_p, 0, 720).expect("Intro 标题应有墨迹");
    let intro_w = ix1 - ix0;
    assert!(
        intro_w <= 944 + 8,
        "Intro 标题墨宽应 ≤944px，实得 {intro_w}"
    );
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut p, 30, "标题");
    assert!(
        ink_bbox_in_y_range(&p, 600, 720).is_none(),
        "Intro 下半部不应有水印墨迹"
    );
}

/// **默认不画水印的四段哨兵。** 配了品牌名、两处水印都留空时，四段里都不
/// 该出现水印。
///
/// 这条是本次改动最该守住的东西：水印从「写死必画」变成「配了才画」，最
/// 容易的回归是有人给 `content_watermark`/`cover_watermark` 塞一个非 `None`
/// 的兜底，那时四段会重新长出水印而其它测试多半仍是绿的。
///
/// 判据按段分开取：Content 段透明底，直接数可见像素；Cover/Outro 是白底，
/// 水印区在下半部，用 `ink_bbox_in_y_range` 扫墨迹。Intro 那一段沿用
/// `intro_has_no_watermark` 的既有口径与它记录的已知盲区。
#[test]
fn no_segment_draws_a_watermark_when_neither_is_configured() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();

    let mut content = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_content(&mut content, 300, &caps()); // 无字幕
    assert_eq!(count_visible(&content), 0, "Content 段不应有水印");

    let mut cover = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut cover, "标题");
    assert!(
        ink_bbox_in_y_range(&cover, 550, 720).is_none(),
        "Cover 下半部不应有水印墨迹"
    );

    let mut intro = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut intro, 30, "标题");
    assert!(
        ink_bbox_in_y_range(&intro, 600, 720).is_none(),
        "Intro 下半部不应有水印墨迹"
    );

    let mut outro = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_outro(&mut outro, 60);
    assert!(
        ink_bbox_in_y_range(&outro, 620, 720).is_none(),
        "Outro 底部不应有水印墨迹"
    );
}

/// 两个水印配置项互不影响。
///
/// 鉴别性判据，照 `each_material_env_var_is_wired_to_exactly_one_function`
/// 的思路：只配一处，断言**另一处仍然不画**。把 `Painter::new` 里两个
/// `branding.watermark*` 读反的变异，只看「配了就有水印」是抓不住的。
#[test]
fn the_two_watermark_settings_are_independent() {
    // 只配正文水印：Content 有，Cover 没有。
    let mut only_content =
        Painter::new(&branding_with(Some(TEST_CONTENT_WM), None), Canvas::BASE).unwrap();
    let mut c = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    only_content.draw_content(&mut c, 300, &caps());
    assert!(count_visible(&c) > 0, "配了 watermark，Content 段应有水印");
    let mut cov = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    only_content.draw_cover(&mut cov, "标题");
    assert!(
        ink_bbox_in_y_range(&cov, 550, 720).is_none(),
        "没配 watermark_cover，Cover 段不该因为配了 watermark 就长出水印"
    );

    // 只配封面水印：Cover 有，Content 没有。
    let mut only_cover =
        Painter::new(&branding_with(None, Some(TEST_COVER_WM)), Canvas::BASE).unwrap();
    let mut c2 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    only_cover.draw_content(&mut c2, 300, &caps());
    assert_eq!(
        count_visible(&c2),
        0,
        "没配 watermark，Content 段不该因为配了 watermark_cover 就长出水印"
    );
    let mut cov2 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    only_cover.draw_cover(&mut cov2, "标题");
    assert!(
        ink_bbox_in_y_range(&cov2, 550, 720).is_some(),
        "配了 watermark_cover，Cover 段应有水印"
    );
}

/// Cover 上排与 Outro 大字画的是**传入的品牌名**，不是任何写死的字符串。
///
/// 判据是「换一个品牌名，那块区域的像素必须变」：两个品牌名墨宽不同（2 字
/// vs 5 字），落在同一块区域上的像素不可能逐字节相同。把 `self.brand` 换回
/// 字面量的变异会让两次渲染完全一致，这条随即变红。
#[test]
fn cover_row_and_outro_title_render_the_configured_brand() {
    let mut short = Painter::new(&Branding::plain("墨风"), Canvas::BASE).unwrap();
    let mut long = Painter::new(&Branding::plain("另一个更长的品牌"), Canvas::BASE).unwrap();

    let mut cover_a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut cover_b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    short.draw_cover(&mut cover_a, "同一个标题");
    long.draw_cover(&mut cover_b, "同一个标题");
    assert_ne!(
        cover_a.data(),
        cover_b.data(),
        "Cover 上排应随品牌名变化，而不是写死的字符串"
    );

    // Outro 取一个大字已经完全淡入、整体淡出尚未开始的帧（淡入 [24,39]、
    // 淡出 [105,119]）。
    let mut outro_a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut outro_b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    short.draw_outro(&mut outro_a, 60);
    long.draw_outro(&mut outro_b, 60);
    assert_ne!(
        outro_a.data(),
        outro_b.data(),
        "Outro 大字应随品牌名变化，而不是写死的字符串"
    );
}

/// 配了 `--logo` 时 Cover 上排与 Outro 画的是**那张**图，不是内嵌的熊猫。
///
/// 判据是「换 logo 后那两段的像素必须变」。用一张纯红方块——它与内嵌 logo
/// （黑白线稿）在任何一处都不可能逐字节相同；同时顺带断言红色确实出现在
/// 画面上，堵住「读了文件但画的还是内嵌那张」这种半吊子实现。
#[test]
fn cover_and_outro_use_the_configured_logo() {
    let dir = std::env::temp_dir().join(format!("panda_logo_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("logo.png");
    image::RgbaImage::from_pixel(64, 64, image::Rgba([0xff, 0, 0, 0xff]))
        .save(&png)
        .unwrap();

    let mut embedded = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut custom = Painter::new(
        &Branding {
            logo: Some(png.to_string_lossy().into_owned()),
            ..test_branding()
        },
        Canvas::BASE,
    )
    .unwrap();

    let mut cover_a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut cover_b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    embedded.draw_cover(&mut cover_a, "同一个标题");
    custom.draw_cover(&mut cover_b, "同一个标题");
    assert_ne!(
        cover_a.data(),
        cover_b.data(),
        "Cover 上排应换成配置的 logo"
    );

    let mut outro_a = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut outro_b = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    embedded.draw_outro(&mut outro_a, 60);
    custom.draw_outro(&mut outro_b, 60);
    assert_ne!(outro_a.data(), outro_b.data(), "Outro 应换成配置的 logo");

    // 红色必须真的出现在 Outro 的 logo 区——只断言「像素变了」的话，
    // 「读了文件但仍画内嵌那张、只是别处偶然有差异」会蒙混过去。
    let has_red = (0..1280).any(|x| {
        (100..450).any(|y| {
            outro_b
                .pixel(x, y)
                .map(|c| c.red() as i32 - c.green() as i32 > 60)
                .unwrap_or(false)
        })
    });
    assert!(has_red, "Outro 的 logo 区应出现配置那张图的红色");

    std::fs::remove_dir_all(&dir).ok();
}

/// 打字机字符数：用一个不会换行的短标题（6 字），断言 frame 5/15/30 的墨宽
/// 阶梯上升，且打完（frame>=60）后与整串标题的墨宽一致（±4px）。
#[test]
fn typewriter_ink_width_steps_up_and_matches_full_title_when_done() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let title = "六个字标题呀"; // 6 字，不会换行
    let mut f5 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f15 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f30 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f70 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
    let mut full = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
    painter
        .renderer
        .draw_centered(&mut full, title, 640.0, 360.0, &style, 1.0, 1.0);
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let title = "标题"; // 2 字：chars_per_sec=1.0，frame<30 时 visible 恒为 0
    let mut bright = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut dim = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
    assert!(
        sum_bright > sum_dim * 2,
        "光标全亮帧的累计 darkness 应显著大于全暗帧：bright={sum_bright} dim={sum_dim}"
    );
    assert!(
        sum_dim > 0,
        "全暗帧（blink≈0.133）光标仍应残留极淡的墨迹，不应完全消失"
    );

    // 打完（local_frame>=60）后不应有光标：与整串标题单独渲染逐字节一致。
    // 同样必须先填白底（原因见上一条测试的注释）。
    let title2 = "六个字标题呀";
    let mut done = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut done, 60, title2);
    let mut full = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
    painter
        .renderer
        .draw_centered(&mut full, title2, 640.0, 360.0, &style, 1.0, 1.0);
    assert_eq!(
        done.data(),
        full.data(),
        "打完后应与整串标题渲染结果逐字节一致（无光标残留）"
    );
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let title = "标题文字标题文字标题"; // 10 字，chars_per_sec=5.0，覆盖多个 visible 台阶
    for f in [0u32, 5, 10, 15, 29, 45, 59, 60] {
        let mut actual = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
        painter.draw_intro(&mut actual, f, title);

        let mut expected = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
        expected.fill(Color::from_rgba8(255, 255, 255, 255));

        let title_chars: Vec<char> = title.chars().collect();
        let total = title_chars.len();
        let chars_per_sec = total as f64 / INTRO_TYPEWRITER_SECONDS;
        let visible = ((f as f64 * chars_per_sec / FPS).floor() as usize).min(total);
        let display_text: String = title_chars[..visible].iter().collect();
        let style = TextStyle {
            size_px: painter.m.intro_title_font_size,
            color: TITLE_COLOR_BLACK,
            stroke: None,
            letter_spacing_px: 0.0,
            max_width_px: painter.m.intro_title_max_width,
            line_height: DEFAULT_LINE_HEIGHT,
            bold: true,
        };
        let intro_center_x = painter.m.intro_title_center_x;
        let intro_center_y = painter.m.intro_title_center_y;
        let fade_opacity = interpolate(f as f64, INTRO_FADE_OUT_RANGE, [1.0, 0.0]) as f32;
        if fade_opacity > 0.0 {
            painter.renderer.draw_centered(
                &mut expected,
                &display_text,
                intro_center_x,
                intro_center_y,
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
                            (intro_center_x + w / 2.0, intro_center_y + top_rel / 2.0)
                        })
                        .unwrap_or((intro_center_x, intro_center_y));
                    let (cursor_w, _) = painter.renderer.measure(INTRO_CURSOR_TEXT, &style);
                    let cursor_center_x =
                        right_edge_x + painter.m.intro_cursor_gap + cursor_w / 2.0;
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
    let mut with_cursor = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut with_cursor, local_frame, title);

    let title_chars: Vec<char> = title.chars().collect();
    let total = title_chars.len();
    let chars_per_sec = total as f64 / INTRO_TYPEWRITER_SECONDS;
    let visible = ((local_frame as f64 * chars_per_sec / FPS).floor() as usize).min(total);
    let display_text: String = title_chars[..visible].iter().collect();
    let style = TextStyle {
        size_px: painter.m.intro_title_font_size,
        color: TITLE_COLOR_BLACK,
        stroke: None,
        letter_spacing_px: 0.0,
        max_width_px: painter.m.intro_title_max_width,
        line_height: DEFAULT_LINE_HEIGHT,
        bold: true,
    };
    let intro_center_y = painter.m.intro_title_center_y;
    let fade_opacity = interpolate(local_frame as f64, INTRO_FADE_OUT_RANGE, [1.0, 0.0]) as f32;

    let mut text_only = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    text_only.fill(Color::from_rgba8(255, 255, 255, 255));
    if fade_opacity > 0.0 {
        painter.renderer.draw_centered(
            &mut text_only,
            &display_text,
            painter.m.intro_title_center_x,
            intro_center_y,
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
            let center_y = intro_center_y + top_rel / 2.0;
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
    let mut with_cursor = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut with_cursor, local_frame, title);

    let title_chars: Vec<char> = title.chars().collect();
    let total = title_chars.len();
    let chars_per_sec = total as f64 / INTRO_TYPEWRITER_SECONDS;
    let visible = ((local_frame as f64 * chars_per_sec / FPS).floor() as usize).min(total);
    let display_text: String = title_chars[..visible].iter().collect();
    let style = TextStyle {
        size_px: painter.m.intro_title_font_size,
        color: TITLE_COLOR_BLACK,
        stroke: None,
        letter_spacing_px: 0.0,
        max_width_px: painter.m.intro_title_max_width,
        line_height: DEFAULT_LINE_HEIGHT,
        bold: true,
    };
    let intro_center_x = painter.m.intro_title_center_x;
    let intro_center_y = painter.m.intro_title_center_y;
    let canvas_h = painter.m.canvas.h_f32();
    let fade_opacity = interpolate(local_frame as f64, INTRO_FADE_OUT_RANGE, [1.0, 0.0]) as f32;

    // 扫描窗口：`measure()` 给出的文本块整体高度，上下各留 40px 安全
    // 边距（光标即便因为某种 bug 跑到相邻行，40px 也足够覆盖一整行
    // 70px 字号 * 1.2 行高 = 84px 的量级）。
    let (_, total_h) = painter.renderer.measure(&display_text, &style);
    const BAND_MARGIN_PX: f32 = 40.0;
    let band_y0 = (intro_center_y - total_h / 2.0 - BAND_MARGIN_PX)
        .max(0.0)
        .floor() as u32;
    let band_y1 = ((intro_center_y + total_h / 2.0 + BAND_MARGIN_PX)
        .min(canvas_h)
        .ceil() as u32)
        .max(band_y0 + 1);

    let mut text_only = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    text_only.fill(Color::from_rgba8(255, 255, 255, 255));
    if fade_opacity > 0.0 {
        painter.renderer.draw_centered(
            &mut text_only,
            &display_text,
            intro_center_x,
            intro_center_y,
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
            let center_y = intro_center_y + top_rel / 2.0;
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
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
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let title = "标题文字"; // 4 字：chars_per_sec=2.0，每 15 帧显示一个字符
    let mut bright = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut dim = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
    assert!(
        diff_count > 0,
        "光标不同透明度应产生像素差异（否则光标根本没画出来）"
    );
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

    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let title = "淡出端点测试";
    let mut f89 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f104 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_intro(&mut f89, 89, title);
    painter.draw_intro(&mut f104, 104, title);
    assert!(
        ink_bbox_in_y_range(&f104, 0, 720).is_none(),
        "frame 104 应完全无墨迹（fade=0）"
    );
    assert!(
        ink_bbox_in_y_range(&f89, 0, 720).is_some(),
        "frame 89 应满不透明，有墨迹"
    );
}

// ------------------------------------------------------------------
// Task 7：Outro。以下 5 条测试是 brief Step 1 原样照抄，一字未改。
// ------------------------------------------------------------------

#[test]
fn outro_paints_an_opaque_white_background() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_outro(&mut p, 0);
    let c = p.pixel(0, 0).unwrap();
    assert_eq!(c.alpha(), 255);
    assert!(c.red() > 240 && c.green() > 240 && c.blue() > 240);
}

#[test]
fn outro_logo_grows_during_the_first_08_seconds() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut f0 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f24 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_outro(&mut f0, 0);
    painter.draw_outro(&mut f24, 24);
    assert_ne!(f0.data(), f24.data(), "logo 应从 0.2 倍放大到 1.0 倍");
}

#[test]
fn outro_title_fades_in_after_the_logo() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut f24 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f39 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_outro(&mut f24, 24);
    painter.draw_outro(&mut f39, 39);
    assert_ne!(f24.data(), f39.data(), "标题应在第 24~39 帧淡入");
}

#[test]
fn outro_fades_out_at_the_end() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let ink = |p: &Pixmap| {
        (0..p.height())
            .flat_map(|y| (0..p.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let c = p.pixel(x, y).unwrap();
                c.red() < 200
            })
            .count()
    };
    let mut f100 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f119 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_outro(&mut f100, 100);
    painter.draw_outro(&mut f119, 119);
    assert!(ink(&f119) < ink(&f100), "第 119 帧应比第 100 帧淡");
}

// ------------------------------------------------------------------
// Task 7 追加的鉴别性断言（协调者要求）：brief 那 5 条只能测出很粗的形状
// （3 条是 `assert_ne!`，改错任何一个常数它们都照样通过）。以下每条都
// 对着协调者点名的变异逐一验证过（记录见报告"变异实验"一节）：
// logo 尺寸 216→180、标题字号 70→50、标题下移 40→0、淡出区间
// [105,119]→[90,119]、圆环半径系数 0.3→0.5、logo 起始 scale 0.2→0.5、
// 标题淡入区间 [24,39]→[24,60]。
//
// **同心圆环在白底上真的不可见**（协调者裁定 2）：白色实心圆合成到白色
// 背景上，`SourceOver` 混合下 `out = src*a + dst*(1-a)`，当 `src==dst==255`
// 时无论 `a` 是多少结果恒为 255——这意味着圆环的 `scale`/`opacity`/
// 绘制顺序（倒序 vs 正序）对最终像素**完全没有可观测影响**，不是这里
// 哪条像素断言能测出来的（与 Task 6 `intro_has_no_watermark` 文档记录的
// 白叠白盲区是同一类现象）。圆环半径系数（`720*0.3`）与钳制上限改用
// 直接的常量/纯函数断言（`outro_ring_radius_step_is_216px`、
// `outro_ring_scale_is_finite_and_clamps_to_100_from_frame_45`），不依赖
// 像素。
// ------------------------------------------------------------------

/// **N: logo 尺寸 216→180**。`Painter::new()` 只应缩一次 216px logo，
/// 直接检查缓存的 `Pixmap` 尺寸——比像素扫描更精确，也不受 logo 图案
/// 本身留白/描边的影响。
#[test]
fn outro_logo_pixmap_is_scaled_to_216px() {
    let painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    assert_eq!(painter.logo_216.width(), 216, "outro logo 应缩放到 216px");
    assert_eq!(painter.logo_216.height(), 216, "outro logo 应缩放到 216px");
}

/// **N: 圆环半径系数 0.3→0.5**。半径公式 `720*0.3*i` 与 logo 尺寸公式
/// `min(1280,720)*0.3` 只是数值恰好相同（都是 216），语义不同（见
/// `outro_ring_radius_step` 文档），不应共用同一个常量、也不能靠
/// 白叠白的圆环像素来验证系数——直接断言常量字面值。
#[test]
fn outro_ring_radius_step_is_216px() {
    let m = Metrics::for_canvas(Canvas::BASE);
    assert!(
        (m.outro_ring_radius_step - 216.0).abs() < 1e-4,
        "圆环半径步长应精确为 720*0.3=216，实得 {}",
        m.outro_ring_radius_step
    );
}

/// **N: 淡出区间 [105,119]→[90,119]（钳制上限被动过）**。`outro_ring_scale`
/// 是抽出来的纯函数（`draw_outro` 内部就调它），直接断言：0..200 帧全程
/// 有限（覆盖 brief 那条测试的 0..120 之外更宽的范围）、`local_frame>=45`
/// 时 `out_progress` 精确钳在 0.99、scale 精确钳在 100.0（不是随便什么
/// 大数——钳制上限本身被改动也会被这条精确值断言抓到，而不只是「有限」
/// 这种弱判据）。
#[test]
fn outro_ring_scale_is_finite_and_clamps_to_100_from_frame_45() {
    for f in 0..200 {
        let s = outro_ring_scale(f as f64);
        assert!(s.is_finite(), "frame {f} 得到非有限 scale {s}");
    }
    for f in [45, 46, 60, 119, 199] {
        let s = outro_ring_scale(f as f64);
        assert!(
            (s - 100.0).abs() < 1e-6,
            "frame {f}（out_progress 已到达终值 1.0）scale 应精确钳在 100.0，实得 {s}"
        );
    }
    // 钳制发生之前，scale 应严格单调递增（弹簧本身单调，钳制只影响终值）。
    let s30 = outro_ring_scale(30.0); // spring 尚未到达 45（delay 30+duration 15）
    let s44 = outro_ring_scale(44.0);
    assert!(
        s30 < s44,
        "钳制生效前 scale 应随帧数单调递增：{s30} vs {s44}"
    );
    assert!(
        s44 < 100.0,
        "frame 44（out_progress 尚未到达 1.0）scale 不应已经钳到 100.0"
    );
}

/// **N: logo 起始 scale 0.2→0.5**。frame 0 的 logo 直径应约为
/// `216*0.2≈43px`；frame 24（缩放动画完成）应接近满尺寸 216px。用同心圆
/// logo 的墨迹纵向跨度（等价于直径）直接量，不依赖任何字体测量。
#[test]
fn outro_logo_diameter_grows_from_about_43px_to_full_size() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut f0 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f24 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_outro(&mut f0, 0);
    painter.draw_outro(&mut f24, 24);

    // 只扫上半屏（0..500），避开 y≈576 的水印，logo/标题纵向组是该范围内
    // 唯一的墨迹来源。
    let (_, y0, _, y1) = ink_bbox_in_y_range(&f0, 0, 500).expect("frame 0 应有 logo 墨迹");
    let diameter0 = (y1 - y0 + 1) as f32;
    assert!(
        (20.0..70.0).contains(&diameter0),
        "frame 0（scale=0.2）logo 直径应约 43px（±较宽容差覆盖圆形留白），实得 {diameter0}"
    );

    let (_, y0, _, y1) = ink_bbox_in_y_range(&f24, 0, 500).expect("frame 24 应有 logo 墨迹");
    let diameter24 = (y1 - y0 + 1) as f32;
    assert!(
        (195.0..220.0).contains(&diameter24),
        "frame 24（scale=1.0）logo 直径应接近满尺寸 216px，实得 {diameter24}"
    );
    assert!(
        diameter24 > diameter0 * 3.0,
        "frame 24 直径应远大于 frame 0：{diameter24} vs {diameter0}"
    );
}

/// **N: 标题字号 70→50**。用与 Task 6 `cover_title_font_size_matches_100px`
/// 同样的手法：拿一个字号已知且与标题文字**完全相同**（都是品牌名）
/// 的参照——Cover 上排文字，38px——比较两者的墨高比值，预期 ≈70/38。
/// 用同一段文字当参照，字形本身的度量特征完全一致，比值只随字号变化，
/// 排除了字形差异带来的噪声。
#[test]
fn outro_title_font_size_matches_70px() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();

    let mut cover_p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_cover(&mut cover_p, "任意标题");
    let (row_h_38px, _) = cover_row_and_title_ink_heights(&mut painter, "任意标题", &cover_p);

    let mut outro_p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_outro(&mut outro_p, 60); // 标题淡入已完成、尚未开始整体淡出
    // 窗口 [420,555]：logo（≈y192..403）与水印（≈y560..591，`cover`
    // 预设垂直中心 576）之间的区间，宽松覆盖标题实际墨迹带（实测
    // ≈455..521），不掺入相邻元素。
    let (_, ty0, _, ty1) = ink_bbox_in_y_range(&outro_p, 420, 555).expect("Outro 标题应有墨迹");
    let title_h_70px = (ty1 - ty0 + 1) as f32;

    let ratio = title_h_70px / row_h_38px;
    let expected = 70.0 / 38.0;
    assert!(
        (ratio - expected).abs() / expected <= 0.12,
        "Outro 标题/Cover 上排墨高比应约为 {expected:.3}±12%，实得 {ratio:.3}\
             （outro_title_h={title_h_70px} cover_row_h={row_h_38px}）"
    );
}

/// **N: 标题下移 40→0**。logo 与标题在纵向组里的墨迹纵向间隙应能反映
/// `Metrics::outro_title_gap`（BASE 上 40px 的布局盒间距）——用字面量而不是常量算期望
/// 值（Task 6 `cover_top_row_left_edge_is_176` 记录过的教训：用同一个
/// 被改动的常量算期望值等于没测）。logo 是近乎顶满 216 盒子的圆形
/// （见报告实测），标题墨迹顶部因为字体上伸空间会比布局盒顶低几像素——
/// 综合下来实测像素间隙比 40px 布局间距略宽，给一个覆盖这个偏移量、
/// 但仍能被「间距归零」清晰打破的区间。
#[test]
fn outro_title_sits_a_visible_gap_below_the_logo() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_outro(&mut p, 60); // 稳态：logo 满尺寸、标题淡入完成

    // logo 与标题分别落在上半屏（0..500，避开水印）里的两段独立墨迹
    // 纵向游程；用逐行扫描找出这两段游程的边界，不预设具体像素窗口。
    let row_has_ink =
        |y: u32| (0..p.width()).any(|x| p.pixel(x, y).map(|c| darkness(c) > 0).unwrap_or(false));
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let mut cur: Option<u32> = None;
    for y in 0..500u32 {
        if row_has_ink(y) {
            cur.get_or_insert(y);
        } else if let Some(s) = cur.take() {
            runs.push((s, y - 1));
        }
    }
    if let Some(s) = cur {
        runs.push((s, 499));
    }
    // 必须是**恰好** 2 段：若 logo 墨迹将来裂成两段游程，`runs[1]` 就变成
    // logo 自己的下半截，量出的「间隙」是 logo 内部空隙而测试照样通过。
    assert_eq!(
        runs.len(),
        2,
        "上半屏应恰好扫到 logo + 标题两段独立墨迹游程，实得 {runs:?}"
    );
    let logo_run = runs[0];
    let title_run = runs[1];
    let gap = title_run.0 as i32 - logo_run.1 as i32;
    assert!(
        (20..90).contains(&gap),
        "logo 底部与标题顶部的墨迹间隙应落在 [20,90]px（40px 布局间距 + 字体量出的余量），实得 {gap}"
    );
}

/// **N: 标题淡入区间 [24,39]→[24,60]；淡出区间 [105,119]→[90,119]**。
/// frame 40 与 frame 104 在正确实现下都处于「已完全稳定、尚未开始整体
/// 淡出」的窗口（logo 缩放在 24 帧完成、标题淡入在 39 帧完成、整体淡出
/// 105 帧才开始），此时圆环虽然还在继续演化但白叠白不产生任何可观测
/// 像素差异（见本节前言）——因此这两帧在正确实现下应当**逐字节相同**。
/// 这条单一断言同时钉住 logo scale 结束帧、标题淡入结束帧、整体淡出
/// 起始帧三个边界常量：任何一个被改动到 (39,105) 这个区间内，都会让
/// 其中一帧落入"仍在动画中"而另一帧"已稳定"，产生字节差异。
#[test]
fn outro_is_pixel_identical_between_settled_frames_40_and_104() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    let mut f40 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    let mut f104 = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
    painter.draw_outro(&mut f40, 40);
    painter.draw_outro(&mut f104, 104);
    // 不用 `assert_eq!` 直接比 921600 字节：失败时会把两个完整数组打进
    // 输出，人根本读不了。先做一次相等判断，不等时用现成的 `diff_bbox`
    // 报出差异区域，一眼就能看出是 logo、标题还是水印在动。
    if f40.data() != f104.data() {
        let bbox = diff_bbox(&f40, &f104);
        panic!(
            "frame 40 与 frame 104 都应处于「已稳定、未开始淡出」窗口，逐字节相同；\
                 实测存在差异，差异像素包围盒 (x0,y0,x1,y1)={bbox:?}"
        );
    }
}

/// **修复轮 1（I1，协调者裁定，豁免 brief 的「原样照抄」要求）**：这条测试
/// 原名 `outro_never_produces_non_finite_geometry`，但它其实测不出那件事——
/// 审查把 `outro_ring_scale` 里的 `.min(OUTRO_RING_OUT_PROGRESS_MAX)` 删掉
/// （真实制造 `outro_ring_scale(45) == inf`）后，这条测试**仍然通过**：
/// `tiny_skia::PathBuilder::from_circle` 遇到非有限半径时静默返回 `None`，
/// `draw_outro` 里 `if let Some(path) = ...` 直接跳过那个圆，既不 panic
/// 也不改变 `pixmap` 的尺寸——`assert_eq!(p.width(), 1280)` 因此是一句
/// 永真断言。真正钉住「钳制没被删」这件事的是下面的
/// `outro_ring_scale_is_finite_and_clamps_to_100_from_frame_45`（直接对
/// `outro_ring_scale` 的返回值断言，不经过这层「非有限半径被静默吞掉」
/// 的 `Option` 屏障）。
///
/// 这条测试改名后测的是它真正能测到的东西：**代表帧渲染不 panic**（换
/// 字体排版参数、除零之外的其它路径出问题时的兜底），用一组覆盖 logo
/// 缩放端点（`0/23/24`）、标题淡入端点（`29/30/44/45/46`——同时也是
/// spring 的 delay/duration 边界）、稳态（`60/104`）、整体淡出起止
/// （`105/118/119`）的代表帧，而不是逐帧扫 0..120（那样跑 27s 却没有
/// 换来任何额外的鉴别力）。写法仿照既有的
/// `typewriter_ink_width_steps_up_and_matches_full_title_when_done` 一带
/// 的代表帧手法（`src/render/draw.rs` 里 `for f in [0u32, 5, 10, 15, 29,
/// 45, 59, 60]`）。
#[test]
fn outro_renders_representative_frames_without_panicking() {
    let mut painter = Painter::new(&test_branding(), Canvas::BASE).unwrap();
    for f in [0u32, 23, 24, 29, 30, 44, 45, 46, 60, 104, 105, 118, 119] {
        let mut p = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
        painter.draw_outro(&mut p, f); // 不 panic 即通过
        assert_eq!(p.width(), 1280);
    }
}

/// **陷阱 1 回归**（规格 §3）：Cover 居中容器的垂直中心必须是 `h / 2`，
/// 不能是写死的 360。
///
/// 720 / 2 恰好等于 360，所以这个错误在 BASE 上**完全看不出来**——判据必须
/// 用一个非 720 高的画布。这里不渲染，直接对 `Metrics::cover_container_center_y`
/// 取值断言：渲染判据在 BASE 上永远绿，起不到作用。
#[test]
fn cover_container_center_y_is_half_the_canvas_height() {
    let mut failures = Vec::new();
    for c in [
        Canvas::BASE,
        Canvas { w: 1920, h: 1080 },
        Canvas { w: 1080, h: 1920 },
    ] {
        let actual = Metrics::for_canvas(c).cover_container_center_y;
        let expected = c.h_f32() / 2.0;
        if actual != expected {
            failures.push(format!(
                "{}x{}: Cover 容器应垂直居中于画布；写死 360 时会偏上（期望 {expected}，实得 {actual}）",
                c.w, c.h
            ));
        }
    }
    assert!(failures.is_empty(), "{:?}", failures);
}

/// **陷阱 2 回归**（规格 §3）：Outro 的 logo 与圆环半径步长必须按**宽度**
/// 推导，不能按高度。
///
/// 判据是「占画布宽度的比例三档相等」。**不能只用 16:9 的画布验**：在任意
/// 16:9 上 `H×0.3` 恰等于 `W×0.16875`，两种写法同值，测试会假绿。9:16 那
/// 一档才是真正的判据。
#[test]
fn outro_logo_and_ring_scale_with_width_not_height() {
    let base = Metrics::for_canvas(Canvas::BASE);
    let base_logo_ratio = base.outro_logo_size as f32 / Canvas::BASE.w_f32();
    let base_ring_ratio = base.outro_ring_radius_step / Canvas::BASE.w_f32();
    assert!(
        (base_logo_ratio - 0.16875).abs() < 1e-6,
        "BASE 上 logo 应为画布宽的 16.875%（216/1280），实得 {base_logo_ratio}"
    );

    let mut failures = Vec::new();
    for c in [Canvas { w: 1920, h: 1080 }, Canvas { w: 1080, h: 1920 }] {
        let m = Metrics::for_canvas(c);
        let logo_ratio = m.outro_logo_size as f32 / c.w_f32();
        let ring_ratio = m.outro_ring_radius_step / c.w_f32();

        // 容差 1e-3 是必要的：logo_size 通过 px() 闭包的 round() 计算成整数像素，
        // 舍入差异会在除以浮点宽度后产生可观的误差；ring_radius_step 是原始的
        // `216.0 * s`，本身没有取整，但仍与 logo_size 共用同一条容差判断。
        if (logo_ratio - base_logo_ratio).abs() >= 1e-3 {
            failures.push(format!(
                "{}x{}: logo 占宽比应与 BASE 一致（{base_logo_ratio}），实得 {logo_ratio}；\
                 按 h*0.3 推导时 9:16 会得到 0.533",
                c.w, c.h
            ));
        }
        if (ring_ratio - base_ring_ratio).abs() >= 1e-3 {
            failures.push(format!(
                "{}x{}: 圆环半径步长占宽比应与 BASE 一致，实得 {ring_ratio}",
                c.w, c.h
            ));
        }
    }
    assert!(failures.is_empty(), "{:?}", failures);
}

/// **陷阱 3 回归**（规格 §3）：「不换行哨兵」必须显著大于画布宽度。
///
/// 重构前是写死的 `2000.0`：1280 宽下是画布的 1.56 倍（安全），1920 宽下
/// 只比画布宽 4.2%——一个长品牌名配 1.5 倍放大的字号真的可能触发换行。
/// 语义是「远大于画布宽」，就该随画布走。
#[test]
fn no_wrap_sentinel_stays_far_wider_than_the_canvas() {
    let mut failures = Vec::new();
    for c in [
        Canvas::BASE,
        Canvas { w: 1920, h: 1080 },
        Canvas { w: 1080, h: 1920 },
    ] {
        let m = Metrics::for_canvas(c);
        if m.no_wrap_width < c.w_f32() * 1.5 {
            failures.push(format!(
                "{}x{}: 不换行哨兵应至少为画布宽的 1.5 倍，实得 {}，期望 ≥ {}",
                c.w,
                c.h,
                m.no_wrap_width,
                c.w_f32() * 1.5
            ));
        }
    }
    assert!(failures.is_empty(), "不换行哨兵回归失败：{:?}", failures);
}

/// Cover 水印的垂直中心是画布高度的 80%，不是写死的 576。
///
/// `576 = 0.8 × 720`，在 BASE 上两种写法同值。
#[test]
fn cover_watermark_center_y_is_eighty_percent_of_height() {
    assert_eq!(
        Metrics::for_canvas(Canvas::BASE).cover_watermark_center_y,
        576.0,
        "BASE 上应与重构前的写死值一致"
    );
    let mut failures = Vec::new();
    for c in [Canvas { w: 1920, h: 1080 }, Canvas { w: 1080, h: 1920 }] {
        let m = Metrics::for_canvas(c);
        let expected = c.h_f32() * 0.8;
        if m.cover_watermark_center_y != expected {
            failures.push(format!(
                "{}x{}: Cover 水印中心应在 0.8h = {}，实得 {}",
                c.w, c.h, expected, m.cover_watermark_center_y
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "Cover 水印垂直中心回归失败：{:?}",
        failures
    );
}

/// `Painter` 按传入的画布建立版式，而不是永远用 BASE。
///
/// 判据：换一个宽度不同的画布，同一段文字的墨迹宽度必须按比例变化。若
/// `Painter` 内部还在读 `Canvas::BASE`，两次渲染会完全一样。
#[test]
fn painter_lays_out_according_to_the_canvas_it_was_given() {
    let b = test_branding();
    let base = Canvas::BASE;
    let wide = Canvas { w: 2560, h: 1440 };

    let mut p_base = Painter::new(&b, base).unwrap();
    let mut p_wide = Painter::new(&b, wide).unwrap();

    let mut a = Pixmap::new(base.w, base.h).unwrap();
    let mut c = Pixmap::new(wide.w, wide.h).unwrap();
    p_base.draw_intro(&mut a, 60, "标题");
    p_wide.draw_intro(&mut c, 60, "标题");

    let (ax0, _, ax1, _) = ink_bbox(&a).expect("BASE 应有墨迹");
    let (cx0, _, cx1, _) = ink_bbox(&c).expect("宽画布应有墨迹");
    let base_ratio = (ax1 - ax0) as f32 / base.w_f32();
    let wide_ratio = (cx1 - cx0) as f32 / wide.w_f32();
    assert!(
        (base_ratio - wide_ratio).abs() < 0.02,
        "标题墨迹占画布宽的比例应与画布无关；BASE {base_ratio} vs 宽画布 {wide_ratio}"
    );
}

/// **漏网陷阱的兜底网**：在一组**刻意不是 16:9、也不是 BASE** 的尺寸上，
/// 四段都要能画完、不 panic、每段都产生墨迹、且墨迹不贴到画布最外圈像素。
///
/// 三个已知陷阱是人工清点出来的（规格 §3），可能还有第四个。16:9 下
/// 「按高推导」与「按宽推导」恰好同值，所以只用 16:9 与 9:16 验证是不够的
/// ——这里特意取 4:3、1:1、21:9 三种比例把这类巧合拆开。首次跑这条测试时
/// 三档全绿，**没有撞出第四个陷阱**——这不代表以后也不会，留着这条测试
/// 就是为了以后万一改坏了能接住。
///
/// **为什么判据是"贴到最外圈"而不是"越出画布"**：tiny-skia 把超出画布范围
/// 的绘制**直接裁剪**，不会报错也不会在扫描到的 `Pixmap` 里留下任何越界的
/// 像素——`ink_bbox`/`non_transparent_bbox` 只扫描 `0..width()`/`0..height()`，
/// 找到的包围盒**结构上不可能**越出画布，一个真正判"越界"的条件永远不会
/// 为真，等于什么都没测（复查记录：修复轮 1 抓到的正是这个缺陷，条件是死
/// 代码）。裁剪之后唯一能观察到的痕迹是元素被推得越远、贴着画布边缘的墨迹
/// 就越多，所以这里改成检查墨迹包围盒是否碰到最外圈像素（`x0==0` /
/// `y0==0` / `x1+1>=c.w` / `y1+1>=c.h`）——这是"被推出画面后又被裁剪掉一
/// 部分"的可观察代理指标，不是真的量出了画布外的东西。
///
/// **这个判据对本项目现有版式是安全的**：正文水印距左/距下各 `40 × scale`，
/// Cover 居中容器是宽度 80% 再居中，标题/字幕的排版宽度上限也封在 80% 以内
/// ——所有元素都天然内缩，正确的版式在任何比例下都不会碰到最外圈。
///
/// collect-then-assert：四段各自的失败信息都收进 `failures`，循环内不
/// `assert!`，一次跑完能看到全部违规的档位与段，而不是撞到第一个就停。
#[test]
fn all_segments_render_within_bounds_on_non_sixteen_nine_canvases() {
    let b = branding_with(Some(TEST_CONTENT_WM), Some(TEST_COVER_WM));
    let mut failures = Vec::new();

    for c in [
        Canvas { w: 1024, h: 768 },  // 4:3
        Canvas { w: 900, h: 900 },   // 1:1
        Canvas { w: 2560, h: 1080 }, // 21:9
    ] {
        let mut p = Painter::new(&b, c).unwrap();

        let mut pm = Pixmap::new(c.w, c.h).unwrap();
        p.draw_cover(&mut pm, "一个足够长的测试标题用来触发换行");
        match ink_bbox(&pm) {
            None => failures.push(format!("{}x{} Cover：应有墨迹，实际全白", c.w, c.h)),
            Some((x0, y0, x1, y1)) => {
                if x0 == 0 || y0 == 0 || x1 + 1 >= c.w || y1 + 1 >= c.h {
                    failures.push(format!(
                        "{}x{} Cover：墨迹贴到画布最外圈，疑似被推出画面后裁剪，bbox=({x0},{y0},{x1},{y1})",
                        c.w, c.h
                    ));
                }
            }
        }

        // 三帧覆盖打字机进行中（60）与稳态尾声（104）；104 并非全淡出终点
        // （那是 Outro 才有的设计），所以三帧里应当至少有一帧有墨迹。
        let mut intro_has_ink = false;
        for f in [0u32, 60, 104] {
            let mut pm = Pixmap::new(c.w, c.h).unwrap();
            p.draw_intro(&mut pm, f, "一个足够长的测试标题用来触发换行");
            if let Some((x0, y0, x1, y1)) = ink_bbox(&pm) {
                intro_has_ink = true;
                if x0 == 0 || y0 == 0 || x1 + 1 >= c.w || y1 + 1 >= c.h {
                    failures.push(format!(
                        "{}x{} Intro frame {f}：墨迹贴到画布最外圈，疑似被推出画面后裁剪，bbox=({x0},{y0},{x1},{y1})",
                        c.w, c.h
                    ));
                }
            }
        }
        if !intro_has_ink {
            failures.push(format!("{}x{} Intro：三个代表帧均无墨迹", c.w, c.h));
        }

        // Content 段背景透明（不是 Cover/Intro/Outro 的不透明白底），
        // 判据要用 alpha 而不是颜色，否则透明底也会被 `ink_bbox`
        // 误判成"整幅都是墨迹"（premultiplied 透明像素的 rgb 恰好是 0）。
        let mut pm = Pixmap::new(c.w, c.h).unwrap();
        p.draw_content(&mut pm, 30, &caps());
        match non_transparent_bbox(&pm) {
            None => failures.push(format!("{}x{} Content：应有墨迹，实际全透明", c.w, c.h)),
            Some((x0, y0, x1, y1)) => {
                if x0 == 0 || y0 == 0 || x1 + 1 >= c.w || y1 + 1 >= c.h {
                    failures.push(format!(
                        "{}x{} Content：墨迹贴到画布最外圈，疑似被推出画面后裁剪，bbox=({x0},{y0},{x1},{y1})",
                        c.w, c.h
                    ));
                }
            }
        }

        // f=119 是整体淡出的终点，设计上就是纯白无墨迹（见
        // `outro_renders_representative_frames_without_panicking` 附近的
        // `OUTRO_FADE_OUT_FRAMES = [105,119]`），所以不能要求每一帧都有墨迹，
        // 只要求三帧里至少一帧有。
        let mut outro_has_ink = false;
        for f in [0u32, 60, 119] {
            let mut pm = Pixmap::new(c.w, c.h).unwrap();
            p.draw_outro(&mut pm, f);
            if let Some((x0, y0, x1, y1)) = ink_bbox(&pm) {
                outro_has_ink = true;
                if x0 == 0 || y0 == 0 || x1 + 1 >= c.w || y1 + 1 >= c.h {
                    failures.push(format!(
                        "{}x{} Outro frame {f}：墨迹贴到画布最外圈，疑似被推出画面后裁剪，bbox=({x0},{y0},{x1},{y1})",
                        c.w, c.h
                    ));
                }
            }
        }
        if !outro_has_ink {
            failures.push(format!("{}x{} Outro：三个代表帧均无墨迹", c.w, c.h));
        }
    }

    assert!(
        failures.is_empty(),
        "非 16:9 画布下发现贴边/无墨迹的段：{failures:#?}"
    );
}

/// 白底上的墨迹包围盒（任一通道显著低于 255 即算墨迹）。
fn ink_bbox(p: &Pixmap) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..p.height() {
        for x in 0..p.width() {
            let c = p.pixel(x, y).unwrap();
            if c.red() < 240 || c.green() < 240 || c.blue() < 240 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 != u32::MAX).then_some((x0, y0, x1, y1))
}
