//! 端到端合成测试。需要 ffmpeg 与真实素材，默认 #[ignore]。
//! 手动运行：cargo test --test render_e2e -- --ignored --nocapture
//!
//! 这是整条 ffmpeg 合成管道唯一的端到端验证：真起 ffmpeg 子进程、真写
//! 帧流、真产出一个 mp4。ffprobe 能核对流类型/编码/时长这些"结构性"
//! 事实，但核不出"反预乘有没有生效"——那只能靠肉眼抽帧看 Content 段的
//! 字幕有没有发暗（详见任务报告里的目视验收记录）。

use std::path::Path;

#[test]
#[ignore = "需要 ffmpeg 与 ../panda-video-ts/public 下的素材，且耗时约 30 秒"]
fn produces_a_playable_mp4_with_video_and_audio_streams() {
    let bg = Path::new("../panda-video-ts/public/video/0.mp4");
    let bgm = Path::new("../panda-video-ts/public/bgm/0.mp3");
    if !bg.exists() || !bgm.exists() {
        eprintln!("跳过：素材不存在");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("panda_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let (intro, typewriter) = panda::assets::write_embedded_audio(&tmp).unwrap();

    // 用打字机音效充当 TTS 音轨——本测试只验证管道通、流齐、时长对，
    // 不验证语音内容。
    let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:03.000\n第一条字幕。\n\n\
               2\n00:00:03.000 --> 00:00:06.000\n第二条字幕，稍微长一点点。\n";
    let mut fs = panda::render::frame::FrameSource::new(vtt, "端到端测试标题".into()).unwrap();
    let out = tmp.join("out.mp4");

    let total = fs.total_frames();
    let audio_secs = fs.audio_secs();
    let content_frames = fs.content_frames();
    panda::ffmpeg::run_render(
        &mut fs,
        &panda::ffmpeg::RenderInputs {
            bg,
            tts_audio: &typewriter,
            bgm,
            typewriter: &typewriter,
            intro: &intro,
            out: &out,
            total_frames: total,
            audio_secs,
            content_frames,
        },
    )
    .unwrap();

    assert!(out.exists(), "成片应存在");
    let meta = std::fs::metadata(&out).unwrap();
    assert!(meta.len() > 50_000, "成片太小，可能是空壳：{} 字节", meta.len());

    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(&out)
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&probe.stdout);
    assert!(info.contains("codec_type=video"), "应有视频流：{info}");
    assert!(info.contains("codec_type=audio"), "应有音频流：{info}");
    assert!(info.contains("codec_name=h264"), "视频应是 h264：{info}");
    assert!(info.contains("codec_name=aac"), "音频应是 aac：{info}");

    // 总帧数 = 240 + ceil((6+2)*30) = 240 + 240 = 480 帧 = 16.0 秒
    let dur: f64 = info
        .lines()
        .find_map(|l| l.strip_prefix("duration="))
        .and_then(|v| v.parse().ok())
        .expect("应有 duration");
    assert!((dur - 16.0).abs() < 0.5, "时长应约 16 秒，实得 {dur}");

    // 修复轮 1 I2：上面这些断言（大小/流类型/编码/时长）对"冻结帧填充"
    // 型的静默坏片完全无感——如果写帧半途出错但错误被吞掉（比如 `run_
    // render` 把 `write_result` 的 `Err` 错当成 `Ok`），`overlay=shortest=0`
    // 配合 `eof_action=repeat` 会用最后写出的那一帧一直填满剩下的时长，
    // 产物依然是一个大小正常、h264/aac 齐全、时长精确到 16.0 秒的"合法"
    // mp4——上面每一条断言都会通过。审查实测过这个具体场景：把帧流截断到
    // 60 帧后静默返回 `Ok`，抽第 200 帧发现是冻结的 Intro 画面，480 帧里
    // 420 帧是同一张静止图，而上面的断言全绿。本任务用同样的手法（临时
    // 包一层只转发前 60 帧字节的 Write，其余静默假装成功）复现过一次，
    // 结果一致：上面所有断言通过，只有下面这条抓住。
    //
    // 这里抽两帧比对像素字节，钉住"后半段确实在变化，不是同一张静止图
    // 复制粘贴"。选 300（Content 段中段）和 470（Outro 段尾部）而不是
    // 简报原来给的 0/300：这两帧都落在时间轴的后半段（Content 120~359、
    // Outro 360~479），一次典型的"写到一半失败被吞掉"无论具体断在哪一帧，
    // 只要断在 300 之前，300 和 470 就会是同一张被复读的冻结帧——比选一
    // 头一尾（0 和 300）更不容易被"恰好断在两个采样点之间"漏过去，因为
    // 断点通常离末尾更远（越早失败越常见，比如某个片段的渲染在处理到
    // 一半就出错）。
    let frame_dir = tmp.join("freeze_check");
    std::fs::create_dir_all(&frame_dir).unwrap();
    let extract = std::process::Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-i"])
        .arg(&out)
        .args(["-vf", r"select='eq(n\,300)+eq(n\,470)'", "-vsync", "0"])
        .arg(frame_dir.join("f_%02d.png"))
        .output()
        .unwrap();
    assert!(
        extract.status.success(),
        "抽帧失败：{}",
        String::from_utf8_lossy(&extract.stderr)
    );
    let frame_300 = std::fs::read(frame_dir.join("f_01.png")).expect("应能读到第 300 帧");
    let frame_470 = std::fs::read(frame_dir.join("f_02.png")).expect("应能读到第 470 帧");
    assert_ne!(
        frame_300, frame_470,
        "第 300 帧（Content 段）与第 470 帧（Outro 段）字节完全相同——很可能是写帧半途失败被静默吞掉，后半段被同一帧冻结填充了"
    );

    std::fs::remove_dir_all(&tmp).ok();
}
