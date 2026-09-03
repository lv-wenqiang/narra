use crate::render::timeline::{FPS, HEIGHT, WIDTH};
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 探测 PATH 上是否有可用的 ffmpeg。对应 TS 版 assertFfmpegAvailable。
pub fn assert_available() -> Result<()> {
    let out = Command::new("ffmpeg").arg("-version").output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        _ => bail!(
            "TTS 的合并与变速步骤需要 ffmpeg。请安装 ffmpeg 并确保它在 PATH 上。"
        ),
    }
}

/// 生成 concat demuxer 的清单内容。路径转为绝对路径，单引号按 shell 规则转义。
pub fn concat_list_body(inputs: &[PathBuf]) -> Result<String> {
    let mut lines = Vec::with_capacity(inputs.len());
    for p in inputs {
        let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let path_str = abs.to_string_lossy();

        // concat 清单是逐行文本格式，无法承载含换行符的路径
        if path_str.contains('\n') || path_str.contains('\r') {
            bail!(
                "concat 清单是逐行文本格式，无法承载含换行符的路径：{}",
                path_str
            );
        }

        let s = path_str.replace('\'', r"'\''");
        lines.push(format!("file '{s}'"));
    }
    Ok(lines.join("\n"))
}

/// 合并多个 mp3 并施加 atempo 变速。参数与 TS 版 mergeMp3WithSpeed 完全一致。
pub fn merge_mp3_with_speed(inputs: &[PathBuf], output: &Path, speed: f64) -> Result<()> {
    if inputs.is_empty() {
        bail!("merge_mp3_with_speed: no input files");
    }

    let list_path = PathBuf::from(format!("{}.concat.txt", output.display()));
    std::fs::write(&list_path, concat_list_body(inputs)?)
        .with_context(|| format!("写 concat 清单失败：{}", list_path.display()))?;

    let result = Command::new("ffmpeg")
        .args([
            "-y", "-f", "concat", "-safe", "0",
            "-i", &list_path.to_string_lossy(),
            "-vn", "-filter:a", &format!("atempo={speed}"),
            "-c:a", "libmp3lame", "-q:a", "2",
            &output.to_string_lossy(),
        ])
        .output();

    let _ = std::fs::remove_file(&list_path);

    let out = result.context("启动 ffmpeg 失败")?;
    if !out.status.success() {
        bail!(
            "ffmpeg 退出码 {:?}：\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Content 段起点（秒）：Cover 15 帧 + Intro 105 帧 = 120 帧 = 4.0s。
const CONTENT_START_SECS: f64 = 4.0;
/// Intro 段起点（秒）：Cover 15 帧 = 0.5s。
const INTRO_START_SECS: f64 = 0.5;
/// BGM 淡出时长（秒），规格 §9.3。
const BGM_FADE_SECS: f64 = 2.0;
/// BGM 在 TTS 之下的基准音量，规格 §9.3。
const BGM_VOLUME: f64 = 0.15;
/// 两段音效的音量，规格 §9.3。
const SFX_VOLUME: f64 = 0.6;

/// 构造四路音频的 filter_complex 片段（不含视频部分）。
///
/// `audio_secs` 是 `A`（VTT 末条字幕的结束时间）；`outro_start_secs` 是
/// Outro 段起点的绝对时间 = `(120 + content_frames) / 30`。
///
/// **BGM 淡出的时间基准**：规格 §9.3 写的 `[A-2, A]` 是 **Content 段内**
/// 时间（TS 原版用的是段内相对时钟）。这里必须换算成全片绝对时间
/// `[CONTENT_START + A - 2, CONTENT_START + A]`。漏掉这个 +4.0 会让 BGM
/// 提前 4 秒开始淡出，而成片照样能播、不报错。
///
/// **为什么用 `afade` 而非规格写的 `volume` 时间表达式**：`volume` 的表达式
/// 形如 `if(lt(t,X),...)`，其中的逗号在 filtergraph 里是滤镜分隔符，必须靠
/// 引号保护——写对一次容易，被后人改错也容易，且失败形式是成片音量不对而
/// 非报错。`afade=t=out` 默认 `curve=tri`（线性），与「2 秒内线性降到 0、
/// 之后保持 0」语义完全等价，且不含逗号。
///
/// **`[N:a]` 而非 `[N]`**：BGM 文件带内嵌 mjpeg 封面图，是第二条流；不显式
/// 选音频流会把封面图当视频流拉进来。
///
/// **`st` 固定三位小数**：`fade_start` 由浮点减法得来，`audio_secs` 又源自
/// VTT 时间戳的累加，实测某些取值（如 `9.999999999999998`）会让 `f64` 的
/// 默认 `Display` 吐出十几位小数（如 `st=11.999999999999998`）。虽然 ffmpeg
/// 能接受，但这类输出既难读也难在测试里断言，故统一用 `{:.3}`（毫秒精度，
/// 与 `adelay` 的单位一致）定长格式化。
pub fn audio_filter_graph(audio_secs: f64, outro_start_secs: f64) -> String {
    let content_ms = (CONTENT_START_SECS * 1000.0).round() as i64;
    let intro_ms = (INTRO_START_SECS * 1000.0).round() as i64;
    let outro_ms = (outro_start_secs * 1000.0).round() as i64;
    let fade_start = CONTENT_START_SECS + audio_secs - BGM_FADE_SECS;

    format!(
        "[2:a]adelay={content_ms}:all=1,volume=1[a_tts];\
         [3:a]adelay={content_ms}:all=1,volume={BGM_VOLUME},\
         afade=t=out:st={fade_start:.3}:d={BGM_FADE_SECS}[a_bgm];\
         [4:a]adelay={intro_ms}:all=1,volume={SFX_VOLUME}[a_type];\
         [5:a]adelay={outro_ms}:all=1,volume={SFX_VOLUME}[a_intro];\
         [a_tts][a_bgm][a_type][a_intro]amix=inputs=4:normalize=0:duration=longest[a]"
    )
}

/// 一次成片合成所需的全部输入与参数。
pub struct RenderInputs<'a> {
    pub bg: &'a Path,
    pub tts_audio: &'a Path,
    pub bgm: &'a Path,
    pub typewriter: &'a Path,
    pub intro: &'a Path,
    pub out: &'a Path,
    pub total_frames: u32,
    pub audio_secs: f64,
    pub content_frames: u32,
}

/// 构造完整的 ffmpeg 参数向量（不含程序名）。
///
/// 纯函数，不碰文件系统也不起进程——规格 §5 要求 `ffmpeg` 模块「只构造参数
/// （可断言命令行）」，进程管理是 `run_render` 的事。
///
/// **输入顺序即 filter_complex 里的编号**：0 背景视频、1 stdin 帧流、
/// 2 TTS、3 BGM、4 打字机、5 片尾音效。改动顺序必须同步改滤镜图。
///
/// **`-stream_loop -1` 是输入选项**，必须紧贴它要循环的那个 `-i`。放错位置
/// 会静默失效（素材播完即止，ffmpeg 不报错）。
pub fn build_render_args(i: &RenderInputs) -> Vec<String> {
    // Outro 起点 = Content 段起点 + content_frames，换算成秒。
    // `CONTENT_START_SECS * FPS` = 120 帧（Cover 15 帧 + Intro 105 帧）——
    // 这个 120 与 `audio_filter_graph` 文档里、以及 CONTENT_START_SECS 本身
    // 表示的是同一件事实，不能各写一份字面量：写两份，其中一份被改错时
    // （比如改成 121）不会编译失败，只会让 Outro 音效错位几十毫秒——听感
    // 上未必能察觉，但确实是错的。这里从 CONTENT_START_SECS 反推，让两处
    // 共享同一个真相源。
    let outro_start_secs =
        (CONTENT_START_SECS * FPS as f64 + i.content_frames as f64) / FPS as f64;
    let total_secs = i.total_frames as f64 / FPS as f64;

    let filter = format!(
        "[0:v]scale={WIDTH}:{HEIGHT}:force_original_aspect_ratio=increase,\
         crop={WIDTH}:{HEIGHT},colorchannelmixer=rr=0.8:gg=0.8:bb=0.8[bg];\
         [bg][1:v]overlay=shortest=0[v];{}",
        audio_filter_graph(i.audio_secs, outro_start_secs)
    );

    let s = |p: &Path| p.to_string_lossy().into_owned();
    vec![
        "-y".into(),
        // 0: 背景视频（循环）
        "-stream_loop".into(),
        "-1".into(),
        "-i".into(),
        s(i.bg),
        // 1: 帧流（stdin）
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-s".into(),
        format!("{WIDTH}x{HEIGHT}"),
        "-r".into(),
        FPS.to_string(),
        "-i".into(),
        "-".into(),
        // 2: TTS
        "-i".into(),
        s(i.tts_audio),
        // 3: BGM（循环）
        "-stream_loop".into(),
        "-1".into(),
        "-i".into(),
        s(i.bgm),
        // 4: 打字机音效
        "-i".into(),
        s(i.typewriter),
        // 5: 片尾音效
        "-i".into(),
        s(i.intro),
        "-filter_complex".into(),
        filter,
        "-map".into(),
        "[v]".into(),
        "-map".into(),
        "[a]".into(),
        "-t".into(),
        format!("{total_secs}"),
        "-c:v".into(),
        "libx264".into(),
        "-crf".into(),
        "23".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        s(i.out),
    ]
}

/// 起 ffmpeg 子进程，另开线程并发读它的 stderr，主线程把帧流写进它的
/// stdin，最后等待结束。真实的可执行文件名固定是 `"ffmpeg"`；测试用
/// [`run_render_with_ffmpeg_binary`] 注入假进程验证编排细节。
pub fn run_render(
    source: &mut crate::render::frame::FrameSource,
    inputs: &RenderInputs,
) -> Result<()> {
    assert_available()?;
    run_render_with_ffmpeg_binary(Path::new("ffmpeg"), source, inputs)
}

/// `run_render` 的实际实现，program 可替换——生产路径永远是字面量
/// `"ffmpeg"`（见 [`run_render`]），测试路径可以指向一个模拟慢速/提前
/// 退出/超量 stderr 的假脚本，从而对「stdin/stderr 并发编排是否正确」
/// 和「ffmpeg 成功但写帧失败时错误不能被吞掉」这两条真实 ffmpeg 很难在
/// 单测规模下稳定复现的语义做确定性验证，且不必去碰进程级的 `PATH`
/// 环境变量（那样会和同一进程里并发跑的其它测试互相干扰）。
///
/// **顺序很要紧**（Task 0 探针实测确认，见 `docs/ffmpeg-pipeline.md` 第 6
/// 节）：先 `spawn`，再 `take()` 走 stdin 与 stderr，再起并发读 stderr 的
/// 线程，然后主线程写帧，最后 `wait()`。stderr 用管道时若不并发读取，
/// ffmpeg 侧的进度输出把 64KB 管道缓冲区写满后会阻塞在 `write()` 上，
/// 而我们如果这时候还在等 `child.wait()`（或者压根没读 stderr），双方
/// 互相等待——死锁，且探针规模的测试完全测不出来（stderr 体积随挂钟时间
/// 线性增长，填满缓冲区约需 4 分钟挂钟时间，真实渲染每帧要过 tiny-skia，
/// 挂钟耗时远高于探针的纯内存 memcpy）。写帧留在主线程、只把 stderr 读
/// 挪到子线程，是刻意的最小改动：只要 stderr 读取与写帧是并发的，就不会
/// 卡在缓冲区上，不需要额外再起一个写帧线程。
///
/// **ffmpeg 提前退出**（参数错误、素材缺失）时写帧会遇到 broken pipe。
/// 那是正常的失败路径：`write_rgba_frames` 返回 `Err`，但我们优先报告
/// ffmpeg 自己的 stderr，因为它说的才是根因，写端的 broken pipe 只是
/// 后果——`if !status.success()` 分支永远先于 `write_result` 被检查。
/// 反过来，**ffmpeg 退出码是 0 但写帧失败**（比如它提前关闭了 stdin、
/// 我们还有帧没写完）属于我们这侧的错，`write_result` 的错误不能被
/// `Ok(())` 吞掉，否则会静默产出一个帧数被截断的坏成片。
fn run_render_with_ffmpeg_binary(
    program: &Path,
    source: &mut crate::render::frame::FrameSource,
    inputs: &RenderInputs,
) -> Result<()> {
    if let Some(parent) = inputs.out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建输出目录失败：{}", parent.display()))?;
    }

    let args = build_render_args(inputs);
    let mut child = Command::new(program)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("启动 ffmpeg 失败")?;

    let mut stdin = child.stdin.take().expect("stdin 已声明为 piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr 已声明为 piped");

    // 必须在 wait() 之前就跑起来，且与写帧并发：否则 ffmpeg 进度输出把
    // 管道缓冲区写满会阻塞在 write() 上，我们又卡在 wait()——死锁。
    let stderr_reader = std::thread::spawn(move || {
        let mut s = String::new();
        stderr_pipe.read_to_string(&mut s).ok();
        s
    });

    let write_result = source.write_rgba_frames(&mut stdin);
    // 显式 drop 关闭管道，让 ffmpeg 知道输入结束——这一行不是可有可无的
    // 收尾清洁，删掉它会让真实 ffmpeg **真的永久挂起**（Task 5 实测确认，
    // 不是理论推测：用真实素材跑端到端渲染，删掉这行后 60 秒仍不退出）。
    // 两条看似矛盾、实则互补的事实都要记住：
    // 1. **光关 stdin 不够**：`build_render_args` 里 `overlay=shortest=0`
    //    配合 `-stream_loop -1` 与 rawvideo 输入的 `eof_action=repeat`，
    //    提前关闭 stdin（帧还没写完）不会让 ffmpeg 收尾，它会用最后一帧
    //    一直填下去，真正的终止条件是 `build_render_args` 算出的那个
    //    `-t`（Task 0 探针实测确认，见 `docs/ffmpeg-pipeline.md` 第 7 节）。
    // 2. **光靠 -t 也不够**：即使写完的帧数正好等于 `-t` 对应的时长，
    //    不主动 drop、让 stdin 停留在"没写但也没关"的状态，ffmpeg 的
    //    rawvideo 分离器会阻塞在读下一帧的 `read()` 上，永远等不到那个
    //    让它意识到"该停了"的信号——`-t` 本身不会主动打断一次阻塞的读。
    // 两者缺一不可：`-t` 定义"该在哪停"，EOF 定义"没有更多数据了，请去检查
    // 是否已经该停"。
    drop(stdin);

    let status = child.wait().context("等待 ffmpeg 结束失败")?;
    let stderr = stderr_reader.join().unwrap_or_default();

    if !status.success() {
        // ffmpeg 的 stderr 说的才是根因，原样透出，不做吞噬或改写。
        bail!("ffmpeg 退出码 {status}，原始输出：\n{stderr}");
    }
    // ffmpeg 成功了但写帧失败：说明帧数/管道对不上，属于我们这侧的错，
    // 不能因为 ffmpeg 自己退出码是 0 就当作整体成功。
    write_result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_graph_uses_explicit_audio_stream_selectors() {
        // BGM 文件带内嵌 mjpeg 封面图（第二条流）。写 [3] 会把封面图当视频流
        // 拉进来；必须写 [3:a]。四路一律显式选音频流。
        let g = audio_filter_graph(10.0, 16.0);
        for label in ["[2:a]", "[3:a]", "[4:a]", "[5:a]"] {
            assert!(g.contains(label), "缺少显式音频流选择器 {label}：{g}");
        }
        for bare in ["[2]", "[3]", "[4]", "[5]"] {
            assert!(!g.contains(bare), "不得使用裸流选择器 {bare}：{g}");
        }
    }

    #[test]
    fn audio_graph_delays_each_source_to_its_segment_start() {
        // TTS 与 BGM 起点 4.0s；打字机 0.5s；片尾音效 = outro_start。
        let g = audio_filter_graph(10.0, 16.0);
        assert!(g.contains("adelay=4000:all=1"), "TTS/BGM 应延迟 4000ms：{g}");
        assert!(g.contains("adelay=500:all=1"), "打字机应延迟 500ms：{g}");
        assert!(
            g.contains("adelay=16000:all=1"),
            "片尾音效应延迟到 Outro 起点 16000ms：{g}"
        );
    }

    #[test]
    fn bgm_fade_starts_two_seconds_before_audio_end_in_absolute_time() {
        // 这是本计划最容易写错的一条（裁定 R2）：
        // 规格 §9.3 的 [A-2, A] 是 Content 段内时间，Content 起点是 4.0s，
        // 所以绝对时间是 [4+A-2, 4+A]。A=10 → 淡出从 12.0s 开始，持续 2s。
        // 漏掉 +4.0 会得到 8.0——成片照样能播，但 BGM 提前 4 秒淡出。
        let g = audio_filter_graph(10.0, 16.0);
        assert!(
            g.contains("afade=t=out:st=12.000:d=2"),
            "淡出应从绝对时间 12s 开始：{g}"
        );
        assert!(!g.contains("st=8"), "st=8 说明漏掉了 Content 起点的 +4.0 偏移：{g}");
    }

    #[test]
    fn bgm_fade_start_tracks_audio_length() {
        // 换一个 A 值，确认 12 不是写死的。A=30 → 4+30-2 = 32。
        let g = audio_filter_graph(30.0, 36.0);
        assert!(
            g.contains("afade=t=out:st=32.000:d=2"),
            "A=30 时淡出应从 32s 开始：{g}"
        );
    }

    #[test]
    fn bgm_is_ducked_to_zero_point_one_five_before_fading() {
        let g = audio_filter_graph(10.0, 16.0);
        assert!(g.contains("volume=0.15"), "BGM 基准音量应为 0.15：{g}");
    }

    #[test]
    fn sound_effects_use_zero_point_six_and_tts_is_unattenuated() {
        let g = audio_filter_graph(10.0, 16.0);
        assert_eq!(g.matches("volume=0.6").count(), 2, "两段音效都应是 0.6：{g}");
        assert!(g.contains("volume=1"), "TTS 不衰减：{g}");
    }

    #[test]
    fn audio_graph_pairs_each_stream_selector_with_its_own_delay() {
        // 补充测试（自查发现的缺口）：`audio_graph_delays_each_source_to_its_
        // segment_start` 只检查各 adelay 数值是否「在字符串某处出现」，不检查
        // 它绑在哪个输入流上。若把打字机（[4:a]）与片尾音效（[5:a]）的 adelay
        // 接反——两者音量同为 0.6，仅起点不同——上面那条测试和音量计数测试都
        // 察觉不到，因为两个数值依旧都在字符串里，只是换了主人。这里改为断言
        // 「流选择器 + adelay」的连续子串，把配对关系钉死。
        let g = audio_filter_graph(10.0, 16.0);
        assert!(
            g.contains("[2:a]adelay=4000:all=1"),
            "TTS 应在 [2:a] 上应用 4000ms 延迟：{g}"
        );
        assert!(
            g.contains("[3:a]adelay=4000:all=1"),
            "BGM 应在 [3:a] 上应用 4000ms 延迟：{g}"
        );
        assert!(
            g.contains("[4:a]adelay=500:all=1"),
            "打字机应在 [4:a] 上应用 500ms 延迟，而非接到片尾音效的延迟：{g}"
        );
        assert!(
            g.contains("[5:a]adelay=16000:all=1"),
            "片尾音效应在 [5:a] 上应用 16000ms 延迟，而非接到打字机的延迟：{g}"
        );
    }

    #[test]
    fn each_stream_carries_its_own_complete_filter_chain() {
        // 修复轮 1：审查实测到一个 audio_graph_pairs_each_stream_selector_
        // with_its_own_delay 抓不住的变异——把 [2:a] 接上 BGM 的链
        // （volume=0.15,afade=...）、[3:a] 接上 TTS 的链（volume=1），字面值
        // 一个不改、只换配对。之前 10 条测试全部通过，原因和 M11 同源：TTS
        // 与 BGM 共用 adelay=4000，`bgm_is_ducked_...` 与
        // `sound_effects_use_...` 都只检查「某个值在字符串某处出现」，不检查
        // 它挂在哪一路上；`audio_graph_pairs_each_stream_selector_with_its_
        // own_delay` 只覆盖「选择器 + adelay」，不覆盖整条链，所以对
        // volume/afade 部分被调包同样无感。
        //
        // 后果比 M11 严重：M11 对调的是两段静默待触发的音效，这条会让背景
        // 音乐播在人声的位置、人声播在背景音乐的位置——一耳朵就能听出来的
        // 成片缺陷，测试却全绿。这里把每一路的整条链作为一个连续子串来断
        // 言，而不是逐项检查「字符串里某处有某个值」，四路一次堵死，而不是
        // 只补出问题的那一对。
        let g = audio_filter_graph(10.0, 16.0);
        for (label, chain) in [
            ("TTS", "[2:a]adelay=4000:all=1,volume=1[a_tts]"),
            (
                "BGM",
                "[3:a]adelay=4000:all=1,volume=0.15,afade=t=out:st=12.000:d=2[a_bgm]",
            ),
            ("打字机", "[4:a]adelay=500:all=1,volume=0.6[a_type]"),
            ("片尾音效", "[5:a]adelay=16000:all=1,volume=0.6[a_intro]"),
        ] {
            assert!(
                g.contains(chain),
                "{label} 这一路的完整链应原样出现：\n期望 {chain}\n实得 {g}"
            );
        }
    }

    #[test]
    fn amix_disables_normalization_and_mixes_four_inputs() {
        // normalize=1（默认）会按输入数自动缩放，把各路相对音量全改掉。
        let g = audio_filter_graph(10.0, 16.0);
        assert!(g.contains("amix=inputs=4"), "应混合四路：{g}");
        assert!(g.contains("normalize=0"), "必须关闭自动归一化：{g}");
    }

    #[test]
    fn audio_graph_has_no_bare_commas_inside_filter_arguments() {
        // 逗号在 filtergraph 里是滤镜分隔符。裁定 R1 选 afade 而非 volume 表达式
        // 正是为了避免 if(lt(t,X),...) 这种带逗号的参数。这条测试钉住这个选择：
        // 如果有人把 afade 换回 volume 表达式，圆括号里就会出现逗号。
        let g = audio_filter_graph(10.0, 16.0);
        let mut depth = 0i32;
        for ch in g.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth > 0 => panic!("滤镜参数的括号内出现逗号，会被当成滤镜分隔符：{g}"),
                _ => {}
            }
        }
        assert_eq!(depth, 0, "括号不配对：{g}");
    }

    #[test]
    fn audio_graph_ends_with_the_mixed_output_label() {
        let g = audio_filter_graph(10.0, 16.0);
        assert!(g.trim_end().ends_with("[a]"), "输出标签应为 [a]：{g}");
    }

    fn sample_inputs() -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        (
            "/m/bg.mp4".into(),
            "/m/audio.mp3".into(),
            "/m/bgm.mp3".into(),
            "/m/typewriter.mp3".into(),
            "/m/intro.mp3".into(),
            "/m/out.mp4".into(),
        )
    }

    fn sample_args() -> Vec<String> {
        let (bg, tts, bgm, tw, intro, out) = sample_inputs();
        build_render_args(&RenderInputs {
            bg: &bg,
            tts_audio: &tts,
            bgm: &bgm,
            typewriter: &tw,
            intro: &intro,
            out: &out,
            total_frames: 600,
            audio_secs: 10.0,
            content_frames: 360,
        })
    }

    /// 取 `args` 里 `flag` 后面紧跟的那个值。
    fn value_after(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    }

    #[test]
    fn inputs_appear_in_the_order_the_filter_graph_assumes() {
        // filter_complex 用 [0:v] 背景、[1:v] 帧流、[2:a] TTS、[3:a] BGM、
        // [4:a] 打字机、[5:a] 片尾音效。-i 的顺序就是编号，错位会静默画错。
        let args = sample_args();
        let inputs: Vec<&String> = args
            .iter()
            .zip(args.iter().skip(1))
            .filter(|(f, _)| *f == "-i")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(inputs.len(), 6, "应有 6 个输入：{args:?}");
        assert_eq!(inputs[0], "/m/bg.mp4");
        assert_eq!(inputs[1], "-", "第二个输入必须是 stdin 帧流");
        assert_eq!(inputs[2], "/m/audio.mp3");
        assert_eq!(inputs[3], "/m/bgm.mp3");
        assert_eq!(inputs[4], "/m/typewriter.mp3");
        assert_eq!(inputs[5], "/m/intro.mp3");
    }

    #[test]
    fn stream_loop_precedes_the_inputs_it_applies_to() {
        // -stream_loop 是输入选项，必须紧接在它要循环的那个 -i 之前。
        // 放错位置会静默失效：背景视频与 BGM 播完就断，而 ffmpeg 不报错。
        let args = sample_args();
        let loops: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "-stream_loop")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(loops.len(), 2, "背景视频与 BGM 都需要循环：{args:?}");
        for i in loops {
            assert_eq!(args[i + 1], "-1", "应是无限循环");
            assert_eq!(args[i + 2], "-i", "-stream_loop 必须紧贴它的 -i：{args:?}");
        }
        // 且这两个 -i 分别是背景视频与 BGM
        let looped: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *a == "-stream_loop" && args.get(i + 3).is_some())
            .map(|(i, _)| &args[i + 3])
            .collect();
        assert!(
            looped.contains(&&"/m/bg.mp4".to_string()),
            "背景视频应被循环：{looped:?}"
        );
        assert!(
            looped.contains(&&"/m/bgm.mp3".to_string()),
            "BGM 应被循环：{looped:?}"
        );
    }

    #[test]
    fn raw_frame_stream_declares_size_rate_and_pixel_format() {
        let args = sample_args();
        assert_eq!(value_after(&args, "-f").as_deref(), Some("rawvideo"));
        assert_eq!(
            value_after(&args, "-pix_fmt").as_deref(),
            Some("rgba"),
            "帧流是 straight-alpha RGBA8"
        );
        assert_eq!(value_after(&args, "-s").as_deref(), Some("1280x720"));
        assert_eq!(value_after(&args, "-r").as_deref(), Some("30"));
    }

    #[test]
    fn background_uses_cover_scaling_and_multiplicative_brightness() {
        let args = sample_args();
        let g = value_after(&args, "-filter_complex").expect("应有 filter_complex");
        assert!(
            g.contains("scale=1280:720:force_original_aspect_ratio=increase"),
            "objectFit:cover 的等价是 increase + crop：{g}"
        );
        assert!(g.contains("crop=1280:720"), "{g}");
        assert!(
            g.contains("colorchannelmixer=rr=0.8:gg=0.8:bb=0.8"),
            "CSS brightness(0.8) 是乘性的：{g}"
        );
        assert!(
            !g.contains("eq=brightness"),
            "eq=brightness 是加性的，语义不同，不得使用：{g}"
        );
    }

    #[test]
    fn background_chain_applies_scale_then_crop_then_brightness_in_order() {
        // 修复轮 1：上面那条测试对 scale/crop/colorchannelmixer 用的是三个
        // 独立的 contains，从不断言相对次序。审查实测：把实现改成
        // 「先 crop 后 scale」，三个 contains 全部照样通过（次序对调不影响
        // 各滤镜「有没有出现」）。但对非 1280x720 的源（背景素材是
        // 1920x1080），先裁后缩会裁错区域，破坏 objectFit:cover 语义——
        // 一类「成片画面裁切错误但测试全绿」的缺陷。这里照 Task 3 的写法，
        // 把整条链当一个连续子串断言，把顺序钉死。
        let args = sample_args();
        let g = value_after(&args, "-filter_complex").unwrap();
        let expected = "scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,colorchannelmixer=rr=0.8:gg=0.8:bb=0.8";
        assert!(
            g.contains(expected),
            "背景处理必须按 scale→crop→colorchannelmixer 的顺序连续出现；先裁后缩会对非 1280x720 的源裁错区域、破坏 cover 语义：{g}"
        );
    }

    #[test]
    fn frame_stream_is_overlaid_on_the_background() {
        let args = sample_args();
        let g = value_after(&args, "-filter_complex").unwrap();
        assert!(
            g.contains("[bg][1:v]overlay=shortest=0[v]"),
            "帧流应叠在处理后的背景之上：{g}"
        );
    }

    #[test]
    fn filter_complex_embeds_the_audio_graph() {
        // 视频与音频合成一条 filter_complex；音频部分由 Task 3 的函数产出。
        let args = sample_args();
        let g = value_after(&args, "-filter_complex").unwrap();
        assert!(
            g.contains(&audio_filter_graph(10.0, 16.0)),
            "应内嵌 audio_filter_graph 的产物：{g}"
        );
    }

    #[test]
    fn outro_start_is_derived_from_content_frames_not_hardcoded() {
        // Outro 起点 = (120 + content_frames) / 30。content_frames=360 → 16.0s。
        // 换一组数验证不是写死的：content_frames=90 → (120+90)/30 = 7.0s。
        let (bg, tts, bgm, tw, intro, out) = sample_inputs();
        let args = build_render_args(&RenderInputs {
            bg: &bg,
            tts_audio: &tts,
            bgm: &bgm,
            typewriter: &tw,
            intro: &intro,
            out: &out,
            total_frames: 330,
            audio_secs: 1.0,
            content_frames: 90,
        });
        let g = value_after(&args, "-filter_complex").unwrap();
        assert!(
            g.contains("adelay=7000:all=1"),
            "Outro 起点应为 7000ms：{g}"
        );
    }

    #[test]
    fn duration_comes_from_total_frames_at_thirty_fps() {
        let args = sample_args();
        assert_eq!(
            value_after(&args, "-t").as_deref(),
            Some("20"),
            "600 帧 / 30fps = 20 秒"
        );
    }

    #[test]
    fn output_encoding_matches_the_spec() {
        let args = sample_args();
        assert!(args.windows(2).any(|w| w[0] == "-c:v" && w[1] == "libx264"));
        assert!(args.windows(2).any(|w| w[0] == "-crf" && w[1] == "23"));
        assert!(
            args.windows(2).any(|w| w[0] == "-c:a" && w[1] == "aac"),
            "mp4 容器需要 aac 音频：{args:?}"
        );
        // 输出的像素格式是 yuv420p（与输入帧流的 rgba 是两回事）
        let pix: Vec<&String> = args
            .iter()
            .zip(args.iter().skip(1))
            .filter(|(f, _)| *f == "-pix_fmt")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(pix, vec!["rgba", "yuv420p"], "输入 rgba、输出 yuv420p：{pix:?}");
        assert_eq!(
            args.last().map(String::as_str),
            Some("/m/out.mp4"),
            "输出路径必须在最后：{args:?}"
        );
    }

    #[test]
    fn maps_only_the_composed_video_and_mixed_audio() {
        let args = sample_args();
        let maps: Vec<&String> = args
            .iter()
            .zip(args.iter().skip(1))
            .filter(|(f, _)| *f == "-map")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(
            maps,
            vec!["[v]", "[a]"],
            "只映射合成后的视频与混合后的音频，不得带上任何原始流：{maps:?}"
        );
    }

    #[test]
    fn overwrites_without_prompting() {
        // 没有 -y 时 ffmpeg 会在目标已存在时交互式询问，管道场景下会挂死。
        assert!(sample_args().contains(&"-y".to_string()));
    }

    #[test]
    fn output_only_options_all_appear_after_the_last_input() {
        // 补充测试（自查发现的缺口）：ffmpeg 的命令行是位置敏感的——输出选项
        // 必须出现在所有 -i 之后，否则会被当成下一个输入的选项，静默改变
        // 行为而不报错。`output_encoding_matches_the_spec` 与
        // `overwrites_without_prompting` 都只用 `contains`/`windows(2).any`
        // 问「有没有」，不问「在哪」：把 `-c:v libx264 -crf 23` 整体挪到
        // 第一个 -i 之前，那两条测试与本文件其余所有测试全部照样通过（实测
        // 验证过）。这里把「最后一个 -i 之后」当成一条硬约束，钉住位置。
        let args = sample_args();
        let last_i_pos = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "-i")
            .map(|(i, _)| i)
            .max()
            .expect("应至少有一个 -i");
        for flag in [
            "-filter_complex",
            "-map",
            "-t",
            "-c:v",
            "-crf",
            "-c:a",
            "-b:a",
        ] {
            let pos = args
                .iter()
                .position(|a| a == flag)
                .unwrap_or_else(|| panic!("缺少 {flag}：{args:?}"));
            assert!(
                pos > last_i_pos,
                "{flag}（位于 {pos}）必须出现在最后一个 -i（位于 {last_i_pos}）之后，\
                 否则会被 ffmpeg 当成输入选项：{args:?}"
            );
        }
        // 输出的 -pix_fmt（第二次出现）同样必须在最后一个 -i 之后；
        // 第一次出现（帧流的 rgba）则必须在它之前。
        let pix_positions: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "-pix_fmt")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(pix_positions.len(), 2);
        assert!(pix_positions[0] < last_i_pos, "输入 -pix_fmt 应在最后一个 -i 之前");
        assert!(pix_positions[1] > last_i_pos, "输出 -pix_fmt 应在最后一个 -i 之后");
    }

    /// 串行化所有依赖「真实 `ffmpeg` 能否通过 `PATH` 解析到」的测试。
    ///
    /// `run_render_returns_assert_available_error_when_ffmpeg_is_missing`
    /// 要临时把进程级 `PATH` 改成不含 ffmpeg 的目录，这个变量是整个进程
    /// 共享的——`cargo test` 默认多线程并发跑测试，如果这时候
    /// `run_render_reports_ffmpeg_stderr_verbatim_when_it_exits_nonzero` /
    /// `run_render_does_not_panic_when_ffmpeg_exits_early` 恰好也在跑（它们
    /// 靠字面量 `"ffmpeg"` 走 `PATH` 查真实 ffmpeg），会撞上被清空的 `PATH`
    /// 假性失败。三条测试都先拿这把锁再动手，串行化掉这段窗口。用假 ffmpeg
    /// 脚本的另外两条测试传的是带 `/` 的绝对路径，`execvp` 语义下根本不查
    /// `PATH`，不受影响，不需要跟着拿锁。
    static REAL_FFMPEG_PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn run_render_reports_ffmpeg_stderr_verbatim_when_it_exits_nonzero() {
        let _guard = REAL_FFMPEG_PATH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 用一个必然失败的参数组合（不存在的输入素材）触发 ffmpeg 非零退出，
        // 确认错误信息把 ffmpeg 自己的话原样透出，而不是吞掉或改写。
        //
        // 输出路径特意放在一个存在且可写的临时目录下（而不是 `/` 根目录下
        // 拼一个不存在的目录）：本机沙箱里当前用户对 `/` 没有写权限，若
        // `out` 的父目录本身建不出来，`run_render` 会在 `create_dir_all`
        // 那一步就先报「创建输出目录失败」，根本走不到 ffmpeg，测不到这
        // 条测试真正想测的东西（ffmpeg 自己的 stderr 有没有被原样透出）。
        // 让 ffmpeg 报错的是不存在的 `bg`/`a`，不是输出路径。
        let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n短。\n";
        let mut fs = crate::render::frame::FrameSource::new(vtt, "标题".into()).unwrap();
        let out_dir = std::env::temp_dir().join("panda_ffmpeg_stderr_test");
        let out = out_dir.join("out.mp4");
        let bg = std::path::Path::new("/nonexistent-bg-xyz.mp4");
        let a = std::path::Path::new("/nonexistent-a-xyz.mp3");
        // 三个数值必须在 &mut fs 之前算好：否则 &mut fs 与 &fs 同时活着，借用检查不过。
        let total_frames = fs.total_frames();
        let audio_secs = fs.audio_secs();
        let content_frames = fs.content_frames();
        let err = run_render(
            &mut fs,
            &RenderInputs {
                bg,
                tts_audio: a,
                bgm: a,
                typewriter: a,
                intro: a,
                out: &out,
                total_frames,
                audio_secs,
                content_frames,
            },
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("ffmpeg"), "错误应指明是 ffmpeg 失败：{msg}");
        // ffmpeg 找不到输入时会说 "No such file or directory"
        assert!(
            msg.contains("No such file") || msg.contains("Invalid"),
            "应原样透出 ffmpeg 的 stderr：{msg}"
        );
        std::fs::remove_dir_all(&out_dir).ok();
    }

    #[test]
    fn run_render_does_not_panic_when_ffmpeg_exits_early() {
        let _guard = REAL_FFMPEG_PATH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // ffmpeg 因参数错误立刻退出时，写帧线程会遇到 broken pipe。
        // 这条测试的全部要求就是：返回 Err，不 panic，不挂死。
        let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:20.000\n够长的一条，保证帧数多到写端会撞上已关闭的管道。\n";
        let mut fs = crate::render::frame::FrameSource::new(vtt, "标题".into()).unwrap();
        let bad = std::path::Path::new("/nonexistent-xyz.mp4");
        // 三个数值必须在 &mut fs 之前算好：否则 &mut fs 与 &fs 同时活着，借用检查不过。
        let total_frames = fs.total_frames();
        let audio_secs = fs.audio_secs();
        let content_frames = fs.content_frames();
        let result = run_render(
            &mut fs,
            &RenderInputs {
                bg: bad,
                tts_audio: bad,
                bgm: bad,
                typewriter: bad,
                intro: bad,
                out: std::path::Path::new("/tmp/panda_early_exit_test.mp4"),
                total_frames,
                audio_secs,
                content_frames,
            },
        );
        assert!(result.is_err(), "应返回 Err");
        std::fs::remove_file("/tmp/panda_early_exit_test.mp4").ok();
    }

    #[test]
    fn run_render_returns_assert_available_error_when_ffmpeg_is_missing() {
        // 鉴别性测试：钉住「`run_render` 真的调用了 `assert_available()`」
        // 这件事本身，补上自审发现的一个盲区。
        //
        // 上面两条 `run_render_*` 测试在本机（ffmpeg 真实存在）跑，无法
        // 区分「有调用 assert_available()」和「删掉这一行」——两种情况下
        // `assert_available()` 都会成功（或者压根没被调用），程序继续往下
        // 走到真正的 spawn，行为完全一样。只有当 ffmpeg 在 PATH 上确实
        // 找不到时，两者才会分道扬镳：有 `assert_available()?` 时报的是
        // 它那句面向用户的安装提示；删掉后，`Command::new("ffmpeg").spawn()`
        // 自己失败，报的是泛泛的「启动 ffmpeg 失败：No such file or
        // directory」——同样是 Err，但说的不是同一件事。
        //
        // 用系统临时目录本身（不含任何 "ffmpeg" 可执行文件）顶替 PATH，
        // 让 `Command::new("ffmpeg")` 无论在哪一步被调用都找不到它。
        let _guard = REAL_FFMPEG_PATH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n短。\n";
        let mut fs = crate::render::frame::FrameSource::new(vtt, "标题".into()).unwrap();
        let bad = std::path::Path::new("/nonexistent-xyz.mp4");
        let total_frames = fs.total_frames();
        let audio_secs = fs.audio_secs();
        let content_frames = fs.content_frames();

        let original_path = std::env::var_os("PATH");
        // SAFETY（就多线程而言）：`REAL_FFMPEG_PATH_LOCK` 保证同一时刻只有
        // 这一条测试在改 PATH；其余会经 PATH 解析 "ffmpeg" 的测试都持有
        // 同一把锁，不会在这段窗口内并发执行。
        unsafe {
            std::env::set_var("PATH", std::env::temp_dir());
        }
        let result = run_render(
            &mut fs,
            &RenderInputs {
                bg: bad,
                tts_audio: bad,
                bgm: bad,
                typewriter: bad,
                intro: bad,
                out: std::path::Path::new("/tmp/panda_missing_ffmpeg_test.mp4"),
                total_frames,
                audio_secs,
                content_frames,
            },
        );
        // 无论断言接下来是否 panic，先把 PATH 恢复原状，不把坏状态泄漏给
        // 同一进程里后续的测试。
        unsafe {
            match &original_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }

        let err = result.expect_err("PATH 上没有 ffmpeg，run_render 应报错");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("请安装 ffmpeg"),
            "应是 assert_available() 那句面向用户的安装提示，说明它真的被调用了；\
             如果这一行被删掉，这里会看到的是 spawn() 自己泛泛的「启动 ffmpeg 失败」：{msg}"
        );
        std::fs::remove_file("/tmp/panda_missing_ffmpeg_test.mp4").ok();
    }

    /// 写一个可执行的假 ffmpeg 脚本到临时目录，返回其路径。仅用于下面两条
    /// 白盒测试：它们要验证的编排细节（并发读 stderr、ffmpeg 成功但写帧
    /// 失败时不吞错误）在真实 ffmpeg 上要么需要填满 64KB 管道缓冲区（约
    /// 4 分钟挂钟时间，见 `docs/ffmpeg-pipeline.md` 第 6 节），要么依赖
    /// ffmpeg 恰好提前关闭 stdin 又恰好退出码 0——都不是能在单测规模下
    /// 稳定复现的条件。用假脚本直接控制这两种行为，比等真实 ffmpeg 巧合
    /// 触发要可靠得多。
    #[cfg(unix)]
    fn write_fake_ffmpeg(name: &str, script: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "panda_fake_ffmpeg_{name}_{}",
            std::process::id()
        ));
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn run_render_reads_stderr_concurrently_with_writing_frames_and_does_not_deadlock() {
        // 鉴别性测试：假 ffmpeg 先往 stderr 塞 200000 字节（远超 64KB 管道
        // 缓冲区），再读 stdin，最后退出 0。
        //
        // 如果 stderr 的读取不是与写帧并发进行（比如被挪到 `wait()` 之后
        // 才读），这里会真挂死：假 ffmpeg 卡在写 stderr（没人读、缓冲区
        // 写满），于是它永远读不到 stdin；我们的主线程又卡在把帧写进
        // stdin（同样没人读、缓冲区写满）。双方互相等待，谁也不会先撒手。
        // 用超时代替死等，超时本身就是「抓到了」的证据。
        //
        // 超时定得比较宽（30 秒）：实测过，本 debug 构建下渲染全时间轴
        // 330 帧（Cover+Intro+Content+Outro 四段都要走一遍 `Painter`）
        // 本身就要约 12～13 秒（`[profile.dev.package."*"]` 只优化了依赖，
        // 没优化本 crate 自己的代码），这是正常但慢的成功路径，不是死锁；
        // 真死锁会一直卡到进程被杀，30 秒对两者的区分足够。
        let script = write_fake_ffmpeg(
            "stderr_then_stdin",
            "#!/bin/sh\nhead -c 200000 /dev/zero | tr '\\0' 'x' 1>&2\ncat >/dev/null\nexit 0\n",
        );
        let script_for_cleanup = script.clone();
        let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n短。\n";
        let mut fs = crate::render::frame::FrameSource::new(vtt, "标题".into()).unwrap();
        let bad = std::path::Path::new("/nonexistent-xyz.mp4");
        let out = std::env::temp_dir().join(format!(
            "panda_stderr_deadlock_test_{}.mp4",
            std::process::id()
        ));
        let total_frames = fs.total_frames();
        let audio_secs = fs.audio_secs();
        let content_frames = fs.content_frames();

        // out 是拥有所有权的局部值，线程要求闭包内容 'static；把 out（以及
        // fs）整个移进闭包，在闭包内部再借用，而不是从外面借一个短命的引用。
        let (tx, rx) = std::sync::mpsc::channel();
        let out_for_join = out.clone();
        std::thread::spawn(move || {
            let inputs = RenderInputs {
                bg: bad,
                tts_audio: bad,
                bgm: bad,
                typewriter: bad,
                intro: bad,
                out: &out,
                total_frames,
                audio_secs,
                content_frames,
            };
            let result = run_render_with_ffmpeg_binary(&script, &mut fs, &inputs);
            let _ = tx.send(result.map(|_| ()).map_err(|e| format!("{e:#}")));
        });

        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(result) => assert!(
                result.is_ok(),
                "假 ffmpeg 正常读完 stdin 后退出 0，不应报错：{result:?}"
            ),
            Err(_) => panic!(
                "30 秒内 run_render 未返回，疑似死锁：stderr 没有被并发读取，\
                 假 ffmpeg 卡在写 stderr、我们卡在写 stdin，双方互相等待对方"
            ),
        }
        std::fs::remove_file(&out_for_join).ok();
        std::fs::remove_file(&script_for_cleanup).ok();
    }

    #[test]
    #[cfg(unix)]
    fn run_render_reports_write_failure_even_when_ffmpeg_exits_zero() {
        // 鉴别性测试：假 ffmpeg 只读 100 字节就退出 0——模拟"ffmpeg 提前
        // 收尾、退出码是 0，但我们这边还有帧没写完"的场景（现实中对应
        // ffmpeg 意外提前结束但没有报错退出的情况）。
        //
        // 这条测试专门堵住 `run_render` 末尾最容易被写错的一行：如果把
        // `write_result.map(|_| ())` 改成无条件 `Ok(())`，ffmpeg 退出码
        // 是 0 会让这条测试从「返回 Err」变成「返回 Ok」——静默产出一个
        // 帧数被截断的坏成片，且没有任何报错。
        let script = write_fake_ffmpeg(
            "read_100_bytes_then_exit",
            "#!/bin/sh\nhead -c 100 >/dev/null\nexit 0\n",
        );
        // 字幕够长，保证 total_frames 对应的字节数远大于假 ffmpeg 会读的 100 字节。
        let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:05.000\n足够长，保证多于一帧要写，写端会在假 ffmpeg 提前退出后撞上已关闭的管道。\n";
        let mut fs = crate::render::frame::FrameSource::new(vtt, "标题".into()).unwrap();
        let bad = std::path::Path::new("/nonexistent-xyz.mp4");
        let out = std::env::temp_dir().join(format!(
            "panda_write_swallow_test_{}.mp4",
            std::process::id()
        ));
        let inputs = RenderInputs {
            bg: bad,
            tts_audio: bad,
            bgm: bad,
            typewriter: bad,
            intro: bad,
            out: &out,
            total_frames: fs.total_frames(),
            audio_secs: fs.audio_secs(),
            content_frames: fs.content_frames(),
        };

        let err = run_render_with_ffmpeg_binary(&script, &mut fs, &inputs)
            .expect_err("假 ffmpeg 提前关闭 stdin 后写帧应失败，即使它自己退出码是 0");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("管道") || msg.contains("pipe") || msg.contains("Broken"),
            "错误应指出是写帧/管道失败，而不是别的：{msg}"
        );
        assert!(
            !msg.contains("退出码"),
            "假 ffmpeg 退出码是 0，不应误报成 ffmpeg 非零退出：{msg}"
        );
        std::fs::remove_file(&out).ok();
        std::fs::remove_file(&script).ok();
    }

    #[test]
    #[cfg(unix)]
    fn run_render_creates_the_output_directory_before_invoking_ffmpeg() {
        // 鉴别性测试：钉住 `create_dir_all` 这一步本身，补上自审发现的
        // 另一个盲区——`run_render_reports_ffmpeg_stderr_verbatim_when_it_
        // exits_nonzero` 虽然也用了一个不存在的输出目录，但它的 `bg`/`a`
        // 同样不存在，ffmpeg 会先在读取输入那一步就失败，根本走不到
        // 「打开输出路径写入」这一步，测不出 `create_dir_all` 有没有跑。
        //
        // 假 ffmpeg 会先把 stdin 读空（避免写端卡住），再取 argv 最后一个
        // 参数（`build_render_args` 里输出路径永远是最后一项）尝试建一个
        // 空文件——这正是真实 ffmpeg 打开输出文件写入时会做的事：目录
        // 不存在就失败。成败完全取决于 `run_render` 有没有先把父目录
        // 建好，和输入素材是否存在无关。
        let script = write_fake_ffmpeg(
            "touch_output_path",
            "#!/bin/sh\ncat >/dev/null\nfor out; do :; done\n: > \"$out\" || exit 3\nexit 0\n",
        );
        let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n短。\n";
        let mut fs = crate::render::frame::FrameSource::new(vtt, "标题".into()).unwrap();
        let bad = std::path::Path::new("/nonexistent-xyz.mp4");
        // 特意让输出的父目录（两层，逼 create_dir_all 而不是单层 mkdir）
        // 在测试开始前不存在。
        let out_dir = std::env::temp_dir().join(format!(
            "panda_create_dir_test_{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&out_dir).ok();
        let out = out_dir.join("nested").join("out.mp4");
        let inputs = RenderInputs {
            bg: bad,
            tts_audio: bad,
            bgm: bad,
            typewriter: bad,
            intro: bad,
            out: &out,
            total_frames: fs.total_frames(),
            audio_secs: fs.audio_secs(),
            content_frames: fs.content_frames(),
        };

        let result = run_render_with_ffmpeg_binary(&script, &mut fs, &inputs);
        assert!(
            result.is_ok(),
            "输出目录应已被建好，假 ffmpeg 应能顺利在那里创建文件并退出 0：{result:?}"
        );
        assert!(out.exists(), "假 ffmpeg 应已在正确路径创建了文件，说明目录确实建好了");

        std::fs::remove_dir_all(&out_dir).ok();
        std::fs::remove_file(&script).ok();
    }
}
