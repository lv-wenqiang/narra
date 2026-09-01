use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

use panda::config;
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
                batch_size: batch_size.map(|n| n.clamp(1, 8)).unwrap_or_else(|| {
                    config::resolve_batch_size(std::env::var("EDGE_TTS_BATCH_SIZE").ok().as_deref())
                }),
                timeout: Duration::from_millis(config::resolve_timeout_ms(
                    std::env::var("EDGE_TTS_TIMEOUT_MS").ok().as_deref(),
                )),
            };

            process_narration_file(&input, &outdir, &opts).await
        }
    }
}
