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

/// `draw.rs` 里不得出现**这两种字面写法**的 `FPS` 定义。
///
/// 与上一条互补：上一条保证「正确的写法在场」，这一条保证「错误的写法不在场」
/// ——两者都需要，因为有人可能在保留正确定义的同时，另加一个写死的副本。
///
/// **它挡住什么**：`const FPS: f64 = 30.0;` 与 `const FPS: f64 = 30f64;`
/// 这两种逐字写法——也就是把上一条要求的那行推导直接改回字面量时，最可能
/// 被写出来的两种形态。
///
/// **它挡不住什么**（有意不扩大匹配范围）：`30_f64`、`3.0e1`、多余空格、
/// 改了名字的常量，以及绕开常量、直接把 `30.0` 内联到各个动画窗口算式里。
/// 别把它当成「draw.rs 里没有任何硬编码帧率」的证明：真正的护栏是上一条
/// （推导写法必须在场），这一条只是补一个最常见的复制粘贴形态。扩大匹配
/// 范围（正则、解析 Rust）会把一条已经过变异验证的廉价断言换成一条需要
/// 自己被验证的复杂断言，不划算。
#[test]
fn draw_rs_has_no_hardcoded_frame_rate() {
    for bad in ["const FPS: f64 = 30.0;", "const FPS: f64 = 30f64;"] {
        assert!(
            !DRAW_RS.contains(bad),
            "src/render/draw.rs 里不该出现写死帧率的 `{bad}`"
        );
    }
}
