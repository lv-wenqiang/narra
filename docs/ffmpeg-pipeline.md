# ffmpeg 合成管道联通性 —— 实测记录

> 本文档记录的是 **2026-09-03 在本仓库 `examples/pipe_probe.rs` 中实际跑通** 的管道形态，
> 所有数字都是在本机（ffmpeg 8.1.2 / libdav1d / libx264，见下文版本信息）跑出来的，
> 不是抄 `task-0-brief.md` 里的预期值。Task 3/4/5 实现时应直接照抄本文档的命令形态。

## 结论

**路线可行：rawvideo RGBA 经 stdin 喂进 `filter_complex`，与 AV1 背景视频 `overlay`
后编码成 h264 mp4，管道端到端跑通，退出码 0，产物用 ffprobe/肉眼核对均正确。**
`-pix_fmt rgba` 在这条管道里被 ffmpeg 的 `overlay` 滤镜当作 **straight（非预乘）alpha**
处理——**R3 的裁定成立**：渲染出的帧在喂给 ffmpeg 之前必须反预乘。

四个问题的简要答案，详细证据见下文各节：

1. **管道通不通**：通。ffmpeg 退出状态 0，写帧线程 `Ok(())`，ffprobe 验得
   `h264 / 1280x720 / 30/1 / duration=3.000000 / nb_frames=90`。
2. **`-pix_fmt rgba` 是 straight 还是预乘**：**straight**。用半透明红做的独立实验
   （见下文「alpha 语义验证」）证明，默认（不显式指定 `overlay` 的 `alpha=` 选项）
   的合成结果与显式 `alpha=straight` **逐字节相同**，与显式 `alpha=premultiplied`
   **明显不同**。
3. **AV1 解码开销**：不小，但不是瓶颈。「叠加+编码」warm 态吞吐约 **142 fps**，
   「纯 AV1→h264 转码（无 overlay）」约 **209 fps**——多出的约 32% 时间花在
   rawvideo stdin 读取 + overlay 合成 + colorchannelmixer 上，AV1 解码本身被
   包含在两个数字里、无法单独摘出，但两者数量级接近，说明 AV1 软解不是
   压倒性瓶颈。两者都远超 30fps 实时线，60 秒成片预计几十秒内可出。
4. **进程编排的正确顺序**：`take() stdin → spawn 写帧线程 → wait() → 事后读 stderr`
   这个顺序，**在本次实测的规模下没有触发死锁**，但 stderr 体积按大约固定的
   **挂钟时间**间隔增长（约 238 字节/挂钟秒，固定基线约 7.6KB），填满 64KB
   缓冲区约需 **4 分钟挂钟时间**——这是修复轮 1 更正过的结论（此前误判成
   "非 tty 就不怎么长"）。本探针写帧是纯内存 memcpy，挂钟耗时约等于编码
   耗时，远低于 4 分钟；但 Task 5 真实管道每帧要过 tiny-skia 渲染，挂钟
   耗时会明显拉长，安全边际不是数量级的。下文（第 6 节）给出触发条件判断
   和推荐的并发读 stderr 写法。另外还实测到一条独立的陷阱（第 7 节）：
   `overlay=shortest=0` 时，**关闭帧流（stdin EOF）完全不会让 ffmpeg 停下来**，
   显式 `-t` 是唯一的终止条件，漏了会让进程安静地无限跑下去且不报错。

---

## 1. 环境与版本

- `ffmpeg version 8.1.2`（`ffprobe` 同版本，同在 PATH）。
- 背景视频编解码器：`libdav1d`（AV1 软解）。编码器：`libx264`。
- 背景素材：`../panda-video-ts/public/video/0.mp4` —— 1920x1080、AV1、
  `yuv420p(tv, bt709)`、30fps、时长 00:02:42.27（162.27s）、码率 317kb/s。
- Linux 管道缓冲区大小（`fcntl(F_GETPIPE_SZ)` 实测）：**65536 字节**。

## 2. 实测通过的完整命令行

`examples/pipe_probe.rs` 里 `Command::new("ffmpeg").args([...])` 拼出的实际参数
（逐个列出，标注转义/顺序要点）：

```
ffmpeg -y \
  -stream_loop -1 -i ../panda-video-ts/public/video/0.mp4 \
  -f rawvideo -pix_fmt rgba -s 1280x720 -r 30 -i - \
  -filter_complex "[0:v]scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,colorchannelmixer=rr=0.8:gg=0.8:bb=0.8[bg];[bg][1:v]overlay=shortest=0[v]" \
  -map "[v]" \
  -t 3 \
  -c:v libx264 -crf 23 -pix_fmt yuv420p \
  /tmp/pipe_probe.mp4
```

关键顺序要点：

- **`-stream_loop -1` 必须紧跟在它所属的 `-i bg` 之前**，因为它是"输入选项"
  （per-input option），ffmpeg 按"选项作用于其后第一个 `-i`"的规则解析。
  **实测验证**：把 `-stream_loop -1` 挪到两个 `-i` 之后（当作全局/输出选项），
  ffmpeg 直接报错拒绝启动，不是静默失效：
  ```
  Option stream_loop (set number of times input stream shall be looped)
  cannot be applied to output url /tmp/wrong_order.mp4 -- you are trying to
  apply an input option to an output file or vice versa. Move this option
  before the file it belongs to.
  Error parsing options for output file /tmp/wrong_order.mp4.
  Error opening output files: Invalid argument
  ```
  退出码 234。结论：这个顺序不是"风格问题"，写错位置直接跑不起来，报错信息
  本身就点明了修复方法。
- rawvideo 输入的 `-f rawvideo -pix_fmt rgba -s 1280x720 -r 30 -i -` 中，
  `-f`/`-pix_fmt`/`-s`/`-r` 同样是输入选项，都在对应 `-i -` 之前，同一条规则。
- `filter_complex` 里 `[0:v]` 是背景视频、`[1:v]` 是 rawvideo stdin 流
  （按 `-i` 声明顺序从 0 开始编号）。
- `-map "[v]"` 只映射合成后的视频流，不映射任何音频（本探针没有音频输入，
  产物是哑巴 mp4，这符合探针目的）。
- `overlay=shortest=0`：不因任一路输入结束而提前收尾，**必须**配合外层 `-t 3`
  硬性截断输出时长——`-t` 在这里不是"顺手截断一下"，而是**唯一的终止条件**。
  这条有陷阱，见第 7 节的专项实验。
- 没有给 `overlay` 显式指定 `format=`，因此走的是默认的 `format=yuv420` 混合路径
  （这一点对 alpha 语义验证很重要，见下节）。

## 3. Step 3 验收结果

```
$ cargo run --release --example pipe_probe
--- 退出状态: exit status: 0
--- 写帧结果: Ok(())
--- 耗时: 2.143696789s（90 帧 → 42.0 fps）   # 首次运行，冷缓存，见第 4 节
--- stderr 末尾 ---
[libx264 @ ...] kb/s:368.55
```

```
$ ffprobe -v error -show_entries stream=codec_name,width,height,r_frame_rate,nb_frames \
          -show_entries format=duration -of default=noprint_wrappers=1 /tmp/pipe_probe.mp4
codec_name=h264
width=1280
height=720
r_frame_rate=30/1
nb_frames=90
duration=3.000000
```

四条验收全部通过（退出码 0 / 编解码器规格吻合 / PNG 肉眼核对见下 / 吞吐数字见第 4 节）。

### 肉眼核对 PNG（验收第 3 条，实际打开看的结果）

```
ffmpeg -y -v error -ss 1 -i /tmp/pipe_probe.mp4 -frames:v 1 /tmp/pipe_probe.png
```

**我实际打开 `/tmp/pipe_probe.png` 看到的画面**：图像左半边（约 640px 宽）是
**纯正的红色矩形，边缘垂直、笔直，颜色均匀、没有渐变或杂色**；右半边完全没有红色
残留，显示的是背景视频的实际内容——**一排书架，木色/白色书脊的书本排列整齐，
书架隔板是白色，能看清楚书脊上的文字轮廓（虽然模糊，因为是运动模糊的视频帧）**。
左右两半的分界线在 x=640 处非常清晰、没有羽化或半透明过渡带。这与预期完全一致：
不透明红色区域完全盖住背景，全透明区域完全不影响背景，且 overlay 确实在做
逐像素 alpha 合成（而不是整块替换或者整块忽略）。

**重要说明——这张图本身不足以证明"straight vs 预乘"**：探针图案只用了两种
极端值——`(255,0,0,255)`（完全不透明）和 `(0,0,0,0)`（完全透明，且 RGB=0）。
无论 ffmpeg 把 alpha 当 straight 还是预乘解释，这两种极端值的合成结果都**完全一样**
（alpha=255 时两种解释下颜色都不变；alpha=0 且 RGB=0 时两种解释下都完全露出背景，
因为 0 乘以任何数还是 0）。所以这张图只回答了"overlay 有没有在做 alpha 合成"
（回答：有），**不能**回答"合成公式是 straight 还是预乘"。这个问题需要半透明色块
才能测出来，见下一节。

## 4. alpha 语义验证（问题 2 的关键证据）

`ffmpeg -h filter=overlay` 显示 `overlay` 滤镜有一个 `alpha` 选项：

```
alpha  <int>  alpha format (from 0 to 2) (default auto)
  auto            0
  unknown         0
  straight        2
  premultiplied   1
```

默认是 `auto`。rawvideo 的 `-pix_fmt rgba` 这个像素格式名本身不携带"是否预乘"的
元信息（ffmpeg 的 `AVPixelFormat` 枚举里 rgba/argb/bgra 这些只是分量顺序，不区分
预乘与否），所以 `auto` 最终会落到某个具体行为——必须用半透明色块实测，不能只看文档。

### 实验设计

用 Python 直接生成一帧 64x64、纯色 `(255,0,0,128)`（半透明红，alpha=128/255≈0.502）
的 rawvideo 二进制，喂给 ffmpeg，叠加到纯蓝色（`0,0,255`）背景上，取中心像素颜色，
分别对比：不指定 `alpha=`（默认 `auto`，且**不指定 `overlay` 的 `format=`，与生产
命令行一致，走默认 `format=yuv420` 混合路径**）、显式 `alpha=straight`、显式
`alpha=premultiplied` 三种情况：

```bash
python3 -c "
import sys
frame = bytearray(64*64*4)
for i in range(64*64):
    frame[i*4:i*4+4] = bytes([255,0,0,128])
sys.stdout.buffer.write(bytes(frame))
" > /tmp/halfalpha_red.raw

# 默认（auto），走 overlay 默认 format=yuv420 混合路径（与生产 filter_complex 一致）
ffmpeg -y -v error \
  -f lavfi -i "color=c=blue:s=64x64:d=1:r=1" \
  -f rawvideo -pix_fmt rgba -s 64x64 -r 1 -i /tmp/halfalpha_red.raw \
  -filter_complex "[0:v][1:v]overlay=shortest=1[v]" \
  -map "[v]" -frames:v 1 /tmp/alpha_test_auto.png
```

（`alpha=straight` / `alpha=premultiplied` 同上，只是在 `overlay=` 后加对应参数。）
用 `ffmpeg -i xxx.png -vf "crop=1:1:32:32" -f rawvideo -pix_fmt rgb24 - | xxd`
取中心像素的 RGB 字节。

### 实测结果

| 配置 | 中心像素 RGB（hex） | 十进制 |
|---|---|---|
| 背景纯色校验（无 overlay，纯蓝） | `00 00 fe` | (0, 0, 254) ≈ 纯蓝，无失真 |
| **默认（不指定 `alpha=`，`format` 亦默认 yuv420）** | `80 00 7c` | **(128, 0, 124)** |
| 显式 `alpha=straight` | `80 00 7c` | (128, 0, 124) —— **与默认逐字节相同** |
| 显式 `alpha=premultiplied` | `3f 00 7d` | (63, 0, 125) —— **与默认明显不同** |

理论上 straight-alpha 合成公式 `out = fg·a + bg·(1-a)`（a=128/255≈0.5020）给出的
精确值应为 `R=255×0.502≈128, G=0, B=255×0.498≈127`——与实测 `(128,0,124)` 高度吻合
（B 通道的 124 vs 127 的 3 个单位误差来自 `format=yuv420` 默认混合路径引入的
4:2:0 色度二次采样/取整，不是合成公式本身的偏差）。而 `premultiplied` 给出的
`(63,0,125)` 与 straight 的差距远超这个量级的取整误差，明显是另一套公式。

**结论：默认配置（生产 `filter_complex` 也没有显式指定 `overlay` 的 `alpha=`）下，
ffmpeg 把我们喂的 `-pix_fmt rgba` 数据当 straight alpha 处理。** 这与 R3 的裁定
一致——渲染帧（tiny-skia 内部是预乘 alpha）在写入 ffmpeg stdin 前必须反预乘，
否则半透明区域的颜色会系统性偏暗（因为预乘值天然更小，被 ffmpeg 当
straight 值用会让"应有的中间色"打了折扣）。**没有出现预乘假设被推翻的情况，
不需要 BLOCKED。**

⚠️ **一个连带发现，值得记录给后续任务**：如果给 `overlay` 显式加上
`format=rgb`（把混合路径从默认 yuv420 换成 RGB 直接混合），三种 `alpha=` 配置
（auto/straight/premultiplied）**会得出完全相同的结果** `(128,0,127)`——也就是说
**RGB 混合路径似乎没有正确实现 `alpha=premultiplied` 的语义区分**（或者说这条
路径下 `alpha=` 选项被忽略/总按 straight 处理）。这不影响本任务结论（因为生产
`filter_complex` 没有加 `format=rgb`，走的是默认 yuv420 路径，行为已验证正确），
但如果 Task 3/4/5 出于画质考虑想给 `overlay` 加 `format=rgb` 或 `format=gbrp`
以避免 yuv420 色度损失，**务必重新跑一遍这个半透明色块实验**，不要想当然认为
alpha 语义不变。

## 5. AV1 解码开销对比（问题 3）

### 5.1 「叠加+编码」（`pipe_probe`，完整管道）

6 次 `cargo run --release --example pipe_probe` 的耗时（90 帧 / 3 秒输出）：

| 次序 | 耗时 | fps |
|---|---|---|
| 1（冷，紧接编译完成后首次运行） | 2.144s | 42.0 |
| 2 | 0.690s | 130.4 |
| 3 | 0.622s | 144.8 |
| 4 | 0.626s | 143.9 |
| 5 | 0.585s | 153.7 |
| 6 | 0.664s | 135.5 |

第 1 次明显是异常值（推测是磁盘缓存冷启动 + 系统刚完成大量 cargo 编译后的资源
竞争，不是管道本身的稳态吞吐）。**稳态（第 2~6 次）平均约 141.7 fps**，标准差
不大（130~154 fps 区间）。

### 5.2 「纯 AV1 转码」（无 overlay，`-vf scale` 直接转码）

```bash
time ffmpeg -y -v error -stream_loop -1 -i ../panda-video-ts/public/video/0.mp4 \
     -t 3 -vf scale=1280:720 -c:v libx264 -crf 23 -pix_fmt yuv420p /tmp/av1_only.mp4
```

6 次运行（含首次），`user+sys` 时间稳定，`total`（wall clock）：

| 次序 | wall time | fps（90帧） |
|---|---|---|
| 1 | 0.685s | 131.4 |
| 2 | 0.437s | 206.0 |
| 3 | 0.424s | 212.3 |
| 4 | 0.441s | 204.1 |
| 5 | 0.413s | 217.9 |
| 6 | 0.434s | 207.4 |

同样第 1 次偏慢（冷启动），**稳态（第 2~6 次）平均约 209.5 fps**。

### 5.3 开销拆分与外推

- 「叠加+编码」稳态 ≈ 142 fps，「纯转码」稳态 ≈ 210 fps，两者比值约 0.68——
  叠加+编码比纯转码慢约 32%。这部分额外开销来自：rawvideo 经 stdin 的读取、
  `overlay` 合成本身的像素运算、多一层 `colorchannelmixer` 滤镜。**AV1 解码
  本身的耗时无法从这两个数字里单独剥离**（两个场景都要解码同一段 AV1 背景），
  但由于两者数量级接近（142 vs 210，同一个量级，不是十倍差距），可以判断
  **AV1 软解不是压倒性瓶颈**，即使换成 H.264 背景，总耗时预计也就是去掉这 32%
  的差值，不会有数量级的提升。
- 外推之前，先厘清「60 秒成片（1860 帧左右）」这句 brief 原话本身有歧义
  （这是 brief 措辞问题，不是本次测量误差；已同步修正 brief 原文）：
  - 若指 **60 秒总视频输出**（即 `-t 60`）：30fps × 60s = **1800 帧**——这正是
    第 5.2 节末尾那次真实 60 秒交叉验证用的帧数。
  - 若指 **`A=60`（60 秒音频）驱动的一份完整成片**：按计划 §2 的段落公式
    `content_frames = ceil((A+2)*30)`，`A=60` 时 `content_frames = ceil(62×30)
    = 1860` 帧——但这**只是 Content 段落自己的帧数**，不含 Cover(15 帧) +
    Intro(105 帧) + Outro(120 帧) 共 240 帧固定开销；一份完整成片的总帧数是
    `240 + content_frames = 2100` 帧，对应最终视频时长 **70 秒**、不是 60 秒。
    这与计划文档 `docs/superpowers/plans/2026-09-03-ffmpeg-compose.md:31`
    「2100 帧（60s 成片）」的表述一致——那里的「60s」指的是 `A=60` 的音频
    时长，不是 70 秒的总视频时长，计划内部本身就有这个术语歧义，容易被
    Task 5 直接抄错「1860 帧」当总帧数用，这里一并澄清。
  - 用稳态吞吐 142 fps 外推：
    - 1800 帧（60 秒纯视频）：1800 / 142 ≈ **12.7 秒**。
    - 2100 帧（`A=60` 音频对应的完整成片）：2100 / 142 ≈ **14.8 秒**。
  - 用本次实测到的最差单次（冷启动 42 fps，不排除生产环境首次调用也会遇到类似
    冷启动开销）：
    - 1800 帧：1800 / 42 ≈ **42.9 秒**。
    - 2100 帧：2100 / 42 ≈ **50.0 秒**。
  - 额外用真实 60 秒时长跑了一次完整 overlay 管道验证（`rawvideo` 输入换成
    `/dev/zero` 以避免另写一个 90000 帧的生成脚本，只测 ffmpeg 侧吞吐，不含
    Rust 侧写帧开销）：
    ```
    time ffmpeg ... -stream_loop -1 -i .../0.mp4 -f rawvideo -pix_fmt rgba \
         -s 1280x720 -r 30 -i /dev/zero -filter_complex "...overlay..." \
         -map "[v]" -t 60 -c:v libx264 -crf 23 -pix_fmt yuv420p out.mp4
    # 1800 帧，wall time 10.184s → 176.7 fps
    ```
    这个数字比稳态 142fps 更快，是因为 `/dev/zero` 读取比真实 Rust 写帧线程
    （每帧要现算 RGBA buffer 再 `write_all`）更快，不代表生产环境真实吞吐，
    但确认了**同一量级、没有随时长增长而劣化**，60 秒渲染在十几秒到几十秒
    之间是合理预期，不会失控到几分钟。
  - **结论：60 秒成片预计在十几秒到不到一分钟之间完成，具体取决于是否有冷启动
    开销；这个量级对本项目的使用场景（离线批量合成，非实时）完全可接受。**

## 6. 进程编排的正确顺序（问题 4）

### 6.1 探针里实际用的顺序（照抄自 brief，未改动）

```rust
let mut child = Command::new("ffmpeg")
    .args([...])
    .stdin(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;

let mut stdin = child.stdin.take().expect("stdin 已 piped");  // ① 先 take stdin

let writer = std::thread::spawn(move || -> std::io::Result<()> {
    // ② 在独立线程里写帧，避免阻塞主线程
    ...
    stdin.write_all(&frame)?;
    ...
    stdin.flush()
});

let status = child.wait()?;                    // ③ 主线程 wait()
let write_result = writer.join().expect(...);  // ④ 等写帧线程收尾

let mut stderr = String::new();
if let Some(mut e) = child.stderr.take() {      // ⑤ 最后才读 stderr（事后，非并发）
    e.read_to_string(&mut stderr).ok();
}
```

**实测结果：这个顺序在 90 帧/3 秒规模下跑了 6 次，全部正常退出，没有一次死锁或
挂起。** 60 秒规模（用 `/dev/zero` 测 ffmpeg 侧吞吐那次）也没有挂起。

### 6.2 为什么没触发死锁 —— 修复轮 1 更正：此前的机制解释是错的

**修复轮 1 更正**：我最初把 stderr 体积"看起来不随渲染时长明显变大"归因于
「ffmpeg 检测到 stderr 不是 tty 就降低刷新频率」，**这个归因是错的，审查已指出**。
真实机制是：**进度行按大约固定的 wall-clock（挂钟时间）间隔输出，与是否
tty、与请求的输出内容时长（`-t` 的值）都无关**；之前几组测试（3/10/20/60 秒
输出内容）之所以看起来 stderr"几乎不变"，只是因为这几次全速编码本身的
wall-clock 耗时都很短（几百毫秒到十秒量级），并不是因为增长机制与时长无关——
**增长是线性的，只是我之前用来对比的几组测试挂钟时间都太接近了，掩盖了这一点**。

死锁的机制本身没有变：ffmpeg 把 stderr 写满 OS 管道缓冲区后阻塞在 `write()` 上，
而主线程又阻塞在 `child.wait()` 上、没人在读 stderr，双方互相等对方，死锁
无法自行解开。真正要紧的是**这个缓冲区多久会被填满**，答案取决于**挂钟时间**，
不是输出内容时长。

**重新做的对照实验**（固定输出内容时长为 20 秒，只改变挂钟耗时——用 `-re`
让 rawvideo 输入按 30fps 实际速率被读取，逼近真实速率而非全速）：

```bash
FILT="[0:v]scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,\
colorchannelmixer=rr=0.8:gg=0.8:bb=0.8[bg];[bg][1:v]overlay=shortest=0[v]"

# 全速
time ffmpeg -y -stream_loop -1 -i .../0.mp4 \
     -f rawvideo -pix_fmt rgba -s 1280x720 -r 30 -i /dev/zero \
     -filter_complex "$FILT" -map "[v]" -t 20 \
     -c:v libx264 -crf 23 -pix_fmt yuv420p runA.mp4 2> stderr_A.txt

# 用 -re 让 rawvideo 输入按 30fps 实速被读取
time ffmpeg -y -stream_loop -1 -i .../0.mp4 \
     -re -f rawvideo -pix_fmt rgba -s 1280x720 -r 30 -i /dev/zero \
     -filter_complex "$FILT" -map "[v]" -t 20 \
     -c:v libx264 -crf 23 -pix_fmt yuv420p runB.mp4 2> stderr_B.txt
```

| | 挂钟耗时 | 进度行数 | stderr 字节 |
|---|---|---|---|
| 全速（A） | 3.480s | 7 | 8593 |
| `-re` 逼近实速（B） | 19.666s | 40 | 12450 |

两点确定一条直线：增长率 `(12450-8593)/(19.666-3.480) ≈ 238 字节/挂钟秒`；
外推到挂钟耗时为 0 的固定基线（启动横幅 + 收尾统计块）≈ **7.6KB**。抽查
Run A 的进度行本身也印证了"按挂钟时间定频"：`elapsed=0:00:00.50` →
`elapsed=0:00:01.00` → `elapsed=0:00:01.50` ……间隔精确是 0.5 秒挂钟时间，
与视频内容进度到哪一秒无关。

按这条线性关系，填满 64KB 管道缓冲区需要 **(65536-7600)/238 ≈ 243 挂钟秒，
约 4 分钟挂钟时间**——这才是需要对照的阈值，**单位是挂钟时间，不是视频内容
时长**。之前几节测的「3/10/20/60 秒输出内容」stderr 字节数
（8014/8250/8605/10257）之所以增长缓慢，是因为那几次全速编码对应的挂钟耗时
本身都很短（60 秒输出内容那次也只跑了 10.184 秒挂钟时间）——套进这条线性
公式：`7600 + 238×10.184 ≈ 10024` 字节，与实测的 10257 字节吻合，**说明
那批早期数据本身没问题，问题出在我当时给它们编的"因为非 tty 所以不怎么长"
这个错误解释上**。

**这条对本项目要紧的地方**：本探针的写帧线程只是内存里 memcpy 一块 buffer，
几乎不占时间，所以整条管道的挂钟耗时约等于 ffmpeg 自己的编码耗时（142fps
量级，远快于 30fps 实时线）。但 **Task 5 的真实管道每帧要先跑一遍 tiny-skia
渲染**（上一份计划实测 8.79ms/帧、113.7fps，见计划文档），写帧线程不再是
瞬时的 memcpy，整条管道的挂钟耗时会明显拉长，向 `-re`（近似实速）这组数字
靠拢，而不是本探针"全速"那组数字。以 2100 帧（一份 `A=60` 音频驱动的完整
成片，见 5.3 节的口径澄清）为例，如果整条管道挂钟耗时落在 1~2 分钟量级，
stderr 离 64KB 阈值（约 4 分钟）还有安全边际，但边际**不是数量级的**——
默认详细度下单独一次渲染大概率不会死锁，**但下一节列的几种情况都会明显
压缩这个安全边际，不能当成"结构性安全"来设计代码**。

**判断的触发条件（更正后）**：默认详细度、单路 overlay、没有解码/滤镜警告的
场景下，只要**挂钟耗时**保持在几分钟以内，stderr 大概率填不满 64KB 缓冲区。
以下情况会显著压缩这个安全边际：
- 挂钟耗时本身变长——渲染越慢（真实 `FrameSource` 渲染 + 反预乘的开销、
  机器负载更高、并发跑多个渲染任务抢 CPU），越接近约 4 分钟这条线。
- 提高详细度到 `-v verbose`/`-v debug`，会为每一帧打印详细日志，体积随
  **帧数**（而不只是挂钟时间）线性增长，很快超过 64KB。
- 源素材有损坏帧、`overlay`/`colorchannelmixer` 遇到异常输入触发警告，
  这类警告通常是**每次触发都打一行**，不像正常进度那样被限频，日志量不可预测。

### 6.3 推荐写法（防御性，Task 5 应采用）

**不要依赖"侥幸不触发"。** 正确做法是把读 stderr 也放到独立线程里，与写 stdin
线程并发进行，都在 `wait()` 之前启动：

```rust
let mut child = Command::new("ffmpeg")
    .args([...])
    .stdin(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;

let mut stdin = child.stdin.take().expect("stdin 已 piped");
let mut stderr = child.stderr.take().expect("stderr 已 piped");

let writer = std::thread::spawn(move || -> std::io::Result<()> {
    // 写帧...
    stdin.write_all(&frame)?;
    stdin.flush()
});

let stderr_reader = std::thread::spawn(move || -> std::io::Result<String> {
    use std::io::Read;
    let mut buf = String::new();
    stderr.read_to_string(&mut buf)?;   // 与写帧线程并发跑，缓冲区不会堆积
    Ok(buf)
});

let status = child.wait()?;                         // 现在 wait() 前两个线程都已在跑
let write_result = writer.join().expect("写帧线程不应 panic");
let stderr_text = stderr_reader.join().expect("读 stderr 线程不应 panic")?;
```

关键点：`stdin.take()` 和 `stderr.take()` 都要在 `spawn 写帧/读 stderr 线程之前`
完成；两个线程都要在调用 `child.wait()` 之前就已经 `spawn` 出去在跑；`wait()`
之后再 `join()` 两个线程收尾。**顺序是"两路 take → 两路 spawn → wait → 两路
join"，stdin 和 stderr 必须对称处理，不能只并发一个。**

## 7. `-t` 是承重的终止条件——`shortest=0`/`shortest=1` 的取舍（修复轮 1 新增，I2）

第 2 节提过 `overlay=shortest=0` 配合外层 `-t 3` 才能截断输出，这里补上被
漏记的关键一半：**`shortest=0` 时，关闭帧流（stdin EOF）本身完全不会让
ffmpeg 停下来**。

原因：`overlay` 滤镜有个默认值是 `repeat` 的 `eof_action` 选项——某一路输入
耗尽时，重复它的最后一帧，而不是结束。`shortest=0` 意味着"不因为任意一路
输入结束就收尾"，配合背景视频 `-stream_loop -1`（永远不会自然耗尽）和 rawvideo
EOF 后 `eof_action=repeat`（重复最后一帧而不是结束），**唯一能让整条命令
停下来的就是显式的 `-t`**。反过来，`shortest=1` 会让"任意一路输入结束"
立刻终止整条命令，包括 rawvideo 因 stdin EOF 而结束。

### 实测验证

```bash
# (a) shortest=0，不给 -t，stdin 只喂 5 帧就关闭
head -c $((1280*720*4*5)) /dev/zero | timeout 15 ffmpeg -y \
  -stream_loop -1 -i ../panda-video-ts/public/video/0.mp4 \
  -f rawvideo -pix_fmt rgba -s 1280x720 -r 30 -i - \
  -filter_complex "[0:v]scale=1280:720[bg];[bg][1:v]overlay=shortest=0[v]" \
  -map "[v]" -c:v libx264 -pix_fmt yuv420p -v error /tmp/i2a.mp4
echo $?
```

结果：`echo $?` 打印 **124**（`timeout 15` 自己把 ffmpeg 杀掉了，说明 ffmpeg
15 秒内没有自行退出）。产物 `/tmp/i2a.mp4` 已经写到 12MB，但用 `ffprobe`/
`ffmpeg` 读取会报 `moov atom not found`——**结构性损坏、完全不可播放**（mp4
的索引结构要等进程正常收尾时才写出，被杀掉的半成品文件天生残缺）。

```bash
# (b) shortest=1，同样只喂 5 帧就关闭 stdin，不给 -t
head -c $((1280*720*4*5)) /dev/zero | timeout 15 ffmpeg -y \
  -stream_loop -1 -i ../panda-video-ts/public/video/0.mp4 \
  -f rawvideo -pix_fmt rgba -s 1280x720 -r 30 -i - \
  -filter_complex "[0:v]scale=1280:720[bg];[bg][1:v]overlay=shortest=1[v]" \
  -map "[v]" -c:v libx264 -pix_fmt yuv420p -v error /tmp/i2b.mp4
echo $?
ffprobe -v error -show_entries stream=nb_frames -show_entries format=duration \
        -of default=noprint_wrappers=1 /tmp/i2b.mp4
```

结果：`echo $?` 打印 **0**（正常退出）；`ffprobe` 报告 `nb_frames=5`、
`duration=0.166667`——与喂入的帧数精确对应。

**这条为什么要紧**：`shortest=0`+ 无 `-t` 的失败形态是**没有任何报错输出，
进程只是安静地一直跑、产物文件一直变大**，要靠外部超时或磁盘写满才会被
发现，排查线索极少——这正是 Task 5 如果照抄"探针写死 `-t 3`"这个形态、
却在真实的**变长**帧流场景里忘记同步算出并传入 `-t` 时会撞上的坑。

### 对 Task 5 的判断：倾向 `shortest=0` + 显式 `-t`（而不是 `shortest=1`）

Task 5 的真实帧流是变长的（帧数由计划 §2 的段落公式决定），有两条路线：

1. **`shortest=0` + 显式 `-t`**：`-t` 的值必须由 Rust 侧用同一份"算总帧数"
   的代码算出（`总帧数/30` 秒），不能是第二处独立硬编码或重新推算的数字。
2. **`shortest=1`，靠 `drop(stdin)` 触发的 EOF 终止**：不需要预先算出
   `-t`，天然和"写帧线程写完就结束"同步。

**我的倾向是方案 1（`shortest=0` + 显式 `-t`）**，理由：

- 总帧数本来就要在 Rust 侧算出来用于分配缓冲区/驱动写帧循环（计划里
  `content_frames` 公式已经是 Task 2/5 要落地的既有逻辑），从这同一个数字
  派生 `-t` 只是多一行代码，不构成新增的"两处要保持同步的地方"——只要
  坚持"`-t` 的实参必须来自算总帧数的同一个函数"这条纪律，就没有双数源
  不一致的风险。
- **方案 2 的隐藏风险更难在测试里覆盖到**：如果写帧线程中途 panic 导致
  `stdin` 被提前 drop（帧还没写完），`shortest=1` 会让 ffmpeg 把这**误判成
  正常的流结束**，悄悄收尾产出一个"不报错、但帧数被截断"的短片——这个
  失败模式和实验 (a) 里的"安静挂住"是同一类问题的另一面：错误被 overlay
  的流结束语义**吸收掉了，没有喊出来**。而方案 1 下，写帧线程 panic 会被
  `writer.join()` 正确捕获并让整个命令报错退出，不会产出一个看起来正常
  实则被截断的文件。
- 唯一要注意的坑：`-t` 必须严格等于「总帧数 / 30」，不能错用「Content 段落
  帧数 / 30」——Cover(15)+Intro(105)+Outro(120) 这 240 帧不属于
  `content_frames`（见 5.3 节的帧数口径澄清），算漏了会把 Outro 那 120 帧
  截掉。

Task 5 如果最终决定用 `shortest=1`，请在报告里明确说明如何堵住"写帧线程
异常提前退出被 ffmpeg 悄悄吸收成正常结束"这个对应的风险点。

## 8. 自审

- 四个问题：已逐一回答（第 0 节摘要 + 第 3/4/5/6/7 节展开）。
- 每个数字都来自本机实测：Section 2 的报错信息、Section 4 的三组像素值、
  Section 5 的全部 fps/耗时表格、Section 6 的 pipe buffer 大小、stderr 字节数
  与挂钟增长率、Section 7 的两组终止行为实测，全部是本次会话里实际跑出来的
  命令输出，没有照抄 brief 里的"预期值"当结论。Section 5.3 的"预计耗时"
  是外推计算（不是直接实测），已明确标注为外推，并给出了一次真实 60 秒
  运行作交叉验证。
- 没有触碰 `src/`；只新增了 `examples/pipe_probe.rs`（探针）、本文档、以及
  `assets/intro.mp3` / `assets/intro_typewriter.mp3`（简单文件复制，供 Task 1 用，
  不含任何生产逻辑代码）。
- 发现的一个值得注意的点（已写进第 4 节末尾的"连带发现"）：如果后续任务给
  `overlay` 加 `format=rgb`/`format=gbrp`，alpha 语义可能表现不同，需要重新验证，
  不能直接照搬本文档"straight"的结论。
- **修复轮 1 记录**：审查独立复现了管道跑通、alpha 三个像素值、吞吐数字、
  pipe buffer 大小、`-stream_loop` 报错，全部逐字/精确吻合，关卡结论维持
  「通过」。审查指出并要求修复两处文档缺陷：(I1) 第 6.2 节此前把 stderr
  体积"看起来不随时长明显变大"错误归因于"非 tty 降频"，实际是**挂钟时间
  线性增长**（本轮用自己的 `-re` 对照实验重新测出约 238 字节/挂钟秒、
  固定基线约 7.6KB、约 4 分钟挂钟时间填满 64KB），已重写该节并更正了
  "几乎不随渲染时长线性增长"这句错误表述；(I2) 漏记了 `shortest=0` 时
  `-t` 是唯一终止条件、stdin EOF 不会让 ffmpeg 停下来这条陷阱，已新增
  第 7 节，用两组对照实验（`shortest=0`/`1` 各喂 5 帧后关闭 stdin，无 `-t`）
  证实，并给出了对 Task 5 的倾向判断（`shortest=0`+ 显式 `-t`，理由见
  第 7 节）。另外顺带修正了 5.3 节"60 秒成片=1860 帧"的口径歧义（区分
  "60 秒纯视频=1800 帧"与"`A=60` 音频驱动的完整成片=2100 帧"）和一处
  中英混排笔误。

## 9. `panda make` / `panda render` 端到端实测（Task 8，2026-09-04）

> 本节记录 Task 8 用真实素材跑通 `panda make` 后、用 `ffprobe`/`volumedetect`
> 核验过的管道形态与吞吐。命令行本身由 `src/ffmpeg.rs` 的 `build_render_args` /
> `audio_filter_graph` 生成（第 1–8 节记的是 Task 0 探针的简化版滤镜图，本节是
> 生产代码的真实四路音频版本）；下面把这次真实运行实际取到的参数
> （`A=16.276s`、Outro 起点 `22.300s`、`total=26.300s`）代入后原样列出。数字
> 与 `.superpowers/sdd/2026-09-03-ffmpeg-compose/task-8-report.md` §4.1/§4.10
> 的实测一致。

### 9.1 本次运行

```
$ printf '大家好，欢迎收看本期节目。\n今天我们来聊一个有意思的话题，这段话稍微长一点，用来测试字幕换行和字号规则，也顺便验证背景音乐的淡出时机。\n希望这期内容对你有帮助，我们下期再见。\n' > /tmp/e2e.txt
$ ./target/release/panda make /tmp/e2e.txt --title "端到端验收标题" \
    --bg ../panda-video-ts/public/video/0.mp4 \
    --bgm ../panda-video-ts/public/bgm/0.mp3 \
    -o /tmp/final.mp4
```

真实 Edge TTS 网络往返一次成功（3 段句子），没有降级。TTS 产出
`output/tts/audio.mp3`（加速后约 16.28s）与 `output/tts/audio.vtt`；VTT
末条字幕结束时间即 `A = 16.276s`。

### 9.2 实测通过的完整命令行（含四路音频与视频滤镜链）

背景视频循环 + rawvideo 帧流 `overlay` + 四路音频 `amix` 的完整参数——
`filter_complex` 里的输入编号固定为 `0` 背景视频、`1` stdin 帧流、`2` TTS、
`3` BGM、`4` 打字机音效、`5` 片尾音效：

```
ffmpeg -y \
  -stream_loop -1 -i ../panda-video-ts/public/video/0.mp4 \
  -f rawvideo -pix_fmt rgba -s 1280x720 -r 30 -i - \
  -i output/tts/audio.mp3 \
  -stream_loop -1 -i ../panda-video-ts/public/bgm/0.mp3 \
  -i assets/intro_typewriter.mp3 \
  -i assets/intro.mp3 \
  -filter_complex "\
[0:v]scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,colorchannelmixer=rr=0.8:gg=0.8:bb=0.8[bg];\
[bg][1:v]overlay=shortest=0[v];\
[2:a]adelay=4000:all=1,volume=1[a_tts];\
[3:a]adelay=4000:all=1,volume=0.15,afade=t=out:st=18.276:d=2[a_bgm];\
[4:a]adelay=500:all=1,volume=0.6[a_type];\
[5:a]adelay=22300:all=1,volume=0.6[a_intro];\
[a_tts][a_bgm][a_type][a_intro]amix=inputs=4:normalize=0:duration=longest[a]" \
  -map "[v]" -map "[a]" \
  -t 26.3 \
  -c:v libx264 -crf 23 -pix_fmt yuv420p \
  -c:a aac -b:a 192k \
  /tmp/final.mp4
```

三个关键数字的来源（都能从这次真实运行的 `A`/帧数反推出来）：

- **`st=18.276`**（BGM 淡出起点）：`CONTENT_START_SECS(4.0) + A(16.276) −
  BGM_FADE_SECS(2.0)`——规格 §9.3「Content 段内 `[A-2, A]`」换算成全片
  绝对时间后的起点（裁定 R2）。
- **`adelay=22300`**（片尾音效延迟到 Outro 起点）：
  `content_frames = ceil((16.276+2)×30) = 549`，
  `Outro 起点 = (120 + 549) / 30 = 22.300s → 22300ms`。
- **`-t 26.3`**：`total_frames(789) / 30 = 26.300`。

### 9.3 产物规格（`ffprobe` 实测）

```
$ ffprobe -v error -show_format -show_streams /tmp/final.mp4
codec_name=h264  width=1280  height=720  pix_fmt=yuv420p  r_frame_rate=30/1
nb_frames=789    duration=26.300000
codec_name=aac   sample_rate=24000  channels=1  duration=26.300000
size=3771709
```

段落切分（帧号 / 全局绝对时间，`content_frames=549` 代入段落公式后的结果）：

| 段落 | 帧 | 绝对时间 |
|---|---|---|
| Cover | 0–14 | 0.000–0.500 |
| Intro | 15–119 | 0.500–4.000 |
| Content | 120–668 | 4.000–22.300 |
| Outro | 669–788 | 22.300–26.300 |

### 9.4 吞吐

| 运行 | 帧数 | 挂钟耗时 | 端到端 fps |
|---|---|---|---|
| `panda make`（含 Edge TTS 3 段网络往返 + 合并 + 合成） | 789 | **16.698s** | — |
| `panda render`（纯合成，静音对照那次） | 789 | **15.474s** | **51.0 fps** |

`panda render` 该次的 `time`：`user 52.03s / sys 6.43s / 377% cpu`——
tiny-skia 渲染与 ffmpeg 编码并行、吃了约 3.8 个核。

与第 5 节 Task 0 探针 **142 fps** 的落差（约 2.8×）：探针的写帧线程只是内存
`memcpy`（无真实渲染），本次是「tiny-skia 逐帧真实渲染 + 反预乘 + 写入
stdin」与编码并行，与第 5.3 节当时的预判——「Task 5 真实管道每帧要先跑一遍
tiny-skia 渲染，挂钟耗时会明显拉长」——一致，不是回归。
