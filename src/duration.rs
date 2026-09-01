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
    if let Some(d) = probe_duration(path) {
        return d;
    }
    std::fs::metadata(path).map(|m| m.len() as f64 / 16000.0).unwrap_or(0.0)
}

fn probe_duration(path: &Path) -> Option<f64> {
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
