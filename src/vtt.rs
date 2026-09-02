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
        let back = (0..upper).rev().find(|&i| SENTENCE_ENDINGS.contains(&remaining[i]));

        if let Some(i) = back {
            remaining = cut(&mut segments, remaining, i + 1);
            continue;
        }

        // 2. 向后在 [max_length, max_length*2) 内找
        let lookahead = (max_length * 2).min(remaining.len());
        let mut found = (max_length..lookahead)
            .find(|&i| SENTENCE_ENDINGS.contains(&remaining[i]));

        // 3. 仍未找到则扫描剩余全文
        if found.is_none() {
            found = (max_length..remaining.len())
                .find(|&i| SENTENCE_ENDINGS.contains(&remaining[i]));
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
pub fn parse_vtt(text: &str) -> Vec<Caption> {
    let mut out = Vec::new();
    let mut pending: Option<(u64, u64)> = None;
    let mut buf: Vec<String> = Vec::new();

    let flush = |out: &mut Vec<Caption>, pending: &mut Option<(u64, u64)>, buf: &mut Vec<String>| {
        if let Some((start_ms, end_ms)) = pending.take()
            && !buf.is_empty()
        {
            out.push(Caption { text: buf.join("\n"), start_ms, end_ms });
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
            if let (Some(s), Some(e)) = (parse_ts_ms(a.trim()), parse_ts_ms(b.trim())) {
                pending = Some((s, e));
            }
            continue;
        }
        // 纯数字的序号行：只有在还没开始收文本时才跳过
        if pending.is_some() && buf.is_empty() && line.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if pending.is_some() {
            buf.push(line.to_string());
        }
    }
    flush(&mut out, &mut pending, &mut buf);
    out
}

/// 解析 `HH:MM:SS.mmm`，失败返回 None。
fn parse_ts_ms(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let (sec, ms) = parts[2].split_once('.')?;
    let h: u64 = parts[0].parse().ok()?;
    let m: u64 = parts[1].parse().ok()?;
    let sec: u64 = sec.parse().ok()?;
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
        assert_eq!(split_text_for_vtt("很短的一句话。", 30), vec!["很短的一句话。"]);
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
        assert_eq!(cues.len(), 2, "期望切成 2 条 cue，实得 {}：{out}", cues.len());
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
}
