use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::time::Duration;

use panda::config;
use panda::render::frame::FrameSource;
use panda::render::timeline::FPS;
use panda::tts::pipeline::{ProcessOptions, process_narration_file};

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
    /// 音频 + 字幕 + 素材 → 成片 mp4
    Render {
        /// TTS 产出的 mp3
        #[arg(long)]
        audio: PathBuf,
        /// TTS 产出的 vtt
        #[arg(long)]
        vtt: PathBuf,
        /// 标题，优先级最高
        #[arg(long)]
        title: Option<String>,
        /// 标题 JSON，默认 public/video/title.json
        #[arg(long)]
        title_json: Option<PathBuf>,
        /// 背景视频，默认 public/video/0.mp4
        #[arg(long)]
        bg: Option<PathBuf>,
        /// 背景音乐，默认 public/bgm/0.mp3
        #[arg(long)]
        bgm: Option<PathBuf>,
        /// 成片输出，默认 output/video/video.mp4
        #[arg(short, long)]
        out: Option<PathBuf>,
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

/// 标题三级兜底的 IO 外壳：读文件（缺失或不可读视作「没有 JSON」），
/// 纯粹的优先级逻辑交给 `config::resolve_title`。
fn read_title(cli: Option<&str>, json_path: &Path) -> String {
    let json_text = std::fs::read_to_string(json_path).ok();
    config::resolve_title(cli, json_text.as_deref())
}

/// `Render` 四个可选参数各自的三级兜底路径：CLI 未给时落到 `config` 里
/// 对应的默认值函数。用具名结构体接收而不是元组，是为了让「哪个字段配
/// 哪个默认值函数」在构造处和调用处都由字段名而不是位置决定——`bg` 与
/// `bgm`、`title_json` 与 `out` 在参数列表里彼此相邻，最容易被写反，而
/// 两组默认值字符串（`public/video/0.mp4` 对 `public/bgm/0.mp3`）看起来
/// 都是「一个路径」，编译器发现不了这种错配，只有测试能。
struct ResolvedRenderPaths {
    title_json: PathBuf,
    bg: PathBuf,
    bgm: PathBuf,
    out: PathBuf,
}

fn resolve_render_paths(
    title_json: Option<PathBuf>,
    bg: Option<PathBuf>,
    bgm: Option<PathBuf>,
    out: Option<PathBuf>,
) -> ResolvedRenderPaths {
    ResolvedRenderPaths {
        title_json: title_json.unwrap_or_else(|| PathBuf::from(config::title_json_path())),
        bg: bg.unwrap_or_else(|| PathBuf::from(config::bg_video_path())),
        bgm: bgm.unwrap_or_else(|| PathBuf::from(config::bgm_path())),
        out: out.unwrap_or_else(|| PathBuf::from(config::video_output_path())),
    }
}

/// 四个输入文件在合成前必须都存在；哪个缺失就点名哪个，不合并成一句笼统的
/// 「素材缺失」，免得用户还要自己猜是四个里的哪一个。
fn check_render_inputs_exist(audio: &Path, vtt: &Path, bg: &Path, bgm: &Path) -> Result<()> {
    for (label, p) in [
        ("音频", audio),
        ("字幕", vtt),
        ("背景视频", bg),
        ("背景音乐", bgm),
    ] {
        if !p.exists() {
            anyhow::bail!("{label}文件不存在：{}", p.display());
        }
    }
    Ok(())
}

/// 落盘两段内嵌音效，并按文件名确认没有被对调。
///
/// `assets::write_embedded_audio` 的返回顺序是 `(intro, typewriter)`，而
/// `ffmpeg::RenderInputs` 的字段声明顺序是 `typewriter` 先于 `intro`——两个
/// 顺序正好相反，是抄一半就能编译通过、只有跑出来的音效才会错位的高危点。
///
/// 用普通 `assert_eq!`（而不是 `debug_assert_eq!`）：审查发现（修复轮 1）
/// 指出 `debug_assert_eq!` 在 release 构建下会被编译掉，唯一暴露面是绕过
/// `cargo test` 直接跑 release 二进制——两次字符串比较的开销小到没有理由
/// 不在所有构建里都留着这条检查。真正兜底的门禁始终是下面 `mod tests`
/// 里的普通测试断言，这里的 `assert_eq!` 只是多一层运行时保险。
fn write_embedded_audio_checked(tmp_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let (intro, typewriter) = panda::assets::write_embedded_audio(tmp_dir)?;
    assert_eq!(
        intro.file_name().and_then(|f| f.to_str()),
        Some("intro.mp3")
    );
    assert_eq!(
        typewriter.file_name().and_then(|f| f.to_str()),
        Some("intro_typewriter.mp3")
    );
    Ok((intro, typewriter))
}

/// 把已解析的路径与 `source` 的三个统计量按 `RenderInputs` 的字段逐一装配。
///
/// 统计量直接从 `source` 取（而不是先在调用处解包成局部变量再传参），是为
/// 了堵住「`audio_secs` 填成 `content_frames as f64`」这类字段级配对错乱——
/// 本计划里 Task 3（TTS/BGM 处理链对调）、Task 4（输入顺序）、Task 6（环境
/// 变量名对调）反复出现同一类回归。纯函数，不碰文件系统、不起进程，可以用
/// 互不相同的哑值单测直接钉住这条接线；真正的 ffmpeg 调用留给
/// `Commands::Render`（Task 8 的 `compose_video` 预期会复用这个函数）。
fn build_render_inputs<'a>(
    source: &FrameSource,
    bg: &'a Path,
    tts_audio: &'a Path,
    bgm: &'a Path,
    typewriter: &'a Path,
    intro: &'a Path,
    out: &'a Path,
) -> panda::ffmpeg::RenderInputs<'a> {
    panda::ffmpeg::RenderInputs {
        bg,
        tts_audio,
        bgm,
        typewriter,
        intro,
        out,
        total_frames: source.total_frames(),
        audio_secs: source.audio_secs(),
        content_frames: source.content_frames(),
    }
}

/// 无论 `run_render` 成功与否都清理临时音效目录；清理本身失败不掩盖真正的
/// 错误——`remove_dir_all` 的 `Err` 被丢弃，只有 `result` 自己的 `Err` 会
/// 继续传播。
fn cleanup_tmp_and_propagate(tmp: &Path, result: Result<()>) -> Result<()> {
    std::fs::remove_dir_all(tmp).ok();
    result
}

/// `run_render` 返回 `Err` 时，磁盘上可能已经留下一个看起来完整、实则被
/// 冻结帧填充的 mp4：`-shortest 0` 下写帧中途出错，我们仍会 `drop(stdin)`，
/// ffmpeg 会用最后一帧补满 `-t` 时长并以 0 退出——`run_render` 正确地把写
/// 端的 `Err` 报了出来，但产物已经落盘（Task 5 交接的已知行为）。
///
/// 干净失败：删掉这个半成品，而不是留一个能播放、时长正确、实则大半静止
/// 的成片让用户自己发现。
fn cleanup_output_on_failure(out: &Path, result: Result<()>) -> Result<()> {
    if result.is_err() {
        std::fs::remove_file(out).ok();
    }
    result
}

/// `compose_video`（及其可注入执行器的核心 `compose_video_with_runner`）
/// 需要的全部已解析输入，收进具名结构体而不是七个位置参数。
///
/// 审查发现（修复轮 1）指出：把 `Commands::Render` 分支体内联逻辑拆成
/// 6 个函数之后，风险并没有消失，只是从「具名结构体字面量里字段写错」
/// 搬到了「调用处把同类型的 `&Path` 位置参数传错顺序」——Rust 类型系统对
/// 一串 `&Path` 位置参数零保护，且分支体本身（把 6 个函数串起来的胶水
/// 代码）此前完全没有测试覆盖。这里用具名结构体重新收拢参数，是把同一个
/// 教训应用到「调用处装配」这一层：构造 `ComposeVideoInputs` 时字段名与
/// 传入值必须对齐，编译器挡一部分；下面 `compose_video_with_runner` 的
/// 单测（用互不相同的哑值/真实文件名，并在真正调用 ffmpeg 之前用注入的
/// 假执行器拦截 `RenderInputs`）挡住剩下的部分。
#[derive(Clone, Copy)]
struct ComposeVideoInputs<'a> {
    audio: &'a Path,
    vtt: &'a Path,
    title: Option<&'a str>,
    title_json: &'a Path,
    bg: &'a Path,
    bgm: &'a Path,
    out: &'a Path,
}

/// `Commands::Render` 分支体的可测核心：把「检查输入是否存在 → 读标题/
/// 字幕 → 落盘内嵌音效 → 构造 `FrameSource` → 组装 `RenderInputs`」这条
/// 调用链跑一遍，最后一步不硬编码调用 `panda::ffmpeg::run_render`，而是
/// 交给 `runner` 参数。
///
/// 这不是为了给 `run_render` 本身增加抽象层——它已经在 `ffmpeg.rs` 里被
/// 假 ffmpeg 脚本测过了——而是为了让测试能在**不起 ffmpeg 子进程**的前提
/// 下，拦下组装好的 `RenderInputs`，断言它的每个字段确实来自
/// `ComposeVideoInputs` 里同名的那个字段，而不是被本函数内部的调用处
/// 装配代码传错了位置：
/// - `write_embedded_audio_checked` 返回值解构成 `(intro, typewriter)`
///   之后，两个变量名有没有被写反；
/// - 组装 `RenderInputs` 时 `tts_audio`/`bgm` 有没有被传反；
/// - `check_render_inputs_exist(audio, vtt, bg, bgm)` 的调用处参数顺序
///   有没有被打乱（用「只有其中一个文件缺失」的场景验证报错点名的确实是
///   那一个，而不是被换了标签的另一个）。
///
/// 生产路径的 `runner` 就是 `panda::ffmpeg::run_render` 本身，见
/// `compose_video`；`panda make`（Task 8）预期直接复用 `compose_video`。
fn compose_video_with_runner(
    inputs: &ComposeVideoInputs,
    runner: impl FnOnce(&mut FrameSource, &panda::ffmpeg::RenderInputs) -> Result<()>,
) -> Result<()> {
    let ComposeVideoInputs {
        audio,
        vtt,
        title,
        title_json,
        bg,
        bgm,
        out,
    } = *inputs;

    check_render_inputs_exist(audio, vtt, bg, bgm)?;

    let resolved_title = read_title(title, title_json);
    let vtt_text =
        std::fs::read_to_string(vtt).with_context(|| format!("读取字幕失败：{}", vtt.display()))?;

    // 两段内嵌音效落到临时目录，供 ffmpeg 作为输入文件读取——stdin
    // 已经被帧流占用，没法再从管道喂第二、第三份数据。
    let tmp = std::env::temp_dir().join(format!("panda_render_{}", std::process::id()));
    std::fs::create_dir_all(&tmp)
        .with_context(|| format!("创建临时目录失败：{}", tmp.display()))?;
    let (intro, typewriter) = write_embedded_audio_checked(&tmp)?;

    let mut source = FrameSource::new(&vtt_text, resolved_title.clone())?;

    println!(
        "标题「{resolved_title}」，音频 {:.2}s，共 {} 帧（{:.2}s），输出 {}",
        source.audio_secs(),
        source.total_frames(),
        source.total_frames() as f64 / FPS as f64,
        out.display()
    );

    let render_inputs = build_render_inputs(&source, bg, audio, bgm, &typewriter, &intro, out);
    let result = runner(&mut source, &render_inputs);
    let result = cleanup_tmp_and_propagate(&tmp, result);
    cleanup_output_on_failure(out, result)?;

    println!("成片已写入 {}", out.display());
    Ok(())
}

/// `compose_video_with_runner` 接上真正的 ffmpeg 执行器。
/// `Commands::Render` 分支体只是它的一层薄胶水（构造 `ComposeVideoInputs`
/// 后调用它），`panda make`（Task 8）预期直接复用本函数。
fn compose_video(inputs: &ComposeVideoInputs) -> Result<()> {
    compose_video_with_runner(inputs, panda::ffmpeg::run_render)
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Tts {
            input,
            outdir,
            voice,
            batch_size,
        } => {
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
        Commands::DebugFrames {
            vtt,
            title,
            out,
            frames,
        } => run_debug_frames(vtt, title, out, frames),
        Commands::Render {
            audio,
            vtt,
            title,
            title_json,
            bg,
            bgm,
            out,
        } => {
            let ResolvedRenderPaths {
                title_json,
                bg,
                bgm,
                out,
            } = resolve_render_paths(title_json, bg, bgm, out);

            compose_video(&ComposeVideoInputs {
                audio: &audio,
                vtt: &vtt,
                title: title.as_deref(),
                title_json: &title_json,
                bg: &bg,
                bgm: &bgm,
                out: &out,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_title_uses_cli_when_given() {
        let missing = std::path::Path::new("/nonexistent-title-xyz.json");
        assert_eq!(read_title(Some("命令行标题"), missing), "命令行标题");
    }

    #[test]
    fn read_title_falls_back_to_default_when_json_is_missing() {
        // 标题 JSON 是可选素材：文件不存在不应报错，应静默回落默认值。
        let missing = std::path::Path::new("/nonexistent-title-xyz.json");
        assert_eq!(read_title(None, missing), panda::config::DEFAULT_TITLE);
    }

    #[test]
    fn read_title_reads_the_json_file_when_it_exists() {
        let p = std::env::temp_dir().join(format!("panda_title_{}.json", std::process::id()));
        std::fs::write(&p, r#"{"title": "文件里的标题"}"#).unwrap();
        assert_eq!(read_title(None, &p), "文件里的标题");
        assert_eq!(read_title(Some("覆盖"), &p), "覆盖", "CLI 优先级最高");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn parse_frame_list_splits_trims_and_rejects_garbage() {
        // 偿还 follow-ups 记账项：CLI 层此前零单测。
        assert_eq!(
            parse_frame_list(Some("0,15,120"), 600).unwrap(),
            vec![0, 15, 120]
        );
        assert_eq!(parse_frame_list(Some(" 3 , 4 "), 600).unwrap(), vec![3, 4]);
        assert!(parse_frame_list(Some("1,x"), 600).is_err());
    }

    #[test]
    fn parse_frame_list_default_sweep_includes_the_last_frame() {
        // 末帧是 Outro 淡出终点，是目视验收最该看的一帧。
        let ids = parse_frame_list(None, 937).unwrap();
        assert_eq!(ids.first(), Some(&0));
        assert_eq!(ids.last(), Some(&936), "默认帧集必须含末帧：{ids:?}");
        let mut dedup = ids.clone();
        dedup.dedup();
        assert_eq!(dedup, ids, "不得有重复帧号");
    }

    /// 变异实验：`--bg` 默认值接成 `config::bgm_path()`（两个默认值对调）。
    /// 直接和 `config` 里对应的函数各自比对，而不是硬编码字符串字面量——
    /// 硬编码的话，把两个默认值调换后再把断言也跟着抄错，测试依然会绿。
    #[test]
    fn resolve_render_paths_defaults_match_the_documented_config_functions() {
        let r = resolve_render_paths(None, None, None, None);
        assert_eq!(r.title_json, PathBuf::from(config::title_json_path()));
        assert_eq!(r.bg, PathBuf::from(config::bg_video_path()));
        assert_eq!(r.bgm, PathBuf::from(config::bgm_path()));
        assert_eq!(r.out, PathBuf::from(config::video_output_path()));
        // 两组默认值本身互不相同，才能保证上面的断言真的有鉴别力：
        // 如果 bg_video_path() 和 bgm_path() 碰巧相等，对调也测不出来。
        assert_ne!(r.bg, r.bgm, "bg 与 bgm 默认值不应相同");
        assert_ne!(r.title_json, r.out, "title_json 与 out 默认值不应相同");
    }

    #[test]
    fn resolve_render_paths_prefers_given_values_over_defaults() {
        let r = resolve_render_paths(
            Some(PathBuf::from("/given/title.json")),
            Some(PathBuf::from("/given/bg.mp4")),
            Some(PathBuf::from("/given/bgm.mp3")),
            Some(PathBuf::from("/given/out.mp4")),
        );
        assert_eq!(r.title_json, PathBuf::from("/given/title.json"));
        assert_eq!(r.bg, PathBuf::from("/given/bg.mp4"));
        assert_eq!(r.bgm, PathBuf::from("/given/bgm.mp3"));
        assert_eq!(r.out, PathBuf::from("/given/out.mp4"));
    }

    /// 变异实验：删掉「四个输入文件存在性检查」。
    /// 对四个位置逐一验证：每次只让其中一个文件缺失，断言错误信息点名的
    /// 正是那一个——不是笼统的「有文件缺失」，而是确认标签和路径没有错位。
    #[test]
    fn check_render_inputs_exist_names_the_missing_file() {
        let dir =
            std::env::temp_dir().join(format!("panda_render_exist_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("a.mp3");
        let vtt = dir.join("a.vtt");
        let bg = dir.join("bg.mp4");
        let bgm = dir.join("bgm.mp3");
        for p in [&audio, &vtt, &bg, &bgm] {
            std::fs::write(p, b"x").unwrap();
        }

        assert!(
            check_render_inputs_exist(&audio, &vtt, &bg, &bgm).is_ok(),
            "四个文件都存在时不应报错"
        );

        let cases: [(&str, &std::path::Path); 4] = [
            ("音频", &audio),
            ("字幕", &vtt),
            ("背景视频", &bg),
            ("背景音乐", &bgm),
        ];
        for (label, missing_path) in cases {
            std::fs::remove_file(missing_path).unwrap();
            let err = check_render_inputs_exist(&audio, &vtt, &bg, &bgm).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains(label),
                "缺 {label} 时错误信息应点名 {label}：{msg}"
            );
            assert!(
                msg.contains(&missing_path.display().to_string()),
                "错误信息应包含缺失文件的路径：{msg}"
            );
            std::fs::write(missing_path, b"x").unwrap();
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 变异实验：`write_embedded_audio` 返回的 `(intro, typewriter)`
    /// 在落盘/接收处被对调。用文件名（而不是变量名）核实顺序没有反。
    #[test]
    fn write_embedded_audio_checked_keeps_intro_and_typewriter_in_order() {
        let dir =
            std::env::temp_dir().join(format!("panda_render_audio_order_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let (intro, typewriter) = write_embedded_audio_checked(&dir).unwrap();
        assert_eq!(
            intro.file_name().and_then(|f| f.to_str()),
            Some("intro.mp3")
        );
        assert_eq!(
            typewriter.file_name().and_then(|f| f.to_str()),
            Some("intro_typewriter.mp3")
        );
        assert_eq!(std::fs::read(&intro).unwrap(), panda::assets::INTRO_MP3);
        assert_eq!(
            std::fs::read(&typewriter).unwrap(),
            panda::assets::INTRO_TYPEWRITER_MP3
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    const RENDER_TEST_VTT: &str = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:10.000\n测试字幕。\n";

    /// 变异实验总覆盖：`RenderInputs` 的 `tts_audio` 与 `bgm` 字段填反、
    /// `typewriter` 与 `intro` 字段填反、`audio_secs` 填成
    /// `content_frames as f64`。六个路径字段与三个数值字段全部用互不相同
    /// 的哑值，任何一处错配都会让对应的断言失败。数值取自
    /// `render::timeline` 已验证过的 `layout(10.0)` 结果（`content_frames
    /// = 360`，`total_frames = 600`），三者互不相同，足够区分「填对」与
    /// 「填错」。
    #[test]
    fn build_render_inputs_wires_every_field_without_swapping() {
        let source = FrameSource::new(RENDER_TEST_VTT, "标题".into()).unwrap();
        assert_eq!(source.audio_secs(), 10.0);
        assert_eq!(source.content_frames(), 360);
        assert_eq!(source.total_frames(), 600);

        let bg = Path::new("/dummy/bg.mp4");
        let tts_audio = Path::new("/dummy/tts_audio.mp3");
        let bgm = Path::new("/dummy/bgm.mp3");
        let typewriter = Path::new("/dummy/typewriter.mp3");
        let intro = Path::new("/dummy/intro.mp3");
        let out = Path::new("/dummy/out.mp4");

        let inputs = build_render_inputs(&source, bg, tts_audio, bgm, typewriter, intro, out);

        assert_eq!(inputs.bg, bg);
        assert_eq!(inputs.tts_audio, tts_audio);
        assert_eq!(inputs.bgm, bgm);
        assert_ne!(
            inputs.tts_audio, inputs.bgm,
            "tts_audio 与 bgm 不应指向同一路径"
        );
        assert_eq!(inputs.typewriter, typewriter);
        assert_eq!(inputs.intro, intro);
        assert_ne!(
            inputs.typewriter, inputs.intro,
            "typewriter 与 intro 不应指向同一路径"
        );
        assert_eq!(inputs.out, out);

        assert_eq!(inputs.total_frames, source.total_frames());
        assert_eq!(inputs.audio_secs, source.audio_secs());
        assert_eq!(inputs.content_frames, source.content_frames());
        // 三个数值互不相同，才能保证上面三条断言真的分得清谁是谁。
        assert_ne!(inputs.audio_secs, inputs.content_frames as f64);
        assert_ne!(inputs.total_frames, inputs.content_frames);
    }

    /// 变异实验：临时目录在失败路径上不清理。Ok 与 Err 两条路径都要验证
    /// 目录被移除，且 `result` 的 Ok/Err 变体（含错误文本）原样透传。
    #[test]
    fn cleanup_tmp_and_propagate_removes_dir_on_both_success_and_failure() {
        let ok_dir = std::env::temp_dir().join(format!("panda_cleanup_ok_{}", std::process::id()));
        std::fs::create_dir_all(&ok_dir).unwrap();
        std::fs::write(ok_dir.join("intro.mp3"), b"x").unwrap();
        let r = cleanup_tmp_and_propagate(&ok_dir, Ok(()));
        assert!(r.is_ok());
        assert!(!ok_dir.exists(), "成功路径也应清理临时目录");

        let err_dir =
            std::env::temp_dir().join(format!("panda_cleanup_err_{}", std::process::id()));
        std::fs::create_dir_all(&err_dir).unwrap();
        std::fs::write(err_dir.join("typewriter.mp3"), b"x").unwrap();
        let r = cleanup_tmp_and_propagate(&err_dir, Err(anyhow::anyhow!("模拟 run_render 失败")));
        assert!(
            !err_dir.exists(),
            "失败路径也应清理临时目录，这是本任务要堵的那个疏漏"
        );
        assert_eq!(
            r.unwrap_err().to_string(),
            "模拟 run_render 失败",
            "错误信息应原样透传"
        );
    }

    /// 决定：`run_render` 失败时删除可能已落盘的误导性成片；成功时绝不触碰
    /// 输出文件。
    #[test]
    fn cleanup_output_on_failure_deletes_only_when_result_is_err() {
        let ok_out =
            std::env::temp_dir().join(format!("panda_cleanup_out_ok_{}.mp4", std::process::id()));
        std::fs::write(&ok_out, b"fake mp4").unwrap();
        let r = cleanup_output_on_failure(&ok_out, Ok(()));
        assert!(r.is_ok());
        assert!(ok_out.exists(), "成功时不应删除成片");
        std::fs::remove_file(&ok_out).ok();

        let err_out =
            std::env::temp_dir().join(format!("panda_cleanup_out_err_{}.mp4", std::process::id()));
        std::fs::write(&err_out, b"frozen-frame fake mp4").unwrap();
        let r = cleanup_output_on_failure(&err_out, Err(anyhow::anyhow!("run_render 写帧失败")));
        assert!(!err_out.exists(), "失败时应删除可能已落盘的误导性成片");
        assert_eq!(r.unwrap_err().to_string(), "run_render 写帧失败");
    }

    /// 修复轮 1：审查发现指出，把 `Commands::Render` 分支体拆成 6 个函数
    /// 之后，风险从「具名结构体字面量字段写错」搬到了「调用处把同类型的
    /// `&Path` 位置参数传错顺序」，而分支体本身（把 6 个函数串起来的胶水
    /// 代码）此前完全没有测试覆盖——`build_render_inputs_wires_every_
    /// field_without_swapping` 只测了 `build_render_inputs` 自身，测不到
    /// 「调用处解构 `write_embedded_audio_checked` 的返回值时把
    /// `(intro, typewriter)` 写反」或「调用 `build_render_inputs` 时把
    /// `tts_audio`/`bgm` 实参对调」这两类真实发生在 `main.rs:300` 与
    /// `main.rs:312`（e4b7aa2 时的行号）的调用处装配错误。
    ///
    /// `compose_video_with_runner` 就是把这段胶水代码搬进来的可测核心：
    /// 在真正调用 ffmpeg 之前，用注入的假 `runner` 拦下组装好的
    /// `RenderInputs`，逐字段核对它确实来自 `ComposeVideoInputs` 里同名的
    /// 那个字段。
    #[test]
    fn compose_video_wires_intro_typewriter_and_audio_bgm_without_swapping_at_the_call_site() {
        let dir = std::env::temp_dir().join(format!("panda_compose_wiring_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let audio = dir.join("real_audio.mp3");
        let bg = dir.join("real_bg.mp4");
        let bgm = dir.join("real_bgm.mp3");
        let vtt = dir.join("real.vtt");
        let title_json = dir.join("nonexistent_title.json"); // 不存在，走 --title 兜底
        let out = dir.join("out.mp4");
        std::fs::write(&audio, "audio").unwrap();
        std::fs::write(&bg, "bg").unwrap();
        std::fs::write(&bgm, "bgm").unwrap();
        std::fs::write(&vtt, RENDER_TEST_VTT).unwrap();

        let inputs = ComposeVideoInputs {
            audio: &audio,
            vtt: &vtt,
            title: Some("测试标题"),
            title_json: &title_json,
            bg: &bg,
            bgm: &bgm,
            out: &out,
        };

        let mut runner_called = false;
        let result = compose_video_with_runner(&inputs, |_source, render_inputs| {
            runner_called = true;
            assert_eq!(render_inputs.bg, bg.as_path(), "bg 不应错位");
            assert_eq!(
                render_inputs.tts_audio,
                audio.as_path(),
                "tts_audio 应指向 --audio，而不是被换成 --bgm"
            );
            assert_eq!(
                render_inputs.bgm,
                bgm.as_path(),
                "bgm 应指向 --bgm，而不是被换成 --audio"
            );
            assert_ne!(
                render_inputs.tts_audio, render_inputs.bgm,
                "tts_audio 与 bgm 不应指向同一路径，否则本断言无鉴别力"
            );
            assert_eq!(
                render_inputs
                    .typewriter
                    .file_name()
                    .and_then(|f| f.to_str()),
                Some("intro_typewriter.mp3"),
                "typewriter 字段应指向打字机音效文件，而不是片尾音效"
            );
            assert_eq!(
                render_inputs.intro.file_name().and_then(|f| f.to_str()),
                Some("intro.mp3"),
                "intro 字段应指向片尾音效文件，而不是打字机音效"
            );
            assert_eq!(render_inputs.out, out.as_path());
            Ok(())
        });

        assert!(
            result.is_ok(),
            "compose_video_with_runner 应成功：{result:?}"
        );
        assert!(runner_called, "runner 应被调用到，否则上面的断言根本没跑");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 修复轮 1：复现审查变异 `main.rs:289`
    /// （`check_render_inputs_exist` 的 `audio`/`vtt` 实参对调）。只测「文件
    /// 缺失时报错」不够——参数对调后依然会报错，只是点名了错的那个文件。
    /// audio 与 vtt 分别单独缺失两个场景都要跑，确认报错点名的确实是真正
    /// 缺失的那一个。
    #[test]
    fn compose_video_names_the_correct_missing_input_without_swapping_audio_and_vtt() {
        let dir =
            std::env::temp_dir().join(format!("panda_compose_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let audio = dir.join("real_audio.mp3");
        let vtt = dir.join("real.vtt");
        let bg = dir.join("real_bg.mp4");
        let bgm = dir.join("real_bgm.mp3");
        let title_json = dir.join("nonexistent_title.json");
        let out = dir.join("out.mp4");
        std::fs::write(&bg, "bg").unwrap();
        std::fs::write(&bgm, "bgm").unwrap();

        // 场景一：只有 audio 缺失，vtt/bg/bgm 都存在。
        std::fs::write(&vtt, RENDER_TEST_VTT).unwrap();
        let inputs = ComposeVideoInputs {
            audio: &audio,
            vtt: &vtt,
            title: Some("t"),
            title_json: &title_json,
            bg: &bg,
            bgm: &bgm,
            out: &out,
        };
        let err = compose_video_with_runner(&inputs, |_, _| {
            panic!("runner 不应被调用：应该在存在性检查这一步就失败")
        })
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("音频"), "audio 缺失时应点名“音频”：{msg}");
        assert!(
            msg.contains(&audio.display().to_string()),
            "应包含 audio 的路径：{msg}"
        );
        assert!(!msg.contains("字幕"), "audio 缺失时不应误报“字幕”：{msg}");

        // 场景二：换成只有 vtt 缺失，audio/bg/bgm 都存在。
        std::fs::write(&audio, "audio").unwrap();
        std::fs::remove_file(&vtt).unwrap();
        let inputs = ComposeVideoInputs {
            audio: &audio,
            vtt: &vtt,
            title: Some("t"),
            title_json: &title_json,
            bg: &bg,
            bgm: &bgm,
            out: &out,
        };
        let err = compose_video_with_runner(&inputs, |_, _| {
            panic!("runner 不应被调用：应该在存在性检查这一步就失败")
        })
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("字幕"), "vtt 缺失时应点名“字幕”：{msg}");
        assert!(
            msg.contains(&vtt.display().to_string()),
            "应包含 vtt 的路径：{msg}"
        );
        assert!(!msg.contains("音频"), "vtt 缺失时不应误报“音频”：{msg}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
