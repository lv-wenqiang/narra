//! 帧分派：把全局帧号路由到 Cover/Intro/Content/Outro 四个段落绘制器。
//!
//! `FrameSource` 是唯一入口——持有 `Painter`（构造一次，约 100ms）、`Layout`
//! （由音频时长算出的分段边界）与字幕列表，`render(global_frame)` 每次只做
//! `segment_at` 查表 + 建画布 + 分发到对应 `draw_*`，不重造任何昂贵对象。

use anyhow::{bail, Result};
use tiny_skia::Pixmap;

use crate::render::draw::Painter;
use crate::render::timeline::{layout, segment_at, Layout, Segment, HEIGHT, WIDTH};
use crate::vtt::{parse_vtt, Caption};

pub struct FrameSource {
    painter: Painter,
    layout: Layout,
    captions: Vec<Caption>,
    title: String,
}

impl FrameSource {
    /// 由 VTT 文本与标题构造。音频时长取最后一条字幕的结束时间。
    pub fn new(vtt_text: &str, title: String) -> Result<Self> {
        let captions = parse_vtt(vtt_text);
        let audio_secs = captions.iter().map(|c| c.end_ms).max().unwrap_or(0) as f64 / 1000.0;
        Ok(Self {
            painter: Painter::new()?,
            layout: layout(audio_secs),
            captions,
            title,
        })
    }

    pub fn total_frames(&self) -> u32 {
        self.layout.total_frames
    }

    /// 渲染一帧。Cover/Intro/Outro 为不透明白底，Content 为透明底。
    pub fn render(&mut self, global_frame: u32) -> Result<Pixmap> {
        let Some((seg, local)) = segment_at(&self.layout, global_frame) else {
            bail!("帧号 {global_frame} 超出总时长 {}", self.layout.total_frames);
        };
        let mut pixmap = Pixmap::new(WIDTH, HEIGHT).expect("画布尺寸应合法");
        match seg {
            Segment::Cover => self.painter.draw_cover(&mut pixmap, &self.title),
            Segment::Intro => self.painter.draw_intro(&mut pixmap, local, &self.title),
            Segment::Content => self.painter.draw_content(&mut pixmap, local, &self.captions),
            Segment::Outro => self.painter.draw_outro(&mut pixmap, local),
        }
        Ok(pixmap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VTT: &str = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:04.000\n第一条。\n\n2\n00:00:04.000 --> 00:00:10.000\n第二条。\n";

    #[test]
    fn total_frames_follows_the_last_cue_end_time() {
        let fs = FrameSource::new(VTT, "标题".into()).unwrap();
        // A = 10 秒 → content = ceil(12*30) = 360 → 总帧 = 600
        assert_eq!(fs.total_frames(), 600);
    }

    #[test]
    fn every_frame_renders_at_the_right_size() {
        let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
        for f in [0, 14, 15, 119, 120, 479, 480, 599] {
            let p = fs.render(f).unwrap();
            assert_eq!((p.width(), p.height()), (1280, 720), "帧 {f} 尺寸错误");
        }
    }

    #[test]
    fn cover_intro_outro_are_opaque_and_content_is_transparent() {
        let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
        for f in [0, 60, 500] {
            let p = fs.render(f).unwrap();
            assert_eq!(p.pixel(0, 0).unwrap().alpha(), 255, "帧 {f} 应不透明");
        }
        let p = fs.render(200).unwrap(); // Content 段
        assert_eq!(p.pixel(0, 0).unwrap().alpha(), 0, "Content 段应透明底");
    }

    #[test]
    fn rendering_past_the_end_is_an_error() {
        let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
        assert!(fs.render(600).is_err());
    }

    #[test]
    fn renders_the_whole_timeline_without_panicking() {
        let mut fs = FrameSource::new(VTT, "标题".into()).unwrap();
        // 全量跑一遍，抓 panic 与非有限几何
        for f in 0..fs.total_frames() {
            fs.render(f).unwrap();
        }
    }

    /// 鉴别性测试：分派到每个段落的输出必须与直接调用对应 `draw_*` 逐字节一致。
    ///
    /// 这条测试专门盯 `render()` 里的 `match` 分支——如果有人把 `Segment::Cover`
    /// 与 `Segment::Intro` 的分支对调、或者把 `draw_intro` 的 `local_frame` 参数
    /// 误传成 `global_frame`，这里会先炸：`Painter` 的绘制结果只取决于传入的
    /// 段内帧号与内容参数（无隐藏的时钟/随机状态，见 `draw.rs` 里 `Painter` 字段
    /// 文档），所以两次独立构造、传相同参数，像素必须完全相等。
    #[test]
    fn dispatch_matches_calling_the_matching_draw_fn_directly_with_the_local_frame() {
        let title = "标题".to_string();
        let mut fs = FrameSource::new(VTT, title.clone()).unwrap();
        let captions = parse_vtt(VTT);
        let mut reference = Painter::new().unwrap();

        // Cover: global 0..15，段内帧号与 global 相同。
        let got = fs.render(0).unwrap();
        let mut want = Pixmap::new(WIDTH, HEIGHT).unwrap();
        reference.draw_cover(&mut want, &title);
        assert_eq!(got.data(), want.data(), "Cover 帧 0 应与直接调 draw_cover 一致");

        // Intro: global 15..120，段内帧号 = global - 15。选 100（段内 85）而不是
        // 随便一个早期帧：局部帧 85 在淡出区间 [90,104) 之前（不透明），而如果
        // 误把 global_frame（100）当成段内帧号传进去，100 恰好落在淡出区间
        // 内、会明显更淡——这个像素差异足够大，能稳定被 `assert_eq!` 抓住
        // （早期尝试过 20/局部 5：因为这份测试用的标题只有 2 个字，打字机在
        // 局部帧 5 与误用的 20 都还没吐出任何字符、光标闪烁相位又碰巧同余
        // 15，两种取法渲染结果毫无差别，测试形同虚设——已实测确认，见任务
        // 报告的「变异实验」一节）。
        let got = fs.render(100).unwrap();
        let mut want = Pixmap::new(WIDTH, HEIGHT).unwrap();
        reference.draw_intro(&mut want, 85, &title);
        assert_eq!(
            got.data(),
            want.data(),
            "Intro 帧 100（段内帧 85）应与直接调 draw_intro(_, 85, _) 一致"
        );

        // Content: global 120..480，段内帧号 = global - 120。
        let got = fs.render(200).unwrap();
        let mut want = Pixmap::new(WIDTH, HEIGHT).unwrap();
        reference.draw_content(&mut want, 80, &captions);
        assert_eq!(
            got.data(),
            want.data(),
            "Content 帧 200（段内帧 80）应与直接调 draw_content(_, 80, _) 一致"
        );

        // Outro: global 480..600，段内帧号 = global - 480。
        let got = fs.render(550).unwrap();
        let mut want = Pixmap::new(WIDTH, HEIGHT).unwrap();
        reference.draw_outro(&mut want, 70);
        assert_eq!(
            got.data(),
            want.data(),
            "Outro 帧 550（段内帧 70）应与直接调 draw_outro(_, 70) 一致"
        );
    }

    /// 鉴别性测试：`total_frames` 必须由「所有字幕结束时间的最大值」决定，
    /// 而不是「最后一条字幕」的结束时间——VTT 里字幕不一定按结束时间单调排列
    /// （现实数据一般是的，但解析器不做这个假设，`FrameSource::new` 也不该做）。
    /// 这里故意把结束时间更晚的字幕放在文件前面，来把 `.max()` 与
    /// `.last().end_ms` 两种实现区分开。
    #[test]
    fn total_frames_uses_the_max_end_time_not_the_last_caption_in_file_order() {
        let vtt = "WEBVTT\n\n\
                   1\n00:00:00.000 --> 00:00:30.000\n时间更晚但排在前面的字幕。\n\n\
                   2\n00:00:05.000 --> 00:00:10.000\n排在后面但结束更早的字幕。\n";
        let fs = FrameSource::new(vtt, "标题".into()).unwrap();
        // max(30, 10) = 30 秒 → content = ceil(32*30) = 960 → 总帧 = 240+960 = 1200
        // 若误用 last().end_ms（=10 秒）会得到 600，与此不同。
        assert_eq!(fs.total_frames(), 1200);
    }
}
