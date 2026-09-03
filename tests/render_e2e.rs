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

    std::fs::remove_dir_all(&tmp).ok();
}
