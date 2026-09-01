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
}
