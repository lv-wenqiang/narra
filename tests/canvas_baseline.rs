//! **BASE（1280×720）逐字节基线**——计划 A 的硬门禁。
//!
//! 画布参数化是一次纯重构：78 个常量要逐条改成由 `Canvas` 推导，
//! 任何一处漏改都会让画面静默错位，而不会编译失败。唯一可靠的证明是
//! 「重构前后渲染同一帧，字节完全相同」。
//!
//! 快照存的是 `render()` 出来的**预乘 RGBA 原始字节**，不经 PNG 编码——
//! PNG 有压缩参数与元数据，比对它等于顺带比对编码器版本。
//!
//! 四帧覆盖四段：0=Cover、100=Intro（打字机进行中）、200=Content（字幕
//! 入场动画中）、550=Outro（logo 已入场、整体淡出未开始）。

use panda::config::Branding;
use panda::render::canvas::Canvas;
use panda::render::frame::FrameSource;

const VTT: &str = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:04.000\n第一条字幕。\n\n2\n00:00:04.000 --> 00:00:10.000\n第二条字幕，稍微长一点。\n";
const FRAMES: [u32; 4] = [0, 100, 200, 550];

fn baseline_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/baseline")
}

/// 固定的品牌装备：两处水印都配上，让水印绘制路径也进基线。
fn fixture() -> Branding {
    Branding {
        brand: "基线品牌".into(),
        watermark: Some("正文水印".into()),
        watermark_cover: Some("封面水印 · 副标题".into()),
        watermark_icon: None,
        logo: None,
    }
}

#[test]
fn base_canvas_rendering_is_byte_identical_to_the_baseline() {
    let dir = baseline_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut fs = FrameSource::new(VTT, "基线标题".into(), &fixture(), Canvas::BASE).unwrap();

    let mut regenerated = Vec::new();
    for f in FRAMES {
        let pixmap = fs.render(f).unwrap();
        let got = pixmap.data().to_vec();
        let path = dir.join(format!("frame_{f}.rgba"));

        if !path.exists() {
            std::fs::write(&path, &got).unwrap();
            regenerated.push(f);
            continue;
        }
        let want = std::fs::read(&path).unwrap();
        assert_eq!(
            got.len(),
            want.len(),
            "第 {f} 帧字节数变了：基线 {} → 现在 {}。\
             重构不该改变画布尺寸；若确实要改基线，删掉 tests/baseline/ 重新生成并在提交里说明理由。",
            want.len(),
            got.len()
        );
        let diff = got.iter().zip(want.iter()).filter(|(a, b)| a != b).count();
        assert_eq!(
            diff,
            0,
            "第 {f} 帧有 {diff} 个字节与基线不同（共 {} 字节）。\
             这是计划 A 的硬门禁：BASE 上的渲染必须逐字节不变。",
            want.len()
        );
    }

    assert!(
        regenerated.is_empty(),
        "首次运行已生成基线快照 {regenerated:?}，请 git add tests/baseline/ 后重跑本测试确认门禁生效"
    );
}
