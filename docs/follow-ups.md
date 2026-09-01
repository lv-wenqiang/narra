# 待办与已知限制

TTS 子系统（`docs/superpowers/plans/2026-09-01-tts-subsystem.md`）完成时留下的账。
每条都经过审查确认，按「是否值得做」而非「是否是问题」排序。

## 值得做

### 1. `merge_mp3_with_speed` 用 `-y` 直写目标路径，非原子

`src/ffmpeg.rs` 调 ffmpeg 时用 `-y` 直接覆写 `audio.mp3`。若**合并过程本身**被外部中断（`kill -9`、OOM killer、断电、磁盘满），旧的 `audio.mp3` 会被替换成一份**可正常解码但内容被腰斩**的音频——不是字节乱码，所以不容易发现。

已实测复现：合并 7200 秒输入时 0.5 秒后 kill，目标文件变成 786KB / 约 216 秒的完整可解码 mp3。

触发需外部进程级中断，窗口通常亚秒级（60 秒音频转码仅需 0.13 秒），故未阻塞合入。
**修法**：先写临时文件，成功后原子 `rename`。

### 2. `config.rs` 的环境变量层没有自动化测试

`resolve_batch_size` / `resolve_timeout_ms` 两个纯函数有测试，但真正读 `std::env` 的三个函数（`spider_output_dir` / `tts_output_dir` / `tts_input_file`）和 `main.rs` 里「命令行参数 > 环境变量 > 默认值」的优先级链**一条测试都没有**，只有人工验证过。

渲染子系统要加 `render` / `make` 两个子命令，会直接改这块代码——补测试的时机就是那时候。

### 3. 常量散落在三个文件、三种可见性

`DEFAULT_VOICE` / `SPEED_FACTOR` 在 `config.rs`（pub），`DEFAULT_BATCH_SIZE` / `BATCH_SIZE_CAP` / 超时常量也在 `config.rs`（private），`DEFAULT_MAX_RETRIES` 在 `pipeline.rs`，切句上限 `30` 是 `pipeline.rs` 里的字面量，`OUTPUT_FORMAT` 在 `edge.rs`。

它们在实施计划的 Global Constraints 里本是并列的一批可调参数。渲染子系统会再加一批（画布尺寸、帧率、各段时长、字号…），现在定个统一位置成本最低。

### 4. `ffmpeg.rs` 的两处同步 `Command::output()`

`assert_available()` 和 `merge_mp3_with_speed()` 都是同步阻塞调用，合计约 150ms。当前流水线里占比 <1% 且此刻没有并发任务在等这个线程，无影响。

**渲染子系统会长时间跑 ffmpeg（逐帧管道 + H.264 编码），届时这里是第一个该换成 `tokio::process` 的地方。**

## 可以不做

- **测试临时文件卫生**：`tests/ffmpeg_test.rs` 和 `tests/duration_test.rs` 用硬编码的 `/tmp/m1.mp3`、`/tmp/merged.mp3`、`/tmp/not_audio.mp3` 且不清理；`tests/edge_smoke.rs` 往 `temp_dir()` 落盘也不清理。`pipeline.rs` 的测试用 `unique_tmp_dir(pid+uuid)` 并清理，是好的范式。个人自用，只是留垃圾文件。
- **错误信息中英文混用**：`pipeline.rs` 的 "Narration file is empty (no non-empty lines)" 和 `ffmpeg.rs` 的 "merge_mp3_with_speed: no input files" 是英文，其余约 13 条都是中文。后者还泄漏了内部函数名。`tests/ffmpeg_test.rs` 有断言匹配 `"no input files"`，改文案要连带改测试。纯观感。
- **`main.rs` 越过 trait 直接调 `normalize_voice_for_edge`**：`EdgeBackend::new` 内部又规范化一次。函数幂等所以无害，但 CLI 层知道了后端细节，而 `TtsBackend` trait 存在的意义就是隔离这一层。
- **`edge.rs` 消息循环无 `saw_turn_end` 守卫**：若 WebSocket 流在收到部分音频后直接以 `None` 结束（既无 `turn.end` 也无 Close 帧），会把截断的 mp3 当成功返回。实际很难走到——服务端发 Close 帧会被拦，连接重置会传播错误。
- **emoji 计数差异**：`vtt.rs` 的切句用 Unicode 标量计数，TS 原版用 UTF-16 code unit，星光平面字符下长度判定不同。已在代码注释文档化，实测未产生可观察的切分差异。
- **`generate_vtt` 的 `durations[i]` 越界 panic**：唯一调用点在成功路径上必然等长，当前不可达。它是 `pub`，将来若被别处调用才有风险，加一行 `debug_assert_eq!` 是零成本保险。

## 记一笔：长字幕

实跑发现，**逗号密集、句号稀疏的中文文稿会产出很长的单条字幕**（实测一段 54 字、中间只有逗号顿号的段落未被切分，输出为一条 54 字的 cue）。这是切句算法的正确行为——设计规格 §8.4 对 >50 字的字幕专门有 52px 字号规则，正是为此准备的。

**渲染子系统开工时，应该拿这类真实输出做排版验证的输入**，而不是只用短句测试。
