# narra

[![crates.io](https://img.shields.io/crates/v/narra.svg)](https://crates.io/crates/narra)
[![license](https://img.shields.io/crates/l/narra.svg)](LICENSE)

口播文稿 → 语音 → 成片的命令行工具。一个 Rust 二进制，外部只依赖 `ffmpeg`。

给一份文稿，它产出一支 1920×1080 / 30fps 的 mp4（`--orientation portrait` 可换成
1080×1920）：Edge TTS 合成旁白与字幕，自绘封面、打字机片头、字幕、片尾四段画面，
叠在循环播放的背景视频上，混进背景音乐与两段音效。

个人自用工具，**没有稳定性承诺**——命令行参数与环境变量名可能随需要调整。

## 安装

```bash
cargo install narra
```

需要 Rust 1.85+（edition 2024）。另外两件事必须自己准备：

- **`ffmpeg` 与 `ffprobe` 在 `PATH` 上**——合成与音频处理全靠它们，`cargo install`
  不会替你装。
- **背景视频与背景音乐**，默认放在 `public/video/0.mp4` 与 `public/bgm/0.mp3`
  （`--bg` / `--bgm` 可改）。仓库不携带素材。

字体、logo、两段音效已内嵌进二进制，不必额外准备。只有 `narra tts` 需要联网
（连 Edge TTS 服务）。可选装 [`just`](https://github.com/casey/just) 来跑一条龙配方。

## 快速开始

```bash
# 一条龙：文稿 → TTS → 成片
just make 文稿.txt

# 一份文稿出两档成片（横版 + 竖版），复用同一次 TTS
just make-both 文稿.txt

# 或者分两步，中间可以先检查 TTS 的产物
narra tts 文稿.txt
narra render --audio output/tts/audio.mp3 --vtt output/tts/audio.vtt -o output/video/video.mp4
```

`just make` 与手动两步的区别只有一个：它在跑 TTS **之前**先检查背景素材是否存在。
`--bg` 打错一个字，否则要先付一整轮 Edge TTS 网络往返才报错。

`just make-both` 把同一次 TTS 的产物渲染两遍——TTS 是整条链上唯一要联网、唯一耗时
以分钟计的一步，两档成片没有理由各付一次。它自己会给两条腿分别加 `--orientation`，
所以透传给它的额外参数里**不要再写 `--orientation`**（重复 flag 会让 clap 报错）。

## 命令

```
narra tts [INPUT] [OUTDIR] [--voice <name>] [--batch-size <n>]
narra render --audio <mp3> --vtt <vtt> [-o <out.mp4>] [素材/品牌选项...]
narra debug-frames --vtt <vtt> -o <dir> [--frames 0,60,150] [品牌选项...]
```

- **`tts`**：按标点切句后并发调用 Edge TTS，合并为单条 mp3 并加速到 1.1×，同时按
  各段实际时长生成对齐的 WebVTT。产出 `audio.mp3` 与 `audio.vtt`。默认音色
  `zh-CN-YunjianNeural`、并发 3（上限 8）；单段失败自动重试，整轮失败时清理中间
  文件、不留半成品。
- **`render`**：把自绘的帧流经管道喂给 ffmpeg，与背景视频、四路音频一次合成。
  写帧失败或 ffmpeg 非零退出时会删掉半成品，不留一支「能播放但大半静止」的坏片。
- **`debug-frames`**：把指定帧渲染成 PNG，用来在合成之前核对视觉；不给 `--frames`
  则每 30 帧导一张。`--vtt` 读的是任意 WebVTT（小时位可省），读不懂的 cue 会打印
  警告并带上被丢掉的正文，不静默跳过。

画面构成、合成链路与版式换算见 [`docs/development.md`](docs/development.md)。

## 配置

一律遵循 `--flag` > 环境变量 > 默认值 三级兜底，每一级都要求非空白：

| 配置 | 环境变量 | 默认 |
|---|---|---|
| `--orientation` | `ORIENTATION` | `landscape`（1920×1080） |
| `--brand` | `BRAND` | `墨` |
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

TTS 侧另有 `EDGE_TTS_VOICE`、`EDGE_TTS_BATCH_SIZE`、`EDGE_TTS_TIMEOUT_MS`、
`TTS_INPUT_FILE`、`TTS_OUTPUT_DIR`、`SPIDER_OUTPUT_DIR`。

几点要紧的：

- 品牌名画在 Cover 上排与 Outro 大字上，同时是标题的最后一级兜底（`--title` >
  `title.json` 的 `title` 字段 > 品牌名）。默认「墨」——**用之前记得改成你自己的**，
  否则片子上印的是这个默认值。
- 两处水印相互独立，各自为空时各自不画；图标与 logo 支持 `.svg`（矢量渲染）与
  `.png`（Lanczos3 缩放），原样保留自身颜色。
- `--font` 支持 `.ttf` / `.otf`。**文件不可用时打印警告并回退到内嵌字体，不中断
  出片**。但渲染器刻意禁用了系统字体回退，所以指定一份不覆盖中文的字体会让字幕
  变成豆腐块，而**工具不检测这件事**（见欠账本）。

## 状态

**可用**。四条链路（TTS、帧渲染、ffmpeg 合成、CLI）都跑通了真实素材的端到端验收，
293 个测试通过（另有 9 个 `#[ignore]` 的重量级用例与吞吐探针）。

默认档实测吞吐约 66fps，即 30fps 实时线的 **2.2×**（竖版略低）。这个余量是分摊
测量之后一步步拿到的：瓶颈不是编码，而是①渲染与 ffmpeg 两段没有重叠、②文字绘制
每次都为整幅画布付一趟清空 + 一趟合成，两条各自修掉后端到端累计少了 44%。

已知限制与待办记在 [`docs/follow-ups.md`](docs/follow-ups.md)，每条都注明了
「值得做 / 可以不做 / 只是记一笔」以及推迟的理由。

## 素材与授权

**本项目的代码以 MIT 授权**（见 [`LICENSE`](LICENSE)）。内嵌的三项素材随二进制
分发，各自保留自己的授权，不因本项目是 MIT 而改变：

| | 来源 | 授权 |
|---|---|---|
| 字体 | 霞鹜文楷 Lite | SIL OFL 1.1 |
| Logo | Lucide `feather` | ISC |
| 两段音效 | Freesound #455044 / #406243 | CC0 |

**如果你打算发布成片，请自行确认所用素材的授权。** 背景视频、背景音乐都有各自的
使用条件，这个工具不对素材来源做任何检查或担保。逐项授权状态、免费可商用的中文
字体与视频/音乐素材站清单，以及一个容易被忽略的坑（授权合规不等于不被平台判定
侵权），见 [`docs/assets-and-licensing.md`](docs/assets-and-licensing.md)。

## 文档

代码里的关键取舍都有对应的实测记录，改到相关部分前值得先读：

| 文档 | 内容 |
|---|---|
| [`docs/development.md`](docs/development.md) | 开发命令、源码结构、测试风格、画面构成与合成链路、发布流程 |
| [`docs/ffmpeg-pipeline.md`](docs/ffmpeg-pipeline.md) | 完整命令行、alpha 语义验证、AV1 解码开销、进程编排的死锁分析、混音电平复测、吞吐分摊与两轮性能改造、单声道素材的上混衰减 |
| [`docs/text-rendering.md`](docs/text-rendering.md) | 字体与排版的实测结论 |
| [`docs/edge-protocol.md`](docs/edge-protocol.md) | Edge TTS 的 WebSocket 协议细节 |
| [`docs/assets-and-licensing.md`](docs/assets-and-licensing.md) | 素材来源与授权、免费可商用素材清单 |
| [`docs/follow-ups.md`](docs/follow-ups.md) | 欠账本：已知限制、待办、以及每条推迟的理由 |
| [`docs/superpowers/`](docs/superpowers/) | 设计规格与各子系统的实施计划 |
