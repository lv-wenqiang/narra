pub const DEFAULT_VOICE: &str = "zh-CN-YunjianNeural";
pub const SPEED_FACTOR: f64 = 1.1;
const DEFAULT_BATCH_SIZE: usize = 3;
const BATCH_SIZE_CAP: usize = 8;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MIN_TIMEOUT_MS: u64 = 15_000;

/// 并发数校验的共同语义：非法（含 0）回落默认 3，超过 8 钳制为 8。
/// `resolve_batch_size`（环境变量路径）与 `resolve_batch_size_from_value`
/// （CLI `--batch-size` 路径）都基于这一条规则，保证同一非法值（如 0）
/// 在两条路径下得到相同结果——这是终审必修项 4 要修的不一致。
fn clamp_or_default(n: Option<usize>) -> usize {
    match n {
        Some(n) if n >= 1 => n.min(BATCH_SIZE_CAP),
        _ => DEFAULT_BATCH_SIZE,
    }
}

/// 解析并发数：非法或 <1 回落默认 3，超过 8 钳制为 8。
pub fn resolve_batch_size(raw: Option<&str>) -> usize {
    clamp_or_default(
        raw.map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.parse::<usize>().ok()),
    )
}

/// 解析并发数（CLI `--batch-size <N>` 路径）：与 `resolve_batch_size`
/// 共享同一条钳制/回落规则，使得 `--batch-size 0` 与
/// `EDGE_TTS_BATCH_SIZE=0` 得到相同结果（回落默认 3），而不是此前 CLI 路径
/// 单独用 `.clamp(1, 8)` 把 0 静默钳到 1 的不一致行为。
pub fn resolve_batch_size_from_value(n: Option<usize>) -> usize {
    clamp_or_default(n)
}

/// 解析单段超时：非法或低于下限 15000 时回落默认 120000。
pub fn resolve_timeout_ms(raw: Option<&str>) -> u64 {
    match raw.map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.parse::<u64>().ok()) {
        Some(n) if n >= MIN_TIMEOUT_MS => n,
        _ => DEFAULT_TIMEOUT_MS,
    }
}

/// `SPIDER_OUTPUT_DIR`，默认 output/spider。
pub fn spider_output_dir() -> String {
    non_empty_env("SPIDER_OUTPUT_DIR").unwrap_or_else(|| "output/spider".into())
}

/// `TTS_OUTPUT_DIR`，默认 output/tts。
pub fn tts_output_dir() -> String {
    non_empty_env("TTS_OUTPUT_DIR").unwrap_or_else(|| "output/tts".into())
}

/// `TTS_INPUT_FILE`，默认 `<SPIDER_OUTPUT_DIR>/input.txt`。
pub fn tts_input_file() -> String {
    non_empty_env("TTS_INPUT_FILE").unwrap_or_else(|| format!("{}/input.txt", spider_output_dir()))
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// 标题的最终兜底值（规格 §6 的三级兜底最后一级）。
pub const DEFAULT_TITLE: &str = "熊猫智研社";

/// 背景视频，默认 `public/video/0.mp4`（`--bg` 覆盖）。
pub fn bg_video_path() -> String {
    non_empty_env("BG_VIDEO").unwrap_or_else(|| "public/video/0.mp4".into())
}

/// BGM，默认 `public/bgm/0.mp3`（`--bgm` 覆盖）。
pub fn bgm_path() -> String {
    non_empty_env("BGM_FILE").unwrap_or_else(|| "public/bgm/0.mp3".into())
}

/// 标题 JSON，默认 `public/video/title.json`（`--title-json` 覆盖）。
pub fn title_json_path() -> String {
    non_empty_env("TITLE_JSON").unwrap_or_else(|| "public/video/title.json".into())
}

/// 成片输出，默认 `output/video/video.mp4`（`-o` 覆盖）。
pub fn video_output_path() -> String {
    non_empty_env("VIDEO_OUTPUT").unwrap_or_else(|| "output/video/video.mp4".into())
}

/// 标题三级兜底（规格 §6）：`--title` > `title.json` 的 `title` 字段 > 默认值。
///
/// 每一级都要求「非空白」才算数：`--title "  "` 与 `{"title": ""}` 都继续往下
/// 兜底，而不是产出一个空标题的封面。JSON 解析失败也回落而非报错——标题文件
/// 是可选素材，缺失或损坏不应让整条合成挂掉。
pub fn resolve_title(cli: Option<&str>, json_text: Option<&str>) -> String {
    if let Some(t) = cli.map(str::trim).filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    if let Some(text) = json_text
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(text)
        && let Some(t) =
            v.get("title").and_then(|t| t.as_str()).map(str::trim).filter(|s| !s.is_empty())
    {
        return t.to_string();
    }
    DEFAULT_TITLE.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_size_defaults_to_three() {
        assert_eq!(resolve_batch_size(None), 3);
    }

    #[test]
    fn batch_size_is_capped_at_eight() {
        assert_eq!(resolve_batch_size(Some("99")), 8);
    }

    #[test]
    fn batch_size_rejects_zero_and_garbage() {
        assert_eq!(resolve_batch_size(Some("0")), 3);
        assert_eq!(resolve_batch_size(Some("abc")), 3);
    }

    /// 终审必修项 4：CLI `--batch-size` 路径与环境变量路径必须对同一非法值
    /// 给出相同结果——`0` 应回落默认 3，而不是被静默钳到 1。
    #[test]
    fn batch_size_from_value_matches_env_path_semantics() {
        assert_eq!(resolve_batch_size_from_value(None), 3);
        assert_eq!(resolve_batch_size_from_value(Some(0)), 3);
        assert_eq!(resolve_batch_size_from_value(Some(99)), 8);
        assert_eq!(resolve_batch_size_from_value(Some(5)), 5);
        // 与环境变量路径逐一对照同一批取值，确保两条路径行为完全一致。
        for n in [0usize, 1, 3, 8, 9, 99] {
            assert_eq!(
                resolve_batch_size_from_value(Some(n)),
                resolve_batch_size(Some(&n.to_string())),
                "n={n} 时两条路径应给出相同结果"
            );
        }
    }

    #[test]
    fn timeout_falls_back_when_below_floor() {
        assert_eq!(resolve_timeout_ms(Some("1000")), 120_000);
        assert_eq!(resolve_timeout_ms(Some("15000")), 15_000);
        assert_eq!(resolve_timeout_ms(None), 120_000);
    }

    // `material_paths_have_the_documented_defaults`（断言 bg_video_path 等
    // 四个函数的默认值）与 `env_backed_paths_read_the_environment_and_fall_back`
    // 都不放在这里：见 tests/config_env.rs 顶部注释——它们要么依赖「进程没设
    // 相关环境变量」这个不由测试自己保证的前提，要么直接串行读写环境变量，
    // 都需要独立测试二进制来保证确定性与隔离，不能和本文件的纯函数测试混在
    // 一起（本文件与 src/tts/pipeline.rs 共享同一个 lib 测试二进制）。

    #[test]
    fn title_prefers_cli_over_json_over_default() {
        let json = r#"{"title": "来自 JSON 的标题"}"#;
        assert_eq!(resolve_title(Some("来自命令行"), Some(json)), "来自命令行");
        assert_eq!(resolve_title(None, Some(json)), "来自 JSON 的标题");
        assert_eq!(resolve_title(None, None), DEFAULT_TITLE);
    }

    #[test]
    fn title_falls_through_blank_and_malformed_json() {
        // 空字符串不算「给了标题」，应继续往下兜底。
        assert_eq!(resolve_title(Some("   "), None), DEFAULT_TITLE);
        // JSON 解析失败、缺 title 字段、title 为空，都应回落默认值而不是报错。
        assert_eq!(resolve_title(None, Some("不是 JSON")), DEFAULT_TITLE);
        assert_eq!(resolve_title(None, Some(r#"{"other": 1}"#)), DEFAULT_TITLE);
        assert_eq!(resolve_title(None, Some(r#"{"title": ""}"#)), DEFAULT_TITLE);
        assert_eq!(resolve_title(None, Some(r#"{"title": "  "}"#)), DEFAULT_TITLE);
    }

    #[test]
    fn title_from_json_is_trimmed() {
        assert_eq!(resolve_title(None, Some(r#"{"title": "  带空格  "}"#)), "带空格");
    }
}
