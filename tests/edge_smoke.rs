// 需要网络，默认忽略；用 `cargo test --test edge_smoke -- --ignored` 显式运行。
//
// 覆盖：短中文文本（基本冒烟）、长文本（200 字以上）、不同音色（zh-CN-XiaoxiaoNeural）。
// 顾虑 3（Task 0 遗留）：探针只验证过单次、单音色、短文本，这里补齐长文本 + 换音色的验证。
//
// 必修/Minor 8（复审，第二轮打回）：最初用 `mp3_duration_seconds` + 时长阈值断言，
// 但该函数解码失败时会回退成"文件字节数 / 16000"，对垃圾数据不会失败，只会算出一个
// 看似合理的"时长"——复审量化过：302688 字节纯垃圾 -> 18.918s，照样能通过
// `> 10.0` 的断言，长文本那条断言完全没有鉴别力。改用 `mp3_duration_seconds_strict`
// （解码失败老实返回 `None`，不做字节数回退），先断言 `Some(_)`（证明是真解码出来的、
// 不是垃圾数据蒙混过关），再对解出的秒数做时长断言。

use panda::duration::mp3_duration_seconds_strict;
use panda::tts::backend::TtsBackend;
use panda::tts::edge::EdgeBackend;
use std::time::Duration;

#[tokio::test]
#[ignore]
async fn synthesizes_a_short_chinese_line() {
    let b = EdgeBackend::new("zh-CN-YunjianNeural", Duration::from_secs(120));
    let r = b.synth("这是一段测试文本。").await.unwrap();
    assert!(r.audio.len() > 5000, "音频过小：{} 字节", r.audio.len());

    let path = std::env::temp_dir().join("panda_edge_smoke_short.mp3");
    std::fs::write(&path, &r.audio).unwrap();
    let secs = mp3_duration_seconds_strict(&path)
        .expect("应能严格解码出真实时长，而不是垃圾数据蒙混过关");
    assert!(secs > 1.0, "落盘后应能解出 > 1 秒的可播放 mp3，实际 {secs}s");
}

#[tokio::test]
#[ignore]
async fn synthesizes_a_long_chinese_paragraph() {
    // 200 字以上的长文本，验证多个 BINARY 音频帧的拼接与多条 WordBoundary 的收集。
    let text = "在一个安静的小镇上，住着一位年迈的钟表匠。他每天清晨都会打开自己的小店，\
        仔细擦拭每一只经过他手中修理的钟表。镇上的人都说，只要是经过他手修好的钟表，\
        走时总是格外精准，仿佛被赋予了新的生命。有一天，一个陌生人带着一只破旧的怀表\
        找到了他，说这只表是家传的宝物，虽然已经停摆多年，但希望能够修复它，让它重新\
        走动起来。老钟表匠接过怀表，仔细端详了许久，然后微笑着点了点头，说这并不是一\
        件容易的事，但他愿意尽全力去尝试。经过整整三天的细心打磨与调试，怀表终于重新\
        发出了清脆的滴答声，仿佛在诉说着一个跨越时光的故事。";
    assert!(text.chars().count() > 200, "测试文本不足 200 字");

    let b = EdgeBackend::new("zh-CN-YunjianNeural", Duration::from_secs(120));
    let r = b.synth(text).await.unwrap();
    assert!(r.audio.len() > 20_000, "长文本音频过小：{} 字节", r.audio.len());

    let path = std::env::temp_dir().join("panda_edge_smoke_long.mp3");
    std::fs::write(&path, &r.audio).unwrap();
    let secs = mp3_duration_seconds_strict(&path)
        .expect("应能严格解码出真实时长，而不是垃圾数据蒙混过关");
    assert!(secs > 10.0, "长文本落盘后应能解出较长时长，实际 {secs}s");

    let timings = r.timings.expect("长文本应收到 WordBoundary 元数据");
    assert!(
        timings.len() > 10,
        "长文本 WordBoundary 条数过少：{}",
        timings.len()
    );
    eprintln!(
        "[long-text] 音频 {} 字节，时长 {secs}s，WordBoundary {} 条",
        r.audio.len(),
        timings.len()
    );
}

#[tokio::test]
#[ignore]
async fn synthesizes_with_a_different_voice() {
    let b = EdgeBackend::new("zh-CN-XiaoxiaoNeural", Duration::from_secs(120));
    let r = b.synth("这是使用另一个音色的测试文本。").await.unwrap();
    assert!(r.audio.len() > 5000, "音频过小：{} 字节", r.audio.len());

    let path = std::env::temp_dir().join("panda_edge_smoke_xiaoxiao.mp3");
    std::fs::write(&path, &r.audio).unwrap();
    let secs = mp3_duration_seconds_strict(&path)
        .expect("应能严格解码出真实时长，而不是垃圾数据蒙混过关");
    assert!(secs > 1.0, "落盘后应能解出 > 1 秒的可播放 mp3，实际 {secs}s");
    eprintln!("[xiaoxiao] 音频 {} 字节，时长 {secs}s", r.audio.len());
}
