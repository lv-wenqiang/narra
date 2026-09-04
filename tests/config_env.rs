//! `src/config.rs` 里读 `std::env` 的函数的测试，单独放在这个独立测试
//! 二进制里，而不是塞进 `src/config.rs` 的 `mod tests`。
//!
//! 原因有两层：
//!
//! 1. `env_backed_paths_read_the_environment_and_fall_back` 用
//!    `unsafe { std::env::set_var / remove_var }` 串行摆弄
//!    `SPIDER_OUTPUT_DIR` / `TTS_OUTPUT_DIR` / `TTS_INPUT_FILE`。
//!    `src/config.rs` 的单测与 `src/tts/pipeline.rs` 里非 `#[ignore]`
//!    的测试处于**同一个** lib 测试二进制、同一个进程；后者会经 PATH
//!    真实 spawn ffmpeg（`Command::spawn` 内部要读 environ）。Rust 2024
//!    把 `set_var` 标成 unsafe，正是因为它与其它线程的 `getenv`（含
//!    `Command::spawn`、`std::env::temp_dir()` 读 TMPDIR）并发是 UB——
//!    这不是靠一把互斥锁缩小窗口就能免责的事，必须让改环境变量这件事
//!    发生在一个不跑其它测试的独立进程里。这与 Task 5 修掉的
//!    `tests/ffmpeg_missing.rs`（改 PATH）是同一类问题、同一个解法。
//!
//! 2. `material_paths_have_the_documented_defaults` 断言的是
//!    `bg_video_path` / `bgm_path` / `title_json_path` / `video_output_path`
//!    在**没有设置** `BG_VIDEO` / `BGM_FILE` / `TITLE_JSON` / `VIDEO_OUTPUT`
//!    时的默认值——它看起来是纯函数测试，实际上依赖「进程环境没设这几个
//!    变量」这个测试自己并不保证的前提。放在这里可以在断言前显式
//!    `remove_var` 清空，把这个前提变成确定性的，而不是寄望于运行测试的
//!    环境恰好没设这些变量。

use std::path::Path;
use std::sync::Mutex;

/// 本文件所有测试共享同一个环境变量命名空间，互相之间必须串行，
/// 防止一条测试设置的变量被另一条测试的断言窗口看到。
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn material_paths_have_the_documented_defaults() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: 拿到 ENV_LOCK 后，本文件里其它测试不会并发读写这几个变量；
    // 本进程（这个独立测试二进制）里没有其它代码路径会 spawn 子进程或
    // 并发调用 getenv。
    unsafe {
        std::env::remove_var("BG_VIDEO");
        std::env::remove_var("BGM_FILE");
        std::env::remove_var("TITLE_JSON");
        std::env::remove_var("VIDEO_OUTPUT");
    }
    assert_eq!(panda::config::bg_video_path(), "public/video/0.mp4");
    assert_eq!(panda::config::bgm_path(), "public/bgm/0.mp3");
    assert_eq!(panda::config::title_json_path(), "public/video/title.json");
    assert_eq!(panda::config::video_output_path(), "output/video/video.mp4");
}

/// 鉴别性测试：把「哪个值属于哪个名字」作为一个整体断言，而不是逐项检查。
/// 逐项检查（`bg_video_path()` 应含 "video"，`bgm_path()` 应含 "bgm"...）
/// 抓不住「把两个函数的返回值对调」或「把两个函数读的环境变量名互相对调」
/// 这类错误——因为对调后每一项单独看仍然「像」一个合理的路径。这里改用
/// 「设置一个变量，断言只有对应的那个函数变了、其它三个都没变」的写法，
/// 把名字与函数的绑定关系当整体钉住。
#[test]
fn each_material_env_var_is_wired_to_exactly_one_function() {
    let _guard = ENV_LOCK.lock().unwrap();
    let clear_all = || {
        // SAFETY: 持有 ENV_LOCK，本文件内串行。
        unsafe {
            std::env::remove_var("BG_VIDEO");
            std::env::remove_var("BGM_FILE");
            std::env::remove_var("TITLE_JSON");
            std::env::remove_var("VIDEO_OUTPUT");
        }
    };

    let snapshot = || {
        (
            panda::config::bg_video_path(),
            panda::config::bgm_path(),
            panda::config::title_json_path(),
            panda::config::video_output_path(),
        )
    };

    clear_all();
    let defaults = snapshot();

    // 设置 BG_VIDEO：只有 bg_video_path() 应该变。
    // SAFETY: 持有 ENV_LOCK。
    unsafe { std::env::set_var("BG_VIDEO", "/tmp/custom_bg.mp4") };
    let after_bg = snapshot();
    assert_eq!(after_bg.0, "/tmp/custom_bg.mp4");
    assert_eq!(after_bg.1, defaults.1, "BGM_FILE 不该受 BG_VIDEO 影响");
    assert_eq!(after_bg.2, defaults.2, "TITLE_JSON 不该受 BG_VIDEO 影响");
    assert_eq!(after_bg.3, defaults.3, "VIDEO_OUTPUT 不该受 BG_VIDEO 影响");
    clear_all();

    // 设置 BGM_FILE：只有 bgm_path() 应该变。
    unsafe { std::env::set_var("BGM_FILE", "/tmp/custom_bgm.mp3") };
    let after_bgm = snapshot();
    assert_eq!(after_bgm.1, "/tmp/custom_bgm.mp3");
    assert_eq!(after_bgm.0, defaults.0, "BG_VIDEO 不该受 BGM_FILE 影响");
    assert_eq!(after_bgm.2, defaults.2, "TITLE_JSON 不该受 BGM_FILE 影响");
    assert_eq!(after_bgm.3, defaults.3, "VIDEO_OUTPUT 不该受 BGM_FILE 影响");
    clear_all();

    // 设置 TITLE_JSON：只有 title_json_path() 应该变。
    unsafe { std::env::set_var("TITLE_JSON", "/tmp/custom_title.json") };
    let after_title = snapshot();
    assert_eq!(after_title.2, "/tmp/custom_title.json");
    assert_eq!(after_title.0, defaults.0, "BG_VIDEO 不该受 TITLE_JSON 影响");
    assert_eq!(after_title.1, defaults.1, "BGM_FILE 不该受 TITLE_JSON 影响");
    assert_eq!(
        after_title.3, defaults.3,
        "VIDEO_OUTPUT 不该受 TITLE_JSON 影响"
    );
    clear_all();

    // 设置 VIDEO_OUTPUT：只有 video_output_path() 应该变。
    unsafe { std::env::set_var("VIDEO_OUTPUT", "/tmp/custom_out.mp4") };
    let after_out = snapshot();
    assert_eq!(after_out.3, "/tmp/custom_out.mp4");
    assert_eq!(after_out.0, defaults.0, "BG_VIDEO 不该受 VIDEO_OUTPUT 影响");
    assert_eq!(after_out.1, defaults.1, "BGM_FILE 不该受 VIDEO_OUTPUT 影响");
    assert_eq!(
        after_out.2, defaults.2,
        "TITLE_JSON 不该受 VIDEO_OUTPUT 影响"
    );
    clear_all();
}

/// 偿还 follow-ups「值得做」第 2 条：`spider_output_dir` / `tts_output_dir` /
/// `tts_input_file` 三个读 `std::env` 的函数此前零测试。
/// 环境变量是进程级全局状态，这些断言必须在同一个测试里串行做，
/// 否则并行测试之间会互相干扰；本文件另外用 ENV_LOCK 防止与本文件
/// 其它测试的窗口重叠。
#[test]
fn env_backed_paths_read_the_environment_and_fall_back() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: 持有 ENV_LOCK，本测试二进制里没有其它代码会并发 spawn
    // 子进程或读取这几个变量。
    unsafe {
        std::env::remove_var("SPIDER_OUTPUT_DIR");
        std::env::remove_var("TTS_OUTPUT_DIR");
        std::env::remove_var("TTS_INPUT_FILE");
    }
    assert_eq!(panda::config::spider_output_dir(), "output/spider");
    assert_eq!(panda::config::tts_output_dir(), "output/tts");
    assert_eq!(
        panda::config::tts_input_file(),
        "output/spider/input.txt",
        "应基于 SPIDER_OUTPUT_DIR 推导"
    );

    unsafe { std::env::set_var("SPIDER_OUTPUT_DIR", "/tmp/spider") };
    assert_eq!(panda::config::spider_output_dir(), "/tmp/spider");
    assert_eq!(
        panda::config::tts_input_file(),
        "/tmp/spider/input.txt",
        "推导应跟随 SPIDER_OUTPUT_DIR"
    );

    // 空白值等同于未设置
    unsafe { std::env::set_var("TTS_OUTPUT_DIR", "   ") };
    assert_eq!(
        panda::config::tts_output_dir(),
        "output/tts",
        "全空白应回落默认值"
    );

    unsafe { std::env::set_var("TTS_INPUT_FILE", "/tmp/x.txt") };
    assert_eq!(
        panda::config::tts_input_file(),
        "/tmp/x.txt",
        "显式设置应优先于推导"
    );

    unsafe {
        std::env::remove_var("SPIDER_OUTPUT_DIR");
        std::env::remove_var("TTS_OUTPUT_DIR");
        std::env::remove_var("TTS_INPUT_FILE");
    }
}

/// 品牌名与两条水印文案的默认值。
///
/// **默认水印是「不画」而不是「画一段空字符串」**：`None` 与 `Some("")` 在
/// 渲染层是两件事——后者会走完整条预渲染 + 贴图路径，画出一个零宽的墨迹块，
/// 还会白付 `prepare_watermark` 的开销。这里把「未配置 = None」钉死。
#[test]
fn brand_and_watermarks_have_the_documented_defaults() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: 同本文件其它测试，持有 ENV_LOCK 期间本进程串行。
    unsafe {
        std::env::remove_var("BRAND");
        std::env::remove_var("WATERMARK");
        std::env::remove_var("WATERMARK_COVER");
        std::env::remove_var("WATERMARK_ICON");
        std::env::remove_var("LOGO_FILE");
        std::env::remove_var("SFX_INTRO");
        std::env::remove_var("SFX_TYPEWRITER");
    }
    assert_eq!(panda::config::brand(), "墨风");
    assert_eq!(panda::config::watermark(), None, "未配置时不画正文水印");
    assert_eq!(
        panda::config::watermark_cover(),
        None,
        "未配置时不画封面/片尾水印"
    );
    assert_eq!(panda::config::watermark_icon(), None, "未配置时不画图标");
    assert_eq!(panda::config::logo_path(), None, "未配置时用内嵌 logo");
    assert_eq!(panda::config::sfx_intro(), None, "未配置时用内嵌片尾音效");
    assert_eq!(
        panda::config::sfx_typewriter(),
        None,
        "未配置时用内嵌打字机音效"
    );
}

/// 鉴别性测试：三个变量各自只驱动一个函数。
///
/// 照 `each_material_env_var_is_wired_to_exactly_one_function` 的写法——
/// 逐项断言抓不住「把 WATERMARK 与 WATERMARK_COVER 读反」这类错误，因为
/// 对调后每一项单独看都仍然「像」一个合理的水印文案。这里改成「设一个、
/// 断言只有对应那个变了、另外两个不变」。
#[test]
fn each_branding_env_var_is_wired_to_exactly_one_function() {
    let _guard = ENV_LOCK.lock().unwrap();
    let clear_all = || {
        // SAFETY: 持有 ENV_LOCK，本文件内串行。
        unsafe {
            std::env::remove_var("BRAND");
            std::env::remove_var("WATERMARK");
            std::env::remove_var("WATERMARK_COVER");
            std::env::remove_var("WATERMARK_ICON");
        }
    };

    clear_all();
    unsafe { std::env::set_var("BRAND", "某某频道") };
    assert_eq!(panda::config::brand(), "某某频道");
    assert_eq!(panda::config::watermark(), None, "BRAND 不应影响正文水印");
    assert_eq!(
        panda::config::watermark_cover(),
        None,
        "BRAND 不应影响封面水印"
    );

    clear_all();
    unsafe { std::env::set_var("WATERMARK", "正文水印") };
    assert_eq!(panda::config::watermark().as_deref(), Some("正文水印"));
    assert_eq!(panda::config::brand(), "墨风", "WATERMARK 不应影响品牌名");
    assert_eq!(
        panda::config::watermark_cover(),
        None,
        "WATERMARK 不应影响封面水印"
    );

    clear_all();
    unsafe { std::env::set_var("WATERMARK_COVER", "封面水印") };
    assert_eq!(
        panda::config::watermark_cover().as_deref(),
        Some("封面水印")
    );
    assert_eq!(panda::config::brand(), "墨风");
    assert_eq!(
        panda::config::watermark(),
        None,
        "WATERMARK_COVER 不应影响正文水印"
    );

    clear_all();
}

/// 三者都走 `non_empty_env` 的「空白视同未设置」语义。
///
/// 这条单独存在，是因为 `docs/follow-ups.md`「ffmpeg 合成 · 值得做 #2」记过
/// 一模一样的缺口：四个素材路径函数的这条分支当时一条测试都没有，把
/// `non_empty_env` 换成裸 `std::env::var().ok()` 的变异**存活**。新加的三个
/// 函数不要重蹈覆辙。
#[test]
fn blank_branding_env_vars_are_treated_as_unset() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: 持有 ENV_LOCK，本文件内串行。
    unsafe {
        std::env::set_var("BRAND", "   ");
        std::env::set_var("WATERMARK", "\t \n");
        std::env::set_var("WATERMARK_COVER", "  ");
        std::env::set_var("WATERMARK_ICON", " \t ");
        std::env::set_var("LOGO_FILE", "   ");
        std::env::set_var("SFX_INTRO", " ");
        std::env::set_var("SFX_TYPEWRITER", "\t");
    }
    assert_eq!(
        panda::config::brand(),
        "墨风",
        "全空白的 BRAND 应回落默认值"
    );
    assert_eq!(
        panda::config::watermark(),
        None,
        "全空白的 WATERMARK 应视同未配置"
    );
    assert_eq!(
        panda::config::watermark_cover(),
        None,
        "全空白的 WATERMARK_COVER 应视同未配置"
    );
    assert_eq!(
        panda::config::watermark_icon(),
        None,
        "全空白的 WATERMARK_ICON 应视同未配置"
    );
    assert_eq!(
        panda::config::logo_path(),
        None,
        "全空白的 LOGO_FILE 应视同未配置"
    );
    assert_eq!(
        panda::config::sfx_intro(),
        None,
        "全空白的 SFX_INTRO 应视同未配置"
    );
    assert_eq!(
        panda::config::sfx_typewriter(),
        None,
        "全空白的 SFX_TYPEWRITER 应视同未配置"
    );

    // SAFETY: 同上。
    unsafe {
        std::env::remove_var("BRAND");
        std::env::remove_var("WATERMARK");
        std::env::remove_var("WATERMARK_COVER");
        std::env::remove_var("WATERMARK_ICON");
        std::env::remove_var("LOGO_FILE");
        std::env::remove_var("SFX_INTRO");
        std::env::remove_var("SFX_TYPEWRITER");
    }
}

/// `Branding::resolve` 的三级兜底：`--flag` > 环境变量 > 默认值。
///
/// 覆盖两件此前零覆盖的事：
///
/// 1. **命令行参数的「全空白视同没给」**——`--brand "  "` 必须继续往下兜底，
///    而不是产出一个空品牌名的片尾。把 `config::non_blank` 退化成恒等函数的
///    变异，只有这条测得出来（`brand()` 那几条走的是 `non_empty_env`，是另
///    一条路径）。
/// 2. **哪个参数落进哪个字段**——三个入参同为 `Option<String>`，相邻两个
///    对调不会编译失败。这里照
///    `each_material_env_var_is_wired_to_exactly_one_function` 的写法，只给
///    一个、断言只有对应那个字段变了。
#[test]
fn branding_resolve_prefers_cli_over_env_over_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    use panda::config::Branding;
    // SAFETY: 持有 ENV_LOCK，本文件内串行。
    let clear = || unsafe {
        std::env::remove_var("BRAND");
        std::env::remove_var("WATERMARK");
        std::env::remove_var("WATERMARK_COVER");
        std::env::remove_var("WATERMARK_ICON");
        std::env::remove_var("LOGO_FILE");
    };

    clear();
    assert_eq!(
        Branding::resolve(None, None, None, None, None),
        Branding::plain("墨风"),
        "四项都没给时应是「默认品牌 + 两处水印与图标都不画」"
    );

    // 环境变量层。
    unsafe {
        std::env::set_var("BRAND", "环境品牌");
        std::env::set_var("WATERMARK", "环境正文水印");
        std::env::set_var("WATERMARK_COVER", "环境封面水印");
        std::env::set_var("WATERMARK_ICON", "/env/mark.svg");
        std::env::set_var("LOGO_FILE", "/env/logo.png");
    }
    let from_env = Branding::resolve(None, None, None, None, None);
    assert_eq!(from_env.brand, "环境品牌");
    assert_eq!(from_env.watermark.as_deref(), Some("环境正文水印"));
    assert_eq!(from_env.watermark_cover.as_deref(), Some("环境封面水印"));
    assert_eq!(from_env.watermark_icon.as_deref(), Some("/env/mark.svg"));
    assert_eq!(from_env.logo.as_deref(), Some("/env/logo.png"));

    // 命令行优先于环境变量，且三个参数各落各的字段。
    let from_cli = Branding::resolve(
        Some("命令行品牌".into()),
        Some("命令行正文水印".into()),
        Some("命令行封面水印".into()),
        Some("/cli/mark.png".into()),
        Some("/cli/logo.svg".into()),
    );
    assert_eq!(from_cli.brand, "命令行品牌");
    assert_eq!(from_cli.watermark.as_deref(), Some("命令行正文水印"));
    assert_eq!(from_cli.watermark_cover.as_deref(), Some("命令行封面水印"));
    assert_eq!(from_cli.watermark_icon.as_deref(), Some("/cli/mark.png"));
    assert_eq!(from_cli.logo.as_deref(), Some("/cli/logo.svg"));

    // 只给一个：另外两个必须仍来自环境变量，不能被这一个带偏。
    let only_brand = Branding::resolve(Some("只给品牌".into()), None, None, None, None);
    assert_eq!(only_brand.brand, "只给品牌");
    assert_eq!(only_brand.watermark.as_deref(), Some("环境正文水印"));
    assert_eq!(only_brand.watermark_cover.as_deref(), Some("环境封面水印"));
    assert_eq!(only_brand.watermark_icon.as_deref(), Some("/env/mark.svg"));
    assert_eq!(only_brand.logo.as_deref(), Some("/env/logo.png"));

    let only_wm = Branding::resolve(None, Some("只给正文水印".into()), None, None, None);
    assert_eq!(only_wm.watermark.as_deref(), Some("只给正文水印"));
    assert_eq!(only_wm.brand, "环境品牌");
    assert_eq!(only_wm.watermark_cover.as_deref(), Some("环境封面水印"));

    let only_cover = Branding::resolve(None, None, Some("只给封面水印".into()), None, None);
    assert_eq!(only_cover.watermark_cover.as_deref(), Some("只给封面水印"));
    assert_eq!(only_cover.brand, "环境品牌");
    assert_eq!(only_cover.watermark.as_deref(), Some("环境正文水印"));

    let only_icon = Branding::resolve(None, None, None, Some("/只给图标.png".into()), None);
    assert_eq!(only_icon.watermark_icon.as_deref(), Some("/只给图标.png"));
    assert_eq!(only_icon.brand, "环境品牌");
    assert_eq!(only_icon.watermark.as_deref(), Some("环境正文水印"));
    assert_eq!(only_icon.watermark_cover.as_deref(), Some("环境封面水印"));
    assert_eq!(only_icon.logo.as_deref(), Some("/env/logo.png"));

    let only_logo = Branding::resolve(None, None, None, None, Some("/只给logo.png".into()));
    assert_eq!(only_logo.logo.as_deref(), Some("/只给logo.png"));
    assert_eq!(only_logo.brand, "环境品牌");
    assert_eq!(only_logo.watermark_icon.as_deref(), Some("/env/mark.svg"));

    // 全空白的命令行参数视同没给，继续往下兜底到环境变量。
    let blank = Branding::resolve(
        Some("   ".into()),
        Some("\t".into()),
        Some("  \n".into()),
        Some(" ".into()),
        Some("\t\t".into()),
    );
    assert_eq!(
        blank, from_env,
        "全空白的命令行参数应视同没给，回落到环境变量层"
    );

    // 环境变量也清掉后，全空白的命令行参数应一路兜底到默认值。
    clear();
    assert_eq!(
        Branding::resolve(
            Some("  ".into()),
            Some("  ".into()),
            Some("  ".into()),
            Some("  ".into()),
            Some("  ".into()),
        ),
        Branding::plain("墨风"),
        "全空白 + 无环境变量应一路兜底到默认值"
    );
}

/// `SfxSources::resolve` 的三级兜底：`--sfx-*` > 环境变量 > 内嵌那段。
///
/// 与 `branding_resolve_prefers_cli_over_env_over_default` 同一套写法与同一个
/// 理由——两个入参同为 `Option<PathBuf>`，对调既不编译失败也不在运行时报错，
/// 只会让打字机音效在片尾响、片尾音效在片头响。
#[test]
fn sfx_resolve_prefers_cli_over_env_over_embedded() {
    let _guard = ENV_LOCK.lock().unwrap();
    use panda::config::SfxSources;
    use std::path::PathBuf;
    // SAFETY: 持有 ENV_LOCK，本文件内串行。
    let clear = || unsafe {
        std::env::remove_var("SFX_INTRO");
        std::env::remove_var("SFX_TYPEWRITER");
    };

    clear();
    assert_eq!(
        SfxSources::resolve(None, None),
        SfxSources::default(),
        "都没给时两段都走内嵌"
    );

    unsafe {
        std::env::set_var("SFX_INTRO", "/env/intro.mp3");
        std::env::set_var("SFX_TYPEWRITER", "/env/typewriter.mp3");
    }
    let from_env = SfxSources::resolve(None, None);
    assert_eq!(from_env.intro.as_deref(), Some(Path::new("/env/intro.mp3")));
    assert_eq!(
        from_env.typewriter.as_deref(),
        Some(Path::new("/env/typewriter.mp3"))
    );

    // 命令行优先，且两个参数各落各的字段。
    let from_cli = SfxSources::resolve(
        Some(PathBuf::from("/cli/intro.mp3")),
        Some(PathBuf::from("/cli/typewriter.mp3")),
    );
    assert_eq!(from_cli.intro.as_deref(), Some(Path::new("/cli/intro.mp3")));
    assert_eq!(
        from_cli.typewriter.as_deref(),
        Some(Path::new("/cli/typewriter.mp3"))
    );

    // 只给一个：另一个必须仍来自环境变量，不能被这一个带偏。
    let only_intro = SfxSources::resolve(Some(PathBuf::from("/cli/intro.mp3")), None);
    assert_eq!(
        only_intro.intro.as_deref(),
        Some(Path::new("/cli/intro.mp3"))
    );
    assert_eq!(
        only_intro.typewriter.as_deref(),
        Some(Path::new("/env/typewriter.mp3")),
        "只给片尾音效不应把打字机音效也带走"
    );

    let only_typewriter = SfxSources::resolve(None, Some(PathBuf::from("/cli/typewriter.mp3")));
    assert_eq!(
        only_typewriter.typewriter.as_deref(),
        Some(Path::new("/cli/typewriter.mp3"))
    );
    assert_eq!(
        only_typewriter.intro.as_deref(),
        Some(Path::new("/env/intro.mp3"))
    );

    clear();
}
