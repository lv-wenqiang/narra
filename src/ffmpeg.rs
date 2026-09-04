use crate::render::canvas::Canvas;
use crate::render::timeline::{COVER_FRAMES, FPS, INTRO_FRAMES};
use anyhow::{Context, Result, bail};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 探测 PATH 上是否有可用的 ffmpeg。对应 TS 版 assertFfmpegAvailable。
pub fn assert_available() -> Result<()> {
    let out = Command::new("ffmpeg").arg("-version").output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        _ => bail!("本步骤需要 ffmpeg。请安装 ffmpeg 并确保它在 PATH 上。"),
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
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            &list_path.to_string_lossy(),
            "-vn",
            "-filter:a",
            &format!("atempo={speed}"),
            "-c:a",
            "libmp3lame",
            "-q:a",
            "2",
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

/// Content 段起点（秒）= Cover + Intro 帧数 ÷ FPS（15 + 105 = 120 帧 = 4.0s）。
///
/// **必须由 `timeline.rs` 的段落常量推导，不能写字面量 `4.0`**：段落边界是
/// `timeline::layout`/`segment_at` 与本模块的滤镜图共用的同一件事实，写两份
/// 时改一份不改另一份既不会编译失败、也不会有任何测试变红（`timeline.rs`
/// 的测试断言 15/105 的字面量，本模块的测试断言 `adelay=4000` 的字面量），
/// 结果是成片里音效相对画面段落整体错位、而成片照样能播。
const CONTENT_START_SECS: f64 = (COVER_FRAMES + INTRO_FRAMES) as f64 / FPS as f64;
/// Intro 段起点（秒）= Cover 帧数 ÷ FPS（15 帧 = 0.5s）。理由同上。
const INTRO_START_SECS: f64 = COVER_FRAMES as f64 / FPS as f64;
/// 混音格式：**每一路在进 `amix` 之前都要先过它**。
///
/// 四路输入的采样率与声道数各不相同（TTS 24kHz 单声道、BGM 48kHz 立体声、
/// 打字机 24kHz 立体声、片尾音效 44.1kHz 立体声），而 `amix` 要求所有输入
/// 格式一致，libavfilter 会自行协商出一个公共格式并插入重采样。**不干预时
/// 协商结果被最低的那一路拉到 24kHz 单声道**：三路立体声素材被砍掉 12kHz
/// 以上的全部频段、丢掉立体声像，且下混用的是功率保持系数（每声道 ≈0.707
/// 而非算术平均 0.5），使 BGM 与音效比规格 §9.3 的 `volume=0.15`/`0.6` 字面
/// 值响约 3dB——而**成片照样能播、ffmpeg 也不报任何警告**。
///
/// **为什么不是在输出侧写 `-ar 48000 -ac 2`**：那条路实测**无效且会掩盖
/// 问题**。`ffmpeg -v verbose` 打出的自动插入点显示，`-ar`/`-ac` 不会经
/// `amix` 反向传播到它的四路输入上，转换发生在 `amix` **之后**：
///
/// ```text
/// [auto_aresample_0] ch:2 chl:stereo r:48000Hz -> ch:1 chl:mono r:24000Hz
/// [Parsed_amix]      inputs:4 fmt:fltp srate:24000 cl:mono
/// [auto_aresample_3] ch:1 chl:mono r:24000Hz -> ch:2 chl:stereo r:48000Hz
/// ```
///
/// 产物的 `ffprobe` 会显示 `sample_rate=48000 channels=2`，而它只是一份
/// 24kHz 单声道混音的升采样：12kHz 以上依然空无一物，两个声道逐样本相同
/// （实测 L−R 差信号 = −91.0 dB 数字静音）。**它把唯一能发现问题的信号
/// （ffprobe 读数）伪造成了正确的**，所以这里不写 `-ar`/`-ac`：让产物的
/// 采样率与声道数如实反映滤镜图真正协商出的格式。
///
/// 每路前置 `aformat` 后，`amix` 实际运行在 `srate:48000 cl:stereo`，BGM
/// 一路零转换，两段音效只升采样、保住立体声，只有单声道 TTS 被上混。
const MIX_FORMAT: &str = "aformat=sample_rates=48000:channel_layouts=stereo";
/// TTS 那一路的格式统一：重采样到 48kHz，再用 `pan` 把单声道**原样**铺到
/// 两个声道。
///
/// **为什么不能和另外三路共用 [`MIX_FORMAT`]**：`channel_layouts=stereo` 交给
/// swresample 做单声道→立体声上混，用的是和下混同一套**功率保持**系数，实测
/// 恰好 −3.01 dB。于是规格 §9.3 逐字写死的 `volume=1` 名不副实——真正进
/// `amix` 的 TTS 是 0.707，整条音轨比应有电平低约 3 dB。失败形式是纯粹的
/// 音量偏差：不报错、不失真、不削顶，ffprobe 的采样率/声道数读数还完全正确。
///
/// 实测（ffmpeg 8.1.2，四路同真实生产格式，其余三路静音以隔离 TTS，PCM 输出
/// 以避开 aac 编码噪声，对比单声道源的 `mean_volume`）：
///
/// | TTS 这一路 | `amix` 协商结果 | 相对单声道源 |
/// |---|---|---|
/// | `aformat=…:channel_layouts=stereo` | 48000 / stereo | −3.0 dB |
/// | `pan=stereo|c0=c0|c1=c0`（仅此一条） | 48000 / stereo | ±0.0 dB |
/// | 本常量（`aformat` + `pan`） | 48000 / stereo | ±0.0 dB |
///
/// **为什么仍保留 `sample_rates=48000`**：`pan` 只改声道布局，不改采样率。
/// 只写 `pan` 也能跑通——但那是靠**另外三路**把 `amix` 的协商拉上去的，
/// 这一路自己不再声明采样率，[`MIX_FORMAT`] 文档里那个「协商被最低的一路
/// 拉走」就少了一道明示的防线。多这一个滤镜的代价是零（实测零转换）。
///
/// **`c0=c0|c1=c0` 假设输入是单声道**：由 `tts::edge` 的
/// `OUTPUT_FORMAT = "audio-24khz-48kbitrate-mono-mp3"` 结构性保证。若将来换成
/// 立体声的 TTS 后端，这里会把左声道复制到两边、丢掉右声道，必须一并改。
const TTS_MIX_FORMAT: &str = "aformat=sample_rates=48000,pan=stereo|c0=c0|c1=c0";
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
/// **每一路都以 `MIX_FORMAT` 开头**（私有常量，故意不做 intra-doc 链接：链到
/// 私有项会让 `cargo rustdoc` 报 `private_intra_doc_links` 告警）：四路素材的
/// 采样率/声道数各不相同，不
/// 显式统一时 `amix` 的格式协商会把整条混音链拉到 24kHz 单声道。理由与
/// 「为什么不能改在输出侧写 `-ar`/`-ac`」的实测证据见该常量自己的文档。
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
        "[2:a]{TTS_MIX_FORMAT},adelay={content_ms}:all=1,volume=1[a_tts];\
         [3:a]{MIX_FORMAT},adelay={content_ms}:all=1,volume={BGM_VOLUME},\
         afade=t=out:st={fade_start:.3}:d={BGM_FADE_SECS}[a_bgm];\
         [4:a]{MIX_FORMAT},adelay={intro_ms}:all=1,volume={SFX_VOLUME}[a_type];\
         [5:a]{MIX_FORMAT},adelay={outro_ms}:all=1,volume={SFX_VOLUME}[a_intro];\
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
    /// 本次渲染的画布尺寸：决定 `-s`（stdin 帧流的宽高）与背景视频
    /// `scale`/`crop` 滤镜的目标尺寸。
    pub canvas: Canvas,
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
    let outro_start_secs = (CONTENT_START_SECS * FPS as f64 + i.content_frames as f64) / FPS as f64;
    let total_secs = i.total_frames as f64 / FPS as f64;

    let (w, h) = (i.canvas.w, i.canvas.h);
    let filter = format!(
        "[0:v]scale={w}:{h}:force_original_aspect_ratio=increase,\
         crop={w}:{h},colorchannelmixer=rr=0.8:gg=0.8:bb=0.8[bg];\
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
        format!("{w}x{h}"),
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
        // `{:.3}`（毫秒精度）而非默认 `Display`：`total_frames` 不是 30 的
        // 倍数时 `total_frames / 30.0` 会吐出 `20.033333333333335` 这样的
        // 十几位小数——ffmpeg 能接受，但难读也难断言。与同一条命令行里
        // `afade` 的 `st` 统一口径。
        "-t".into(),
        format!("{total_secs:.3}"),
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
/// `run_render_with_ffmpeg_binary`（私有，故意不做 intra-doc 链接：链到私有
/// 项会让 `cargo rustdoc` 报 `private_intra_doc_links` 告警）注入假进程验证
/// 编排细节。
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
        assert!(
            g.contains("adelay=4000:all=1"),
            "TTS/BGM 应延迟 4000ms：{g}"
        );
        assert!(g.contains("adelay=500:all=1"), "打字机应延迟 500ms：{g}");
        assert!(
            g.contains("adelay=16000:all=1"),
            "片尾音效应延迟到 Outro 起点 16000ms：{g}"
        );
    }

    /// 从滤镜图里取出某条输入支路的 `adelay` 毫秒数。
    ///
    /// 只认「流选择器紧跟 `adelay=`」这个形状——本模块生成的滤镜图里每条支路
    /// 都以它开头；形状变了这里会 panic 而不是悄悄返回一个错的数字。
    fn delay_ms_of(graph: &str, selector: &str) -> u64 {
        let after = graph
            .split(selector)
            .nth(1)
            .unwrap_or_else(|| panic!("滤镜图里找不到 {selector}：{graph}"));
        // 三路立体声素材走 MIX_FORMAT，单声道的 TTS 走 TTS_MIX_FORMAT——
        // 两者都必须紧跟 adelay=，认哪一个由支路自己决定，不接受第三种形状。
        let after = [MIX_FORMAT, TTS_MIX_FORMAT]
            .iter()
            .find_map(|prefix| after.strip_prefix(*prefix))
            .and_then(|r| r.strip_prefix(","))
            .and_then(|r| r.strip_prefix("adelay="))
            .unwrap_or_else(|| {
                panic!(
                    "{selector} 之后不是 {MIX_FORMAT},adelay= 或 {TTS_MIX_FORMAT},adelay=：{graph}"
                )
            });
        after
            .split(':')
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{selector} 的 adelay 值不是整数：{graph}"))
    }

    /// **跨模块一致性测试**：`timeline` 报出的段落起始帧，换算成毫秒后必须
    /// 逐一等于滤镜图里对应支路的 `adelay`。
    ///
    /// 为什么需要单独一条：段落边界在两个模块里各有用途——`timeline.rs` 的
    /// `COVER_FRAMES`/`INTRO_FRAMES` 决定画面在第几帧切段，`ffmpeg.rs` 的
    /// `INTRO_START_SECS`/`CONTENT_START_SECS` 决定音效延迟多少毫秒进来。修复
    /// 前两边各写一份字面量，本模块的测试断言 `adelay=500`/`adelay=4000`、
    /// `timeline` 的测试断言 15/105，**没有任何一条测试把两者摆在一起比**：把
    /// `COVER_FRAMES` 改成 30（Cover 段变成 1 秒）而 `INTRO_START_SECS` 仍是
    /// 0.5，结果是打字机音效落在 Intro 段开始之前 0.5 秒，而成片照样能播。
    ///
    /// 本测试不断言任何字面量（15/105/500/4000 一个都不出现），断言的是两个
    /// 模块之间的**关系**：段落边界怎么改都行，改完两边必须还对得上。
    #[test]
    fn segment_starts_match_the_audio_delays_in_the_filter_graph() {
        use crate::render::timeline::{Segment, layout, segment_at};

        let audio_secs = 10.0;
        let l = layout(audio_secs);
        let first_frame_of = |want: Segment| -> u32 {
            (0..l.total_frames)
                .find(|f| segment_at(&l, *f).map(|(seg, _)| seg) == Some(want))
                .unwrap_or_else(|| panic!("时间轴里应至少有一帧属于 {want:?}"))
        };

        let intro_start = first_frame_of(Segment::Intro);
        let content_start = first_frame_of(Segment::Content);
        let outro_start = first_frame_of(Segment::Outro);

        let g = audio_filter_graph(audio_secs, outro_start as f64 / FPS as f64);
        let ms_of_frame = |f: u32| u64::from(f) * 1000 / u64::from(FPS);

        assert_eq!(
            delay_ms_of(&g, "[4:a]"),
            ms_of_frame(intro_start),
            "打字机音效的 adelay 必须等于 Intro 段起始帧（{intro_start}）换算的毫秒：{g}"
        );
        assert_eq!(
            delay_ms_of(&g, "[2:a]"),
            ms_of_frame(content_start),
            "TTS 的 adelay 必须等于 Content 段起始帧（{content_start}）换算的毫秒：{g}"
        );
        assert_eq!(
            delay_ms_of(&g, "[3:a]"),
            ms_of_frame(content_start),
            "BGM 的 adelay 必须等于 Content 段起始帧（{content_start}）换算的毫秒：{g}"
        );
        assert_eq!(
            delay_ms_of(&g, "[5:a]"),
            ms_of_frame(outro_start),
            "片尾音效的 adelay 必须等于 Outro 段起始帧（{outro_start}）换算的毫秒：{g}"
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
        assert!(
            !g.contains("st=8"),
            "st=8 说明漏掉了 Content 起点的 +4.0 偏移：{g}"
        );
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
        assert_eq!(
            g.matches("volume=0.6").count(),
            2,
            "两段音效都应是 0.6：{g}"
        );
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
            g.contains(&format!("[2:a]{TTS_MIX_FORMAT},adelay=4000:all=1")),
            "TTS 应在 [2:a] 上应用 4000ms 延迟：{g}"
        );
        assert!(
            g.contains(&format!("[3:a]{MIX_FORMAT},adelay=4000:all=1")),
            "BGM 应在 [3:a] 上应用 4000ms 延迟：{g}"
        );
        assert!(
            g.contains(&format!("[4:a]{MIX_FORMAT},adelay=500:all=1")),
            "打字机应在 [4:a] 上应用 500ms 延迟，而非接到片尾音效的延迟：{g}"
        );
        assert!(
            g.contains(&format!("[5:a]{MIX_FORMAT},adelay=16000:all=1")),
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
            (
                "TTS",
                format!("[2:a]{TTS_MIX_FORMAT},adelay=4000:all=1,volume=1[a_tts]"),
            ),
            (
                "BGM",
                format!(
                    "[3:a]{MIX_FORMAT},adelay=4000:all=1,volume=0.15,\
                     afade=t=out:st=12.000:d=2[a_bgm]"
                ),
            ),
            (
                "打字机",
                format!("[4:a]{MIX_FORMAT},adelay=500:all=1,volume=0.6[a_type]"),
            ),
            (
                "片尾音效",
                format!("[5:a]{MIX_FORMAT},adelay=16000:all=1,volume=0.6[a_intro]"),
            ),
        ] {
            assert!(
                g.contains(&chain),
                "{label} 这一路的完整链应原样出现：\n期望 {chain}\n实得 {g}"
            );
        }
    }

    /// 四路都必须在 `amix` **之前**统一到 48kHz 立体声。
    ///
    /// 不统一时 `amix` 的格式协商会被最低的那一路（TTS 24kHz 单声道）拉着
    /// 走，实测 `amix` 会运行在 `srate:24000 cl:mono`：三路立体声素材被砍掉
    /// 12kHz 以上的全部频段、丢掉立体声像，且下混用功率保持系数（≈0.707）
    /// 让 BGM/音效比规格 §9.3 的 volume 字面值响约 3dB。成片照样能播、
    /// ffmpeg 一句警告也没有——只能靠 ffprobe 看采样率才发现。
    ///
    /// 断言「每一路的流选择器紧跟它**该用的那个**归一化前缀」而不是「图里有
    /// 4 个 aformat」：后者对「漏掉某一路、另一路写了两遍」无感，而漏掉的
    /// 那一路正是会把整条混音链拉回 24kHz 单声道的那一路。
    ///
    /// **四路并不共用同一个前缀**：三路立体声素材用 [`MIX_FORMAT`]，单声道的
    /// TTS 用 [`TTS_MIX_FORMAT`]（`pan` 上混，不衰减 3.01 dB，理由见该常量）。
    /// 所以「统一到 48kHz」这一半按 `sample_rates=48000` 计数——它是两个前缀
    /// 的共同部分，也是防止协商被拉走的那一道；「统一到立体声」这一半由每路
    /// 各自的前缀断言覆盖。
    #[test]
    fn every_branch_is_normalized_to_forty_eight_k_stereo_before_amix() {
        let g = audio_filter_graph(10.0, 16.0);
        for (sel, prefix) in [
            ("[2:a]", TTS_MIX_FORMAT),
            ("[3:a]", MIX_FORMAT),
            ("[4:a]", MIX_FORMAT),
            ("[5:a]", MIX_FORMAT),
        ] {
            assert!(
                g.contains(&format!("{sel}{prefix},")),
                "{sel} 这一路必须先过 {prefix} 再进 amix：{g}"
            );
        }
        assert_eq!(
            g.matches(RATE_DECL).count(),
            4,
            "四路各声明一次 48kHz，不多不少：{g}"
        );
        // 归一化必须在 amix 之前——写在 amix 之后只是给混完的结果补一次
        // 升采样，改不了混音本身的格式。
        let amix_pos = g.find("amix=").expect("应有 amix");
        assert!(
            g.rfind(RATE_DECL).unwrap() < amix_pos,
            "所有归一化都必须出现在 amix 之前：{g}"
        );
    }

    /// 两个归一化前缀的共同部分：把这一路自己的采样率钉在 48kHz。
    ///
    /// 单独抽出来，是为了让上面那条测试按「声明了 48kHz 的支路数」计数——
    /// 四路里有两种前缀，按任一个前缀计数都只能数到它那几路。
    const RATE_DECL: &str = "sample_rates=48000";

    #[test]
    fn tts_branch_upmixes_mono_with_pan_not_channel_layouts() {
        let g = audio_filter_graph(10.0, 16.0);
        let tts = branch_of(&g, "[2:a]");
        assert!(
            tts.contains("pan=stereo|c0=c0|c1=c0"),
            "TTS 这一路应用 pan 显式上混：{tts}"
        );
        assert!(
            !tts.contains("channel_layouts=stereo"),
            "TTS 这一路不得用 channel_layouts=stereo 上混——会静默衰减 3.01 dB：{tts}"
        );
        assert!(
            tts.contains("sample_rates=48000"),
            "TTS 这一路仍须自己声明 48kHz，不能只靠另外三路把协商拉上去：{tts}"
        );
    }

    /// 取出某条输入支路的完整滤镜链（从流选择器到该支路的 `;` 为止）。
    ///
    /// 断言「某个片段出现在**这一路**里」而不是「出现在整张图里某处」——
    /// 后者对四路之间的调包无感，本模块已经为此栽过两次（M11、修复轮 1）。
    fn branch_of<'a>(graph: &'a str, selector: &str) -> &'a str {
        let start = graph
            .find(selector)
            .unwrap_or_else(|| panic!("滤镜图里找不到 {selector}：{graph}"));
        let rest = &graph[start..];
        match rest.find(';') {
            Some(end) => &rest[..end],
            None => rest,
        }
    }

    /// **非整秒的 `content_frames`**：把 `adelay` 的取整口径钉死。
    ///
    /// 销 `docs/follow-ups.md`「ffmpeg 合成 · 取整口径」：`adelay` 的毫秒用
    /// `.round() as i64`，`afade` 的 `st` 与 `-t` 用 `{:.3}`。两者在整秒输入下
    /// 结果**相同**，而此前所有测试的 `content_frames` 全是 30 的倍数（360、
    /// 90…），Outro 起点因此总落在整秒上——把 `.round()` 换成截断
    /// （`as i64` 直接丢小数）不会有任何测试变红。
    ///
    /// 这里取 `content_frames = 361`（真实场景：`A = 10.01s` →
    /// `ceil((10.01 + 2) × 30) = 361`）：
    ///
    /// - Outro 起点 = `(120 + 361) / 30` = `16.0333…s` → `16033.33…ms`
    /// - **四舍五入** → `16033`；**截断** → `16033`（这一位分不出来）
    ///
    /// 所以还要一个小数部分 ≥ 0.5 的用例：`content_frames = 362` →
    /// `(120 + 362) / 30 = 16.0666…s` → `16066.66…ms`，四舍五入得 **16067**、
    /// 截断得 **16066**。两条合起来才真正钉住口径。
    #[test]
    fn adelay_rounds_rather_than_truncates_on_non_integer_seconds() {
        // 小数部分 < 0.5：两种口径同值，作为对照说明这一条本身不足以区分。
        let g = audio_filter_graph(10.01, (120.0 + 361.0) / 30.0);
        assert_eq!(
            delay_ms_of(&g, "[5:a]"),
            16033,
            "content_frames=361 → Outro 起点 16.0333…s → 16033ms：{g}"
        );

        // 小数部分 > 0.5：四舍五入 16067，截断 16066——这一条能区分。
        let g = audio_filter_graph(10.08, (120.0 + 362.0) / 30.0);
        assert_eq!(
            delay_ms_of(&g, "[5:a]"),
            16067,
            "content_frames=362 → Outro 起点 16.0666…s，应四舍五入到 16067 而非截断到 16066：{g}"
        );

        // 再取一个小数部分恰好 .5 的：content_frames = 363 →
        // (120+363)/30 = 16.1s → 16100ms，两种口径同值；这里断言的是
        // 「毫秒数不带小数」，即 `adelay` 的参数始终是整数。
        let g = audio_filter_graph(10.1, (120.0 + 363.0) / 30.0);
        assert_eq!(delay_ms_of(&g, "[5:a]"), 16100, "{g}");
    }

    /// 非整秒输入下 `afade` 的 `st` 保持毫秒精度的定长格式，不被取整。
    ///
    /// 与上面那条成对：`adelay` 取整到毫秒、`afade` 的 `st` 保留三位小数，
    /// 这是两种**不同**的口径，各管各的。`A = 10.01` → BGM 淡出起点
    /// `4.0 + 10.01 - 2.0 = 12.01`，若有人把 `st` 也按 `adelay` 那样取整成秒，
    /// 会得到 `st=12` —— 淡出提前 10ms 开始，听不出来，但口径就散了。
    #[test]
    fn afade_start_keeps_millisecond_precision_on_non_integer_seconds() {
        let g = audio_filter_graph(10.01, (120.0 + 361.0) / 30.0);
        assert!(
            g.contains("afade=t=out:st=12.010:d=2"),
            "BGM 淡出起点应为 4.0 + 10.01 - 2.0 = 12.010（三位小数定长）：{g}"
        );
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
            canvas: Canvas::BASE,
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
        assert_eq!(
            value_after(&args, "-s").as_deref(),
            Some(format!("{}x{}", Canvas::BASE.w, Canvas::BASE.h).as_str())
        );
        assert_eq!(value_after(&args, "-r").as_deref(), Some("30"));
    }

    /// **Fix round 1（画布链变异验证）**：`build_render_args` 必须按传入的
    /// `i.canvas` 拼 `-s` 与 `scale=`/`crop=`，而不是隐式假定 `Canvas::BASE`。
    ///
    /// 此前仓库里全部 `RenderInputs` 都填 `canvas: Canvas::BASE`，所以
    /// 「把 `build_render_args` 里的 `i.canvas` 换回 `Canvas::BASE`」这个变异
    /// 能骗过全部 250 条测试——`-s`/`scale=`/`crop=` 用的期望值本来就等于
    /// `Canvas::BASE`，测的是「等于 BASE」而不是「等于传进去的画布」。这里
    /// 特意用一个非 BASE 的真实目标尺寸（Plan B 的竖版 1920x1080）、且期望
    /// 字符串写成字面量而不是从 `canvas` 反推——两边都从 `canvas` 算，变异
    /// 会两边一起错，测不出来。
    #[test]
    fn build_render_args_follows_the_canvas_field_not_base() {
        let (bg, tts, bgm, tw, intro, out) = sample_inputs();
        let args = build_render_args(&RenderInputs {
            bg: &bg,
            tts_audio: &tts,
            bgm: &bgm,
            typewriter: &tw,
            intro: &intro,
            out: &out,
            total_frames: 600,
            audio_secs: 10.0,
            content_frames: 360,
            canvas: Canvas { w: 1920, h: 1080 },
        });
        assert_eq!(
            value_after(&args, "-s").as_deref(),
            Some("1920x1080"),
            "帧流尺寸应跟着 i.canvas 走，不是写死的 BASE：{args:?}"
        );
        let g = value_after(&args, "-filter_complex").expect("应有 filter_complex");
        assert!(
            g.contains("scale=1920:1080"),
            "背景 scale 应跟着 i.canvas 走：{g}"
        );
        assert!(
            g.contains("crop=1920:1080"),
            "背景 crop 应跟着 i.canvas 走：{g}"
        );
    }

    #[test]
    fn background_uses_cover_scaling_and_multiplicative_brightness() {
        let args = sample_args();
        let g = value_after(&args, "-filter_complex").expect("应有 filter_complex");
        let (w, h) = (Canvas::BASE.w, Canvas::BASE.h);
        assert!(
            g.contains(&format!(
                "scale={w}:{h}:force_original_aspect_ratio=increase"
            )),
            "objectFit:cover 的等价是 increase + crop：{g}"
        );
        assert!(g.contains(&format!("crop={w}:{h}")), "{g}");
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
        let (w, h) = (Canvas::BASE.w, Canvas::BASE.h);
        let expected = format!(
            "scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},colorchannelmixer=rr=0.8:gg=0.8:bb=0.8"
        );
        assert!(
            g.contains(&expected),
            "背景处理必须按 scale→crop→colorchannelmixer 的顺序连续出现；先裁后缩会对非 {w}x{h} 的源裁错区域、破坏 cover 语义：{g}"
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
            canvas: Canvas::BASE,
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
            Some("20.000"),
            "600 帧 / 30fps = 20 秒（毫秒精度定长格式）"
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
        assert!(
            args.windows(2).any(|w| w[0] == "-b:a" && w[1] == "192k"),
            "音频码率 192k：{args:?}"
        );
        // 输出侧**不得**出现 -ar/-ac：它们不会经 amix 反向传播（实测见
        // MIX_FORMAT 的文档），只会在 amix 之后补一次升采样，把一份 24kHz
        // 单声道混音包装成"ffprobe 看起来是 48kHz 立体声"的产物——恰恰把
        // 唯一能发现问题的读数伪造成正确的。混音格式由每路的 aformat 决定。
        assert!(
            !args.iter().any(|a| a == "-ar" || a == "-ac"),
            "混音格式必须由每路前置的 aformat 决定，输出侧写 -ar/-ac 只会掩盖协商结果：{args:?}"
        );
        // 输出的像素格式是 yuv420p（与输入帧流的 rgba 是两回事）
        let pix: Vec<&String> = args
            .iter()
            .zip(args.iter().skip(1))
            .filter(|(f, _)| *f == "-pix_fmt")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(
            pix,
            vec!["rgba", "yuv420p"],
            "输入 rgba、输出 yuv420p：{pix:?}"
        );
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
        assert!(
            pix_positions[0] < last_i_pos,
            "输入 -pix_fmt 应在最后一个 -i 之前"
        );
        assert!(
            pix_positions[1] > last_i_pos,
            "输出 -pix_fmt 应在最后一个 -i 之后"
        );
    }

    #[test]
    /// **这条同时承担「ffmpeg 提前退出时写端不 panic、不挂死」那份职责。**
    /// 曾经另有一条 `run_render_does_not_panic_when_ffmpeg_exits_early` 专测
    /// 它，用同一套坏输入、只断言 `result.is_err()`——经七次变异零响应，被
    /// 判定为实证零鉴别力后删除（`docs/follow-ups.md` 已销账）。本条用同一条
    /// 代码路径（ffmpeg 因找不到输入立刻退出 → 写帧端撞上已关闭的管道），
    /// 却断言更强的性质：错误必须原样透出 ffmpeg 的 stderr。若写端真的
    /// panic 或挂死，本条一样会 panic 或超时——「不 panic」是它的前提，
    /// 不需要再单列一条只断言这个前提的测试。
    fn run_render_reports_ffmpeg_stderr_verbatim_when_it_exits_nonzero() {
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
        let mut fs = crate::render::frame::FrameSource::new(
            vtt,
            "标题".into(),
            &crate::config::Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        let out_dir_guard =
            crate::tmp::TempPath::new(std::env::temp_dir().join("panda_ffmpeg_stderr_test"));
        let out_dir = out_dir_guard.path().to_path_buf();
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
                canvas: Canvas::BASE,
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
    }

    /// 写一个可执行的假 ffmpeg 脚本到临时目录，返回其路径。仅用于下面三条
    /// 白盒测试：它们要验证的编排细节（并发读 stderr、ffmpeg 成功但写帧
    /// 失败时不吞错误、输出目录建没建好）在真实 ffmpeg 上要么需要填满
    /// 64KB 管道缓冲区（约 4 分钟挂钟时间，见 `docs/ffmpeg-pipeline.md`
    /// 第 6 节），要么依赖 ffmpeg 恰好提前关闭 stdin 又恰好退出码 0，要么
    /// 依赖输入素材缺失又恰好走到打开输出路径那一步——都不是能在单测
    /// 规模下稳定复现的条件。用假脚本直接控制这几种行为，比等真实 ffmpeg
    /// 巧合触发要可靠得多。
    #[cfg(unix)]
    /// 写一个假 ffmpeg 脚本，返回**自清理的守卫**。
    ///
    /// 交 `TempPath` 而不是裸 `PathBuf`（销 `docs/follow-ups.md`「族 A」测试
    /// 侧的两条）：这几条测试都经 `run_with_timeout` 跑被测闭包，闭包 panic
    /// 或超时时 `run_with_timeout` 自己就 `panic!`，写在测试尾部的
    /// `remove_file(&script)` 永远执行不到——而「闭包 panic」恰恰是这些测试
    /// 最想抓的那类回归，也就是说**越是抓到了 bug，越会留下垃圾文件**。
    fn write_fake_ffmpeg(name: &str, script: &str) -> crate::tmp::TempPath {
        use std::os::unix::fs::PermissionsExt;
        let path =
            std::env::temp_dir().join(format!("panda_fake_ffmpeg_{name}_{}", std::process::id()));
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        crate::tmp::TempPath::new(path)
    }

    /// 三条用假 ffmpeg 脚本的测试共用的超时预算。
    ///
    /// `cargo test` 本身没有全局超时：一次把编排逻辑改错到真的死锁/永久
    /// 挂起的回归，如果测试没有自己的超时兜底，会一路挂到 CI 作业级的
    /// 外层超时才被发现，而且不会打印任何一行「哪条测试挂了」的结果
    /// （修复轮 1 I3 实测：删掉 `drop(stdin)` 后，`run_render_creates_
    /// the_output_directory_before_invoking_ffmpeg` 之前没有超时保护，
    /// `timeout 90 cargo test` 直接把整个进程杀掉，一行结果都没打印）。
    ///
    /// 120 秒定得比表面看起来宽：这个超时只在"真的挂死"时才会被吃满，
    /// 通过路径命中 `recv_timeout` 会立刻返回，不会真等 120 秒。实测过
    /// （20 核空载机单独跑这一条）本 debug 构建下渲染全时间轴 330 帧只要
    /// 约 15 秒；即便机器负载重、多个测试并发抢 CPU，30 秒的预算也只有
    /// 约 2 倍余量——假阳性（把慢速渲染误判成死锁）曾经真的发生过一次
    /// （见任务报告「自审发现」）。既然假阴性风险是零（真死锁永远不会
    /// 提前返回），把预算调宽到 120 秒对通过路径零成本，只是让失败路径
    /// 多等一会儿——这个方向没有理由省。
    const FAKE_FFMPEG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

    /// 在独立线程里跑 `f`，用 [`FAKE_FFMPEG_TIMEOUT`] 兜底。
    ///
    /// 用 `recv_timeout` 而不是直接在当前线程调用 `f`：真死锁下 `f` 本身
    /// 永远不返回，只有把它挪到别的线程、主线程改为"等消息或等超时"，
    /// 才能把"挂死"变成一条可读的 panic，而不是让整个测试进程被外部
    /// 工具杀掉、什么都不打印。
    fn run_with_timeout<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(FAKE_FFMPEG_TIMEOUT).unwrap_or_else(|_| {
            panic!(
                "{}秒内未返回，疑似死锁/永久挂起：假 ffmpeg 与我们的写端/读端谁也没有先撒手",
                FAKE_FFMPEG_TIMEOUT.as_secs()
            )
        })
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
        // 用 `run_with_timeout` 代替死等，超时本身就是「抓到了」的证据。
        let script = write_fake_ffmpeg(
            "stderr_then_stdin",
            "#!/bin/sh\nhead -c 200000 /dev/zero | tr '\\0' 'x' 1>&2\ncat >/dev/null\nexit 0\n",
        );
        let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n短。\n";
        let mut fs = crate::render::frame::FrameSource::new(
            vtt,
            "标题".into(),
            &crate::config::Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        let bad = std::path::Path::new("/nonexistent-xyz.mp4");
        // 守卫在作用域末尾删产物：`run_with_timeout` 的闭包 panic 或超时时
        // 它自己就 `panic!`，写在测试尾部的清理执行不到（销「族 A」测试侧）。
        let out_guard = crate::tmp::TempPath::new(std::env::temp_dir().join(format!(
            "panda_stderr_deadlock_test_{}.mp4",
            std::process::id()
        )));
        let out = out_guard.path().to_path_buf();
        let total_frames = fs.total_frames();
        let audio_secs = fs.audio_secs();
        let content_frames = fs.content_frames();

        let script_for_run = script.path().to_path_buf();
        let out_for_run = out.clone();
        let result = run_with_timeout(move || {
            let inputs = RenderInputs {
                bg: bad,
                tts_audio: bad,
                bgm: bad,
                typewriter: bad,
                intro: bad,
                out: &out_for_run,
                total_frames,
                audio_secs,
                content_frames,
                canvas: Canvas::BASE,
            };
            run_render_with_ffmpeg_binary(&script_for_run, &mut fs, &inputs)
                .map_err(|e| format!("{e:#}"))
        });
        assert!(
            result.is_ok(),
            "假 ffmpeg 正常读完 stdin 后退出 0，不应报错：{result:?}"
        );
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
        // 帧数被截断的坏成片，且没有任何报错。也走 `run_with_timeout`：
        // 假 ffmpeg 提前退出、我们仍在写的场景理论上不该挂死，但既然三条
        // 假 ffmpeg 测试共用同一套编排代码，统一套上超时兜底，不去赌
        // "这条肯定不会挂"。
        let script = write_fake_ffmpeg(
            "read_100_bytes_then_exit",
            "#!/bin/sh\nhead -c 100 >/dev/null\nexit 0\n",
        );
        // 字幕够长，保证 total_frames 对应的字节数远大于假 ffmpeg 会读的 100 字节。
        let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:05.000\n足够长，保证多于一帧要写，写端会在假 ffmpeg 提前退出后撞上已关闭的管道。\n";
        let mut fs = crate::render::frame::FrameSource::new(
            vtt,
            "标题".into(),
            &crate::config::Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        let bad = std::path::Path::new("/nonexistent-xyz.mp4");
        // 同上：清理挂到作用域上，不写在尾部。
        let out_guard = crate::tmp::TempPath::new(std::env::temp_dir().join(format!(
            "panda_write_swallow_test_{}.mp4",
            std::process::id()
        )));
        let out = out_guard.path().to_path_buf();
        let total_frames = fs.total_frames();
        let audio_secs = fs.audio_secs();
        let content_frames = fs.content_frames();

        let script_for_run = script.path().to_path_buf();
        let out_for_run = out.clone();
        let result = run_with_timeout(move || {
            let inputs = RenderInputs {
                bg: bad,
                tts_audio: bad,
                bgm: bad,
                typewriter: bad,
                intro: bad,
                out: &out_for_run,
                total_frames,
                audio_secs,
                content_frames,
                canvas: Canvas::BASE,
            };
            run_render_with_ffmpeg_binary(&script_for_run, &mut fs, &inputs)
                .map_err(|e| format!("{e:#}"))
        });
        let msg = result.expect_err("假 ffmpeg 提前关闭 stdin 后写帧应失败，即使它自己退出码是 0");
        assert!(
            msg.contains("管道") || msg.contains("pipe") || msg.contains("Broken"),
            "错误应指出是写帧/管道失败，而不是别的：{msg}"
        );
        assert!(
            !msg.contains("退出码"),
            "假 ffmpeg 退出码是 0，不应误报成 ffmpeg 非零退出：{msg}"
        );
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
        // 建好，和输入素材是否存在无关。也走 `run_with_timeout`：修复轮 1
        // 实测过，删掉 `drop(stdin)` 那个变异会让这条测试之前没有超时
        // 保护，`cat >/dev/null` 永远等不到 EOF、整条 `cargo test` 被外部
        // `timeout` 杀掉、一行结果都不打印——统一走超时兜底后，同样的
        // 变异会得到一条可读的 panic，而不是一次静默的挂起。
        let script = write_fake_ffmpeg(
            "touch_output_path",
            "#!/bin/sh\ncat >/dev/null\nfor out; do :; done\n: > \"$out\" || exit 3\nexit 0\n",
        );
        let vtt = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n短。\n";
        let mut fs = crate::render::frame::FrameSource::new(
            vtt,
            "标题".into(),
            &crate::config::Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        let bad = std::path::Path::new("/nonexistent-xyz.mp4");
        // 特意让输出的父目录（两层，逼 create_dir_all 而不是单层 mkdir）
        // 在测试开始前不存在。
        let out_dir_guard = crate::tmp::TempPath::new(
            std::env::temp_dir().join(format!("panda_create_dir_test_{}", std::process::id())),
        );
        let out_dir = out_dir_guard.path().to_path_buf();
        std::fs::remove_dir_all(&out_dir).ok();
        let out = out_dir.join("nested").join("out.mp4");
        let total_frames = fs.total_frames();
        let audio_secs = fs.audio_secs();
        let content_frames = fs.content_frames();

        let script_for_run = script.path().to_path_buf();
        let out_for_run = out.clone();
        let result = run_with_timeout(move || {
            let inputs = RenderInputs {
                bg: bad,
                tts_audio: bad,
                bgm: bad,
                typewriter: bad,
                intro: bad,
                out: &out_for_run,
                total_frames,
                audio_secs,
                content_frames,
                canvas: Canvas::BASE,
            };
            run_render_with_ffmpeg_binary(&script_for_run, &mut fs, &inputs)
                .map_err(|e| format!("{e:#}"))
        });
        assert!(
            result.is_ok(),
            "输出目录应已被建好，假 ffmpeg 应能顺利在那里创建文件并退出 0：{result:?}"
        );
        assert!(
            out.exists(),
            "假 ffmpeg 应已在正确路径创建了文件，说明目录确实建好了"
        );
    }
}
