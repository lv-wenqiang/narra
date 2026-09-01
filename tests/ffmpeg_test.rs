use std::path::{Path, PathBuf};

#[test]
fn concat_list_escapes_single_quotes() {
    let body = panda::ffmpeg::concat_list_body(&[PathBuf::from("/tmp/it's here.mp3")]).unwrap();
    assert_eq!(body, r"file '/tmp/it'\''s here.mp3'");
}

#[test]
fn merges_two_clips_and_applies_atempo() {
    // 两段各 2 秒，1.1 倍速后应约 4 / 1.1 ≈ 3.64 秒
    for (i, name) in ["/tmp/m1.mp3", "/tmp/m2.mp3"].iter().enumerate() {
        std::process::Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i",
                   &format!("sine=frequency={}:duration=2", 300 + i * 100),
                   "-c:a", "libmp3lame", "-b:a", "48k", "-ar", "24000", "-ac", "1", name])
            .output().unwrap();
    }
    let out = Path::new("/tmp/merged.mp3");
    panda::ffmpeg::merge_mp3_with_speed(
        &[PathBuf::from("/tmp/m1.mp3"), PathBuf::from("/tmp/m2.mp3")], out, 1.1,
    ).unwrap();

    let d = panda::duration::mp3_duration_seconds(out);
    assert!((d - 3.64).abs() < 0.2, "期望约 3.64 秒，实际 {d}");
    // 中间的 concat 清单文件必须被清理
    assert!(!Path::new("/tmp/merged.mp3.concat.txt").exists());
}

#[test]
fn rejects_empty_input_list() {
    let err = panda::ffmpeg::merge_mp3_with_speed(&[], Path::new("/tmp/x.mp3"), 1.1).unwrap_err();
    assert!(err.to_string().contains("no input files"));
}

#[test]
fn rejects_paths_with_newlines() {
    let err = panda::ffmpeg::concat_list_body(&[PathBuf::from("/tmp/with\nnewline.mp3")]).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("换行符") || msg.contains("newline"));
    assert!(msg.contains("/tmp/with"));
}
