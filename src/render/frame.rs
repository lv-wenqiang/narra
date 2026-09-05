//! 帧分派：把全局帧号路由到 Cover/Intro/Content/Outro 四个段落绘制器。
//!
//! `FrameSource` 是唯一入口——持有 `Painter`（构造一次，约 100ms）、`Layout`
//! （由音频时长算出的分段边界）与字幕列表，`render(global_frame)` 每次只做
//! `segment_at` 查表 + 建画布 + 分发到对应 `draw_*`，不重造任何昂贵对象。

use anyhow::{Result, bail};
use std::io::Write;
use tiny_skia::Pixmap;

use crate::config::Branding;
use crate::render::canvas::Canvas;
use crate::render::draw::Painter;
use crate::render::timeline::{Layout, Segment, layout, segment_at};
use crate::vtt::{Caption, parse_vtt};

pub struct FrameSource {
    painter: Painter,
    layout: Layout,
    captions: Vec<Caption>,
    title: String,
    /// 本次渲染的画布尺寸。`ffmpeg` 侧要用它决定 `-s` 与滤镜里的尺寸
    /// （见 [`FrameSource::canvas`]）。
    canvas: Canvas,
}

impl FrameSource {
    /// 由 VTT 文本与标题构造。音频时长取**所有字幕结束时间的最大值**
    /// （不是文件里最后一条字幕的结束时间——VTT 不保证按结束时间单调排列，
    /// 见 `audio_secs_uses_the_max_end_time_not_the_last_caption_in_file_order`）。
    ///
    /// **解析不出任何字幕时报错而不是继续**：`parse_vtt` 对任何无法解析的输入
    /// 都返回空 `Vec` 而不报错（无时间行、编码错乱、拿错文件……）。若放行，
    /// `audio_secs` 会是 0 → `content_frames` 取 `layout()` 撑出的下限 60 帧 →
    /// 总帧 300，整条流程会一路跑到底，产出一个 **10 秒**、字幕全空、把长旁白
    /// 截断到 10 秒的 mp4，退出码 0 并打印「成片已写入」。拿错 `--vtt` 是最常
    /// 见的用户错误，而这个失败形式完全静默。
    pub fn new(vtt_text: &str, title: String, branding: &Branding, canvas: Canvas) -> Result<Self> {
        let captions = parse_vtt(vtt_text);
        if captions.is_empty() {
            bail!(
                "字幕文件里没有解析出任何字幕，请确认 --vtt 指向的是 TTS 产出的 audio.vtt（WebVTT 格式，含 `00:00:00.000 --> 00:00:03.000` 这样的时间行）"
            );
        }
        let audio_secs = captions.iter().map(|c| c.end_ms).max().unwrap_or(0) as f64 / 1000.0;
        Ok(Self {
            painter: Painter::new(branding, canvas)?,
            layout: layout(audio_secs),
            captions,
            title,
            canvas,
        })
    }

    /// 本次渲染的画布尺寸。`ffmpeg` 侧要用它决定 `-s` 与滤镜里的尺寸。
    pub fn canvas(&self) -> Canvas {
        self.canvas
    }

    pub fn total_frames(&self) -> u32 {
        self.layout.total_frames
    }

    /// 渲染一帧。Cover/Intro/Outro 为不透明白底，Content 为透明底。
    pub fn render(&mut self, global_frame: u32) -> Result<Pixmap> {
        let Some((seg, local)) = segment_at(&self.layout, global_frame) else {
            bail!(
                "帧号 {global_frame} 超出总时长 {}",
                self.layout.total_frames
            );
        };
        let mut pixmap = Pixmap::new(self.canvas.w, self.canvas.h).expect("画布尺寸应合法");
        match seg {
            Segment::Cover => self.painter.draw_cover(&mut pixmap, &self.title),
            Segment::Intro => self.painter.draw_intro(&mut pixmap, local, &self.title),
            Segment::Content => self
                .painter
                .draw_content(&mut pixmap, local, &self.captions),
            Segment::Outro => self.painter.draw_outro(&mut pixmap, local),
        }
        Ok(pixmap)
    }

    /// 音频时长 `A`（秒）：**所有字幕结束时间的最大值**（不是文件里最后一条
    /// 字幕的结束时间——VTT 不保证按结束时间单调排列，见
    /// `audio_secs_uses_the_max_end_time_not_the_last_caption_in_file_order`）。
    /// Content 段长 `ceil((A+2)*30)` 帧，BGM 的淡出区间由它推出。
    pub fn audio_secs(&self) -> f64 {
        self.captions.iter().map(|c| c.end_ms).max().unwrap_or(0) as f64 / 1000.0
    }

    /// Content 段的帧数。Outro 起点 = `120 + content_frames`。
    pub fn content_frames(&self) -> u32 {
        self.layout.content_frames
    }

    /// 逐帧渲染整条时间轴并写出 straight-alpha RGBA8，返回写出的帧数。
    ///
    /// **渲染与写出跑在两个线程上**（`crate::render::stream::stream_frames`）：
    /// 渲染留在调用线程，写出挪到作用域线程，中间一个容量 `CAPACITY` 的有界
    /// 通道。同一个线程上串行做这两件事时，`render → write` 严格轮流——`write`
    /// 一直卡到 ffmpeg 吃完这一帧，期间下一帧连渲染都没开始，实测端到端白丢
    /// 约 18%（`docs/ffmpeg-pipeline.md` §11）。
    ///
    /// 缓冲区在两端之间循环复用（一帧 8.3MB，每帧重新分配是纯浪费）。写失败
    /// 立即停止渲染——ffmpeg 提前退出时写端会收到 broken pipe，属于正常的失败
    /// 路径，不是 panic。
    pub fn write_rgba_frames<W: Write + Send>(&mut self, out: &mut W) -> Result<u32> {
        let frame_bytes = self.canvas.w as usize * self.canvas.h as usize * 4;
        crate::render::stream::stream_frames(
            self.total_frames(),
            frame_bytes,
            crate::render::stream::CAPACITY,
            |f, buf| {
                let pixmap = self.render(f)?;
                unpremultiply_into(&pixmap, buf);
                Ok(())
            },
            out,
        )
    }
}

/// 把 `Pixmap` 的**预乘** RGBA8 转成 ffmpeg `-pix_fmt rgba` 要求的
/// **straight** alpha，写进 `buf`（会先 `clear()`，容量复用）。
///
/// 为什么必须做这一步：`tiny_skia::Pixmap` 内部存的是预乘值（R 已经乘过
/// alpha），而 ffmpeg 的 `rgba` 是 straight。直接喂过去，每个半透明像素会
/// 被再乘一次 alpha——Content 段字幕的入场动画（opacity 0→1）与 Outro 的
/// 整体淡出会整体发暗，而**成片能播、不报错**，属于不容易发现的那类。
///
/// `alpha == 0` 时 RGB 没有定义，统一输出 0，避免把预乘残留的垃圾值喂出去。
///
/// **`alpha == 0` 走快路径**：`PremultipliedColorU8::demultiply()` 只对
/// `alpha == 255` 短路，`alpha == 0` 仍要走三次浮点除法（除数为 0）。而
/// Content 段是**透明底**，绝大多数像素正是这一支——全时间轴下它是最热的
/// 一条路径。直接写四个 0 与 `demultiply()` 的结果逐字节相同
/// （`unpremultiply_fast_path_is_byte_identical_to_plain_demultiply` 拿真实
/// 渲染帧对着未优化的参考实现比对过）。
fn unpremultiply_into(pixmap: &Pixmap, buf: &mut Vec<u8>) {
    buf.clear();
    buf.reserve(pixmap.width() as usize * pixmap.height() as usize * 4);
    for px in pixmap.pixels() {
        if px.alpha() == 0 {
            buf.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let c = px.demultiply();
        buf.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VTT: &str = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:04.000\n第一条。\n\n2\n00:00:04.000 --> 00:00:10.000\n第二条。\n";

    /// `alpha == 0` 的像素必须输出四个 0。
    ///
    /// 这是 `unpremultiply_into` 里 `alpha == 0` 快路径依赖的不变量：预乘表示
    /// 下 alpha 为 0 时 RGB 没有定义（存的是乘过 0 的残留），必须统一归零而
    /// 不是把残留值喂给 ffmpeg。Content 段是透明底，绝大多数像素走这一支。
    #[test]
    fn fully_transparent_pixels_unpremultiply_to_four_zero_bytes() {
        let p = Pixmap::new(4, 4).unwrap(); // 新建的 Pixmap 全透明
        let mut buf = Vec::new();
        unpremultiply_into(&p, &mut buf);
        assert_eq!(buf.len(), 4 * 4 * 4);
        assert!(
            buf.iter().all(|&b| b == 0),
            "全透明画布应逐字节为 0，实得非零字节"
        );
    }

    /// **快路径与逐像素 `demultiply()` 逐字节等价。**
    ///
    /// `alpha == 0` 时直接写四个 0，是一条基于「`demultiply()` 对 alpha 为 0
    /// 的像素也返回全 0」的短路。这条测试拿真实渲染帧（含不透明、半透明、
    /// 全透明三类像素）对着**未优化的参考实现**逐字节比对——优化若改变了
    /// 任何一个像素，这里立刻变红。
    ///
    /// 参考实现留在测试里而不是留一个 `#[cfg(feature)]` 开关：它只有三行，
    /// 而把两条路径都编进生产二进制会让「到底跑的是哪条」多一种说法。
    #[test]
    fn unpremultiply_fast_path_is_byte_identical_to_plain_demultiply() {
        /// 优化前的写法：无条件逐像素 `demultiply()`。
        fn reference(pixmap: &Pixmap, buf: &mut Vec<u8>) {
            buf.clear();
            for px in pixmap.pixels() {
                let c = px.demultiply();
                buf.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
            }
        }

        let mut fs = FrameSource::new(
            VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        // 四段各取一帧：Cover(0) 不透明白底、Intro(100) 白底 + 文字、
        // Content(200) 透明底 + 半透明抗锯齿边缘、Outro(550) 白底 + 缩放动画。
        for f in [0u32, 100, 200, 550] {
            let pixmap = fs.render(f).unwrap();
            let mut fast = Vec::new();
            let mut slow = Vec::new();
            unpremultiply_into(&pixmap, &mut fast);
            reference(&pixmap, &mut slow);
            assert_eq!(fast, slow, "第 {f} 帧的快路径输出应与参考实现逐字节相同");
        }
    }

    #[test]
    fn total_frames_follows_the_last_cue_end_time() {
        let fs = FrameSource::new(
            VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        // A = 10 秒 → content = ceil(12*30) = 360 → 总帧 = 600
        assert_eq!(fs.total_frames(), 600);
    }

    #[test]
    fn every_frame_renders_at_the_right_size() {
        let mut fs = FrameSource::new(
            VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        for f in [0, 14, 15, 119, 120, 479, 480, 599] {
            let p = fs.render(f).unwrap();
            assert_eq!((p.width(), p.height()), (1280, 720), "帧 {f} 尺寸错误");
        }
    }

    /// **Fix round 1（画布链变异验证）**：`render()` 建的 `Pixmap` 尺寸、
    /// `canvas()` 的返回值都必须来自构造时传入的 `canvas`，不是隐式的
    /// `Canvas::BASE`。
    ///
    /// 仓库里此前全部 `FrameSource::new` 调用都传 `Canvas::BASE`，所以
    /// 「`render()`/`canvas()` 内部改回读 `Canvas::BASE`」这两个变异能骗过
    /// 全部既有测试——断言的「尺寸应为 1280x720」在两种实现下都成立。这里
    /// 用一个非 BASE 的真实目标尺寸（Plan B 的竖版 1920x1080）钉住。
    #[test]
    fn render_and_canvas_accessor_follow_the_canvas_given_at_construction() {
        let non_base = Canvas { w: 1920, h: 1080 };
        let mut fs =
            FrameSource::new(VTT, "标题".into(), &Branding::plain("测试品牌"), non_base).unwrap();
        assert_eq!(fs.canvas(), non_base, "canvas() 应原样回传构造时传入的画布");
        let p = fs.render(0).unwrap();
        assert_eq!(
            (p.width(), p.height()),
            (non_base.w, non_base.h),
            "render() 建的 Pixmap 尺寸应跟着构造时的 canvas 走，不是写死的 BASE"
        );
    }

    #[test]
    fn cover_intro_outro_are_opaque_and_content_is_transparent() {
        let mut fs = FrameSource::new(
            VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        for f in [0, 60, 500] {
            let p = fs.render(f).unwrap();
            assert_eq!(p.pixel(0, 0).unwrap().alpha(), 255, "帧 {f} 应不透明");
        }
        let p = fs.render(200).unwrap(); // Content 段
        assert_eq!(p.pixel(0, 0).unwrap().alpha(), 0, "Content 段应透明底");
    }

    #[test]
    fn rendering_past_the_end_is_an_error() {
        let mut fs = FrameSource::new(
            VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        assert!(fs.render(600).is_err());
    }

    #[test]
    fn renders_the_whole_timeline_without_panicking() {
        let mut fs = FrameSource::new(
            VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
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
        let mut fs = FrameSource::new(
            VTT,
            title.clone(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        let captions = parse_vtt(VTT);
        let mut reference = Painter::new(&Branding::plain("测试品牌"), Canvas::BASE).unwrap();

        // Cover: global 0..15，段内帧号与 global 相同。
        let got = fs.render(0).unwrap();
        let mut want = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
        reference.draw_cover(&mut want, &title);
        assert_eq!(
            got.data(),
            want.data(),
            "Cover 帧 0 应与直接调 draw_cover 一致"
        );

        // Intro: global 15..120，段内帧号 = global - 15。选 100（段内 85）而不是
        // 随便一个早期帧：局部帧 85 在淡出区间 [90,104) 之前（不透明），而如果
        // 误把 global_frame（100）当成段内帧号传进去，100 恰好落在淡出区间
        // 内、会明显更淡——这个像素差异足够大，能稳定被 `assert_eq!` 抓住
        // （早期尝试过 20/局部 5：因为这份测试用的标题只有 2 个字，打字机在
        // 局部帧 5 与误用的 20 都还没吐出任何字符、光标闪烁相位又碰巧同余
        // 15，两种取法渲染结果毫无差别，测试形同虚设——已实测确认，见任务
        // 报告的「变异实验」一节）。
        let got = fs.render(100).unwrap();
        let mut want = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
        reference.draw_intro(&mut want, 85, &title);
        assert_eq!(
            got.data(),
            want.data(),
            "Intro 帧 100（段内帧 85）应与直接调 draw_intro(_, 85, _) 一致"
        );

        // Content: global 120..480，段内帧号 = global - 120。
        let got = fs.render(200).unwrap();
        let mut want = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
        reference.draw_content(&mut want, 80, &captions);
        assert_eq!(
            got.data(),
            want.data(),
            "Content 帧 200（段内帧 80）应与直接调 draw_content(_, 80, _) 一致"
        );

        // Outro: global 480..600，段内帧号 = global - 480。
        let got = fs.render(550).unwrap();
        let mut want = Pixmap::new(Canvas::BASE.w, Canvas::BASE.h).unwrap();
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
        let fs = FrameSource::new(
            vtt,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        // max(30, 10) = 30 秒 → content = ceil(32*30) = 960 → 总帧 = 240+960 = 1200
        // 若误用 last().end_ms（=10 秒）会得到 600，与此不同。
        assert_eq!(fs.total_frames(), 1200);
    }

    use tiny_skia::{Paint, Rect, Transform};

    /// 造一张含半透明像素的画布：左半边 alpha=128 的纯红，右半边全透明。
    fn half_transparent_red() -> Pixmap {
        let mut p = Pixmap::new(4, 1).unwrap();
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 0, 0, 128);
        p.fill_rect(
            Rect::from_xywh(0.0, 0.0, 2.0, 1.0).unwrap(),
            &paint,
            Transform::identity(),
            None,
        );
        p
    }

    #[test]
    fn unpremultiply_restores_full_intensity_red_for_half_alpha_pixels() {
        let p = half_transparent_red();
        // 预乘态下红通道已经被 alpha 乘过：128/255*255 ≈ 128，而不是 255。
        assert!(
            p.data()[0] < 200,
            "前提检查：Pixmap 应是预乘的，实得 R={}",
            p.data()[0]
        );

        let mut buf = Vec::new();
        unpremultiply_into(&p, &mut buf);

        #[allow(clippy::identity_op)] // 保留 w*h*4 的字面形状，w=4/h=1 只是这张探针画布的尺寸
        {
            assert_eq!(buf.len(), 4 * 1 * 4, "输出应是 w*h*4 字节");
        }
        // 反预乘后红通道应回到接近 255（整数除法允许 ±2 误差）。
        assert!(buf[0] >= 253, "反预乘后 R 应接近 255，实得 {}", buf[0]);
        assert_eq!(buf[3], 128, "alpha 通道不应被改动");
        // 全透明像素：alpha=0 时 RGB 无意义，但必须是 0 而不是垃圾值。
        assert_eq!(&buf[8..12], &[0, 0, 0, 0], "全透明像素应输出全 0");
    }

    #[test]
    fn unpremultiply_leaves_opaque_pixels_byte_identical() {
        // alpha=255 时反预乘是恒等运算——这条保证 Cover/Intro/Outro 三段不受影响。
        let mut p = Pixmap::new(2, 1).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(200, 100, 50, 255));
        let mut buf = Vec::new();
        unpremultiply_into(&p, &mut buf);
        assert_eq!(buf, vec![200, 100, 50, 255, 200, 100, 50, 255]);
    }

    #[test]
    fn unpremultiply_reuses_the_buffer_without_growing_it() {
        // 写帧是热路径，缓冲区必须复用而不是每帧重新分配。
        let p = half_transparent_red();
        let mut buf = Vec::new();
        unpremultiply_into(&p, &mut buf);
        let cap = buf.capacity();
        for _ in 0..10 {
            unpremultiply_into(&p, &mut buf);
            assert_eq!(buf.len(), 16);
        }
        assert_eq!(buf.capacity(), cap, "重复调用不应导致重新分配");
    }

    #[test]
    fn audio_secs_and_content_frames_follow_the_layout() {
        let fs = FrameSource::new(
            VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        // A = 10 秒 → content = ceil(12*30) = 360 → 总帧 600
        assert!(
            (fs.audio_secs() - 10.0).abs() < 1e-9,
            "实得 {}",
            fs.audio_secs()
        );
        assert_eq!(fs.content_frames(), 360);
        assert_eq!(fs.total_frames(), 600);
    }

    /// 鉴别性测试：`audio_secs` 必须由「所有字幕结束时间的最大值」决定，而不是
    /// 「文件里最后一条字幕」的结束时间——理由与
    /// `total_frames_uses_the_max_end_time_not_the_last_caption_in_file_order`
    /// 完全一样，但那条测的是 `total_frames`（经 `FrameSource::new` 里另一份
    /// 独立的 `.max()` 计算得出），并不经过 `audio_secs()` 这个方法本身；本任务
    /// 模块级 `VTT` 常量里两条 cue 恰好按结束时间升序排列，`.last().end_ms` 与
    /// `.max()` 在那份输入上结果相同，不足以把两种实现区分开，必须用一份「结束
    /// 更晚的字幕排在文件前面」的输入才能让 `audio_secs()` 的这两种写法分道扬镳。
    #[test]
    fn audio_secs_uses_the_max_end_time_not_the_last_caption_in_file_order() {
        let vtt = "WEBVTT\n\n\
                   1\n00:00:00.000 --> 00:00:30.000\n时间更晚但排在前面的字幕。\n\n\
                   2\n00:00:05.000 --> 00:00:10.000\n排在后面但结束更早的字幕。\n";
        let fs = FrameSource::new(
            vtt,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        // max(30, 10) = 30 秒。若误用 last().end_ms（=10 秒）会得到 10.0，与此不同。
        assert!(
            (fs.audio_secs() - 30.0).abs() < 1e-9,
            "实得 {}",
            fs.audio_secs()
        );
    }

    /// 只统计字节数、不保存内容。整条时间轴是 600 帧 × 3.5MB ≈ 2.1GB，
    /// 攒进 `Vec<u8>` 是不可接受的；写帧本来就是流式的，测试也该是流式的。
    struct CountingWriter {
        bytes: usize,
    }
    impl std::io::Write for CountingWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.bytes += b.len();
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 只记住指定帧的**首像素**，其余字节丢弃。用来在字节流里验证段落语义
    /// 而不占内存。依赖 `write_rgba_frames` 每帧恰好一次 `write_all`。
    struct FirstPixelPicker {
        frame_bytes: usize,
        seen: usize,
        picked: std::collections::HashMap<usize, [u8; 4]>,
    }
    impl std::io::Write for FirstPixelPicker {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            assert_eq!(b.len(), self.frame_bytes, "每帧应恰好一次 write_all 整帧");
            self.picked.insert(self.seen, [b[0], b[1], b[2], b[3]]);
            self.seen += 1;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 这两条「写满整条时间轴」的测试用的 VTT 与其余测试不同（空字幕，而不是
    /// 模块级 `VTT` 常量）：`opt-level=3` 的 dev profile 覆盖只作用于依赖
    /// crate（见 `Cargo.toml` 里 `[profile.dev.package."*"]` 上方的注释），
    /// `unpremultiply_into` 是本任务新加的、属于我们自己 crate 的逐像素循环，
    /// 不在覆盖范围内——实测过 600 帧全量跑 `write_rgba_frames` 单条要 ~27s，
    /// 而 `render()`（同样全量 600 帧、但不走 `unpremultiply_into`）只要 6.6s，
    /// 说明多出来的时间几乎全在这个新循环上，不是 profile 没生效。
    ///
    /// 参照仓库既有的代表帧收窄手法（`src/render/draw.rs` 里
    /// `outro_renders_representative_frames_without_panicking` 一带），把
    /// 输入换成空字幕（`audio_secs=0` → `content_frames` 取 `layout()` 里
    /// `CONTENT_TAIL_SECS` 撑出的下限 60 帧 → 总帧数 300，是能达到的最小
    /// 时间轴），实测把单条压到 ~12.7s。这不是跳过帧——`write_rgba_frames`
    /// 仍然对返回的全部帧各自调用恰好一次 `write_all`，只是全量意义上的
    /// 「全部帧」从 600 条缩到 300 条；两个测试真正要钉住的性质（字节数精确
    /// 等于 帧数×W×H×4、Cover 不透明、Content 透明）在 300 帧下同样成立。
    ///
    /// **为什么不是空字幕（原来是 `"WEBVTT\n\n"`）**：`FrameSource::new` 现在
    /// 对「解析不出任何字幕」报错——那是拿错 `--vtt` 时唯一能拦住静默坏片的
    /// 地方，不能为了让这两条测试跑得快就留一条 `new_unchecked` 后门（后门
    /// 一旦存在，生产代码哪天改去调它也不会有测试变红）。改成一条**零长度
    /// 的占位 cue**：`end_ms = 0` → `audio_secs = 0`，`layout()` 算出的帧数与
    /// 空字幕时**逐帧相同**（仍是 `CONTENT_TAIL_SECS` 撑出的 300 帧），两条
    /// 测试的断言值一个字都不用改，耗时也不变。
    const SHORT_VTT: &str = "WEBVTT\n\n1\n00:00:00.000 --> 00:00:00.000\n占位。\n";

    /// `FrameSource::new` 必须在解析不出任何字幕时报错。
    ///
    /// 没有这条守卫时，`narra render --vtt 拿错的文件.txt` 会走完整条流程：
    /// `audio_secs=0` → `content_frames=60` → 总帧 300 → 产出一个 10 秒、字幕
    /// 全空、把长旁白截断到 10 秒的 mp4，**退出码 0 并打印「成片已写入」**。
    #[test]
    fn constructing_from_a_vtt_with_no_parsable_cues_is_an_error() {
        // 三种「解析不出字幕」的真实形态：空文件、只有 WEBVTT 头、
        // 拿错文件（纯文本，没有任何时间行）。
        for (name, text) in [
            ("空文件", ""),
            ("只有头", "WEBVTT\n\n"),
            ("拿错文件", "这是一份旁白文稿，不是字幕。\n第二行。\n"),
        ] {
            let err = FrameSource::new(
                text,
                "标题".into(),
                &Branding::plain("测试品牌"),
                Canvas::BASE,
            )
            .err()
            .unwrap_or_else(|| panic!("{name} 应当报错，而不是产出一个 10 秒空片"));
            let msg = err.to_string();
            assert!(
                msg.contains("--vtt"),
                "{name} 的错误文案应指导用户去检查 --vtt，实得：{msg}"
            );
        }
    }

    #[test]
    fn write_rgba_frames_emits_exactly_one_frame_worth_of_bytes_per_frame() {
        let mut fs = FrameSource::new(
            SHORT_VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        assert_eq!(fs.total_frames(), 300, "空字幕应给出最小时间轴 300 帧");
        let mut w = CountingWriter { bytes: 0 };
        let n = fs.write_rgba_frames(&mut w).unwrap();
        assert_eq!(n, 300);
        assert_eq!(
            w.bytes,
            300 * 1280 * 720 * 4,
            "字节数必须精确等于 帧数×W×H×4"
        );
    }

    #[test]
    fn write_rgba_frames_emits_opaque_cover_and_transparent_content() {
        // 在字节流里验证段落语义：帧 0（Cover）左上角必须不透明，
        // 帧 150（Content 段，120..180 之间）左上角必须全透明。
        // 这条同时钉住「反预乘没有破坏 alpha」。
        let mut fs = FrameSource::new(
            SHORT_VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        let mut w = FirstPixelPicker {
            frame_bytes: 1280 * 720 * 4,
            seen: 0,
            picked: std::collections::HashMap::new(),
        };
        fs.write_rgba_frames(&mut w).unwrap();

        let cover = w.picked[&0];
        let content = w.picked[&150];
        assert_eq!(cover[3], 255, "Cover 帧左上角应不透明");
        assert!(
            cover[0] > 240,
            "Cover 帧左上角应接近白色，实得 {}",
            cover[0]
        );
        assert_eq!(content[3], 0, "Content 帧左上角应全透明");
    }

    /// 只保留指定帧的完整字节（一帧 3.5MB，可接受），其余帧丢弃。
    struct WholeFramePicker {
        frame_bytes: usize,
        target: usize,
        seen: usize,
        captured: Option<Vec<u8>>,
    }
    impl std::io::Write for WholeFramePicker {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            assert_eq!(b.len(), self.frame_bytes, "每帧应恰好一次 write_all 整帧");
            if self.seen == self.target {
                self.captured = Some(b.to_vec());
            }
            self.seen += 1;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 鉴别性测试：钉住「`write_rgba_frames` 里那次对 `unpremultiply_into` 的调用
    /// 没有被漏掉」这件事本身。上面 `write_rgba_frames_emits_opaque_cover_and_
    /// transparent_content` 只在左上角（帧 0 全不透明、帧 150 全透明）取样，这两个
    /// 位置的 alpha 恰好都在“预乘/straight 两种语义唯一重合”的地方（0 或 255），
    /// 所以就算 `write_rgba_frames` 里忘了调 `unpremultiply_into`、直接把 `Pixmap`
    /// 的预乘字节写出去，那条测试也发现不了——已实测确认（变异实验，写进任务
    /// 报告）。这里改为动态在 Content 帧里找一个半透明像素（水印图标的抗锯齿
    /// 边缘天然会有，不依赖具体坐标、不因排版微调而碎），直接对着同一张
    /// `render()` 出来的 `Pixmap`、用 `unpremultiply_into` 算出期望值，跟
    /// `write_rgba_frames` 实际写出的字节比对，这样才是真的在测「写出的字节确实
    /// 经过了反预乘」，而不是只测「alpha 没被反预乘弄坏」。
    #[test]
    fn write_rgba_frames_applies_unpremultiply_to_semi_transparent_pixels_too() {
        const CONTENT_FRAME: u32 = 150;

        // **必须显式配一个水印**：`SHORT_VTT` 只有一条零长度 cue，Content 段
        // 从头到尾没有任何字幕，水印是这条时间轴上唯一的墨迹来源。水印默认
        // 不画之后，不配就是一张全透明的帧，找不到半透明像素，这条测试也就
        // 无从测起。
        //
        // 只造一个 FrameSource（`Painter::new()` 约 100ms，两次全时间轴测试已经
        // 各付一次这个成本，这里复用同一个实例，避免再多付一次）：先直接渲染
        // 同一帧，找一个半透明像素（水印抗锯齿边缘），算出期望的 straight-alpha
        // 字节；`render()` 不带跨帧状态，之后接着跑 `write_rgba_frames` 不受影响
        // （`renders_the_whole_timeline_without_panicking` 等既有测试也是同一个
        // `FrameSource` 反复调 `render()`，顺序不敏感）。
        let branding = Branding {
            brand: "测试品牌".into(),
            watermark: Some("正文水印".into()),
            watermark_cover: None,
            watermark_icon: None,
            logo: None,
            font: None,
        };
        let mut fs = FrameSource::new(SHORT_VTT, "标题".into(), &branding, Canvas::BASE).unwrap();
        let pixmap = fs.render(CONTENT_FRAME).unwrap();
        let idx = pixmap
            .pixels()
            .iter()
            .position(|px| px.alpha() > 0 && px.alpha() < 255)
            .expect("Content 帧应存在半透明的抗锯齿边缘像素（水印文字边缘）");
        let want = pixmap.pixels()[idx].demultiply();
        let want_bytes = [want.red(), want.green(), want.blue(), want.alpha()];
        assert_ne!(
            want.red(),
            pixmap.pixels()[idx].red(),
            "前提检查：这个像素的反预乘结果应与预乘原值不同，否则这条测试测不出东西"
        );

        // 再走 write_rgba_frames，取同一帧的完整字节，比对同一个像素偏移。
        let mut w = WholeFramePicker {
            frame_bytes: 1280 * 720 * 4,
            target: CONTENT_FRAME as usize,
            seen: 0,
            captured: None,
        };
        fs.write_rgba_frames(&mut w).unwrap();
        let frame = w.captured.expect("目标帧应被捕获");
        let got_bytes = &frame[idx * 4..idx * 4 + 4];
        assert_eq!(
            got_bytes, want_bytes,
            "write_rgba_frames 写出的半透明像素字节应等于直接对 render() 结果做反预乘"
        );
    }

    #[test]
    fn write_rgba_frames_propagates_writer_errors_instead_of_panicking() {
        // ffmpeg 提前退出时写端会遇到 broken pipe，必须变成 Err 而不是 panic。
        struct FailAfter(usize);
        impl std::io::Write for FailAfter {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "管道已关闭",
                    ));
                }
                self.0 -= 1;
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut fs = FrameSource::new(
            VTT,
            "标题".into(),
            &Branding::plain("测试品牌"),
            Canvas::BASE,
        )
        .unwrap();
        let err = fs.write_rgba_frames(&mut FailAfter(3)).unwrap_err();
        assert!(
            format!("{err:#}").contains("管道"),
            "错误应透出底层原因：{err:#}"
        );
    }
}
