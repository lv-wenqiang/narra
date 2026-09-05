use std::path::Path;

#[test]
fn reads_mp3_duration_within_tolerance() {
    let d = narra::duration::mp3_duration_seconds(Path::new("tests/fixtures/tone_3s5.mp3"));
    assert!((d - 3.5).abs() < 0.1, "期望约 3.5 秒，实际 {d}");
}

#[test]
fn falls_back_to_byte_estimate_for_unreadable_file() {
    // 非 mp3 内容：解析失败后按 TS 版的 len/16000 估算
    std::fs::write("/tmp/not_audio.mp3", vec![0u8; 32000]).unwrap();
    let d = narra::duration::mp3_duration_seconds(Path::new("/tmp/not_audio.mp3"));
    assert!((d - 2.0).abs() < 1e-9, "期望回退估算 2.0 秒，实际 {d}");
}

#[test]
fn strict_decodes_real_mp3_to_some() {
    let d = narra::duration::mp3_duration_seconds_strict(Path::new("tests/fixtures/tone_3s5.mp3"));
    let d = d.expect("真实 mp3 应该解码成功");
    assert!((d - 3.5).abs() < 0.1, "期望约 3.5 秒，实际 {d}");
}

#[test]
fn strict_returns_none_for_garbage_bytes() {
    // 与 falls_back_to_byte_estimate_for_unreadable_file 用的是同一类垃圾数据，
    // 但严格版不应该套用字节数回退公式，必须老实返回 None。
    std::fs::write("/tmp/not_audio_strict.mp3", vec![0u8; 32000]).unwrap();
    let d = narra::duration::mp3_duration_seconds_strict(Path::new("/tmp/not_audio_strict.mp3"));
    assert!(d.is_none(), "垃圾字节不应该被解码出时长，实际 {d:?}");
}
