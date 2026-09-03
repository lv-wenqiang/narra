use crate::render::timeline::{FPS, HEIGHT, WIDTH};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

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
}
