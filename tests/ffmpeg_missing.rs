//! `run_render` 真的调用了 `assert_available()` 这件事，只能在 ffmpeg
//! 在 PATH 上确实找不到时才能和「删掉那一行」区分开——两种情况下，如果
//! ffmpeg 本来就在，行为完全一样（见 `src/ffmpeg.rs` 里
//! `run_render_reports_ffmpeg_stderr_verbatim_when_it_exits_nonzero` 与
//! `run_render_does_not_panic_when_ffmpeg_exits_early` 两条测试）。
//!
//! 要验证这一点就得把进程级 `PATH` 换成不含 ffmpeg 的目录，这个操作是
//! 进程全局的。单独放进这个文件（独立编译成自己的测试二进制、独立进程），
//! 而不是塞进 `src/ffmpeg.rs` 的 `mod tests`（和其它 24+ 条测试共享同一个
//! 进程、同一份 `PATH`），是为了让这个变量的读写和其它测试**彻底**没有
//! 竞争——不是靠互斥锁把窗口缩小到很难触发，而是压根不存在共享状态。
//! `src/tts/pipeline.rs` 里非 `#[ignore]` 的测试也会经 `PATH` 真实 spawn
//! ffmpeg，如果这条测试留在 lib 单测里，光靠一把只有本文件知道要拿的锁
//! 是保护不到它的（修复轮 1 I5）。

use std::path::Path;

#[test]
fn run_render_returns_assert_available_error_when_ffmpeg_is_missing() {
    // 鉴别性测试：钉住「`run_render` 真的调用了 `assert_available()`」
    // 这件事本身。有 `assert_available()?` 时报的是它那句面向用户的安装
    // 提示；删掉后，`Command::new("ffmpeg").spawn()` 自己失败，报的是
    // 泛泛的「启动 ffmpeg 失败：No such file or directory」——同样是
    // `Err`，但说的不是同一件事。
    //
    // 用系统临时目录本身（不含任何 "ffmpeg" 可执行文件）顶替 PATH，让
    // `Command::new("ffmpeg")` 无论在哪一步被调用都找不到它。这个进程
    // 只跑这一条测试，改 PATH 不会影响任何其它测试。
    let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n短。\n";
    let mut fs = panda::render::frame::FrameSource::new(vtt, "标题".into()).unwrap();
    let bad = Path::new("/nonexistent-xyz.mp4");
    let total_frames = fs.total_frames();
    let audio_secs = fs.audio_secs();
    let content_frames = fs.content_frames();

    // SAFETY（就多线程而言）：这个测试二进制里只有这一条 #[test]，没有
    // 其它线程会并发读写 PATH。
    unsafe {
        std::env::set_var("PATH", std::env::temp_dir());
    }
    let result = panda::ffmpeg::run_render(
        &mut fs,
        &panda::ffmpeg::RenderInputs {
            bg: bad,
            tts_audio: bad,
            bgm: bad,
            typewriter: bad,
            intro: bad,
            out: Path::new("/tmp/panda_missing_ffmpeg_test.mp4"),
            total_frames,
            audio_secs,
            content_frames,
        },
    );

    let err = result.expect_err("PATH 上没有 ffmpeg，run_render 应报错");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("请安装 ffmpeg"),
        "应是 assert_available() 那句面向用户的安装提示，说明它真的被调用了；\
         如果这一行被删掉，这里会看到的是 spawn() 自己泛泛的「启动 ffmpeg 失败」：{msg}"
    );
    std::fs::remove_file("/tmp/panda_missing_ffmpeg_test.mp4").ok();
}
