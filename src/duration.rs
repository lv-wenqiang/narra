use std::fs::File;
use std::path::Path;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Duration as SymphoniaDuration;

/// 读 mp3 时长（秒）。对应 packages/tts-node/src/duration.ts。
/// 解析失败时按 TS 版回退：文件字节数 / 16000。
pub fn mp3_duration_seconds(path: &Path) -> f64 {
    if let Some(d) = mp3_duration_seconds_strict(path) {
        return d;
    }
    std::fs::metadata(path).map(|m| m.len() as f64 / 16000.0).unwrap_or(0.0)
}

/// 严格版：只有真正解码出音频时长才返回 `Some`，解析失败（包括非 mp3 内容、
/// 空文件、损坏数据等）一律返回 `None`，**不做任何字节数回退估算**。
///
/// 存在的理由：`mp3_duration_seconds` 的字节数回退是流水线需要的兜底行为，
/// 但这个回退对垃圾数据不会失败——它会把任意字节流换算成一个"看似合理"的时长，
/// 导致"用时长断言校验音频确实可播放"这种测试失去鉴别力（垃圾数据也能凑出一个
/// 大于阈值的数字）。调用方如果需要区分"真解码成功"与"回退估算"，应该用这个
/// 严格版而不是 `mp3_duration_seconds`。
pub fn mp3_duration_seconds_strict(path: &Path) -> Option<f64> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");

    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .ok()?;

    let track = format.default_track(TrackType::Audio)?;
    let track_id = track.id;
    let time_base = track.time_base?;

    // MP3 通常无 Xing 头，n_frames 缺失，因此累加每个包的时长
    let mut total: u64 = 0;
    while let Ok(Some(packet)) = format.next_packet() {
        if packet.track_id == track_id {
            total += packet.dur.get();
        }
    }
    if total == 0 {
        return None;
    }
    let time = time_base.calc_duration(SymphoniaDuration::new(total))?;
    Some(time.as_secs_f64())
}
