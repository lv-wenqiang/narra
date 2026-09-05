/// 格式化为 WebVTT 时间戳 `HH:MM:SS.mmm`。
/// 先把秒四舍五入到毫秒再拆分时/分/秒，因此进位始终正确
/// （`59.9996` → `00:01:00.000`，而非 TS 原版会产生的非法值 `00:00:60.000`）。
pub fn format_vtt_time(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0).round().max(0.0) as u64;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let secs = total_secs % 60;
    let minutes = (total_secs / 60) % 60;
    let hours = total_secs / 3600;
    format!("{hours:02}:{minutes:02}:{secs:02}.{ms:03}")
}

const SENTENCE_ENDINGS: [char; 3] = ['。', '！', '？'];

/// 移植自 packages/tts-node/src/vtt.ts 的 splitTextForVtt。
/// TS 原版基于 UTF-16 code unit 计数；此处用 Unicode 标量。
/// 中文（BMP 内）两者一致，emoji（星光平面）会有差异，见 tests。
pub fn split_text_for_vtt(text: &str, max_length: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_length {
        return vec![text.to_string()];
    }

    let mut segments: Vec<String> = Vec::new();
    let mut remaining: Vec<char> = chars;

    while remaining.len() > max_length {
        // 1. 在前 max_length 个字符内从后往前找句末标点
        let upper = max_length.min(remaining.len());
        let back = (0..upper)
            .rev()
            .find(|&i| SENTENCE_ENDINGS.contains(&remaining[i]));

        if let Some(i) = back {
            remaining = cut(&mut segments, remaining, i + 1);
            continue;
        }

        // 2. 向后在 [max_length, max_length*2) 内找
        let lookahead = (max_length * 2).min(remaining.len());
        let mut found = (max_length..lookahead).find(|&i| SENTENCE_ENDINGS.contains(&remaining[i]));

        // 3. 仍未找到则扫描剩余全文
        if found.is_none() {
            found =
                (max_length..remaining.len()).find(|&i| SENTENCE_ENDINGS.contains(&remaining[i]));
        }

        match found {
            Some(i) => remaining = cut(&mut segments, remaining, i + 1),
            None => {
                // 全文无句末标点：整段作为一个片段，结束循环
                let whole: String = remaining.iter().collect();
                segments.push(whole.trim().to_string());
                remaining = Vec::new();
            }
        }
    }

    // TS 原版此处不 trim，保持一致
    if !remaining.is_empty() {
        segments.push(remaining.iter().collect());
    }
    segments
}

/// 在 `pos` 处切开：前半 trim 后（非空才）推入 segments，返回 trim 后的后半。
fn cut(segments: &mut Vec<String>, remaining: Vec<char>, pos: usize) -> Vec<char> {
    let head: String = remaining[..pos].iter().collect();
    let head = head.trim();
    if !head.is_empty() {
        segments.push(head.to_string());
    }
    let tail: String = remaining[pos..].iter().collect();
    tail.trim().chars().collect()
}

/// 由段落文本与各段时长生成 WebVTT。
/// 段落过长时先切句，再按 `片段字符数 / 原段字符数` 比例摊分该段时长。
/// 片段经过 trim，其字符数之和可能小于原段，故摊分后的总时长可能略小于 duration。
pub fn generate_vtt(lines: &[String], durations: &[f64], vtt_max_length: usize) -> String {
    let mut out: Vec<String> = vec!["WEBVTT".to_string(), String::new()];
    let mut current_time = 0.0_f64;
    let mut vtt_index = 1_usize;

    for (i, text) in lines.iter().enumerate() {
        let duration = durations[i];
        let segments = split_text_for_vtt(text, vtt_max_length);

        let total_chars = text.chars().count().max(1) as f64;
        let segment_durations: Vec<f64> = if segments.len() == 1 {
            vec![duration]
        } else {
            segments
                .iter()
                .map(|s| duration * s.chars().count() as f64 / total_chars)
                .collect()
        };

        for (segment_text, segment_duration) in segments.iter().zip(segment_durations) {
            let start_time = current_time;
            let end_time = current_time + segment_duration;

            out.push(vtt_index.to_string());
            out.push(format!(
                "{} --> {}",
                format_vtt_time(start_time),
                format_vtt_time(end_time)
            ));
            out.push(segment_text.clone());
            out.push(String::new());

            current_time = end_time;
            vtt_index += 1;
        }
    }

    out.join("\n")
}

/// 一条字幕。
#[derive(Debug, Clone, PartialEq)]
pub struct Caption {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// 解析 WebVTT。只认时间行与其后的文本行，忽略 WEBVTT 头、序号行与空行。
/// 多行文本用 `\n` 连接。无法解析的时间行整条跳过。
/// [`parse_vtt_reporting`] 的产物：读出来的字幕，以及读的过程中被跳过的东西。
#[derive(Debug, Default)]
pub struct ParsedVtt {
    pub captions: Vec<Caption>,
    /// 每条一句人话，说明跳过了什么、为什么。
    pub warnings: Vec<String>,
}

/// 解析 WebVTT 并**报告跳过了什么**。
///
/// **为什么要报告**：自产自销的路径（`generate_vtt` → 这里）永远不会有读不懂的
/// 东西，但 `panda debug-frames --vtt` 读的是用户给的任意文件。静默跳过一条 cue
/// 的表现是「某段字幕莫名其妙不见了」——用户手上没有任何线索可查。
pub fn parse_vtt_reporting(text: &str) -> ParsedVtt {
    /// 当前正在收的这一条 cue 处于什么状态。
    ///
    /// `Unreadable` 这一支是**故意继续往下收正文**的：时间行读不懂时正文还在
    /// 后面几行，只有把它一并收下来，警告里才带得上「丢掉的是哪一段」。
    enum Pending {
        /// 还没读到时间行——此刻的非空行是 cue 序号/标识行，按规范丢弃。
        None,
        Cue(u64, u64),
        Unreadable(String),
    }

    let mut out = ParsedVtt::default();
    let mut pending = Pending::None;
    let mut buf: Vec<String> = Vec::new();

    let flush = |out: &mut ParsedVtt, pending: &mut Pending, buf: &mut Vec<String>| {
        match std::mem::replace(pending, Pending::None) {
            Pending::Cue(start_ms, end_ms) if !buf.is_empty() => out.captions.push(Caption {
                text: buf.join("\n"),
                start_ms,
                end_ms,
            }),
            Pending::Unreadable(ts_line) => out.warnings.push(format!(
                "跳过一条读不懂的字幕：时间行「{ts_line}」解析不了，正文「{}」被丢弃",
                buf.join("\n")
            )),
            _ => {}
        }
        buf.clear();
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line == "WEBVTT" || line.starts_with("NOTE") {
            continue;
        }
        if line.is_empty() {
            flush(&mut out, &mut pending, &mut buf);
            continue;
        }
        if let Some((a, b)) = line.split_once("-->") {
            flush(&mut out, &mut pending, &mut buf);
            pending = match (parse_ts_ms(a.trim()), parse_ts_ms(b.trim())) {
                (Some(s), Some(e)) => Pending::Cue(s, e),
                _ => Pending::Unreadable(line.to_string()),
            };
            continue;
        }
        // 已经读到时间行，此后的非空行一律是正文。
        //
        // **不能再看这行「像不像 cue 序号」**：序号行在规范里出现在时间行
        // **之前**，那一支已经由 `Pending::None` 挡掉了；在时间行之后还去
        // 认数字，只会把正文里独占一行的年份（`2024`）当序号吃掉。
        if !matches!(pending, Pending::None) {
            buf.push(line.to_string());
        }
    }
    flush(&mut out, &mut pending, &mut buf);
    out
}

/// [`parse_vtt_reporting`] 的薄壳：把警告打到 stderr，只交字幕。
///
/// 警告而不是报错，与 `--font` 不可用时的处置一致——少一条字幕不该让整支片子
/// 出不来，但用户必须知道少了什么。
pub fn parse_vtt(text: &str) -> Vec<Caption> {
    let parsed = parse_vtt_reporting(text);
    for w in &parsed.warnings {
        eprintln!("警告：{w}");
    }
    parsed.captions
}

/// 解析 `HH:MM:SS.mmm` 或 `MM:SS.mmm`，失败返回 None。
///
/// **小时位在 WebVTT 规范里是可选的**，两种写法同样合法。只认三段式会让别家
/// 工具产出的 VTT 整份解析为空——`FrameSource::new` 那一侧看到的是「一条字幕
/// 都没有」，报的错会指向文件格式不对，而文件其实好好的。
fn parse_ts_ms(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, rest) = match parts.as_slice() {
        [h, m, rest] => (*h, *m, *rest),
        [m, rest] => ("0", *m, *rest),
        _ => return None,
    };
    let (sec, ms) = rest.split_once('.')?;
    let h: u64 = h.parse().ok()?;
    let m: u64 = m.parse().ok()?;
    let sec: u64 = sec.parse().ok()?;
    // 右补零后按字节切片，因此必须先确认毫秒位全是 ASCII 数字：
    // 多字节字符（如「éé」补成「éé0」）会让 [..3] 落在字符中间而 panic。
    if !ms.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ms: u64 = format!("{ms:0<3}")[..3].parse().ok()?;
    Some(h * 3_600_000 + m * 60_000 + sec * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_vtt_time_basics() {
        assert_eq!(format_vtt_time(0.0), "00:00:00.000");
        assert_eq!(format_vtt_time(1.5), "00:00:01.500");
        assert_eq!(format_vtt_time(61.25), "00:01:01.250");
        assert_eq!(format_vtt_time(3661.007), "01:01:01.007");
    }

    #[test]
    fn format_vtt_time_rounds_to_three_decimals() {
        assert_eq!(format_vtt_time(12.3456), "00:00:12.346");
    }

    #[test]
    fn format_vtt_time_carries_into_minutes() {
        // 四舍五入到毫秒后恰好满 60 秒，必须进位而不是输出非法的 :60.000
        assert_eq!(format_vtt_time(59.9996), "00:01:00.000");
        assert_eq!(format_vtt_time(3599.9999), "01:00:00.000");
        assert_eq!(format_vtt_time(59.9994), "00:00:59.999");
    }

    #[test]
    fn split_short_text_returns_whole() {
        assert_eq!(
            split_text_for_vtt("很短的一句话。", 30),
            vec!["很短的一句话。"]
        );
    }

    #[test]
    fn split_at_last_sentence_end_within_limit() {
        // 前 30 字符内从后往前找到「。」，在其后切开
        let text = "第一句话到这里结束。第二句话比较长一些继续往后写下去直到超过限制为止收尾。";
        let got = split_text_for_vtt(text, 30);
        assert_eq!(got[0], "第一句话到这里结束。");
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn split_looks_ahead_when_no_ending_within_limit() {
        // 前 30 字符内无句末标点，向后找到第一个「！」
        let text = "这段文字前面很长一直没有任何标点符号一直写下去写到很后面才终于出现了标点！后面还有一点。";
        let got = split_text_for_vtt(text, 30);
        assert!(got[0].ends_with('！'));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn split_returns_whole_when_no_sentence_ending_at_all() {
        let text = "这段文字完全没有句末标点符号但是长度已经远远超过了三十个字符的上限了继续写";
        assert_eq!(split_text_for_vtt(text, 30), vec![text]);
    }

    #[test]
    fn split_counts_unicode_scalars_not_bytes() {
        // 30 个汉字 = 90 字节；若误用字节长度会错误触发切分
        let text = "一二三四五六七八九十一二三四五六七八九十一二三四五六七八九十";
        assert_eq!(text.chars().count(), 30);
        assert_eq!(split_text_for_vtt(text, 30), vec![text]);
    }

    #[test]
    fn split_scans_beyond_lookahead_in_phase3() {
        // 专门测试 Phase 3 分支：标点落在 [max_length*2, ...) 外
        // 构造：30 个"一" + 30 个"二" + "。" + 10 个字
        // 总长 71。Phase 2 扫描 [30, 60)（不含 60），标点恰好在 60，所以 Phase 2 找不到，
        // 必须进 Phase 3 从 [30, 71) 全扫描才能找到。
        let text = "一一一一一一一一一一一一一一一一一一一一一一一一一一一一一一二二二二二二二二二二二二二二二二二二二二二二二二二二二二二二。一二三四五六七八九十";
        assert_eq!(text.chars().count(), 71);
        let got = split_text_for_vtt(text, 30);
        assert_eq!(got.len(), 2, "期望切成 2 个片段");
        assert!(got[0].ends_with('。'), "第一个片段应以「。」结尾");
        assert_eq!(got[0].chars().count(), 61, "第一个片段应为 61 个字符");
        assert_eq!(got[1], "一二三四五六七八九十", "第二个片段应为剩余 10 字");
    }

    #[test]
    fn vtt_starts_with_header_and_numbers_cues_from_one() {
        let lines = vec!["第一段。".to_string(), "第二段。".to_string()];
        let out = generate_vtt(&lines, &[2.0, 3.0], 30);
        assert!(out.starts_with("WEBVTT\n\n"), "缺少 WEBVTT 头：{out}");
        assert!(out.contains("\n1\n00:00:00.000 --> 00:00:02.000\n第一段。\n"));
        assert!(out.contains("\n2\n00:00:02.000 --> 00:00:05.000\n第二段。\n"));
    }

    #[test]
    fn long_paragraph_splits_and_shares_duration_by_char_count() {
        // 一段 40 字、含一个句号，会被切成两片；两片时长按字符数比例摊分
        let text = "第一句话写得比较长一点用来触发切分。第二句话也在这里继续往后写。";
        let lines = vec![text.to_string()];
        let out = generate_vtt(&lines, &[10.0], 30);
        let cues: Vec<&str> = out.lines().filter(|l| l.contains("-->")).collect();
        assert_eq!(
            cues.len(),
            2,
            "期望切成 2 条 cue，实得 {}：{out}",
            cues.len()
        );
        // 切片文本不丢字
        assert!(out.contains("第一句话写得比较长一点用来触发切分。"));
        assert!(out.contains("第二句话也在这里继续往后写。"));
    }

    #[test]
    fn timeline_is_monotonically_increasing() {
        let lines = vec![
            "短句。".to_string(),
            "这是一段明显更长的文字用来触发切句逻辑从而产生多条字幕。后面还有一句。".to_string(),
            "结尾。".to_string(),
        ];
        let out = generate_vtt(&lines, &[1.5, 8.25, 2.0], 30);
        let mut prev_end = 0.0_f64;
        let mut cue_count = 0;
        for line in out.lines().filter(|l| l.contains("-->")) {
            let (start, end) = line.split_once(" --> ").unwrap();
            let (s, e) = (parse_ts(start), parse_ts(end));
            assert!(s >= prev_end - 1e-6, "时间轴回退：{line}");
            assert!(e >= s, "cue 结束早于开始：{line}");
            prev_end = e;
            cue_count += 1;
        }
        assert!(cue_count >= 4, "期望至少 4 条 cue，实得 {cue_count}");
    }

    /// 把 `HH:MM:SS.mmm` 解析回秒，仅测试用。
    fn parse_ts(s: &str) -> f64 {
        let p: Vec<&str> = s.trim().split(':').collect();
        let sec: Vec<&str> = p[2].split('.').collect();
        p[0].parse::<f64>().unwrap() * 3600.0
            + p[1].parse::<f64>().unwrap() * 60.0
            + sec[0].parse::<f64>().unwrap()
            + sec[1].parse::<f64>().unwrap() / 1000.0
    }

    #[test]
    fn parse_vtt_reads_cues_with_text() {
        let s = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:02.500\n第一句。\n\n2\n00:00:02.500 --> 00:00:06.000\n第二句。\n";
        let caps = parse_vtt(s);
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0].start_ms, 0);
        assert_eq!(caps[0].end_ms, 2500);
        assert_eq!(caps[0].text, "第一句。");
        assert_eq!(caps[1].start_ms, 2500);
        assert_eq!(caps[1].end_ms, 6000);
    }

    #[test]
    fn parse_vtt_roundtrips_generated_output() {
        let lines = vec!["第一段。".to_string(), "第二段。".to_string()];
        let out = generate_vtt(&lines, &[2.0, 3.0], 30);
        let caps = parse_vtt(&out);
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0].text, "第一段。");
        assert_eq!(caps[1].end_ms, 5000);
    }

    #[test]
    fn parse_vtt_ignores_header_and_blank_lines() {
        assert!(parse_vtt("WEBVTT\n\n").is_empty());
        assert!(parse_vtt("").is_empty());
    }

    #[test]
    fn parse_ts_ms_rejects_non_ascii_milliseconds_without_panicking() {
        // 毫秒位是多字节字符时，右补零后按字节切片会落在字符中间。
        // 「éé」补成「éé0」共 5 字节，[..3] 恰好切进第二个 é（字节 2..4）。
        // 函数文档承诺「失败返回 None」，这里必须返回 None 而不是 panic。
        assert_eq!(parse_ts_ms("00:00:01.éé"), None);
        assert_eq!(parse_ts_ms("00:00:01.é"), None);
        assert_eq!(
            parse_ts_ms("00:00:01.１２３"),
            None,
            "全角数字不是 ASCII 数字"
        );
        // 合法输入不受影响
        assert_eq!(parse_ts_ms("00:00:01.500"), Some(1500));
        assert_eq!(parse_ts_ms("00:00:01.5"), Some(1500));
    }

    #[test]
    fn parse_vtt_skips_cue_with_non_ascii_timestamp_instead_of_panicking() {
        // debug-frames 读的是用户给的任意文件，这条路径必须扛得住脏数据。
        let s = "WEBVTT\n\n1\n00:00:01.éé --> 00:00:02.000\n坏的一条。\n\n\
                 2\n00:00:03.000 --> 00:00:04.000\n好的一条。\n";
        let caps = parse_vtt(s);
        assert_eq!(caps.len(), 1, "坏 cue 应被跳过，好 cue 应保留：{caps:?}");
        assert_eq!(caps[0].text, "好的一条。");
        assert_eq!(caps[0].start_ms, 3000);
    }

    /// 正文首行全是数字（年份独占一行）时，内容不能被当成 cue 序号丢掉。
    ///
    /// 序号行在 WebVTT 里出现在**时间戳之前**，而正文出现在时间戳之后——
    /// 「已经读到时间戳」本身就足以区分两者，不需要再看这行像不像序号。
    #[test]
    fn parse_vtt_keeps_a_body_line_that_is_all_digits() {
        let s = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:02.000\n2024\n那一年。\n";
        let caps = parse_vtt(s);
        assert_eq!(caps.len(), 1, "序号行仍应被跳过：{caps:?}");
        assert_eq!(
            caps[0].text, "2024\n那一年。",
            "正文里独占一行的年份不能被当成序号丢掉"
        );
    }

    /// 跳过一条读不懂的 cue 时必须留下话，不能静默。
    ///
    /// `panda debug-frames --vtt` 读的是用户给的任意文件。静默跳过的表现是
    /// 「某段字幕莫名其妙不见了」，用户没有任何线索可查——而 cue 的正文就在
    /// 手边，警告里带上它，用户一眼就能定位到是文件哪一段。
    #[test]
    fn parse_vtt_warns_about_a_cue_it_could_not_read_instead_of_dropping_it_silently() {
        let s = "WEBVTT\n\n1\n00:00:01.éé --> 00:00:02.000\n坏的一条。\n\n\
                 2\n00:00:03.000 --> 00:00:04.000\n好的一条。\n";
        let parsed = parse_vtt_reporting(s);
        assert_eq!(parsed.captions.len(), 1, "好 cue 应保留");
        assert_eq!(parsed.warnings.len(), 1, "坏 cue 应留下且只留下一条警告");
        let w = &parsed.warnings[0];
        assert!(
            w.contains("00:00:01.éé --> 00:00:02.000"),
            "警告应带上读不懂的那一行原文，用户才定位得到：{w}"
        );
        assert!(
            w.contains("坏的一条。"),
            "警告应带上被丢掉的正文，否则用户不知道少了什么：{w}"
        );
    }

    /// WebVTT 规范里小时位是可选的：`MM:SS.mmm` 与 `HH:MM:SS.mmm` 同样合法。
    #[test]
    fn parse_ts_ms_accepts_the_optional_hour_form_from_the_spec() {
        assert_eq!(parse_ts_ms("01:30.500"), Some(90_500));
        assert_eq!(parse_ts_ms("00:00.000"), Some(0));
        // 三段式不受影响
        assert_eq!(parse_ts_ms("01:01:30.500"), Some(3_690_500));
        // 段数不对仍然拒绝
        assert_eq!(parse_ts_ms("30.500"), None, "只有秒不是合法时间戳");
        assert_eq!(parse_ts_ms("1:2:3:4.500"), None);
    }

    /// 别家工具产出的、省掉小时位的 VTT 不能被整份判为空。
    #[test]
    fn parse_vtt_reads_a_file_written_without_the_hour_field() {
        let s = "WEBVTT\n\n1\n00:00.000 --> 00:02.500\n第一句。\n\n2\n00:02.500 --> 01:06.000\n第二句。\n";
        let caps = parse_vtt(s);
        assert_eq!(caps.len(), 2, "省掉小时位的 VTT 应能读出来：{caps:?}");
        assert_eq!(caps[0].end_ms, 2500);
        assert_eq!(caps[1].end_ms, 66_000);
    }
}
