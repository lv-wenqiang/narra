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
   这个顺序，**在本次实测的规模（3 秒、60 秒单条 overlay 编码）下没有触发死锁**，
   因为本机 ffmpeg 8.1.2 在 stderr 非 tty 时输出量很小（60 秒渲染仅约 10KB，
   远低于 Linux 64KB 的管道缓冲区）。但这是"侥幸不触发"，不是"没有风险"——
   下文给出了触发条件的判断和推荐的防御性写法。

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
- `overlay=shortest=0`：不提前结束，配合外层 `-t 3` 硬性截断输出时长。
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
否则半透明区域的颜色会systematically偏暗（因为预乘值天然更小，被 ffmpeg 当
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
- 外推到 60 秒成片（按 brief 给的量级，约 1860 帧）：
  - 用稳态吞吐 142 fps：1860 / 142 ≈ **13.1 秒**。
  - 用本次实测到的最差单次（冷启动 42 fps，不排除生产环境首次调用也会遇到类似
    冷启动开销）：1860 / 42 ≈ **44.3 秒**。
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

### 6.2 为什么没触发死锁 —— 量化到底有多"侥幸"

死锁的机制是：ffmpeg 把 stderr 写满 OS 管道缓冲区后阻塞在 `write()` 上；
如果 ffmpeg 同时又在等我们从 stdin 继续喂数据（或者反过来，我们的写帧线程
把 stdin 管道缓冲区写满、阻塞在我们自己的 `write_all()` 上，而 ffmpeg 忙于
往一个没人读的 stderr 里写日志、顾不上读 stdin），双方互相等对方，
而主线程又阻塞在 `child.wait()` 上——没有人在读 stderr，死锁无法自行解开。

实测量化：
- Linux 管道缓冲区大小（`fcntl(F_GETPIPE_SZ)` 实测）：**65536 字节**。
- 探针默认详细度（没设 `-v error`/`-v quiet`）下，3 秒渲染产生的 stderr：
  **8014 字节**（约占缓冲区 12%）。
- 同样配置跑到 10 秒、20 秒、60 秒（1800 帧）：stderr 分别是 **8250 / 8605 /
  10257 字节**——**几乎不随渲染时长线性增长**，60 秒也只涨到约 10KB，
  远低于 64KB 上限。

原因：ffmpeg 检测到 stderr 不是 tty 时，进度统计行（`frame=... fps=... time=...`）
的刷新频率和/或格式与写终端时不同，不会像交互式终端那样高频重复输出；stderr
里的主体内容是启动横幅（版本、编译配置，这条 configure 字符串本身就有几 KB）
和收尾时 libx264 打印的一次性统计块，这两部分是**近似固定大小**的，不随渲染
时长显著变化。

**判断的触发条件**：在"默认详细度、单路 overlay、没有解码/滤镜警告"的场景下，
即使渲染到几分钟量级，stderr 也不太可能填满 64KB 缓冲区，"先 wait() 后读 stderr"
在这个具体场景下大概率不会死锁。但以下情况会显著推高 stderr 体积、把风险
变成现实：
- 提高详细度到 `-v verbose`/`-v debug`，会为每一帧打印详细日志，体积随帧数
  线性增长，很快超过 64KB。
- 源素材有损坏帧、`overlay`/`colorchannelmixer` 遇到异常输入触发警告，
  这类警告通常是**每次触发都打一行**，不像正常进度那样被限频，日志量不可预测。
- 渲染时长远超 60 秒（比如本项目要拼多个段落、合成分钟级长视频）。

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

## 7. 自审

- 四个问题：已逐一回答（第 0 节摘要 + 第 3/4/5/6 节展开）。
- 每个数字都来自本机实测：Section 2 的报错信息、Section 4 的三组像素值、
  Section 5 的全部 fps/耗时表格、Section 6 的 pipe buffer 大小与 stderr 字节数，
  全部是本次会话里实际跑出来的命令输出，没有照抄 brief 里的"预期值"当结论。
  Section 5.3 的"60 秒预计耗时"是外推计算（不是直接实测），已明确标注为外推，
  并给出了一次真实 60 秒运行作交叉验证。
- 没有触碰 `src/`；只新增了 `examples/pipe_probe.rs`（探针）、本文档、以及
  `assets/intro.mp3` / `assets/intro_typewriter.mp3`（简单文件复制，供 Task 1 用，
  不含任何生产逻辑代码）。
- 发现的一个值得注意的点（已写进第 4 节末尾的"连带发现"）：如果后续任务给
  `overlay` 加 `format=rgb`/`format=gbrp`，alpha 语义可能表现不同，需要重新验证，
  不能直接照搬本文档"straight"的结论。
