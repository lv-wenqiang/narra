use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::time::Duration;

use panda::config;
use panda::config::{Branding, SfxSources};
use panda::render::canvas::Canvas;
use panda::render::frame::FrameSource;
use panda::render::timeline::FPS;
use panda::tmp::TempPath;
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
        /// 标题，默认取品牌名
        #[arg(long)]
        title: Option<String>,
        /// 品牌名，画在封面上排与片尾大字上；不给则取 $BRAND，再不给为「墨风」
        #[arg(long)]
        brand: Option<String>,
        /// 正文左下角水印文案；不给则取 $WATERMARK，再不给则不画
        #[arg(long)]
        watermark: Option<String>,
        /// 封面与片尾的水印文案；不给则取 $WATERMARK_COVER，再不给则不画
        #[arg(long)]
        watermark_cover: Option<String>,
        /// 水印文字左侧的图标（.svg 或 .png，两处水印共用）；不给则取 $WATERMARK_ICON，再不给则不画
        #[arg(long)]
        watermark_icon: Option<String>,
        /// 封面上排与片尾的 logo（.svg 或 .png）；不给则取 $LOGO_FILE，再不给用内嵌的那张
        #[arg(long)]
        logo: Option<String>,
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
        /// 品牌名，画在封面上排与片尾大字上；不给则取 $BRAND，再不给为「墨风」
        #[arg(long)]
        brand: Option<String>,
        /// 正文左下角水印文案；不给则取 $WATERMARK，再不给则不画
        #[arg(long)]
        watermark: Option<String>,
        /// 封面与片尾的水印文案；不给则取 $WATERMARK_COVER，再不给则不画
        #[arg(long)]
        watermark_cover: Option<String>,
        /// 水印文字左侧的图标（.svg 或 .png，两处水印共用）；不给则取 $WATERMARK_ICON，再不给则不画
        #[arg(long)]
        watermark_icon: Option<String>,
        /// 封面上排与片尾的 logo（.svg 或 .png）；不给则取 $LOGO_FILE，再不给用内嵌的那张
        #[arg(long)]
        logo: Option<String>,
        /// 标题 JSON，默认 public/video/title.json
        #[arg(long)]
        title_json: Option<PathBuf>,
        /// 背景视频，默认 public/video/0.mp4
        #[arg(long)]
        bg: Option<PathBuf>,
        /// 背景音乐，默认 public/bgm/0.mp3
        #[arg(long)]
        bgm: Option<PathBuf>,
        /// 片尾音效 mp3；不给则取 $SFX_INTRO，再不给用内嵌的那段
        #[arg(long)]
        sfx_intro: Option<PathBuf>,
        /// 打字机音效 mp3；不给则取 $SFX_TYPEWRITER，再不给用内嵌的那段
        #[arg(long)]
        sfx_typewriter: Option<PathBuf>,
        /// 成片输出，默认 output/video/video.mp4
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

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
    branding: Branding,
    out: PathBuf,
    frames: Option<String>,
) -> Result<()> {
    let vtt_text = std::fs::read_to_string(&vtt)
        .with_context(|| format!("读取 VTT 文件失败：{}", vtt.display()))?;
    let title = config::non_blank(title).unwrap_or_else(|| branding.brand.clone());

    let mut source = FrameSource::new(&vtt_text, title, &branding, Canvas::BASE)?;
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

/// 跑一遍 TTS 流水线，返回**实际使用的输出目录**——`make` 需要它来定位
/// `audio.mp3`/`audio.vtt`，所以兜底后的目录必须由本函数交回调用方，而不是
/// 让调用方各自再算一遍（两处各算一次就是两个真相源）。
///
/// `panda tts` 与 `panda make` 共用这一条 TTS 路径：音色、并发段数、超时的
/// 环境变量兜底只在这里出现一次。
async fn run_tts(
    input: Option<PathBuf>,
    outdir: Option<PathBuf>,
    voice: Option<String>,
    batch_size: Option<usize>,
) -> Result<PathBuf> {
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
            None => {
                config::resolve_batch_size(std::env::var("EDGE_TTS_BATCH_SIZE").ok().as_deref())
            }
        },
        timeout: Duration::from_millis(config::resolve_timeout_ms(
            std::env::var("EDGE_TTS_TIMEOUT_MS").ok().as_deref(),
        )),
    };

    process_narration_file(&input, &outdir, &opts).await?;
    Ok(outdir)
}

/// 标题三级兜底的 IO 外壳：读文件（缺失或不可读视作「没有 JSON」），
/// 纯粹的优先级逻辑交给 `config::resolve_title`。
fn read_title(cli: Option<&str>, json_path: &Path, brand: &str) -> String {
    let json_text = std::fs::read_to_string(json_path).ok();
    config::resolve_title(cli, json_text.as_deref(), brand)
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

/// 落实两段音效的最终路径：自备的直接用（先校验存在），没自备的才把内嵌那份
/// 写进临时目录。
///
/// **两段都自备时完全不落盘**：内嵌音效落到临时目录只是为了给 ffmpeg 一个
/// 文件路径（stdin 已被帧流占用），用户自备时那一步没有意义。
///
/// **自备文件的存在性在这里报错而不是留给 ffmpeg**：ffmpeg 对缺失输入的报错
/// 混在一大段滤镜图日志里，且要等整条管道起来才发生；这里点名哪一段缺失。
fn resolve_sfx_paths(sfx: &SfxSources, tmp_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    for (label, p) in [("片尾音效", &sfx.intro), ("打字机音效", &sfx.typewriter)] {
        if let Some(p) = p
            && !p.exists()
        {
            anyhow::bail!("{label}文件不存在：{}", p.display());
        }
    }
    if let (Some(i), Some(t)) = (&sfx.intro, &sfx.typewriter) {
        return Ok((i.clone(), t.clone()));
    }
    let (intro, typewriter) = write_embedded_audio_checked(tmp_dir)?;
    Ok((
        sfx.intro.clone().unwrap_or(intro),
        sfx.typewriter.clone().unwrap_or(typewriter),
    ))
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
/// `compose_video`。
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
        canvas: source.canvas(),
    }
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
    branding: &'a Branding,
    sfx: &'a SfxSources,
    title_json: &'a Path,
    bg: &'a Path,
    bgm: &'a Path,
    out: &'a Path,
}

/// `render` 与 `make` 收敛到同一条合成路径的那个汇合点：把「音频 / 字幕 /
/// 标题」这三个来源不同的输入（`render` 来自命令行，`make` 来自刚跑完的
/// TTS）与四条已兜底的素材路径拼成 `ComposeVideoInputs`。
///
/// 两个分支都只经由本函数构造合成输入，装配错位（`audio`/`vtt` 对调、
/// `bg`/`bgm` 对调、`title_json`/`out` 对调）就只有一处可能发生，一条单测
/// 同时守住两个分支。
fn compose_inputs<'a>(
    audio: &'a Path,
    vtt: &'a Path,
    title: Option<&'a str>,
    branding: &'a Branding,
    sfx: &'a SfxSources,
    paths: &'a ResolvedRenderPaths,
) -> ComposeVideoInputs<'a> {
    ComposeVideoInputs {
        audio,
        vtt,
        title,
        branding,
        sfx,
        title_json: &paths.title_json,
        bg: &paths.bg,
        bgm: &paths.bgm,
        out: &paths.out,
    }
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
/// `compose_video`；`panda render` 与 `panda make` 都经由 `compose_video`
/// 走这一条路径，没有第二条合成路径。
fn compose_video_with_runner(
    inputs: &ComposeVideoInputs,
    runner: impl FnOnce(&mut FrameSource, &panda::ffmpeg::RenderInputs) -> Result<()>,
) -> Result<()> {
    let ComposeVideoInputs {
        audio,
        vtt,
        title,
        branding,
        sfx,
        title_json,
        bg,
        bgm,
        out,
    } = *inputs;

    check_render_inputs_exist(audio, vtt, bg, bgm)?;

    let resolved_title = read_title(title, title_json, &branding.brand);
    let vtt_text =
        std::fs::read_to_string(vtt).with_context(|| format!("读取字幕失败：{}", vtt.display()))?;

    // 两段内嵌音效落到临时目录，供 ffmpeg 作为输入文件读取——stdin
    // 已经被帧流占用，没法再从管道喂第二、第三份数据。
    // 作用域守卫：`Drop` 里删目录。此前是「`create_dir_all` → …… →
    // `cleanup_tmp_and_propagate`」的写法，中间夹着两个 `?` 出口
    // （`resolve_sfx_paths` 与 `FrameSource::new`），走那两条路时临时目录连同
    // 两个 mp3 泄漏在 /tmp；注入的 `runner` 闭包 panic 时同样跳过清理。
    // 把清理挂到作用域上之后，出口有几个、走的是 `?` 还是 unwind 都不必再数。
    let tmp = TempPath::create_dir("panda_render")?;
    let (intro, typewriter) = resolve_sfx_paths(sfx, tmp.path())?;

    let mut source = FrameSource::new(&vtt_text, resolved_title.clone(), branding, Canvas::BASE)?;

    println!(
        "标题「{resolved_title}」，音频 {:.2}s，共 {} 帧（{:.2}s），输出 {}",
        source.audio_secs(),
        source.total_frames(),
        source.total_frames() as f64 / FPS as f64,
        out.display()
    );

    let render_inputs = build_render_inputs(&source, bg, audio, bgm, &typewriter, &intro, out);
    let result = runner(&mut source, &render_inputs);
    // `tmp` 的清理由 `Drop` 负责（含下面 `?` 提前返回与 panic 展开两条路）。
    cleanup_output_on_failure(out, result)?;

    println!("成片已写入 {}", out.display());
    Ok(())
}

/// `compose_video_with_runner` 接上真正的 ffmpeg 执行器。全仓库唯一的一条
/// 合成路径：`Commands::Render` 与 `Commands::Make` 的分支体都只是它的一层
/// 薄胶水（备齐输入 → `compose_inputs` → 调用本函数），区别仅在音频/字幕
/// 是命令行给的还是刚跑完的 TTS 产的。
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
            run_tts(input, outdir, voice, batch_size).await?;
            Ok(())
        }
        Commands::DebugFrames {
            vtt,
            title,
            brand,
            watermark,
            watermark_cover,
            watermark_icon,
            logo,
            out,
            frames,
        } => run_debug_frames(
            vtt,
            title,
            Branding::resolve(brand, watermark, watermark_cover, watermark_icon, logo),
            out,
            frames,
        ),
        Commands::Render {
            audio,
            vtt,
            title,
            brand,
            watermark,
            watermark_cover,
            watermark_icon,
            logo,
            title_json,
            bg,
            bgm,
            sfx_intro,
            sfx_typewriter,
            out,
        } => {
            let paths = resolve_render_paths(title_json, bg, bgm, out);
            let branding =
                Branding::resolve(brand, watermark, watermark_cover, watermark_icon, logo);
            let sfx = SfxSources::resolve(sfx_intro, sfx_typewriter);
            compose_video(&compose_inputs(
                &audio,
                &vtt,
                title.as_deref(),
                &branding,
                &sfx,
                &paths,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用品牌名，**刻意不等于生产默认值「墨风」**——若写成默认值，
    /// 把兜底错写成硬编码字面量的变异就检不出来。
    const TEST_BRAND: &str = "测试品牌";

    #[test]
    fn read_title_uses_cli_when_given() {
        let missing = std::path::Path::new("/nonexistent-title-xyz.json");
        assert_eq!(
            read_title(Some("命令行标题"), missing, TEST_BRAND),
            "命令行标题"
        );
    }

    #[test]
    fn read_title_falls_back_to_brand_when_json_is_missing() {
        // 标题 JSON 是可选素材：文件不存在不应报错，应静默回落品牌名。
        let missing = std::path::Path::new("/nonexistent-title-xyz.json");
        assert_eq!(read_title(None, missing, TEST_BRAND), TEST_BRAND);
    }

    #[test]
    fn read_title_reads_the_json_file_when_it_exists() {
        let p = std::env::temp_dir().join(format!("panda_title_{}.json", std::process::id()));
        std::fs::write(&p, r#"{"title": "文件里的标题"}"#).unwrap();
        assert_eq!(read_title(None, &p, TEST_BRAND), "文件里的标题");
        assert_eq!(
            read_title(Some("覆盖"), &p, TEST_BRAND),
            "覆盖",
            "CLI 优先级最高"
        );
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
    /// 两段音效的来源解析：自备优先、缺失点名、两段都自备时完全不落盘。
    ///
    /// 「不落盘」这一条是有意断言的：内嵌音效写进临时目录只是为了给 ffmpeg
    /// 一个文件路径（stdin 已被帧流占用），两段都自备时那一步没有意义。把
    /// `resolve_sfx_paths` 写成「无条件先写内嵌再覆盖路径」的变异，只看返回
    /// 值是察觉不到的——这里直接断言临时目录里没有多出文件。
    #[test]
    fn sfx_prefers_user_files_and_only_writes_the_embedded_ones_when_needed() {
        let dir =
            std::env::temp_dir().join(format!("panda_sfx_{}_{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let mine_a = dir.join("mine_a.mp3");
        let mine_b = dir.join("mine_b.mp3");
        std::fs::write(&mine_a, b"fake").unwrap();
        std::fs::write(&mine_b, b"fake").unwrap();

        let tmp = dir.join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();

        // 两段都自备：原样返回，且临时目录仍是空的。
        let both = SfxSources {
            intro: Some(mine_a.clone()),
            typewriter: Some(mine_b.clone()),
        };
        let (i, t) = resolve_sfx_paths(&both, &tmp).unwrap();
        assert_eq!((i, t), (mine_a.clone(), mine_b.clone()));
        assert_eq!(
            std::fs::read_dir(&tmp).unwrap().count(),
            0,
            "两段都自备时不应把内嵌音效写进临时目录"
        );

        // 只自备片尾音效：打字机回落内嵌，且自备那段没被换掉。
        let only_intro = SfxSources {
            intro: Some(mine_a.clone()),
            typewriter: None,
        };
        let (i, t) = resolve_sfx_paths(&only_intro, &tmp).unwrap();
        assert_eq!(i, mine_a, "自备的片尾音效应原样保留");
        assert_eq!(
            t.file_name().and_then(|f| f.to_str()),
            Some("intro_typewriter.mp3"),
            "没自备的打字机音效应回落到内嵌那份"
        );

        // 两段都不自备：都走内嵌。
        let (i, t) = resolve_sfx_paths(&SfxSources::default(), &tmp).unwrap();
        assert_eq!(i.file_name().and_then(|f| f.to_str()), Some("intro.mp3"));
        assert_eq!(
            t.file_name().and_then(|f| f.to_str()),
            Some("intro_typewriter.mp3")
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 自备的音效文件不存在时在这里报错并点名是哪一段，而不是留给 ffmpeg。
    #[test]
    fn missing_user_supplied_sfx_is_reported_by_name() {
        let tmp = std::env::temp_dir().join(format!("panda_sfx_missing_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let err = resolve_sfx_paths(
            &SfxSources {
                intro: Some(PathBuf::from("/nonexistent-intro-xyz.mp3")),
                typewriter: None,
            },
            &tmp,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("片尾音效"), "应点名是片尾音效：{err}");
        assert!(
            err.contains("nonexistent-intro-xyz.mp3"),
            "应点名文件：{err}"
        );

        let err = resolve_sfx_paths(
            &SfxSources {
                intro: None,
                typewriter: Some(PathBuf::from("/nonexistent-typewriter-xyz.mp3")),
            },
            &tmp,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("打字机音效"), "应点名是打字机音效：{err}");

        std::fs::remove_dir_all(&tmp).ok();
    }

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
    ///
    /// **Fix round 1**：`canvas` 字段此前漏在断言之外——本条测试的名字与
    /// 文档都说"every field"，实际却少了这一个。`source` 特意建在一个非
    /// `Canvas::BASE` 的画布上：若仍用 BASE，`inputs.canvas ==
    /// Canvas::BASE` 这条断言即使把 `build_render_inputs` 里的 `canvas:
    /// source.canvas()` 错填成 `canvas: Canvas::BASE`也会侥幸通过。
    #[test]
    fn build_render_inputs_wires_every_field_without_swapping() {
        let source = FrameSource::new(
            RENDER_TEST_VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas { w: 1920, h: 1080 },
        )
        .unwrap();
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

        assert_eq!(
            inputs.canvas,
            source.canvas(),
            "canvas 字段应原样来自 source.canvas()，而不是写死的 Canvas::BASE"
        );
    }

    /// **临时目录在所有出口上都被清理**（销 `docs/follow-ups.md`「族 A」）。
    ///
    /// 覆盖三条出口：runner 成功、runner 返回 `Err`、**runner panic**。第三条
    /// 是此前真正漏掉的那一条——清理写在 `runner(..)` 之后的一行上，panic
    /// 展开会直接越过它，临时目录连同两个 mp3（约 120 KB）留在 `/tmp`。
    ///
    /// 判据不靠「扫 /tmp 找残留」（同一进程里并发跑的其它测试也会在那儿建
    /// 目录，会互相干扰），而是让注入的 runner **把临时目录的路径捞出来**：
    /// `render_inputs.intro` 正指向临时目录里的那份内嵌音效，取它的父目录即可。
    /// 调用返回后断言那个具体路径不存在——确定性，无竞态。
    #[test]
    fn compose_video_cleans_up_the_tmp_dir_on_every_exit_path() {
        let dir = std::env::temp_dir().join(format!("panda_compose_leak_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("a.mp3");
        let bg = dir.join("bg.mp4");
        let bgm = dir.join("bgm.mp3");
        let vtt = dir.join("a.vtt");
        let title_json = dir.join("no_such_title.json");
        let out = dir.join("out.mp4");
        for p in [&audio, &bg, &bgm] {
            std::fs::write(p, b"x").unwrap();
        }
        std::fs::write(&vtt, RENDER_TEST_VTT).unwrap();

        let branding = Branding::plain(TEST_BRAND);
        let sfx = SfxSources::default();
        let make_inputs = || ComposeVideoInputs {
            branding: &branding,
            sfx: &sfx,
            audio: &audio,
            vtt: &vtt,
            title: Some("测试标题"),
            title_json: &title_json,
            bg: &bg,
            bgm: &bgm,
            out: &out,
        };

        // 出口 1：runner 成功。
        let seen = std::cell::Cell::new(PathBuf::new());
        compose_video_with_runner(&make_inputs(), |_, ri| {
            seen.set(ri.intro.parent().unwrap().to_path_buf());
            Ok(())
        })
        .unwrap();
        let tmp_ok = seen.take();
        assert!(!tmp_ok.as_os_str().is_empty(), "runner 应当被调用过");
        assert!(!tmp_ok.exists(), "成功路径应清理临时目录：{tmp_ok:?}");

        // 出口 2：runner 返回 Err。
        let seen = std::cell::Cell::new(PathBuf::new());
        let err = compose_video_with_runner(&make_inputs(), |_, ri| {
            seen.set(ri.intro.parent().unwrap().to_path_buf());
            Err(anyhow::anyhow!("模拟 run_render 失败"))
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "模拟 run_render 失败", "错误应原样透传");
        let tmp_err = seen.take();
        assert!(!tmp_err.exists(), "失败路径应清理临时目录：{tmp_err:?}");

        // 出口 3：runner panic —— 此前漏掉的那一条。
        let captured = std::sync::Arc::new(std::sync::Mutex::new(PathBuf::new()));
        let sink = std::sync::Arc::clone(&captured);
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // 别把预期中的 panic 打到测试输出里
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compose_video_with_runner(&make_inputs(), |_, ri| {
                *sink.lock().unwrap() = ri.intro.parent().unwrap().to_path_buf();
                panic!("模拟 runner 炸掉");
            })
        }));
        std::panic::set_hook(hook);
        assert!(unwound.is_err(), "runner 应当确实 panic 了");
        let tmp_panic = captured.lock().unwrap().clone();
        assert!(
            !tmp_panic.as_os_str().is_empty(),
            "panic 前应已记下临时目录"
        );
        assert!(
            !tmp_panic.exists(),
            "panic 展开时也应清理临时目录，这正是「族 A」要堵的那一条：{tmp_panic:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
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

        let branding = Branding::plain(TEST_BRAND);
        let sfx = SfxSources::default();
        let inputs = ComposeVideoInputs {
            branding: &branding,
            sfx: &sfx,
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
        let branding = Branding::plain(TEST_BRAND);
        let sfx = SfxSources::default();
        let inputs = ComposeVideoInputs {
            branding: &branding,
            sfx: &sfx,
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
        let branding = Branding::plain(TEST_BRAND);
        let sfx = SfxSources::default();
        let inputs = ComposeVideoInputs {
            branding: &branding,
            sfx: &sfx,
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

    /// 变异实验：`compose_inputs` 里把 `audio`/`vtt`、`bg`/`bgm` 或
    /// `title_json`/`out` 任意一对填反。这是 `render` 与 `make` 唯一的合成
    /// 输入装配点，一条测试同时守住两个分支。
    #[test]
    fn compose_inputs_wires_the_resolved_paths_without_swapping() {
        let paths = resolve_render_paths(
            Some(PathBuf::from("/given/title.json")),
            Some(PathBuf::from("/given/bg.mp4")),
            Some(PathBuf::from("/given/bgm.mp3")),
            Some(PathBuf::from("/given/out.mp4")),
        );
        let audio = PathBuf::from("/given/audio.mp3");
        let vtt = PathBuf::from("/given/audio.vtt");

        let branding = Branding::plain(TEST_BRAND);
        let sfx = SfxSources::default();
        let inputs = compose_inputs(&audio, &vtt, Some("标题"), &branding, &sfx, &paths);

        assert_eq!(inputs.audio, audio.as_path());
        assert_eq!(inputs.vtt, vtt.as_path());
        assert_eq!(inputs.title, Some("标题"));
        assert_eq!(inputs.title_json, paths.title_json.as_path());
        assert_eq!(inputs.bg, paths.bg.as_path());
        assert_eq!(inputs.bgm, paths.bgm.as_path());
        assert_eq!(inputs.out, paths.out.as_path());
    }

    /// 一条龙侧的接线：TTS 跑完之后，喂给合成的 `audio`/`vtt` 必须正是
    /// `pipeline` 那两个常量约定的文件，且顺序没有对调（mp3 进 `audio`、
    /// vtt 进 `vtt`）——两者都是 `&Path`，编译器分不出来。
    ///
    /// 编排本身现在在 `justfile` 里，但这条接线约定仍是 Rust 侧的事：
    /// `justfile` 拼路径靠的就是这两个常量（见 `tests/justfile_defaults.rs`）。
    #[test]
    fn tts_artifacts_feed_into_the_shared_compose_inputs() {
        let outdir = PathBuf::from("/tmp/panda-make-tts-out");
        let audio = outdir.join(panda::tts::pipeline::AUDIO_FILE_NAME);
        let vtt = outdir.join(panda::tts::pipeline::VTT_FILE_NAME);
        let paths = resolve_render_paths(None, None, None, None);

        let branding = Branding::plain(TEST_BRAND);
        let sfx = SfxSources::default();
        let inputs = compose_inputs(&audio, &vtt, None, &branding, &sfx, &paths);

        assert_eq!(
            inputs.audio,
            Path::new("/tmp/panda-make-tts-out/audio.mp3"),
            "audio 应指向 TTS 的 mp3 产物"
        );
        assert_eq!(
            inputs.vtt,
            Path::new("/tmp/panda-make-tts-out/audio.vtt"),
            "vtt 应指向 TTS 的 vtt 产物"
        );
        // 与 render 分支相同的素材兜底，不是 make 自己另算一份。
        assert_eq!(inputs.bg, Path::new(&config::bg_video_path()));
        assert_eq!(inputs.bgm, Path::new(&config::bgm_path()));
        assert_eq!(inputs.out, Path::new(&config::video_output_path()));
    }
}
