/// 移植自 packages/tts-node/src/vtt.ts 的 formatVttTime。
/// 时分用 floor 独立计算，秒取 `seconds % 60` 后四舍五入到 3 位小数——
/// 与 TS 版一致，包括秒进位不回填的缺陷。
pub fn format_vtt_time(seconds: f64) -> String {
    let hours = (seconds / 3600.0).floor() as i64;
    let minutes = ((seconds % 3600.0) / 60.0).floor() as i64;
    let secs = seconds % 60.0;
    let formatted = format!("{:.3}", secs);
    let (int_part, frac_part) = formatted.split_once('.').unwrap_or((formatted.as_str(), "000"));
    format!("{:02}:{:02}:{:0>2}.{}", hours, minutes, int_part, frac_part)
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

    /// TS 版的已知缺陷：时分由 floor 独立计算，秒进位时不回填。
    /// 首版刻意复刻以保证逐字节一致。
    #[test]
    fn format_vtt_time_replicates_ts_carry_bug() {
        assert_eq!(format_vtt_time(59.9996), "00:00:60.000");
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
}
