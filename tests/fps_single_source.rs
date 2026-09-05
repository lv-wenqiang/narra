//! `draw::FPS` 必须**派生自** `timeline::FPS`，而不只是碰巧相等。
//!
//! **为什么要比对源码文本**：`src/render/draw.rs` 里 `FPS` 现在写作
//! `crate::render::timeline::FPS as f64`，而既有的那条运行期断言
//! （`ffmpeg::tests::draw_fps_derives_from_the_timeline_single_source_of_truth`）
//! 只能检查两者**取值**相等——把它改回字面量 `30.0` 之后那条断言照样通过，
//! 因为 `30.0 == 30 as f64`。它守住了「取值分歧」这个有害状态，却守不住
//! 「推导被改回硬编码」这一步，而后者正是双真相源重新长出来的方式。
//!
//! Rust 没有办法在类型层表达「这个常量必须派生自那个常量」，所以只能比对
//! 源码文本。同一手法在 `tests/justfile_defaults.rs` 用过（justfile 是
//! shell，读不到 `config::*`，只能把默认值抄一份、再用测试把两边钉住）。
//!
//! **只做子串匹配，不解析 Rust**：这里要防的是「有人把推导换成字面量」，
//! 不是「语法正确」——后者由编译器负责。

const DRAW_RS: &str = include_str!("../src/render/draw.rs");

/// `draw.rs` 的 `FPS` 必须以 `timeline::FPS` 的形式写出来。
#[test]
fn draw_fps_is_written_as_a_derivation_not_a_literal() {
    let want = "const FPS: f64 = crate::render::timeline::FPS as f64;";
    assert!(
        DRAW_RS.contains(want),
        "src/render/draw.rs 里的 FPS 必须写成 `{want}`——\n\
         写成字面量会让它与 timeline::FPS 重新变成两个真相源：\n\
         改 timeline::FPS 之后 layout()/ffmpeg -r/段落起点都会跟着走，\n\
         而 draw.rs 里每个动画窗口仍按旧帧率算，动画变速且测试全绿。"
    );
}

/// `draw.rs` 里不得再出现把 30 写死成帧率的形态。
///
/// 与上一条互补：上一条保证「正确的写法在场」，这一条保证「错误的写法不在场」
/// ——两者都需要，因为有人可能在保留正确定义的同时，另加一个写死的副本。
#[test]
fn draw_rs_has_no_hardcoded_frame_rate() {
    for bad in ["const FPS: f64 = 30.0;", "const FPS: f64 = 30f64;"] {
        assert!(
            !DRAW_RS.contains(bad),
            "src/render/draw.rs 里不该出现写死帧率的 `{bad}`"
        );
    }
}
