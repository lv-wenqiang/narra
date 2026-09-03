//! ffmpeg 管道联通性探针（Task 0 关卡，用完即弃）。
//!
//! 目的：确认「rawvideo RGBA 经 stdin 喂进 filter_complex，与 AV1 背景视频
//! overlay 后编码成 mp4」这条路走得通，并测出编码吞吐。
//!
//! 运行：cargo run --release --example pipe_probe

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Instant;

const W: usize = 1280;
const H: usize = 720;
const FPS: usize = 30;
const FRAMES: usize = 90; // 3 秒

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bg = "../panda-video-ts/public/video/0.mp4";
    let out = "/tmp/pipe_probe.mp4";

    let mut child = Command::new("ffmpeg")
        .args([
            "-y",
            "-stream_loop", "-1", "-i", bg,
            "-f", "rawvideo", "-pix_fmt", "rgba",
            "-s", "1280x720", "-r", "30", "-i", "-",
            "-filter_complex",
            "[0:v]scale=1280:720:force_original_aspect_ratio=increase,\
             crop=1280:720,colorchannelmixer=rr=0.8:gg=0.8:bb=0.8[bg];\
             [bg][1:v]overlay=shortest=0[v]",
            "-map", "[v]",
            "-t", "3",
            "-c:v", "libx264", "-crf", "23", "-pix_fmt", "yuv420p",
            out,
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("stdin 已 piped");
    let started = Instant::now();

    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        // 画一个「左半边不透明红、右半边全透明」的图案：
        // 成片里应能看到左半边被红色盖住、右半边露出背景视频。
        // 这同时验证了 overlay 是否真的按 alpha 混合。
        let mut frame = vec![0u8; W * H * 4];
        for y in 0..H {
            for x in 0..W {
                let i = (y * W + x) * 4;
                if x < W / 2 {
                    frame[i] = 255;     // R
                    frame[i + 3] = 255; // A
                }
            }
        }
        for _ in 0..FRAMES {
            stdin.write_all(&frame)?;
        }
        stdin.flush()
    });

    let status = child.wait()?;
    let elapsed = started.elapsed();
    let write_result = writer.join().expect("写帧线程不应 panic");

    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        use std::io::Read;
        e.read_to_string(&mut stderr).ok();
    }

    println!("--- 退出状态: {status}");
    println!("--- 写帧结果: {write_result:?}");
    println!("--- 耗时: {elapsed:?}（{} 帧 → {:.1} fps）",
             FRAMES, FRAMES as f64 / elapsed.as_secs_f64());
    println!("--- stderr 末尾 ---\n{}",
             stderr.lines().rev().take(15).collect::<Vec<_>>()
                   .into_iter().rev().collect::<Vec<_>>().join("\n"));
    let _ = FPS;
    Ok(())
}
