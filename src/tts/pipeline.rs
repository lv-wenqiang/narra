use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

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
/// 用 `tokio::spawn`（而非 `spawn_local`）调度每段合成任务：`TtsBackend::synth`
/// 现在显式声明返回 `impl Future<...> + Send`（见 `src/tts/backend.rs`），
/// 所以这里可以直接要求 `B: TtsBackend + Send + Sync + 'static`，让
/// `run_pipeline`（进而 `process_narration_file`）返回的 Future 保持 `Send`，
/// 可以被外部 `tokio::spawn` 调度——这是下一个任务（CLI 需要并行跑多个文稿 /
/// 与进度 UI 并跑）能编译通过的前提。此前用 `LocalSet` + `spawn_local` 绕开
/// Send 约束的做法已经废弃：那样会让 `process_narration_file` 返回的 Future
/// 变成 `!Send`（内部持有 `Rc<tokio::task::local::Context>`），外部一
/// `tokio::spawn` 包裹就编译失败。
async fn run_pipeline<B>(
    backend: Arc<B>,
    lines: Vec<String>,
    output_dir: &Path,
    opts: &ProcessOptions,
) -> Result<()>
where
    B: TtsBackend + Send + Sync + 'static,
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

        set.spawn(async move {
            let _permit = sem.acquire_owned().await?;
            let preview: String = text.chars().take(40).collect();
            let ellipsis = if text.chars().count() > 40 { "…" } else { "" };
            println!("[{index}/{total}] {preview}{ellipsis}");

            let audio = synth_with_retry(backend.as_ref(), &text).await?;
            // 有意用同步 `std::fs::write` 而非 `tokio::fs::write`：后者内部走
            // `spawn_blocking`，一旦已经派发给阻塞线程池，`abort()` 取消不掉
            // 它——写操作可能在 cleanup 跑完之后才落地。段落音频只有几十 KB，
            // 同步写入让这一步对 abort 变成原子的（要么在被取消前完整写完，
            // 要么根本没开始写），零成本封死这条极窄的竞争窗口。
            std::fs::write(&path, &audio)
                .with_context(|| format!("写入 {} 失败", path.display()))?;
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
        // 走到这里说明失败发生在合并之前：本次运行从未写过 audio.mp3，
        // 传 `false` 确保清理不会碰 output_dir 里可能已经存在的、上一次
        // 成功运行留下的 audio.mp3（见 cleanup_after_failure 文档注释）。
        cleanup_after_failure(output_dir, total, false).await;
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
        // 合并本身失败：audio.mp3 没有被本次运行成功产出，传 `false`——
        // 不动 output_dir 里可能已经存在的上一次成功产物。
        cleanup_after_failure(output_dir, total, false).await;
        return Err(e);
    }

    let adjusted: Vec<f64> = durations.iter().map(|d| d / opts.speed_factor).collect();

    let vtt_path = output_dir.join("audio.vtt");
    if let Err(e) = tokio::fs::write(&vtt_path, generate_vtt(&lines, &adjusted, 30)).await {
        // 合并已经成功，audio.mp3 是本次运行刚写出的半成品：传 `true`，
        // 必须清理掉，否则会留下一个没有对应 VTT 的孤儿音频文件。
        cleanup_after_failure(output_dir, total, true).await;
        return Err(e.into());
    }
    println!("📝 VTT：{}", vtt_path.display());

    println!("🗑️  清理 sentence*.mp3…");
    remove_sentence_files(output_dir, total).await;

    let total_secs: f64 = adjusted.iter().sum();
    println!(
        "\n✅ 完成 — 音频 {}，加速后总长约 {total_secs:.2}s",
        merged.display()
    );
    Ok(())
}

/// 尽力而为地删除 `sentence1.mp3..sentenceN.mp3`。路径是可预知的，不依赖
/// 成功任务的返回值——失败时我们恰恰拿不到那些返回值。忽略删除失败（文件
/// 本就可能从未写入过）。成功路径也用它清理中间文件，此时不动 `audio.mp3`
/// （那是本次运行的交付物）。
async fn remove_sentence_files(output_dir: &Path, total: usize) {
    for i in 1..=total {
        let p = output_dir.join(format!("sentence{i}.mp3"));
        let _ = tokio::fs::remove_file(p).await;
    }
}

/// 失败退出时的清理：`sentence*.mp3` 总是无条件清理（它们只可能是本次运行
/// 写出来的中间文件）。`audio.mp3` 则只有在 `this_run_wrote_audio_mp3` 为
/// `true`（即本次运行已经跑完合并、audio.mp3 确实是这次运行的半成品——例如
/// 合并成功但随后写 VTT 失败）时才删除。
///
/// 这个参数是本轮终审必修项的核心：CLI 把 `output_dir` 接成了跨多次运行
/// 稳定复用的目录（例如 `output/tts`），一次瞬时失败（网络抖动、Edge 临时
/// 403、音色打错）如果在合并之前就发生——此时 `audio.mp3` 根本不存在于这
/// 次运行的产出中，output_dir 里若有 `audio.mp3` 只可能是上一次成功运行
/// 留下的好产物——无条件删除会把它连带上一次成功的 `audio.vtt` 一起变成
/// 误导性的“看起来正常、实际半成品”状态（更糟的是留下一个没有音频对应的
/// 孤儿 vtt）。只有当本次运行真的执行到“合并成功”这一步之后又失败时，
/// `audio.mp3` 才是这次运行自己产出的半成品，才需要清理。
async fn cleanup_after_failure(output_dir: &Path, total: usize, this_run_wrote_audio_mp3: bool) {
    remove_sentence_files(output_dir, total).await;
    if this_run_wrote_audio_mp3 {
        let _ = tokio::fs::remove_file(output_dir.join("audio.mp3")).await;
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

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "panda_pipeline_{tag}_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    /// 假后端：按文本内容配置行为，用于离线、确定性地测试重试/取消/清理逻辑，
    /// 不依赖真实网络也不依赖真实的 Edge 服务端。
    enum Behavior {
        AlwaysFail,
        SucceedAfter(Duration),
        /// 返回调用方提供的真实音频字节（而非占位的全零字节），用于需要真的
        /// 跑到 ffmpeg 合并阶段（进而验证合并成功后 `audio.mp3` 清理标志）
        /// 的测试——全零字节不是合法 mp3，ffmpeg 合并会直接失败。
        SucceedWithAudio(Vec<u8>),
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
                Some(Behavior::SucceedWithAudio(bytes)) => Ok(Synthesized {
                    audio: bytes.clone(),
                    timings: None,
                }),
                _ => anyhow::bail!("模拟合成失败：{text}"),
            }
        }
    }

    /// 用 ffmpeg 现生成一段极短的合法 mp3（1 秒正弦波），供需要真的跑通合并
    /// 阶段的测试使用。若本机没有 ffmpeg，直接 panic——这些测试本就依赖
    /// ffmpeg（`process_narration_file`/`run_pipeline` 的合并步骤本身就
    /// 需要它），与 `tests/ffmpeg_test.rs` 里生成测试音频的方式一致。
    fn tiny_valid_mp3_bytes() -> Vec<u8> {
        let path = std::env::temp_dir().join(format!(
            "panda_pipeline_tiny_mp3_{}_{}.mp3",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let out = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:a",
                "libmp3lame",
                "-b:a",
                "48k",
                "-ar",
                "24000",
                "-ac",
                "1",
                &path.to_string_lossy(),
            ])
            .output()
            .expect("启动 ffmpeg 失败");
        assert!(
            out.status.success(),
            "ffmpeg 生成测试音频失败：{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let bytes = std::fs::read(&path).expect("读取生成的测试音频失败");
        let _ = std::fs::remove_file(&path);
        bytes
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

    /// 必修项回归：复现审查者的孤儿任务实验，并用耗时断言把 `abort_all` 钉住
    /// （复审的变异测试发现：只看"目录是否为空"测不出删掉 `abort_all` 的
    /// 回归——因为即便不 abort，drain 循环最终也会等到 B/C 真正跑完再清理，
    /// 目录最终还是空的，只是要多等 2 秒）。
    ///
    /// 场景：段落 A 会在重试耗尽后（虚拟时间 6s）永久失败；段落 B/C 配置为
    /// 8s 后才会"成功写文件"。
    /// - 有 `abort_all`：A 失败后立刻取消 B/C，drain 在 t=6s 就结束。
    /// - 没有 `abort_all`：B/C 不受影响地跑到 t=8s 才自然完成，drain 才结束。
    ///
    /// 所以断言总耗时**恰好** 6s（而不是 8s），才能把 `abort_all` 这个机制
    /// 本身钉住，而不只是钉住"最终目录为空"这个由 drain 循环单独就能保证
    /// 的结果。
    #[tokio::test(start_paused = true)]
    async fn failing_segment_aborts_in_flight_tasks_and_leaves_no_orphan_files() {
        let dir = unique_tmp_dir("orphan_test");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let mut behavior = HashMap::new();
        behavior.insert("A".to_string(), Behavior::AlwaysFail);
        behavior.insert(
            "B".to_string(),
            Behavior::SucceedAfter(Duration::from_secs(8)),
        );
        behavior.insert(
            "C".to_string(),
            Behavior::SucceedAfter(Duration::from_secs(8)),
        );
        let backend = Arc::new(FakeBackend { behavior });

        let lines = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let opts = ProcessOptions {
            voice: "irrelevant".into(),
            speed_factor: 1.1,
            batch_size: 3,
            timeout: Duration::from_secs(60),
        };

        let start = tokio::time::Instant::now();
        let result = run_pipeline(backend, lines, &dir, &opts).await;
        assert!(result.is_err(), "段落 A 永久失败后整体应返回 Err");

        // 关键断言：恰好 6s，不是 8s。这把 abort_all 钉住——去掉它之后
        // drain 循环要等 B/C 在 t=8s 自然完成才能结束。
        assert_eq!(
            start.elapsed(),
            Duration::from_secs(6),
            "应在 A 耗尽重试的 t=6s 就返回（B/C 应已被 abort_all 取消，\
             而不是等到它们 t=8s 自然完成）"
        );

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

    /// 加固项：把 cleanup 钉住。复审的变异测试发现：只删掉合成失败出口的
    /// `cleanup_sentence_files` 调用，上面那条孤儿任务测试仍然通过——因为
    /// 那条测试里 B/C 从未真正落盘过（它们在写文件之前就被 abort 了），
    /// 所以"目录为空"这个结果跟 cleanup 是否被调用无关，测不出 cleanup
    /// 被删掉的回归。
    ///
    /// 这里构造一个不同的场景：段落 S 在 F 永久失败之前就已经**成功落盘**
    /// （S 在 t=1s 完成，F 在 t=6s 才耗尽重试），所以 sentence1.mp3 在错误
    /// 出现前就已经是磁盘上的真实文件，不依赖任何取消时机。如果 cleanup
    /// 被删掉，它会在函数返回后原样残留；断言它必须消失，就把 cleanup
    /// 这个机制本身钉住了。
    #[tokio::test(start_paused = true)]
    async fn cleanup_removes_a_segment_file_that_had_already_succeeded_before_the_failure() {
        let dir = unique_tmp_dir("cleanup_pin_test");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let mut behavior = HashMap::new();
        behavior.insert(
            "S".to_string(),
            Behavior::SucceedAfter(Duration::from_secs(1)),
        );
        behavior.insert("F".to_string(), Behavior::AlwaysFail);
        let backend = Arc::new(FakeBackend { behavior });

        let lines = vec!["S".to_string(), "F".to_string()];
        let opts = ProcessOptions {
            voice: "irrelevant".into(),
            speed_factor: 1.1,
            batch_size: 2,
            timeout: Duration::from_secs(60),
        };

        let result = run_pipeline(backend, lines, &dir, &opts).await;
        assert!(result.is_err(), "段落 F 永久失败后整体应返回 Err");

        // sentence1.mp3 对应 "S"：它在 t=1s 就已经真实成功落盘，早于 F 在
        // t=6s 才暴露的失败。如果没有 cleanup，它会原样残留在目录里。
        assert!(
            !dir.join("sentence1.mp3").exists(),
            "已经成功落盘的 sentence1.mp3 应该在失败退出时被 cleanup 清理掉"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// 终审必修项 1 的核心回归：合成失败（发生在合并之前，`audio.mp3` 根本
    /// 不是本次运行的产物）时，不能删掉 `output_dir` 里已经存在的、上一次
    /// 成功运行留下的 `audio.mp3`。这正是复审报告里描述的真实场景：CLI 把
    /// `output_dir` 接成跨运行复用的稳定目录，一次瞬时失败（网络抖动 /
    /// 音色打错）不该抹掉上一份好音频。
    #[tokio::test(start_paused = true)]
    async fn synth_failure_does_not_delete_preexisting_audio_mp3_from_a_previous_run() {
        let dir = unique_tmp_dir("preexisting_audio_test");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let audio_path = dir.join("audio.mp3");
        let previous_run_bytes = b"previous successful run's audio bytes".to_vec();
        tokio::fs::write(&audio_path, &previous_run_bytes)
            .await
            .unwrap();

        let mut behavior = HashMap::new();
        behavior.insert("F".to_string(), Behavior::AlwaysFail);
        let backend = Arc::new(FakeBackend { behavior });

        let lines = vec!["F".to_string()];
        let opts = ProcessOptions {
            voice: "irrelevant".into(),
            speed_factor: 1.1,
            batch_size: 1,
            timeout: Duration::from_secs(60),
        };

        let result = run_pipeline(backend, lines, &dir, &opts).await;
        assert!(result.is_err(), "段落 F 永久失败后整体应返回 Err");

        let bytes_after = tokio::fs::read(&audio_path)
            .await
            .expect("上一次成功运行留下的 audio.mp3 不应被删除");
        assert_eq!(
            bytes_after, previous_run_bytes,
            "失败清理不应删除或改动不是本次运行产生的 audio.mp3"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// 终审必修项 1 的另一半：本次运行如果已经跑到“合并成功”这一步，
    /// `audio.mp3` 就确实是这次运行自己产出的东西——随后如果写 VTT 失败，
    /// 这个半成品必须被清理掉，否则会留下一个没有对应 VTT 的孤儿音频文件。
    ///
    /// 用预先把 `audio.vtt` 建成一个目录的方式，制造“合并成功、写 VTT 失败”
    /// 这个场景（对一个目录路径 `tokio::fs::write` 必然失败）。
    #[tokio::test]
    async fn merge_success_but_vtt_write_failure_cleans_up_this_runs_audio_mp3() {
        let dir = unique_tmp_dir("vtt_failure_cleanup_test");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // 让 audio.vtt 这个路径本身是个目录，逼 `tokio::fs::write` 失败。
        tokio::fs::create_dir_all(dir.join("audio.vtt"))
            .await
            .unwrap();

        let mut behavior = HashMap::new();
        behavior.insert(
            "S".to_string(),
            Behavior::SucceedWithAudio(tiny_valid_mp3_bytes()),
        );
        let backend = Arc::new(FakeBackend { behavior });

        let lines = vec!["S".to_string()];
        let opts = ProcessOptions {
            voice: "irrelevant".into(),
            speed_factor: 1.1,
            batch_size: 1,
            timeout: Duration::from_secs(60),
        };

        let result = run_pipeline(backend, lines, &dir, &opts).await;
        assert!(
            result.is_err(),
            "audio.vtt 路径被占用为目录，写 VTT 应该失败，整体应返回 Err"
        );

        assert!(
            !dir.join("audio.mp3").exists(),
            "合并已经成功产出的 audio.mp3 是本次运行的半成品，写 VTT 失败后应被清理掉"
        );
        assert!(
            !dir.join("sentence1.mp3").exists(),
            "sentence*.mp3 中间文件也应被清理"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// 必修项 1 的核心验收点：`process_narration_file` 返回的 Future 必须是
    /// `Send`，否则下一个任务（CLI）想 `tokio::spawn` 并行处理多个文稿、或
    /// 与进度 UI 并跑时会直接编译失败（这正是引入 `LocalSet` 后出现过的
    /// 真实回归：`E0277: Rc<tokio::task::local::Context> cannot be sent
    /// between threads safely`）。这里不是纸面上的编译期断言，而是真的
    /// `tokio::spawn` 一次、真的 `.await` 它、真的断言返回值——如果 Future
    /// 不是 Send，这个测试函数本身就编译不过。
    #[tokio::test]
    async fn process_narration_file_future_is_send_and_spawnable() {
        let dir = unique_tmp_dir("send_probe");
        let missing_input = dir.join("does-not-exist.txt");
        let opts = ProcessOptions {
            voice: "zh-CN-XiaoxiaoNeural".into(),
            speed_factor: 1.1,
            batch_size: 1,
            timeout: Duration::from_millis(50),
        };

        let handle =
            tokio::spawn(async move { process_narration_file(&missing_input, &dir, &opts).await });

        let result = handle.await.expect("spawn 出去的任务不应 panic 或被取消");
        assert!(
            result.is_err(),
            "读取不存在的文稿应该报错，而不是 panic 或挂起"
        );
    }
}
