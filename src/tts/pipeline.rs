use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

use crate::duration::mp3_duration_seconds;
use crate::ffmpeg;
use crate::tts::backend::TtsBackend;
use crate::tts::edge::EdgeBackend;
use crate::vtt::generate_vtt;

const DEFAULT_MAX_RETRIES: u32 = 3;

pub struct ProcessOptions {
    pub voice: String,
    pub speed_factor: f64,
    pub batch_size: usize,
    pub timeout: Duration,
}

/// 非空行即一段。对应 process.ts 的 parseParagraphs。
pub fn parse_paragraphs(content: &str) -> Vec<String> {
    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// 重试包装：失败后退避 (attempt+1) * 2000ms，最多 DEFAULT_MAX_RETRIES 次。
async fn synth_with_retry(backend: &EdgeBackend, text: &str) -> Result<Vec<u8>> {
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..DEFAULT_MAX_RETRIES {
        match backend.synth(text).await {
            Ok(s) => return Ok(s.audio),
            Err(e) => {
                let wait = Duration::from_millis(((attempt + 1) as u64) * 2000);
                eprintln!(
                    "  ⚠️  {e} — {}ms 后重试（{}/{}）",
                    wait.as_millis(),
                    attempt + 1,
                    DEFAULT_MAX_RETRIES
                );
                tokio::time::sleep(wait).await;
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("合成失败且无错误记录")))
}

/// 读文稿 → 并发合成 → 合并加速 → 写 audio.mp3 与 audio.vtt → 清理中间文件。
pub async fn process_narration_file(
    input: &Path,
    output_dir: &Path,
    opts: &ProcessOptions,
) -> Result<()> {
    ffmpeg::assert_available()?;

    let content = tokio::fs::read_to_string(input)
        .await
        .with_context(|| format!("读取文稿失败：{}", input.display()))?;
    let lines = parse_paragraphs(&content);
    if lines.is_empty() {
        bail!("Narration file is empty (no non-empty lines)");
    }

    tokio::fs::create_dir_all(output_dir).await?;

    let backend = Arc::new(EdgeBackend::new(&opts.voice, opts.timeout));
    let sem = Arc::new(Semaphore::new(opts.batch_size.max(1)));
    let total = lines.len();

    println!("🎙️  Edge-TTS (Rust)");
    println!("🔊 音色：{}", opts.voice);
    println!(
        "⏱  单段超时 {}ms · 并发 {}",
        opts.timeout.as_millis(),
        opts.batch_size
    );
    println!("📝 共 {total} 段\n");

    let mut handles = Vec::with_capacity(total);
    for (i, text) in lines.iter().enumerate() {
        let index = i + 1;
        let path = output_dir.join(format!("sentence{index}.mp3"));
        let (backend, sem, text) = (backend.clone(), sem.clone(), text.clone());

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await?;
            let preview: String = text.chars().take(40).collect();
            let ellipsis = if text.chars().count() > 40 { "…" } else { "" };
            println!("[{index}/{total}] {preview}{ellipsis}");

            let audio = synth_with_retry(&backend, &text).await?;
            tokio::fs::write(&path, &audio).await?;
            let dur = mp3_duration_seconds(&path);
            println!("    ✅ {} ({dur:.2}s)\n", path.display());
            Ok::<(usize, PathBuf, f64), anyhow::Error>((index, path, dur))
        }));
    }

    // 按 index 归位，保证顺序与文稿一致（并发完成顺序不确定）
    let mut rows: Vec<(usize, PathBuf, f64)> = Vec::with_capacity(total);
    for h in handles {
        rows.push(h.await??);
    }
    rows.sort_by_key(|r| r.0);

    let temp_paths: Vec<PathBuf> = rows.iter().map(|r| r.1.clone()).collect();
    let durations: Vec<f64> = rows.iter().map(|r| r.2).collect();

    let merged = output_dir.join("audio.mp3");
    println!(
        "🔗 合并并以 atempo {} 加速 → {}",
        opts.speed_factor,
        merged.display()
    );
    ffmpeg::merge_mp3_with_speed(&temp_paths, &merged, opts.speed_factor)?;

    let adjusted: Vec<f64> = durations.iter().map(|d| d / opts.speed_factor).collect();

    let vtt_path = output_dir.join("audio.vtt");
    tokio::fs::write(&vtt_path, generate_vtt(&lines, &adjusted, 30)).await?;
    println!("📝 VTT：{}", vtt_path.display());

    println!("🗑️  清理 sentence*.mp3…");
    for p in &temp_paths {
        let _ = tokio::fs::remove_file(p).await;
    }

    let total_secs: f64 = adjusted.iter().sum();
    println!(
        "\n✅ 完成 — 音频 {}，加速后总长约 {total_secs:.2}s",
        merged.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_are_non_empty_trimmed_lines() {
        let got = parse_paragraphs("第一段。\n\n  第二段。  \n\t\n第三段。\n");
        assert_eq!(got, vec!["第一段。", "第二段。", "第三段。"]);
    }

    #[test]
    fn paragraphs_of_blank_input_is_empty() {
        assert!(parse_paragraphs("\n\n   \n\t\n").is_empty());
    }
}
