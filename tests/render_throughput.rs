//! 渲染器单独跑的吞吐探针（`#[ignore]`，手动跑）。
//!
//! **为什么需要它**：`narra render` 的挂钟耗时是「tiny-skia 逐帧渲染 + 反预乘 +
//! 写 stdin」与「ffmpeg 解码背景 + overlay + x264 编码」两段**并行**跑出来的，
//! 端到端数字（`docs/ffmpeg-pipeline.md` §10：LANDSCAPE 约 35fps，实时余量只有
//! 1.17×）本身分不出这两段谁是瓶颈——而这个问题的答案决定了下一步该往哪使劲：
//! 编码侧吃紧就调 `-preset`/`-crf`，渲染侧吃紧才轮到动
//! `docs/follow-ups.md` 帧渲染节「值得做」第 2 条那个 `draw_centered` 的 3× 乘数。
//!
//! 这条探针把 ffmpeg 整个摘掉，只留渲染侧：帧流直接写 `/dev/null`。得到的 fps
//! 是渲染侧单独的上限。**必须用 `--release` 跑**，debug 构建下 tiny-skia 慢一个
//! 数量级，测出来的比例没有意义：
//!
//! ```bash
//! cargo test --release --test render_throughput -- --ignored --nocapture
//! ```

use narra::config::Branding;
use narra::render::canvas::Canvas;
use narra::render::frame::FrameSource;

/// 与 §10 实测同源的输入：`output/tts/audio.vtt`（A=16.276s，789 帧）。
/// 找不到就退回一段内置 VTT，只是帧数不同，比例仍可读。
fn vtt_text() -> String {
    std::fs::read_to_string("output/tts/audio.vtt").unwrap_or_else(|_| {
        eprintln!("（没有 output/tts/audio.vtt，退回内置 VTT，帧数与 §10 不可比）");
        "WEBVTT\n\n1\n00:00:00.000 --> 00:00:16.276\n吞吐探针。\n".to_string()
    })
}

fn measure(canvas: Canvas, label: &str) {
    let mut fs = FrameSource::new(
        &vtt_text(),
        "吞吐实测".into(),
        &Branding::plain("墨"),
        canvas,
    )
    .unwrap();
    let total = fs.total_frames();

    // 写 /dev/null 而不是 `io::sink()`：前者仍然逐帧过一次 `write` 系统调用，
    // 与生产路径写 ffmpeg stdin 的形状更接近；后者会把这部分开销整个抹掉。
    let mut sink = std::fs::File::create("/dev/null").unwrap();
    let t0 = std::time::Instant::now();
    let written = fs.write_rgba_frames(&mut sink).unwrap();
    let secs = t0.elapsed().as_secs_f64();

    assert_eq!(written, total, "应写满整条时间轴");
    println!(
        "{label} {}x{}：{written} 帧 / {secs:.3}s = {:.2} fps（{:.2}× 实时线）",
        canvas.w,
        canvas.h,
        written as f64 / secs,
        written as f64 / secs / 30.0
    );
}

#[test]
#[ignore = "吞吐探针，需 --release 才有意义；手动跑"]
fn renderer_alone_landscape() {
    measure(Canvas::LANDSCAPE, "渲染侧单独");
}

#[test]
#[ignore = "吞吐探针，需 --release 才有意义；手动跑"]
fn renderer_alone_portrait() {
    measure(Canvas::PORTRAIT, "渲染侧单独");
}

#[test]
#[ignore = "吞吐探针，需 --release 才有意义；手动跑"]
fn renderer_alone_base() {
    measure(Canvas::BASE, "渲染侧单独");
}

/// 逐段的渲染成本（ms/帧）。四段的画法差别很大——Cover/Intro/Outro 是白底加
/// 少量元素，Content 是透明底加字幕入场动画——总吞吐里谁占大头，得分段看。
#[test]
#[ignore = "吞吐探针，需 --release 才有意义；手动跑"]
fn renderer_cost_by_segment_landscape() {
    use narra::render::timeline::{Segment, layout, segment_at};

    let canvas = Canvas::LANDSCAPE;
    let mut fs = FrameSource::new(
        &vtt_text(),
        "吞吐实测".into(),
        &Branding::plain("墨"),
        canvas,
    )
    .unwrap();
    let lay = layout(fs.audio_secs());
    let total = fs.total_frames();

    let mut acc = [(0u32, 0.0f64); 4];
    for f in 0..total {
        let (seg, _) = segment_at(&lay, f).unwrap();
        let t0 = std::time::Instant::now();
        let _ = fs.render(f).unwrap();
        let dt = t0.elapsed().as_secs_f64();
        let i = match seg {
            Segment::Cover => 0,
            Segment::Intro => 1,
            Segment::Content => 2,
            Segment::Outro => 3,
        };
        acc[i].0 += 1;
        acc[i].1 += dt;
    }

    let names = ["Cover", "Intro", "Content", "Outro"];
    let whole: f64 = acc.iter().map(|a| a.1).sum();
    for (i, (n, secs)) in acc.iter().enumerate() {
        println!(
            "{:<8} {n:>4} 帧  {:>7.2} ms/帧  合计 {:>6.2}s（占 {:>4.1}%）",
            names[i],
            secs * 1000.0 / *n as f64,
            secs,
            secs / whole * 100.0
        );
    }
    println!(
        "合计 {total} 帧 {whole:.2}s = {:.2} fps",
        total as f64 / whole
    );
}
