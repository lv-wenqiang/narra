# 开发

面向改这份代码的人。只想用它出片的话看 [README](../README.md) 就够了。

## 常用命令

```bash
cargo test                              # 全量，约 40s
cargo test --test render_e2e -- --ignored              # 端到端，需 ffmpeg 与 public/ 下的素材
cargo test --release --test render_throughput -- --ignored --nocapture  # 吞吐探针
cargo run --release --example text_probe               # 字体/排版探针，产出 text_probe*.png
cargo clippy --all-targets              # 应零告警
cargo fmt --check
just --list                             # 看有哪些配方
```

`cargo update` 之外的日常构建不需要网络。需要网络的测试（Edge TTS）与需要本地
素材的测试（`public/`）都已 `#[ignore]`，所以 `cargo test` 在任何机器上都能跑。

## 源码结构

```
src/
  main.rs            CLI 装配：参数解析、三级兜底、合成路径的胶水
  config.rs          环境变量层、Branding / SfxSources 的三级兜底
  tmp.rs             临时路径的作用域守卫（Drop 里删）
  vtt.rs             切句与 WebVTT 生成/解析
  duration.rs        mp3 时长探测
  ffmpeg.rs          滤镜图与命令行构造、帧流管道、进程编排
  assets.rs          内嵌资源（字体 / logo / 音效）与外部图标加载
  tts/
    edge.rs          Edge TTS 的 WebSocket 协议
    pipeline.rs      切句 → 并发合成 → 合并加速 → 写产物
  render/
    stream.rs        帧流的两段流水线（渲染线程 / 写出线程 + 有界通道）
    timeline.rs      四段时间轴与段内帧换算
    anim.rs          插值与弹簧曲线
    text.rs          文字排版与绘制
    draw.rs          四段画面的绘制
    draw_tests.rs    draw.rs 的测试（`#[path]` 外置）
    frame.rs         逐帧渲染与 RGBA 帧流写出
```

## 这个仓库的测试风格

测试不只求覆盖率，更求**鉴别力**——写完一条测试之后，会拿变异（把生产代码改
错一处）去验证它真的会变红。`follow-ups.md` 里有若干条记的正是「某测试经 N 次
变异零响应」，那种测试会被改写或删掉，因为它占测试计数、给虚假信心。

几类反复出现的写法：

- **配对断言**优先于逐项断言。检查「`volume=0.15` 在字符串某处出现」抓不住
  「TTS 与 BGM 的处理链被对调」；断言整条链作为连续子串才能。
- **跨模块一致性测试**。同一个事实在两处各写一遍时（`timeline` 的段落帧数 vs
  滤镜图的 `adelay` 毫秒；`config` 的默认值 vs `justfile` 里镜像的那份），
  补一条把两边钉在一起的测试，改一边不改另一边就变红。
- **清理挂在作用域上**，不写在函数尾部。`src/tmp.rs` 的 `TempPath` 在 `Drop`
  里删路径——尾部清理在 panic 展开时不可达，而「闭包 panic」恰恰是那些测试
  最想抓的回归，也就是说越是抓到 bug 越会留下垃圾文件。
- **存活的变异要记下来**，不要假装没发生。例如 `stream.rs` 里「写端失败就停」
  那一行，去掉它测试照样全绿（生产端会在下一轮取缓冲区时停下），这一点写在
  代码注释与欠账本里，而不是留一条看起来在保护它、实则不保护的测试。

## 画面构成

四段画面，总时长随旁白长度伸缩（`A` = 字幕的最末结束时间）：

| 段落 | 时长 | 内容 |
|---|---|---|
| Cover | 15 帧（0.5s） | 白底，logo + 品牌名上排，居中主标题 |
| Intro | 105 帧（3.5s） | 白底，标题逐字打出 + 闪烁光标，末尾淡出 |
| Content | `ceil((A+2)×30)` 帧 | **透明底**，仅字幕，带入场动画（弹簧曲线：缩放 / 透明度 / 位移 / 字距） |
| Outro | 120 帧（4s） | 白底，同心圆环 + logo 缩放入场 + 品牌名，整体淡出 |

Content 段透明是有意的——只有它需要透出背景视频，四段共用一条覆盖层流，
ffmpeg 侧一次 `overlay` 就够。文字排版用 `cosmic-text` + `tiny-skia` 自绘：
合成粗体、描边、字距、按字符数切换字号（>50 字用 52px，否则 80px）。

两档画幅共用同一套版式：尺寸类的量按「画布宽度 / 1280」统一缩放，所以每个元素
占画布**宽度**的比例在两档之间保持一致。竖版画布只有 1080 宽，元素因而画得更
小、上下留白更明显——这是「共用一套版式」换来的既定代价，不是缺陷。

## 合成链路

- **视频**：背景视频循环播放 → 缩放裁切到目标画幅 → 压暗到 80% →
  `overlay` 覆盖层 → H.264
- **音频**：TTS 旁白、背景音乐（0.15 音量，末尾 2 秒线性淡出）、打字机音效、
  片尾音效，四路 `amix`
- 四路进 `amix` 前统一到 48kHz 立体声。**每一路按自己的实际声道数选上混写法**
  （`ffprobe` 逐路探测）：单声道走单位增益的 `pan`，立体声走 `aformat`——反过来
  用会让单声道轻 3 dB、或让立体声丢掉右声道（`ffmpeg-pipeline.md` §14）
- 渲染与写出跑在两个线程上，中间挂一个容量 2 的有界通道
  （`ffmpeg-pipeline.md` §12）

写帧失败、ffmpeg 非零退出时会删掉可能已落盘的半成品，不留一支「能播放、时长
正确、实则大半静止」的坏片。

## 发布

推 `v*` 标签由 `.github/workflows/publish.yml` 完成：装 ffmpeg → fmt → clippy
（`-D warnings`）→ 全量测试 → `cargo publish --dry-run` → 校验标签与
`Cargo.toml` 版本一致 → 发布。手动触发（workflow_dispatch）会跑完同样的检查但
**止步于 dry-run**，用来在打标签之前先验证 CI 本身是通的。

**不要从本机 `cargo publish`**：2026-09-05 实测，本机到 crates.io 的上传链路撑
不住 8.4 MiB 的请求体（Fastly 边缘 503 / TLS 中断，四次全败），而同一个包在 CI
上不到 1 秒就传完了。
