# panda

口播文稿 → 语音 → 成片的命令行工具。一个 Rust 二进制，外部只依赖 `ffmpeg`。

给一份文稿，它产出一支 1920×1080 / 30fps 的 mp4（`--orientation portrait`
可换成 1080×1920）：Edge TTS 合成旁白与字幕，
自绘封面、打字机片头、字幕、片尾四段画面，叠在循环播放的背景视频上，
混进背景音乐与两段音效。

---

## 状态

**可用**。四条链路（TTS、帧渲染、ffmpeg 合成、CLI）都已跑通真实素材的端到端
验收，293 个测试通过（另有 8 个 `#[ignore]` 的重量级用例与吞吐探针，需要 ffmpeg
与本地素材，手动跑）。

默认档（`landscape`，1920×1080）实测吞吐约 66fps，即 30fps 实时线的 **2.2×**
（竖版略低）。这个余量是分摊测量之后一步步拿到的：瓶颈**不是编码**（`-preset`
三档实测无差别），而是①渲染与 ffmpeg 两段没有重叠、②文字绘制每次都为整幅画布
付一趟清空 + 一趟合成。两条各自修掉之后端到端累计少了 44%（21.3s → 12.0s）。
全过程见 [`docs/ffmpeg-pipeline.md`](docs/ffmpeg-pipeline.md) §10~§13。

个人自用工具，没有稳定性承诺——命令行参数与环境变量名可能随需要调整。
已知的限制与待办记在 [`docs/follow-ups.md`](docs/follow-ups.md)，每条都注明了
「值得做 / 可以不做 / 只是记一笔」以及推迟的理由。

## 环境要求

| | |
|---|---|
| Rust | edition 2024（需较新的稳定版工具链） |
| `ffmpeg` / `ffprobe` | 必须在 `PATH` 上，合成与音频处理全靠它 |
| 网络 | 仅 `panda tts` 需要（连 Edge TTS 服务） |
| [`just`](https://github.com/casey/just) | 可选，只用来跑「一条龙」配方 |

字体、logo、两段音效已内嵌进二进制，不必额外准备，且**来源与授权都已记录**
（字体：霞鹜文楷 Lite / SIL OFL 1.1；logo：Lucide feather / ISC；两段音效：
Freesound CC0）。背景视频与背景音乐要
自备，默认放在 `public/video/0.mp4` 与 `public/bgm/0.mp3`（这两个路径在
`.gitignore` 里，仓库不携带素材）。

## 快速开始

```bash
# 一条龙：文稿 → TTS → 成片
just make 文稿.txt

# 一份文稿出两档成片（横版 + 竖版），复用同一次 TTS
just make-both 文稿.txt

# 或者分两步，中间可以先检查 TTS 的产物
cargo run --release -- tts 文稿.txt
cargo run --release -- render \
    --audio output/tts/audio.mp3 \
    --vtt   output/tts/audio.vtt \
    -o      output/video/video.mp4
```

`just make` 与手动两步的区别只有一个：它在跑 TTS **之前**先检查背景素材是否
存在。`--bg` 打错一个字，否则要先付一整轮 Edge TTS 网络往返才报错。

`just make-both` 把同一次 TTS 的产物渲染两遍，得到
`output/video/landscape.mp4`（1920×1080）与 `output/video/portrait.mp4`
（1080×1920）——TTS 是整条链上唯一要联网、唯一耗时以分钟计的一步，两档成片
没有理由各付一次。它自己会给两条腿分别加上 `--orientation`，所以透传给它的
额外参数里**不要再写 `--orientation`**（会变成重复 flag，clap 直接报错）。

---

## 功能

### 语音与字幕（`panda tts`）

```
panda tts [INPUT] [OUTDIR] [--voice <name>] [--batch-size <n>]
```

按标点切句后并发调用 Edge TTS，合并为单条 mp3 并加速到 1.1×，同时按各段实际
时长生成对齐的 WebVTT 字幕。产出 `audio.mp3` 与 `audio.vtt`。

- 默认音色 `zh-CN-YunjianNeural`，默认并发 3（上限 8）
- 单段失败自动重试；整轮失败时清理中间文件，不留半成品
- 文稿为空、或解析不出任何内容时直接报错

### 帧渲染（`panda debug-frames`）

```
panda debug-frames --vtt <vtt> -o <dir> [--frames 0,60,150] [品牌选项...]
```

把指定帧渲染成 PNG，用来在合成之前核对视觉。不给 `--frames` 则每 30 帧导一张。

`--vtt` 读的是任意 WebVTT：小时位可省（`MM:SS.mmm` 与 `HH:MM:SS.mmm` 都认），
读不懂的 cue 会**打印警告并带上被丢掉的正文**，不静默跳过。

画面分四段，总时长随旁白长度伸缩：

| 段落 | 时长 | 内容 |
|---|---|---|
| Cover | 15 帧（0.5s） | 白底，logo + 品牌名上排，居中主标题 |
| Intro | 105 帧（3.5s） | 白底，标题逐字打出 + 闪烁光标，末尾淡出 |
| Content | `ceil((A+2)×30)` 帧 | **透明底**，仅字幕，带入场动画（弹簧曲线：缩放 / 透明度 / 位移 / 字距） |
| Outro | 120 帧（4s） | 白底，同心圆环 + logo 缩放入场 + 品牌名，整体淡出 |

`A` 是字幕的最末结束时间。Content 段透明是有意的——只有它需要透出背景视频，
四段共用一条覆盖层流，ffmpeg 侧一次 `overlay` 就够。

文字排版用 `cosmic-text` + `tiny-skia` 自绘：合成粗体、描边、字距、按字符数
切换字号（>50 字用 52px，否则 80px）。

### 成片合成（`panda render`）

```
panda render --audio <mp3> --vtt <vtt> [-o <out.mp4>] [素材/品牌选项...]
```

把帧流经管道喂给 ffmpeg，与背景视频、四路音频一次合成：

- **视频**：背景视频循环播放 → 缩放裁切到目标画幅（默认 1920×1080，`--orientation
  portrait` 出 1080×1920）→ 压暗到 80% → `overlay` 覆盖层 → H.264
- **音频**：TTS 旁白、背景音乐（0.15 音量，末尾 2 秒线性淡出）、打字机音效、片尾音效，四路 `amix`
- 四路进 `amix` 前统一到 48kHz 立体声。**每一路按自己的实际声道数选上混写法**
  （`ffprobe` 逐路探测）：单声道走单位增益的 `pan`，立体声走 `aformat`——反过来
  用会让单声道轻 3 dB、或让立体声丢掉右声道

两档画幅共用同一套版式：尺寸类的量按「画布宽度 / 1280」统一缩放，所以每个
元素占画布**宽度**的比例在两档之间保持一致。竖版画布只有 1080 宽（比旧的
1280 更窄），元素因而画得更小，且上下会留出更明显的空白——这是「共用一套
版式」换来的既定代价，不是缺陷。

写帧失败、ffmpeg 非零退出时会删掉可能已落盘的半成品，不留一支「能播放、时长
正确、实则大半静止」的坏片。

### 品牌与素材

全部可配置，一律遵循 `--flag` > 环境变量 > 默认值 三级兜底，每一级都要求非空白：

| 配置 | 环境变量 | 默认 |
|---|---|---|
| `--orientation` | `ORIENTATION` | `landscape`（1920×1080） |
| `--brand` | `BRAND` | `墨风` |
| `--watermark` | `WATERMARK` | **不画** |
| `--watermark-cover` | `WATERMARK_COVER` | **不画** |
| `--watermark-icon` | `WATERMARK_ICON` | **不画** |
| `--logo` | `LOGO_FILE` | 内嵌的那张 |
| `--font` | `FONT_FILE` | 内嵌的霞鹜文楷 |
| `--sfx-intro` | `SFX_INTRO` | 内嵌的那段 |
| `--sfx-typewriter` | `SFX_TYPEWRITER` | 内嵌的那段 |
| `--bg` | `BG_VIDEO` | `public/video/0.mp4` |
| `--bgm` | `BGM_FILE` | `public/bgm/0.mp3` |
| `--title-json` | `TITLE_JSON` | `public/video/title.json` |
| `-o` | `VIDEO_OUTPUT` | `output/video/video.mp4` |

品牌名画在 Cover 上排与 Outro 大字上，同时是标题的最后一级兜底
（`--title` > `title.json` 的 `title` 字段 > 品牌名）。

两处水印相互独立，各自为空时各自不画；图标与 logo 支持 `.svg`（矢量渲染）
与 `.png`（Lanczos3 缩放），原样保留自身颜色，只施加水印预设的不透明度。

`--font` 支持 `.ttf` 与 `.otf`，四段画面共用同一份。**文件不可用时打印警告并
回退到内嵌字体，不中断出片**——警告会写明失败原因和实际生效的字体。注意渲染器
刻意禁用了系统字体回退（见下），所以指定一份不覆盖中文的字体会让字幕变成豆腐块，
工具不对此做检测。

TTS 侧另有 `EDGE_TTS_VOICE`、`EDGE_TTS_BATCH_SIZE`、`EDGE_TTS_TIMEOUT_MS`、
`TTS_INPUT_FILE`、`TTS_OUTPUT_DIR`、`SPIDER_OUTPUT_DIR`。

---

## 开发

```bash
cargo test                              # 全量，约 40s
cargo test --test render_e2e -- --ignored   # 端到端，需 ffmpeg 与 public/ 下的素材
cargo test --release --test render_throughput -- --ignored --nocapture  # 渲染侧吞吐探针
cargo clippy --all-targets              # 应零告警
cargo fmt --check
just --list                             # 看有哪些配方
```

`cargo update` 之外的日常构建不需要网络。

### 源码结构

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

### 这个仓库的测试风格

测试不只求覆盖率，更求**鉴别力**——写完一条测试之后，会拿变异（把生产代码改
错一处）去验证它真的会变红。`docs/follow-ups.md` 里有若干条记的正是「某测试经
N 次变异零响应」，那种测试会被改写或删掉，因为它占测试计数、给虚假信心。

几类反复出现的写法：

- **配对断言**优先于逐项断言。检查「`volume=0.15` 在字符串某处出现」抓不住
  「TTS 与 BGM 的处理链被对调」；断言整条链作为连续子串才能。
- **跨模块一致性测试**。同一个事实在两处各写一遍时（`timeline` 的段落帧数 vs
  滤镜图的 `adelay` 毫秒；`config` 的默认值 vs `justfile` 里镜像的那份），
  补一条把两边钉在一起的测试，改一边不改另一边就变红。
- **清理挂在作用域上**，不写在函数尾部。`src/tmp.rs` 的 `TempPath` 在 `Drop`
  里删路径——尾部清理在 panic 展开时不可达，而「闭包 panic」恰恰是那些测试
  最想抓的回归，也就是说越是抓到 bug 越会留下垃圾文件。

### 深入的实测记录

代码里的关键取舍都有对应的实测文档，改到相关部分前值得先读：

| 文档 | 内容 |
|---|---|
| [`docs/ffmpeg-pipeline.md`](docs/ffmpeg-pipeline.md) | 完整命令行、alpha 语义验证、AV1 解码开销、进程编排的死锁分析、混音格式与分窗电平复测、素材循环点实测、渲染/编码的吞吐分摊 |
| [`docs/text-rendering.md`](docs/text-rendering.md) | 字体与排版的实测结论 |
| [`docs/edge-protocol.md`](docs/edge-protocol.md) | Edge TTS 的 WebSocket 协议细节 |
| [`docs/assets-and-licensing.md`](docs/assets-and-licensing.md) | 素材来源与授权：自带素材的授权状态、免费可商用的字体/视频/音乐站清单及其条件 |
| [`docs/follow-ups.md`](docs/follow-ups.md) | 欠账本：已知限制、待办、以及每条推迟的理由 |
| [`docs/superpowers/`](docs/superpowers/) | 设计规格与各子系统的实施计划 |

## 素材与授权

仓库不携带背景视频与背景音乐（`public/` 在 `.gitignore` 里）。内嵌的字体、logo
与两段音效随二进制分发，三者的来源与授权都已记录：

| | 来源 | 授权 |
|---|---|---|
| 字体 | 霞鹜文楷 Lite | SIL OFL 1.1 |
| Logo | Lucide `feather` | ISC |
| 两段音效 | Freesound #455044 / #406243 | CC0（公有领域，无需署名） |

字体与 logo 的许可证原文按各自要求附在 `assets/LICENSE-LXGWWenKai.txt` 与
`assets/LICENSE-lucide.txt`——**打包时别把它们漏掉**。

**如果你打算发布成片，请自行确认所用素材的授权。** 背景视频、背景音乐都有各自的
使用条件，这个工具不对素材来源做任何检查或担保。

[`docs/assets-and-licensing.md`](docs/assets-and-licensing.md) 里整理了：本项目
自带素材的逐项授权状态与复现方式、免费可商用的中文字体清单、
视频与音乐素材站及其授权条件，以及一个容易被忽略的坑——**授权合规不等于不被平台
判定侵权**。
