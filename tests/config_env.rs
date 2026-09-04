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
