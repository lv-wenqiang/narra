use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

use panda::config;
use panda::render::frame::FrameSource;
use panda::tts::pipeline::{process_narration_file, ProcessOptions};

#[derive(Parser)]
#[command(name = "panda", about = "熊猫视频自动化引擎（Rust）")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 口播文稿 → audio.mp3 + audio.vtt
    Tts {
        /// 文稿路径，默认 $TTS_INPUT_FILE 或 $SPIDER_OUTPUT_DIR/input.txt
        input: Option<PathBuf>,
        /// 输出目录，默认 $TTS_OUTPUT_DIR 或 output/tts
        outdir: Option<PathBuf>,
        /// 音色，默认 $EDGE_TTS_VOICE 或 zh-CN-YunjianNeural
        #[arg(long)]
        voice: Option<String>,
        /// 并发段数，默认 $EDGE_TTS_BATCH_SIZE 或 3，上限 8
        #[arg(long)]
        batch_size: Option<usize>,
    },
    /// 把指定帧渲染成 PNG，用于人工核对视觉
    DebugFrames {
        /// VTT 文件路径
        #[arg(long)]
        vtt: PathBuf,
        /// 标题，默认「熊猫智研社」
        #[arg(long)]
        title: Option<String>,
        /// 输出目录
        #[arg(short, long)]
        out: PathBuf,
        /// 要导出的帧号，逗号分隔；不给则每 30 帧导一张
        #[arg(long)]
        frames: Option<String>,
    },
}

/// 默认标题：VTT 上游没有专门的标题字段，`--title` 缺省时用品牌名占位。
const DEFAULT_DEBUG_TITLE: &str = "熊猫智研社";

/// 解析 `--frames`：逗号分隔的帧号列表；缺省时按每 30 帧取一张覆盖整条时间轴，
/// 并补上末帧——末帧是 Outro 淡出的终点，正是最该人工核对的一帧，而
/// `step_by(30)` 只在总帧数恰好是 30 的倍数加一时才会命中它。
fn parse_frame_list(frames: Option<&str>, total_frames: u32) -> Result<Vec<u32>> {
    match frames {
        Some(s) => s
            .split(',')
            .map(|part| {
                part.trim()
                    .parse::<u32>()
                    .with_context(|| format!("--frames 里的 “{part}” 不是合法帧号"))
            })
            .collect(),
        None => {
            let mut ids: Vec<u32> = (0..total_frames).step_by(30).collect();
            if let Some(last) = total_frames.checked_sub(1)
                && ids.last() != Some(&last)
            {
                ids.push(last);
            }
            Ok(ids)
        }
    }
}

fn run_debug_frames(
    vtt: PathBuf,
    title: Option<String>,
    out: PathBuf,
    frames: Option<String>,
) -> Result<()> {
    let vtt_text = std::fs::read_to_string(&vtt)
        .with_context(|| format!("读取 VTT 文件失败：{}", vtt.display()))?;
    let title = title.unwrap_or_else(|| DEFAULT_DEBUG_TITLE.to_string());

    let mut source = FrameSource::new(&vtt_text, title)?;
    let total_frames = source.total_frames();
    let frame_ids = parse_frame_list(frames.as_deref(), total_frames)?;

    std::fs::create_dir_all(&out)
        .with_context(|| format!("创建输出目录失败：{}", out.display()))?;

    let mut exported = Vec::with_capacity(frame_ids.len());
    for f in frame_ids {
        // 不套外层 with_context：`FrameSource::render` 的 bail! 已经写明了
        // 「帧号 N 超出总时长 M」，再包一层只会把同样的两个数字说第二遍。
        let pixmap = source.render(f)?;
        let path = out.join(format!("frame_{f:05}.png"));
        pixmap
            .save_png(&path)
            .with_context(|| format!("写出 PNG 失败：{}", path.display()))?;
        exported.push(path);
    }

    println!("总帧数：{total_frames}");
    println!("已导出 {} 帧：", exported.len());
    for path in &exported {
        println!("  {}", path.display());
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Tts { input, outdir, voice, batch_size } => {
            let input = input.unwrap_or_else(|| PathBuf::from(config::tts_input_file()));
            let outdir = outdir.unwrap_or_else(|| PathBuf::from(config::tts_output_dir()));

            let voice_raw = voice
                .or_else(|| std::env::var("EDGE_TTS_VOICE").ok())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| config::DEFAULT_VOICE.to_string());

            let opts = ProcessOptions {
                voice: panda::tts::edge::normalize_voice_for_edge(&voice_raw),
                speed_factor: config::SPEED_FACTOR,
                batch_size: match batch_size {
                    Some(n) => config::resolve_batch_size_from_value(Some(n)),
                    None => config::resolve_batch_size(
                        std::env::var("EDGE_TTS_BATCH_SIZE").ok().as_deref(),
                    ),
                },
                timeout: Duration::from_millis(config::resolve_timeout_ms(
                    std::env::var("EDGE_TTS_TIMEOUT_MS").ok().as_deref(),
                )),
            };

            process_narration_file(&input, &outdir, &opts).await
        }
        Commands::DebugFrames { vtt, title, out, frames } => {
            run_debug_frames(vtt, title, out, frames)
        }
    }
}
