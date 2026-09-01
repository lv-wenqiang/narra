use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::{JoinSet, LocalSet};

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
/// 最后一次尝试失败后不再等待——反正不会再重试了，白等没有意义（Minor 1 修复）。
async fn synth_with_retry<B: TtsBackend + ?Sized>(backend: &B, text: &str) -> Result<Vec<u8>> {
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..DEFAULT_MAX_RETRIES {
        match backend.synth(text).await {
            Ok(s) => return Ok(s.audio),
            Err(e) => {
                let is_last_attempt = attempt + 1 == DEFAULT_MAX_RETRIES;
                if is_last_attempt {
                    last = Some(e);
                    break;
                }
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
    run_pipeline(backend, lines, output_dir, opts).await
}

/// 核心编排逻辑：对合成后端泛型化（而非写死 `EdgeBackend`），使得失败路径 /
/// 取消 / 清理这些无法用真实网络稳定复现的行为，能够用一个可控的假后端
/// 做确定性的离线测试（见下方 `tests` 模块）。
///
/// 用 `JoinSet::spawn_local` 而非 `tokio::spawn`：原生 `async fn` trait 方法
/// （`TtsBackend::synth`）返回的 Future 类型没有对外暴露、也无法在不修改
/// `backend.rs` 的前提下约束其 `Send`，`tokio::spawn` 要求 Future: Send 会
/// 在泛型场景下无法通过编译。`spawn_local` 不要求 Send，只需要在 `LocalSet`
/// 内运行；由于本流水线是网络 IO 密集型而非 CPU 密集型，单线程协作式并发
/// 仍能拿到并发等待网络的收益（不影响此前审查已确认的并发上限/顺序行为）。
async fn run_pipeline<B>(
    backend: Arc<B>,
    lines: Vec<String>,
    output_dir: &Path,
    opts: &ProcessOptions,
) -> Result<()>
where
    B: TtsBackend + 'static,
{
    let local = LocalSet::new();
    local
        .run_until(run_pipeline_inner(backend, lines, output_dir, opts))
        .await
}

async fn run_pipeline_inner<B>(
    backend: Arc<B>,
    lines: Vec<String>,
    output_dir: &Path,
    opts: &ProcessOptions,
) -> Result<()>
where
    B: TtsBackend + 'static,
{
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

    let mut set: JoinSet<Result<(usize, PathBuf, f64)>> = JoinSet::new();
    for (i, text) in lines.iter().enumerate() {
        let index = i + 1;
        let path = output_dir.join(format!("sentence{index}.mp3"));
        let (backend, sem, text) = (backend.clone(), sem.clone(), text.clone());

        set.spawn_local(async move {
            let _permit = sem.acquire_owned().await?;
            let preview: String = text.chars().take(40).collect();
            let ellipsis = if text.chars().count() > 40 { "…" } else { "" };
            println!("[{index}/{total}] {preview}{ellipsis}");

            let audio = synth_with_retry(backend.as_ref(), &text).await?;
            tokio::fs::write(&path, &audio).await?;
            let dur = mp3_duration_seconds(&path);
            println!("    ✅ {} ({dur:.2}s)\n", path.display());
            Ok::<(usize, PathBuf, f64), anyhow::Error>((index, path, dur))
        });
    }

    // 收集所有任务的结果。一旦发现失败，立即 abort_all 取消尚未完成的任务，
    // 并持续 drain 直到 JoinSet 清空——这保证了函数返回时，不会再有任何
    // 在飞任务在背后继续往 output_dir 写文件（避免孤儿任务 + 事后写入的竞争）。
    let mut rows: Vec<(usize, PathBuf, f64)> = Vec::with_capacity(total);
    let mut first_err: Option<anyhow::Error> = None;
    let mut aborted = false;

    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(row)) => rows.push(row),
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
                if !aborted {
                    set.abort_all();
                    aborted = true;
                }
            }
            Err(join_err) => {
                // 被我们自己 abort_all 取消的任务也会走到这里，属于预期内，不当作新错误。
                if !join_err.is_cancelled() && first_err.is_none() {
                    first_err = Some(anyhow::anyhow!("合成任务异常终止：{join_err}"));
                }
                if !aborted {
                    set.abort_all();
                    aborted = true;
                }
            }
        }
    }

    if let Some(e) = first_err {
        cleanup_sentence_files(output_dir, total).await;
        return Err(e);
    }

    // 按 index 归位，保证顺序与文稿一致（并发完成顺序不确定）
    rows.sort_by_key(|r| r.0);

    let temp_paths: Vec<PathBuf> = rows.iter().map(|r| r.1.clone()).collect();
    let durations: Vec<f64> = rows.iter().map(|r| r.2).collect();

    let merged = output_dir.join("audio.mp3");
    println!(
        "🔗 合并并以 atempo {} 加速 → {}",
        opts.speed_factor,
        merged.display()
    );
    if let Err(e) = ffmpeg::merge_mp3_with_speed(&temp_paths, &merged, opts.speed_factor) {
        cleanup_sentence_files(output_dir, total).await;
        return Err(e);
    }

    let adjusted: Vec<f64> = durations.iter().map(|d| d / opts.speed_factor).collect();

    let vtt_path = output_dir.join("audio.vtt");
    if let Err(e) = tokio::fs::write(&vtt_path, generate_vtt(&lines, &adjusted, 30)).await {
        cleanup_sentence_files(output_dir, total).await;
        return Err(e.into());
    }
    println!("📝 VTT：{}", vtt_path.display());

    println!("🗑️  清理 sentence*.mp3…");
    cleanup_sentence_files(output_dir, total).await;

    let total_secs: f64 = adjusted.iter().sum();
    println!(
        "\n✅ 完成 — 音频 {}，加速后总长约 {total_secs:.2}s",
        merged.display()
    );
    Ok(())
}

/// 尽力而为地删除 `sentence1.mp3..sentenceN.mp3`。路径是可预知的，不依赖
/// 成功任务的返回值——失败时我们恰恰拿不到那些返回值。忽略删除失败（文件
/// 本就可能从未写入过）。
async fn cleanup_sentence_files(output_dir: &Path, total: usize) {
    for i in 1..=total {
        let p = output_dir.join(format!("sentence{i}.mp3"));
        let _ = tokio::fs::remove_file(p).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::backend::Synthesized;
    use std::collections::HashMap;

    #[test]
    fn paragraphs_are_non_empty_trimmed_lines() {
        let got = parse_paragraphs("第一段。\n\n  第二段。  \n\t\n第三段。\n");
        assert_eq!(got, vec!["第一段。", "第二段。", "第三段。"]);
    }

    #[test]
    fn paragraphs_of_blank_input_is_empty() {
        assert!(parse_paragraphs("\n\n   \n\t\n").is_empty());
    }

    /// Minor 2：CRLF 换行的文稿也应正确切分。`content.lines()` 本身会剥离 `\r`，
    /// 这里只是把这个已经正确的行为钉成回归用例。
    #[test]
    fn paragraphs_handle_crlf_line_endings() {
        let got = parse_paragraphs("第一行\r\n\r\n  第二行  \r\n\r\n第三行\r\n");
        assert_eq!(got, vec!["第一行", "第二行", "第三行"]);
    }

    /// 假后端：按文本内容配置行为，用于离线、确定性地测试重试/取消/清理逻辑，
    /// 不依赖真实网络也不依赖真实的 Edge 服务端。
    enum Behavior {
        AlwaysFail,
        SucceedAfter(Duration),
    }

    struct FakeBackend {
        behavior: HashMap<String, Behavior>,
    }

    impl TtsBackend for FakeBackend {
        async fn synth(&self, text: &str) -> Result<Synthesized> {
            match self.behavior.get(text) {
                Some(Behavior::SucceedAfter(d)) => {
                    tokio::time::sleep(*d).await;
                    Ok(Synthesized {
                        audio: vec![0u8; 64],
                        timings: None,
                    })
                }
                _ => anyhow::bail!("模拟合成失败：{text}"),
            }
        }
    }

    /// Minor 1 回归：最后一次重试失败后不应再白等一次退避。
    /// 用 `start_paused` 的虚拟时钟精确断言总耗时恰好是两次退避
    /// （2000ms + 4000ms = 6000ms），而不是修复前会多等的 12000ms。
    #[tokio::test(start_paused = true)]
    async fn last_retry_failure_returns_immediately_without_extra_backoff() {
        struct AlwaysFail;
        impl TtsBackend for AlwaysFail {
            async fn synth(&self, _text: &str) -> Result<Synthesized> {
                anyhow::bail!("boom")
            }
        }

        let start = tokio::time::Instant::now();
        let err = synth_with_retry(&AlwaysFail, "x").await.unwrap_err();
        assert!(err.to_string().contains("boom"));
        assert_eq!(start.elapsed(), Duration::from_millis(6000));
    }

    /// 必修项回归：复现审查者的孤儿任务实验。段落 A 会在重试耗尽后
    /// （虚拟时间 6s）永久失败；段落 B/C 配置为 8s 后才会"成功写文件"。
    /// 修复前的行为是：`run_pipeline` 在 t=6s 就返回 Err，但 B/C 的
    /// `tokio::spawn` 任务不受影响地继续跑，在 t=8s 各自补写了
    /// `sentence2.mp3` / `sentence3.mp3`——调用方早已认为这次运行失败，
    /// 目录里却"事后"冒出了新文件。
    ///
    /// 修复后：一旦观测到 A 失败就 `abort_all` 并 drain 到底，因此
    /// (a) 函数返回瞬间目录里不应有任何 sentence*.mp3；
    /// (b) 把虚拟时钟推过原本 B/C 会完成的 t=8s 之后，目录里依然没有
    ///     新文件出现——证明 B/C 是被真正取消掉了，而不是碰巧还没跑到。
    #[tokio::test(start_paused = true)]
    async fn failing_segment_aborts_in_flight_tasks_and_leaves_no_orphan_files() {
        let dir = std::env::temp_dir().join(format!(
            "panda_pipeline_orphan_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let mut behavior = HashMap::new();
        behavior.insert("A".to_string(), Behavior::AlwaysFail);
        behavior.insert("B".to_string(), Behavior::SucceedAfter(Duration::from_secs(8)));
        behavior.insert("C".to_string(), Behavior::SucceedAfter(Duration::from_secs(8)));
        let backend = Arc::new(FakeBackend { behavior });

        let lines = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let opts = ProcessOptions {
            voice: "irrelevant".into(),
            speed_factor: 1.1,
            batch_size: 3,
            timeout: Duration::from_secs(60),
        };

        let result = run_pipeline(backend, lines, &dir, &opts).await;
        assert!(result.is_err(), "段落 A 永久失败后整体应返回 Err");

        let list = |dir: &Path| -> Vec<String> {
            std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        };

        assert!(
            list(&dir).is_empty(),
            "函数返回瞬间目录应已清空，实际：{:?}",
            list(&dir)
        );

        // 把虚拟时钟推过 B/C 原本会完成的时刻（t=8s），确认它们没有"事后"补写文件。
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(
            list(&dir).is_empty(),
            "把时间推过原定完成点之后，目录里不应冒出新文件（孤儿任务应已被取消），实际：{:?}",
            list(&dir)
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
