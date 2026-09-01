# panda-video-rs 设计文档

日期：2026-09-01
状态：待评审

## 1. 背景与动机

现有 `panda-video-ts` 的口播视频链路依赖 Node 20 + pnpm + 大量 npm 包，其中 Remotion 渲染还会拉起 headless Chrome。对于个人自用场景，这条工具链的安装和维护成本远高于它产出的价值。

本项目把其中**「文稿 → 语音 → 视频」**这一段用 Rust 重写为单个可执行文件，除系统已安装的 ffmpeg 外零外部依赖。

使用场景限定为**个人自用，不分发、不商用**。因此 Edge TTS 的非公开接口、ffmpeg 的 GPL 传染性等授权问题不在考量范围内。

## 2. 目标

- 单个可执行文件完成：`input.txt` + 标题 → `video.mp4`
- 不依赖 Node、npm、浏览器
- 输出与现有 Remotion 成片在视觉与时间轴上高度一致
- 首版目标平台：**Linux（WSL2）**

## 3. 非目标

| 不做 | 原因 | 继续用什么 |
|---|---|---|
| 网页抓取 | 知乎反爬需要 stealth 浏览器 | `packages/spider`（puppeteer） |
| 多平台发布 | 上传必须真实浏览器 | `pva`（playwright） |
| LLM 文稿改写 | 实际流程中由 AI Agent 直接出稿，包内 LLM 仅为兜底 | 低优先级，见 §11 |
| Windows exe | 首版先跑通 Linux | 交叉编译后续单独决策，见 §11 |
| 竖版（1080×1920）合成 | 首版只做横版 | 见 §11 |

交接边界是文件系统，与现状完全一致：上游产出 `input.txt` 和 `title.json`，本工具消费它们。

## 4. 总体架构

```
input.txt ──► [TTS 子系统] ──► audio.mp3 + audio.vtt
                                      │
title.json ───────────────────────────┤
                                      ▼
                             [渲染子系统]
                                      │
                    逐帧 RGBA 覆盖层（stdin 管道）
                                      │
                                      ▼
  bg.mp4 / bgm.mp3 / 内嵌音效 ──► [ffmpeg] ──► video.mp4
```

核心决策：**Rust 不碰视频编解码**。它只负责在透明画布上光栅化文字与图形，把 RGBA 帧流通过管道交给 ffmpeg；背景视频解码、叠加、混音、H.264 编码全部由 ffmpeg 完成。

这个划分的理由：
- 视频编解码是 ffmpeg 的强项，用户机器上已有
- 2D 光栅化是 `tiny-skia` 的强项，且能 100% 还原现有动画（动画曲线都是纯数学）
- 避免静态链接 ffmpeg 带来的跨平台构建复杂度

## 5. 模块划分

```
panda-video-rs/
  Cargo.toml
  assets/                      内嵌资源（从 panda-video-ts/public 复制）
    dingliesongtypeface.ttf
    logo.png
    intro.mp3
    intro_typewriter.mp3
  src/
    main.rs                    CLI 入口（clap）
    config.rs                  参数 + 环境变量解析
    assets.rs                  include_bytes! 内嵌资源
    tts/
      backend.rs               trait TtsBackend
      edge.rs                  Edge read-aloud WebSocket 客户端
      pipeline.rs              分段 → 并发合成 → 合并加速
    vtt.rs                     切句 / 生成 / 解析
    render/
      timeline.rs              时长与序列布局
      anim.rs                  spring / interpolate
      text.rs                  cosmic-text 排版 + 描边
      draw.rs                  封面 / 片头 / 字幕 / 片尾 / 水印
      frames.rs                逐帧生成 → 写管道
    ffmpeg.rs                  命令构造 + 进程管理
```

每个模块的可测性：`vtt`、`anim`、`timeline` 是纯函数（可单测）；`tts` 只依赖网络；`render::draw` 输出位图（可抽帧核对）；`ffmpeg` 只构造参数（可断言命令行）。

## 6. CLI 契约

```
panda tts    [INPUT] [OUTDIR]
             --voice <name>  --batch-size <n>

panda render --audio <mp3> --vtt <vtt> [--title <s>] [--title-json <path>]
             --bg <mp4> --bgm <mp3> -o <out.mp4>

panda make   [INPUT] [--title <s>] [--bg <mp4>] [--bgm <mp3>] -o <out.mp4>
```

沿用现有环境变量作为默认值兜底，便于与现有 pnpm 脚本互换：

| 变量 | 默认 |
|---|---|
| `TTS_INPUT_FILE` | `$SPIDER_OUTPUT_DIR/input.txt` |
| `TTS_OUTPUT_DIR` | `output/tts` |
| `SPIDER_OUTPUT_DIR` | `output/spider` |
| `EDGE_TTS_VOICE` | `zh-CN-YunjianNeural` |
| `EDGE_TTS_BATCH_SIZE` | `3`（上限 8） |
| `EDGE_TTS_TIMEOUT_MS` | `120000`（下限 15000） |

素材与产物的默认路径：

| 项 | 默认 | 覆盖方式 |
|---|---|---|
| 背景视频 | `public/video/0.mp4` | `--bg` |
| BGM | `public/bgm/0.mp3` | `--bgm` |
| 标题 JSON | `public/video/title.json` | `--title-json` |
| 成片输出 | `output/video/video.mp4` | `-o` |

背景视频与 BGM **保持为外部文件**（不内嵌），因为现有流程本来就在用 `shuffle:bg-video` / `shuffle:bgm` 替换它们。

标题三级兜底：`--title` > `--title-json` 指向文件的 `title` 字段 > `熊猫智研社`。

## 7. TTS 子系统

### 7.1 流程

照搬 `packages/tts-node/src/process.ts` 的语义：

1. 读文稿，**非空行 = 一段**；无非空行则报错退出
2. `tokio::sync::Semaphore` 限制并发（默认 3），每段独立合成为 `sentence{i}.mp3`
3. 每段失败重试 3 次，退避 `(attempt+1) * 2000ms`
4. `symphonia` 读每段真实时长
5. ffmpeg `concat` demuxer 合并 + `atempo=1.1` 加速 → `audio.mp3`
6. 每段时长除以 1.1 得到加速后时长，生成 `audio.vtt`
7. 删除 `sentence*.mp3`

### 7.2 后端抽象

```rust
trait TtsBackend {
    async fn synth(&self, text: &str) -> Result<Synthesized>;
}

struct Synthesized {
    audio: Vec<u8>,
    /// 词级时间戳，Edge 的 WordBoundary 事件可提供；首版不使用
    timings: Option<Vec<WordTiming>>,
}
```

首版只实现 `EdgeBackend`。留 trait 是为了后续接 Azure（有官方 REST API）或本地模型，而不是为了首版多态。

### 7.3 字幕时间轴

首版沿用现有的估算方式：按字符数比例把段时长摊给切出来的字幕片段。

```
segment_duration = paragraph_duration * segment_chars / paragraph_chars
```

理由是这套算法简单、无额外依赖，且在现有成片里表现可接受。WordBoundary 词级精确对齐作为后续增强项，首版不引入。

### 7.4 切句规则（移植自 `vtt.ts`）

`splitTextForVtt(text, max_length = 30)`：

1. 文本长度 ≤ 30 → 整段作为一个片段
2. 否则在前 30 个字符内**从后往前**找 `。！？`，找到则在其后切分
3. 找不到则从第 30 个字符向后找，先在 `[30, 60)` 范围找，再在剩余全文找
4. 全文都没有句末标点 → 整段不切

**移植注意**：TS 用的是 UTF-16 code unit 计数（`String.prototype.length`）。中文字符在 BMP 内两者一致，但 emoji 会产生差异。Rust 侧用 `chars().count()`，并在测试中显式覆盖含 emoji 的用例，记录已知差异。

### 7.5 时间格式

`HH:MM:SS.mmm`。TS 实现是 `secs.toFixed(3).split('.')` 后 `slice(0,3).padEnd(3,'0')`——由于 `toFixed(3)` 恒定产出 3 位小数，后两步是恒等操作，因此实际语义是**四舍五入到 3 位小数**，Rust 用 `format!("{:.3}", secs)` 等价。

TS 原版的时、分由 `Math.floor` 独立计算，秒由 `seconds % 60` 得出，**在秒数进位时会产生非法时间戳**：`59.9996` 渲染成 `00:00:60.000`。本项目**不复刻该缺陷**——先把秒四舍五入到毫秒，再由归一化后的总毫秒数推导时、分、秒，保证输出恒为合法 WebVTT 时间戳。

## 8. 渲染子系统

### 8.1 画布参数

`1280 × 720`，`30 fps`，H.264，`crf 23`。

### 8.2 时间轴布局

设 `A` = 音频时长（VTT 最后一条字幕的结束时间，秒）：

| 段 | 起始帧 | 时长（帧） |
|---|---|---|
| Cover | 0 | `ceil(0.5 * 30)` = 15 |
| Intro | 15 | `ceil(3.5 * 30)` = 105 |
| Content | 120 | `ceil((A + 2) * 30)` |
| Outro | `120 + content_frames` | `ceil(4 * 30)` = 120 |

总帧数 `= 240 + ceil((A + 2) * 30)`。

各段内部的帧计数**从该段起点重新从 0 开始**（对应 Remotion 的 `Sequence` 相对帧语义）。

### 8.3 覆盖层策略

Rust 每帧输出一张 `1280×720` RGBA 位图：

- **Cover / Intro / Outro**：不透明白底铺满（alpha = 255），直接盖住背景视频
- **Content**：完全透明底，只画字幕和水印

这样单条覆盖层流即可表达全部视觉，ffmpeg 侧只需一次 `overlay`。

### 8.4 各段视觉规格

**Cover（0.5s）**
- 白底 `#FFFFFF`
- 居中容器：画面正中，宽度 80%，内容居中对齐
  - **上排**（整体 `opacity 0.30`，左偏移 40px，水平排列、垂直居中）：
    - `logo.png`，36px（`min(1280,720) * 0.1 / 2`），四周 margin 8px
    - 紧邻的「熊猫智研社」：38px 粗体，`dingliesongtypeface`，行高 1.2
  - **主标题**：100px 粗体，`dingliesongtypeface`，左右 padding 40px，行高 1.2，支持换行（`pre-line` + 长词断行）
- 水印（`cover` 预设）

**Intro（3.5s）— 打字机**
- 白底
- 标题字号 70px，粗体，`dingliesongtypeface`，居中，宽度 80%，左右 padding 40px，支持换行
- 打字机：2 秒内打完全部字符
  - `chars_per_sec = title.chars().count() / 2`
  - `visible = min(floor(frame * chars_per_sec / fps), len)`
- 光标 `|`：打字未完成时显示，2 次/秒闪烁，左边距 4px
  - `opacity = interpolate(frame % (fps/2), [0, fps/4, fps/2], [1, 1, 0], clamp)`
- 3.0s → 3.5s 线性淡出：`interpolate(frame, [90, 104], [1, 0], clamp)`
- 无水印

**Content（A + 2s）— 字幕**
- 透明底
- 当前字幕 = VTT 中满足 `start <= t < end` 的第一条
- 字号：去除所有空白后字符数 > 50 → 52px，否则 80px
- 粗体，`dingliesongtypeface`，居中于画面中心，宽度 80%，padding 20px/40px，保留换行
- **描边**：先用 6px 黑色 `#000000` 描边（居中描边，即向外 3px），再填充白色 `#FFFFFF`
- 入场动画，持续 `min(500ms, 字幕时长 * 0.3)`：
  - `p = spring(frame, fps, damping = 200, duration_frames)`
  - `scale`：`1.2 → 1.0`
  - `opacity`：`0 → 1`
  - `translate_x`：`100 → 0`（像素）
  - `letter_spacing`：`8 → 0`（像素）
- 左下角水印（`content` 预设）

**Outro（4s）**
- 白底（由 Content 之后的段落提供）
- 同心圆环：5 个白色实心圆，半径 `720 * 0.3 * i`（i = 0..4，倒序绘制），整体 `scale = 1 / (1 - out_progress)`
  - `out_progress = spring(frame, fps, damping = 200, duration = 0.5s, delay = 1s)`
  - **边界**：`out_progress → 1` 时 scale 发散，需钳制上限避免数值溢出
- Logo：`logo.png`，尺寸 `min(1280, 720) * 0.3 = 216px`，`scale = interpolate(frame, [0, 24], [0.2, 1.0], clamp)`
- 固定标题「熊猫智研社」：70px 粗体黑色，logo 下方 40px
  - `opacity = interpolate(frame, [24, 39], [0, 1], clamp)`
  - `translate_y = interpolate(frame, [24, 39], [-50, 0], clamp)`
- 整体淡出：`interpolate(frame, [105, 119], [1, 0], clamp)`
- 水印（`cover` 预设）

> **已知差异**：现有 TS 版片尾标题指定的是 Inter 字体，但内容是中文，Chrome 实际回退到系统字体渲染。Rust 版统一使用 `dingliesongtypeface`，成片会与现状有可见差异。这被视为修正而非回归。

### 8.6 水印规格

两处预设，内容都是 **GitHub 图标 + `Panda Video Generator`**，`cover` 预设额外追加 ` · 熊猫视频自动化引擎`。

| | `cover`（用于 Cover / Outro） | `content`（用于 Content） |
|---|---|---|
| 位置 | 画面水平居中，垂直方向自顶部偏移 432px | 左下角，距左 40px、距下 40px |
| 字号 | 28px | 24px |
| 颜色 | `rgba(23, 23, 23, 0.4)` | `rgba(255, 255, 255, 0.27)` |
| 字重 | 400 | 500 |
| 字距 | 默认 | `0.01em` |
| 图标尺寸 | 32px | 28px |
| 图标与文字间距 | 12px | 10px |
| 中文后缀 | 有（间隔点 `·` 不透明度 0.75） | 无 |

**两处实现决策**（TS 原版行为与 Rust 实现的取舍）：

1. **字体**：TS 原版水印用 Inter。Rust 版**统一用 `dingliesongtypeface`**，不再内嵌第二套字体——可省约 500KB 二进制体积，代价是水印的拉丁字形与现状有可见差异。水印在 27%~40% 不透明度下属装饰元素，判定可接受。
2. **GitHub 图标**：原版是单条 SVG path（`fill="currentColor"`，纯单色，含 `a` 圆弧指令）。Rust 版**内嵌 SVG 源码，启动时用 `resvg` 光栅化一次并缓存**，绘制时按预设颜色着色。

   选它而非"预先光栅化成 PNG 内嵌"的原因有二：(a) 本机没有任何 SVG 光栅化工具（`resvg`/`rsvg-convert`/`inkscape`/`magick` 均不存在），生成不出那个 PNG；(b) `resvg` 的渲染后端本来就是 `tiny-skia`，与本项目的 2D 栈同源，不引入第二套图形依赖。路径里的圆弧指令也意味着手写 path 解析不划算。

### 8.5 动画函数移植

`interpolate(x, input_range, output_range, clamp)` — 线性插值 + 边界钳制，直接实现。

`spring` — Remotion 使用阻尼谐振子解析解，参数 `mass = 1, stiffness = 100, damping = 200, overshoot_clamping = false`。传入 `durationInFrames` 时 Remotion 会把曲线在时间轴上**重新缩放**到指定帧数，移植时必须包含这一步，否则动画节奏会错。

移植后用单测覆盖端点与钳制行为（见 §10）：端点精确、区间内单调、无 NaN。

## 9. ffmpeg 调用

### 9.1 命令结构

```
ffmpeg -y
  -stream_loop -1 -i <bg.mp4>
  -f rawvideo -pix_fmt rgba -s 1280x720 -r 30 -i -
  -i <audio.mp3>
  -stream_loop -1 -i <bgm.mp3>
  -i <intro_typewriter.mp3>
  -i <intro.mp3>
  -filter_complex "<见下>"
  -map "[v]" -map "[a]"
  -t <总时长> -c:v libx264 -crf 23 -pix_fmt yuv420p
  <out.mp4>
```

### 9.2 视频滤镜

```
[0:v] scale=1280:720:force_original_aspect_ratio=increase,
      crop=1280:720,
      colorchannelmixer=rr=0.8:gg=0.8:bb=0.8 [bg];
[bg][1:v] overlay=shortest=0 [v]
```

> **注意**：CSS 的 `filter: brightness(0.8)` 是**乘性**的（每通道 × 0.8）。ffmpeg 的 `eq=brightness` 是**加性**的，语义不同。正确的等价滤镜是 `colorchannelmixer`。

`objectFit: cover` 的等价是 `scale=force_original_aspect_ratio=increase` 后 `crop`。

### 9.3 音频滤镜

四路音频，各自延迟到对应段落起点后混合：

| 源 | 起点 | 音量 |
|---|---|---|
| TTS `audio.mp3` | 4.0s（Content 起点） | 1.0 |
| BGM（循环） | 4.0s | 0.15，在 `[A-2, A]` 区间线性降到 0，之后保持 0 |
| `intro_typewriter.mp3` | 0.5s（Intro 起点） | 0.6 |
| `intro.mp3` | Outro 起点 | 0.6 |

BGM 音量包络用 `volume` 滤镜的时间表达式实现，`amix` 时需设 `normalize=0` 以免自动归一化改变各路相对音量。

### 9.4 进程管理

- 启动前探测 `ffmpeg -version`，失败则明确报错并退出
- Rust 侧另起线程向 stdin 写帧，主线程等待进程结束
- 捕获 stderr 全文；非零退出时原样透出，不做吞噬或改写
- ffmpeg 提前退出（如参数错误）时，写帧线程会遇到 broken pipe，需正常处理而非 panic

## 10. 验证策略

验收标准是**功能正确可用**，不要求与 TS 版逐字节一致。TS 版仅作为行为参考实现，不作为测试基线，因此**不依赖 `pnpm` 环境**。

| 层 | 方法 | 通过标准 |
|---|---|---|
| `vtt` 切句与生成 | 单测覆盖各分支与边界 | 时间轴单调递增、时间戳合法、切片不丢字 |
| `anim`（spring / interpolate） | 单测覆盖端点与钳制行为 | 端点精确、区间内单调、无 NaN |
| `timeline` | 单测覆盖若干音频时长下的段落布局 | 各段首尾相接无空洞、总帧数正确 |
| TTS 音频 | 端到端跑真实文稿 | 音频可播放、时长与文稿相称、字幕与音频不明显错位 |
| 渲染 | 抽关键帧人工核对 | 各段视觉符合规格描述，字幕与音频同步 |
| 端到端 | `panda make` 跑通真实文稿 | 产出可播放 mp4，人工观感验收 |

## 11. 分期计划

| 阶段 | 内容 | 风险 |
|---|---|---|
| **0** | **Edge read-aloud WebSocket 协议验证**：能否完成 `Sec-MS-GEC` 签名并成功合成一段音频 | **高，唯一未知数** |
| 1 | TTS 全链路（分段、并发、重试、合并、VTT），端到端跑通 | 低 |
| 2 | `timeline` + `anim` + `text`：只渲染 Content 字幕层，与 Remotion 比对 | 中（CJK 排版调参） |
| 3 | Cover / Intro / Outro / 水印 | 低 |
| 4 | ffmpeg 滤镜图 + 音频混合 + `make` 一条龙 | 低 |
| 5 | 资源内嵌，产出单一 Linux 二进制 | 低 |

**阶段 0 是关卡**：若 Edge 协议无法跑通，方案需回退到 Azure Speech（有官方 REST API 和 SDK，且原生提供词级时间戳），此时 §7 需重写但 §8、§9 不受影响。因此它必须排在最前。

## 12. 后续（不在本期范围）

- Windows exe 交叉编译（`x86_64-pc-windows-gnu` + mingw），需处理 ffmpeg 可执行名与路径分隔差异
- 竖版 1080×1920 合成（对应 `Video-Vertical`）
- 封面图导出（对应现有 `Cover-Still` → `cover.jpg`，发布时使用）
- WordBoundary 词级精确字幕对齐
- LLM 文稿改写（DeepSeek / Kimi，OpenAI 兼容接口）
- 额外 TTS 后端（Azure / 本地模型）
