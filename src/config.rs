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
        raw.map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse::<usize>().ok()),
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
    match raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok())
    {
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
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// 一条片子的品牌装备：品牌名与两处水印文案，整片恒定。
///
/// **为什么是一个结构体而不是三个平行参数**：三者同为 `String`/`Option<String>`，
/// 平行传参时相邻两个对调不会编译失败，只会让成片上的字串默默换了位置——
/// 本仓库为同一类风险写过 `each_material_env_var_is_wired_to_exactly_one_function`。
/// 具名字段让构造处和使用处都由名字而非位置决定。
///
/// 三级兜底（`--flag` > 环境变量 > 默认值）在 `main.rs` 的构造处装配，与
/// `ResolvedRenderPaths` 的四条素材路径同一套写法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branding {
    /// 画在 Cover 上排与 Outro 大字上，同时是标题兜底的最后一级。
    pub brand: String,
    /// Content 段左下角水印，`None` = 不画。
    pub watermark: Option<String>,
    /// Cover 与 Outro 段水印，`None` = 不画。
    pub watermark_cover: Option<String>,
    /// 水印文字左侧的图标文件路径，两处共用，`None` = 不画图标。
    pub watermark_icon: Option<String>,
}

impl Branding {
    /// 三级兜底：`--flag` > 环境变量 > 默认值，三项各自独立地走一遍。
    ///
    /// **住在 config 里而不是 `main.rs`**：它组合的三个函数（[`brand`]、
    /// [`watermark`]、[`watermark_cover`]）都在这里，装配逻辑跟着它们走才
    /// 测得到——放在 `main.rs`（bin crate）里，`tests/` 下的环境变量夹具
    /// 够不着它，「哪个参数落进哪个字段」就成了零覆盖的装配层。
    /// `docs/follow-ups.md`「ffmpeg 合成 · 值得做」里记过同一类缺口。
    ///
    /// 三个入参同为 `Option<String>`，位置传参时相邻两个对调既不编译失败也
    /// 不在任何一次运行里报错，只会让文案默默画到另一处去——构造处用具名
    /// 字段，测试用「设一个、断言只有对应那个变」的写法，两头一起堵。
    pub fn resolve(
        brand: Option<String>,
        watermark: Option<String>,
        watermark_cover: Option<String>,
        watermark_icon: Option<String>,
    ) -> Self {
        Self {
            brand: non_blank(brand).unwrap_or_else(self::brand),
            watermark: non_blank(watermark).or_else(self::watermark),
            watermark_cover: non_blank(watermark_cover).or_else(self::watermark_cover),
            watermark_icon: non_blank(watermark_icon).or_else(self::watermark_icon),
        }
    }

    /// 只有品牌名、两处水印都不画的装备——也就是用户什么都没配时的形态。
    ///
    /// 生产路径上由 `main.rs` 的 `resolve_branding` 装配；这个构造子是给
    /// 「只关心品牌名、不关心水印」的调用方（含大量测试）用的短写法。
    pub fn plain(brand: &str) -> Self {
        Self {
            brand: brand.to_string(),
            watermark: None,
            watermark_cover: None,
            watermark_icon: None,
        }
    }
}

/// 「全空白视同没给」——命令行参数侧的 `non_empty_env` 对应物（私有项，
/// 故意不做 intra-doc 链接：链到私有项会让 `cargo rustdoc` 报
/// `private_intra_doc_links` 告警）。
///
/// `--brand "  "` 应当继续往下兜底，而不是产出一个空品牌名的片尾；这与
/// [`resolve_title`] 对 `--title` 的处理是同一条规矩。
pub fn non_blank(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 品牌名，默认「墨风」（`--brand` 覆盖）。
///
/// 画在 Cover 上排（logo 旁）与 Outro 大字上——「这是谁做的」。它同时是
/// 标题三级兜底的最后一级（见 [`resolve_title`]）：没给标题时封面显示频道名。
pub fn brand() -> String {
    non_empty_env("BRAND").unwrap_or_else(|| "墨风".into())
}

/// 正文（Content 段）左下角水印文案，默认**不画**（`--watermark` 覆盖）。
///
/// **`None` 与 `Some("")` 不是一回事**：前者跳过整条预渲染 + 贴图路径，后者
/// 会白付一次 `prepare_watermark` 再贴一张零宽的图。`non_empty_env` 的
/// 「空白视同未设置」正好让 `WATERMARK=""` 落到 `None`。
pub fn watermark() -> Option<String> {
    non_empty_env("WATERMARK")
}

/// 水印文字左侧的图标文件（`.svg` 或 `.png`），默认**不画**
/// （`--watermark-icon` 覆盖）。
///
/// **两处水印共用一个**：文案两处不同是因为长短场合不同，但品牌标记就一个。
/// 各自缩到自己预设的尺寸（正文 28px、封面/片尾 32px）。图标只在对应那处的
/// **文案也配了**的时候才会出现——没有文案就没有水印，图标自然无处可挂。
pub fn watermark_icon() -> Option<String> {
    non_empty_env("WATERMARK_ICON")
}

/// Cover 与 Outro 段的水印文案，默认**不画**（`--watermark-cover` 覆盖）。
///
/// 与 [`watermark`] 相互独立：两处的字号、颜色、位置本就不同（正文是白色
/// 27%、24px、左下角；封面/片尾是深色 40%、28px、水平居中），文案也各配各的。
pub fn watermark_cover() -> Option<String> {
    non_empty_env("WATERMARK_COVER")
}

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

/// 标题三级兜底（规格 §6）：`--title` > `title.json` 的 `title` 字段 > 品牌名。
///
/// **最后一级是 [`brand`] 而不是一个独立常量**：没给标题时，封面显示频道名
/// 是有意义的兜底；再多一个「默认标题」配置项只会让两处必须同步维护。
///
/// 每一级都要求「非空白」才算数：`--title "  "` 与 `{"title": ""}` 都继续往下
/// 兜底，而不是产出一个空标题的封面。JSON 解析失败也回落而非报错——标题文件
/// 是可选素材，缺失或损坏不应让整条合成挂掉。
pub fn resolve_title(cli: Option<&str>, json_text: Option<&str>, brand: &str) -> String {
    if let Some(t) = cli.map(str::trim).filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    if let Some(text) = json_text
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(text)
        && let Some(t) = v
            .get("title")
            .and_then(|t| t.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        return t.to_string();
    }
    brand.to_string()
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

    /// 测试里用一个**不是**生产默认值的品牌名。
    ///
    /// 若这里写 "墨风"，`resolve_title` 把最后一级错写成硬编码 "墨风" 的变异
    /// 就检不出来了——测试断言的必须是「回落到传进去的那个 brand」，而不是
    /// 「回落到某个恰好等于默认值的字符串」。
    const TEST_BRAND: &str = "测试品牌";

    #[test]
    fn title_prefers_cli_over_json_over_default() {
        let json = r#"{"title": "来自 JSON 的标题"}"#;
        assert_eq!(
            resolve_title(Some("来自命令行"), Some(json), TEST_BRAND),
            "来自命令行"
        );
        assert_eq!(
            resolve_title(None, Some(json), TEST_BRAND),
            "来自 JSON 的标题"
        );
        assert_eq!(resolve_title(None, None, TEST_BRAND), TEST_BRAND);
    }

    #[test]
    fn title_falls_through_blank_and_malformed_json() {
        // 空字符串不算「给了标题」，应继续往下兜底。
        assert_eq!(resolve_title(Some("   "), None, TEST_BRAND), TEST_BRAND);
        // JSON 解析失败、缺 title 字段、title 为空，都应回落默认值而不是报错。
        assert_eq!(
            resolve_title(None, Some("不是 JSON"), TEST_BRAND),
            TEST_BRAND
        );
        assert_eq!(
            resolve_title(None, Some(r#"{"other": 1}"#), TEST_BRAND),
            TEST_BRAND
        );
        assert_eq!(
            resolve_title(None, Some(r#"{"title": ""}"#), TEST_BRAND),
            TEST_BRAND
        );
        assert_eq!(
            resolve_title(None, Some(r#"{"title": "  "}"#), TEST_BRAND),
            TEST_BRAND
        );
    }

    #[test]
    fn title_from_json_is_trimmed() {
        assert_eq!(
            resolve_title(None, Some(r#"{"title": "  带空格  "}"#), TEST_BRAND),
            "带空格"
        );
    }
}
