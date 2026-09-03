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
}
