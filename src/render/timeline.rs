pub const FPS: u32 = 30;
/// 时间轴模块沿用的画布尺寸。**不再各写一份**：由 `Canvas::BASE` 推导，
/// 与 `render::draw` 共享唯一真相源（销 `docs/follow-ups.md` TTS 子系统
/// 第 3 条：画布尺寸此前同时存在于两个文件且互不引用）。
///
/// 计划 B 会把这两个 re-export 换成随运行期 `Canvas` 走的参数；本计划只做
/// 收敛，不改数值。
pub const WIDTH: u32 = crate::render::canvas::Canvas::BASE.w;
pub const HEIGHT: u32 = crate::render::canvas::Canvas::BASE.h;

/// Cover 段帧数。**`ffmpeg.rs` 的 `INTRO_START_SECS` 由它推导**——打字机音效
/// 的起点就是 Intro 段的起点，两处必须是同一个真相源：写成两份字面量时，
/// 改了这里而没改那里，`timeline.rs` 的测试（断言 15/105）与 `ffmpeg.rs` 的
/// 测试（断言 `adelay=500`/`adelay=4000`）会各自照旧全绿，成片里音效却和画面
/// 段落错位。跨模块一致性由 `ffmpeg.rs` 的
/// `segment_starts_match_the_audio_delays_in_the_filter_graph` 钉住。
pub const COVER_FRAMES: u32 = 15;
/// Intro 段帧数。`ffmpeg.rs` 的 `CONTENT_START_SECS` 由
/// `(COVER_FRAMES + INTRO_FRAMES) / FPS` 推导，理由同上。
pub const INTRO_FRAMES: u32 = 105;
const OUTRO_FRAMES: u32 = 120;
const CONTENT_TAIL_SECS: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    Cover = 0,
    Intro = 1,
    Content = 2,
    Outro = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub content_frames: u32,
    pub total_frames: u32,
}

/// 由音频时长（秒）算出各段帧数。对应规格 §8.2。
pub fn layout(audio_secs: f64) -> Layout {
    let content_frames = ((audio_secs + CONTENT_TAIL_SECS) * FPS as f64)
        .ceil()
        .max(0.0) as u32;
    Layout {
        content_frames,
        total_frames: COVER_FRAMES + INTRO_FRAMES + content_frames + OUTRO_FRAMES,
    }
}

/// 全局帧号 → (段落, 段内帧号)。超出总时长返回 None。
pub fn segment_at(layout: &Layout, global_frame: u32) -> Option<(Segment, u32)> {
    let intro_start = COVER_FRAMES;
    let content_start = intro_start + INTRO_FRAMES;
    let outro_start = content_start + layout.content_frames;

    if global_frame < intro_start {
        Some((Segment::Cover, global_frame))
    } else if global_frame < content_start {
        Some((Segment::Intro, global_frame - intro_start))
    } else if global_frame < outro_start {
        Some((Segment::Content, global_frame - content_start))
    } else if global_frame < layout.total_frames {
        Some((Segment::Outro, global_frame - outro_start))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_spec_numbers() {
        // A = 10 秒 → content = ceil(12 * 30) = 360，总帧 = 240 + 360 = 600
        let l = layout(10.0);
        assert_eq!(l.content_frames, 360);
        assert_eq!(l.total_frames, 600);
    }

    #[test]
    fn layout_rounds_content_frames_up() {
        // A = 10.01 → ceil(12.01 * 30) = ceil(360.3) = 361
        assert_eq!(layout(10.01).content_frames, 361);
    }

    #[test]
    fn segments_tile_the_timeline_without_gaps() {
        let l = layout(10.0);
        let mut counts = [0u32; 4];
        for f in 0..l.total_frames {
            let (seg, _) = segment_at(&l, f).expect("每一帧都应属于某个段落");
            counts[seg as usize] += 1;
        }
        assert_eq!(counts, [15, 105, 360, 120]);
    }

    #[test]
    fn segment_local_frames_restart_at_zero() {
        let l = layout(10.0);
        assert_eq!(segment_at(&l, 0), Some((Segment::Cover, 0)));
        assert_eq!(segment_at(&l, 14), Some((Segment::Cover, 14)));
        assert_eq!(segment_at(&l, 15), Some((Segment::Intro, 0)));
        assert_eq!(segment_at(&l, 119), Some((Segment::Intro, 104)));
        assert_eq!(segment_at(&l, 120), Some((Segment::Content, 0)));
        assert_eq!(segment_at(&l, 479), Some((Segment::Content, 359)));
        assert_eq!(segment_at(&l, 480), Some((Segment::Outro, 0)));
        assert_eq!(segment_at(&l, 599), Some((Segment::Outro, 119)));
    }

    #[test]
    fn segment_at_returns_none_past_the_end() {
        let l = layout(10.0);
        assert_eq!(segment_at(&l, 600), None);
    }
}
