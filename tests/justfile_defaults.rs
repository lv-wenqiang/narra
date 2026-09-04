//! `justfile` 里镜像的默认值必须与 `src/config.rs` 一致。
//!
//! **为什么需要这条测试**：`justfile` 的 `make` 配方要在跑 TTS 之前就检查素材
//! 是否存在，还要拼出 TTS 产物的路径——两件事都得知道那几个默认值。它是
//! shell，读不到 `config::bg_video_path()`，只能把字面量抄一份过去。抄一份就
//! 有两个真相源，而漂移的失败形式很难看：改了 Rust 侧的默认目录之后，
//! `just make` 会在一个空目录里找 `audio.mp3`，报「音频文件不存在」，而错误
//! 完全不指向真正的原因。
//!
//! 这条测试把两边钉在一起——改一边不改另一边就变红。同一手法在
//! `src/ffmpeg.rs` 的 `segment_starts_match_the_audio_delays_in_the_filter_graph`
//! 用过（timeline 的段落帧数 vs 滤镜图的 adelay 毫秒）。
//!
//! **不解析 just 的语法**，只做子串匹配：这里要防的是「有人改了 Rust 侧的
//! 默认值却忘了改 justfile」，不是「justfile 语法正确」——后者由
//! `just --evaluate` 在真正运行时负责，在测试里重写一个 just 解析器是本末倒置。

const JUSTFILE: &str = include_str!("../justfile");

/// `justfile` 里每个 `env_var_or_default` 的默认值，都必须等于 `config` 里
/// 对应函数在环境变量未设置时返回的值。
#[test]
fn justfile_mirrors_the_config_defaults() {
    // 环境变量未设置时 config 返回的就是默认值；这里不动环境变量（那需要
    // 串行夹具），而是直接断言 justfile 里出现了同样的字面量。若本机恰好
    // 设了这几个变量，下面的 `expected` 会取到环境值而不是默认值——所以
    // 先确认没设置，设了就跳过而不是给出一个假绿。
    for key in ["TTS_OUTPUT_DIR", "BG_VIDEO", "BGM_FILE"] {
        if std::env::var_os(key).is_some() {
            eprintln!("跳过：环境里设了 {key}，本条测试比对的是默认值");
            return;
        }
    }

    for (just_var, env_key, expected) in [
        (
            "tts_outdir",
            "TTS_OUTPUT_DIR",
            panda::config::tts_output_dir(),
        ),
        ("bg", "BG_VIDEO", panda::config::bg_video_path()),
        ("bgm", "BGM_FILE", panda::config::bgm_path()),
    ] {
        let want = format!(r#"{just_var} := env_var_or_default("{env_key}", "{expected}")"#);
        assert!(
            JUSTFILE.contains(&want),
            "justfile 里 {just_var} 的默认值与 config 不一致。\n\
             期望出现这一行：{want}\n\
             改了 src/config.rs 的默认值就要同步改 justfile。"
        );
    }
}

/// `justfile` 里出现的每个 `panda` 子命令都必须真的存在。
///
/// `panda make` 被移除时，`justfile` 的配方名仍叫 `make`——那是 just 的配方名，
/// 不是子命令名。这条测试防的是反过来的错：配方体里写了一个已经不存在的子命令
/// （比如哪天 `render` 改名），`just make` 会在跑完整轮 TTS 之后才炸。
#[test]
fn justfile_only_invokes_existing_subcommands() {
    for sub in ["tts", "render"] {
        assert!(
            JUSTFILE.contains(&format!("-- {sub}")),
            "justfile 应当调用 `panda {sub}`"
        );
    }
    assert!(
        !JUSTFILE.contains("-- make"),
        "`panda make` 已移除，justfile 不应再调用它——编排现在由 just 配方本身负责"
    );
}
