# 待办与已知限制

本项目的欠账本，按**子系统**分节，每节内部再按「值得做 / 可以不做 / 记一笔」
三段排列——排序依据是「是否值得做」而非「是否是问题」。每条都经过审查确认，
带得出实测数字的一律附上数字。

已记账的子系统：

- [TTS 子系统](#tts-子系统)（`docs/superpowers/plans/2026-09-01-tts-subsystem.md`）
- [帧渲染子系统](#帧渲染子系统)（`docs/superpowers/plans/2026-09-02-render-frames.md`）
- [ffmpeg 合成子系统](#ffmpeg-合成子系统)（`docs/superpowers/plans/2026-09-03-ffmpeg-compose.md`）

跨子系统的条目记在**最早提出它的**那一节里，后来的子系统只往上追加，不另开新条
（例如帧渲染补充的画布常量、CLI 单测，分别并进了 TTS 节的第 3、第 2 条）。

---

## TTS 子系统

### 值得做

#### 1. `merge_mp3_with_speed` 用 `-y` 直写目标路径，非原子

`src/ffmpeg.rs` 调 ffmpeg 时用 `-y` 直接覆写 `audio.mp3`。若**合并过程本身**被外部中断（`kill -9`、OOM killer、断电、磁盘满），旧的 `audio.mp3` 会被替换成一份**可正常解码但内容被腰斩**的音频——不是字节乱码，所以不容易发现。

已实测复现：合并 7200 秒输入时 0.5 秒后 kill，目标文件变成 786KB / 约 216 秒的完整可解码 mp3。

触发需外部进程级中断，窗口通常亚秒级（60 秒音频转码仅需 0.13 秒），故未阻塞合入。
**修法**：先写临时文件，成功后原子 `rename`。

#### 2. `config.rs` 的环境变量层没有自动化测试

`resolve_batch_size` / `resolve_timeout_ms` 两个纯函数有测试，但真正读 `std::env` 的三个函数（`spider_output_dir` / `tts_output_dir` / `tts_input_file`）和 `main.rs` 里「命令行参数 > 环境变量 > 默认值」的优先级链**一条测试都没有**，只有人工验证过。

渲染子系统要加 `render` / `make` 两个子命令，会直接改这块代码——补测试的时机就是那时候。

**帧渲染子系统补充**：`main.rs` 的 CLI 层至今**一条单测都没有**——`parse_frame_list`
（逗号切分、`trim`、缺省帧集 + 补末帧）全靠人工跑 `debug-frames` 验证，`main.rs` 里
没有 `#[cfg(test)] mod tests`。和上面同一个时机、同一次改动一起补。

#### 3. 常量散落在三个文件、三种可见性

`DEFAULT_VOICE` / `SPEED_FACTOR` 在 `config.rs`（pub），`DEFAULT_BATCH_SIZE` / `BATCH_SIZE_CAP` / 超时常量也在 `config.rs`（private），`DEFAULT_MAX_RETRIES` 在 `pipeline.rs`，切句上限 `30` 是 `pipeline.rs` 里的字面量，`OUTPUT_FORMAT` 在 `edge.rs`。

它们在实施计划的 Global Constraints 里本是并列的一批可调参数。渲染子系统会再加一批（画布尺寸、帧率、各段时长、字号…），现在定个统一位置成本最低。

**帧渲染子系统补充（这一批已经落地，而且落成了两份真相源）**：画布尺寸与帧率
**同时存在于两个文件**且互不引用——`src/render/timeline.rs` 有 `WIDTH` / `HEIGHT` /
`FPS`（`u32`，用于分段计算），`src/render/draw.rs` 另有 `CANVAS_W` / `CANVAS_H` /
`FPS`（`f32`/`f64`，用于排版）。`frame.rs` 按 timeline 的尺寸建 `Pixmap`，四个
`draw_*` 却按 draw.rs 的常量排版：**两边一旦不一致，结果是画面静默错位，而不是
编译失败**。修法是让一边从另一边推导，例如
`const CANVAS_W: f32 = crate::render::timeline::WIDTH as f32;`。

#### 4. `ffmpeg.rs` 的两处同步 `Command::output()`

`assert_available()` 和 `merge_mp3_with_speed()` 都是同步阻塞调用，合计约 150ms。当前流水线里占比 <1% 且此刻没有并发任务在等这个线程，无影响。

**渲染子系统会长时间跑 ffmpeg（逐帧管道 + H.264 编码），届时这里是第一个该换成 `tokio::process` 的地方。**

### 可以不做

- **测试临时文件卫生**：`tests/ffmpeg_test.rs` 和 `tests/duration_test.rs` 用硬编码的 `/tmp/m1.mp3`、`/tmp/merged.mp3`、`/tmp/not_audio.mp3` 且不清理；`tests/edge_smoke.rs` 往 `temp_dir()` 落盘也不清理。`pipeline.rs` 的测试用 `unique_tmp_dir(pid+uuid)` 并清理，是好的范式。个人自用，只是留垃圾文件。
- **错误信息中英文混用**：`pipeline.rs` 的 "Narration file is empty (no non-empty lines)" 和 `ffmpeg.rs` 的 "merge_mp3_with_speed: no input files" 是英文，其余约 13 条都是中文。后者还泄漏了内部函数名。`tests/ffmpeg_test.rs` 有断言匹配 `"no input files"`，改文案要连带改测试。纯观感。
- **`main.rs` 越过 trait 直接调 `normalize_voice_for_edge`**：`EdgeBackend::new` 内部又规范化一次。函数幂等所以无害，但 CLI 层知道了后端细节，而 `TtsBackend` trait 存在的意义就是隔离这一层。
- **`edge.rs` 消息循环无 `saw_turn_end` 守卫**：若 WebSocket 流在收到部分音频后直接以 `None` 结束（既无 `turn.end` 也无 Close 帧），会把截断的 mp3 当成功返回。实际很难走到——服务端发 Close 帧会被拦，连接重置会传播错误。
- **emoji 计数差异**：`vtt.rs` 的切句用 Unicode 标量计数，TS 原版用 UTF-16 code unit，星光平面字符下长度判定不同。已在代码注释文档化，实测未产生可观察的切分差异。
- **`generate_vtt` 的 `durations[i]` 越界 panic**：唯一调用点在成功路径上必然等长，当前不可达。它是 `pub`，将来若被别处调用才有风险，加一行 `debug_assert_eq!` 是零成本保险。

### 记一笔：长字幕

实跑发现，**逗号密集、句号稀疏的中文文稿会产出很长的单条字幕**（实测一段 54 字、中间只有逗号顿号的段落未被切分，输出为一条 54 字的 cue）。这是切句算法的正确行为——设计规格 §8.4 对 >50 字的字幕专门有 52px 字号规则，正是为此准备的。

**渲染子系统开工时，应该拿这类真实输出做排版验证的输入**，而不是只用短句测试。


---

## 帧渲染子系统

帧渲染子系统（`docs/superpowers/plans/2026-09-02-render-frames.md`，Task 0–8）
完成、经四轮逐任务审查与一轮全分支终审后留下的账。

### 已销账

- **`draw.rs` 已 2581 行，`mod tests` 该拆出去**（原「值得做」第 1 条）。那条写明
  「应该在下一次真要改 `draw.rs` 的计划里，作为开工第一步做掉」——品牌与水印
  可配置这次正是那一次，也就照它说的当第一步做了。`#[cfg(test)] #[path =
  "draw_tests.rs"] mod tests;`，测试模块仍是 `draw` 的子模块、私有项可见性不变，
  测试代码一行没改。拆完 `draw.rs` 931 行实现 + `draw_tests.rs` 1887 行测试，
  移动前后同为 185 passed / 0 failed。

### 值得做

#### 1. `parse_vtt` 对外部文件不够宽容（三处）

自产自销路径（`generate_vtt` → `parse_vtt`）不会触发，但 `panda debug-frames --vtt`
读的是**用户给的任意文件**，这三处都会被踩到：

- **把正文当序号行丢掉**：正文首行若全是数字（`"2024"`、`"1998"` 这类年份独占一行），
  会被当成 cue 序号跳过，字幕内容凭空少一行。
- **时间戳解析失败时整条 cue 连正文一起静默跳过**：既不报错也不警告，用户只会看到
  某段字幕莫名其妙不见了。
- **只认 `HH:MM:SS.mmm`**：WebVTT 规范同样允许 `MM:SS.mmm`，别家工具产出的 VTT 会被
  整份判为空。

（终审波次已经修掉了同一函数里那个真会 panic 的 char-boundary bug——毫秒位是多字节
字符时按字节切片会炸；上面三条是剩下的**静默**行为，不炸但会丢内容。）

#### 2. `draw_centered` 每次调用都做全画布 scratch clear + composite

热路径上一个约 **3×** 的乘数。实测（1280×720）：

| 操作 | 耗时 |
|---|---|
| 全画布 `draw_pixmap` | 3.07–3.25ms |
| 文本块大小（1024×130）`draw_pixmap` | 0.557ms |
| `Pixmap::fill` | 0.093ms |

两个互不冲突的局部修法：

- **(a) scratch 只覆盖文本块包围盒**：包围盒可以从 `layout_runs` 直接算出，外扩
  `(stroke_w + bold_w) / 2 * scale` 即可容纳描边与合成粗体。
- **(b) `opacity >= 1.0` 时跳过 scratch，直接画进目标 pixmap**：Porter-Duff `over`
  满足结合律，组透明度为 1 时两条路径等价（可能有 1 LSB 的量化差）。

两条合起来预计 8.79ms → 约 3ms/帧。

**但现在不是瓶颈**：全时间轴 2100 帧 18.5s = 3.8× 实时，这点开销会被 libx264 编码
整个遮住。**等 ffmpeg 链路真的测出渲染是瓶颈时再做**——它会改动所有像素测试的地基，
不该在没有收益的时候动。

#### 3. `Painter::new()` 把 2048² 的 logo 解码了两遍

`scaled_logo` 每次调用都重新调 `logo_rgba()`，而 `new()` 要缩两个尺寸（216px 与
36px）。实测单次解码 **18ms**，两次 36ms，占 `new()` 总耗时 132ms 的四分之一还多。
解一次、缩两次即可。

顺带：`assets/logo.png` 是 **1.4MB 的 2048²** 原图，全项目只用在 216px 和 36px 两处。
换成 256px 预缩版能省约 **1.3MB 二进制体积 + 90ms 启动时间**。

### 可以不做

- **Outro 圆环在当前实现下完全不可观测**：5 个纯白实心圆画在纯白底上，像素路径**根本
  测不出来**——`outro_ring_radius_step_is_216px` 因此只能断言常量字面值，是变更探测器
  而非行为测试。这是物理限制，不是测试偷懒。**但若下一份计划加了 `box-shadow`、或把
  圆环改成非纯白，就必须补上真正的像素断言**，那时它才第一次变得可观测。
- **`cursor_and_last_line_bboxes` 与 `..._narrow` 是 90 行重复**（`draw.rs`），唯一
  差别是扫描的 y 带；把 y 带做成 `Option<(u32, u32)>` 参数即可消掉。是测试重构，收益
  纯粹是行数，终审判定放在合入前的最后一道闸门上风险收益不匹配。
- **`spring` 的 `fps` 是死参数**：`src/render/anim.rs` 里明写着 `let _ = fps;`。本实现
  按归一化时间映射曲线，fps 确实不影响结果——但一个**公开函数**留着不起作用的参数，
  等于邀请下一个人以为改 fps 会改动画节奏。要么删掉，要么改名 `_fps` 并在签名文档上
  写清"本实现按归一化时间映射，fps 不参与计算"。
- **Outro logo 的双线性采样在 0.2× 下仍然欠采样**：bilinear 只取 2×2 个源像素，216px
  降到 43px 时中间跳过了大量像素。Outro logo 全程在缩放运动中，运动模糊会掩盖大半。
  若要彻底解决，`Painter` 里多缓存一张小尺寸 logo、按帧的缩放比例选用即可。

### 记一笔：Cover 主标题的行数上限

Cover 的主标题容器**垂直居中**，而 `cover` 水印是**绝对定位**在 y=576——两者互不感知。
标题一长，行数一多，就会向下压穿水印。逐像素扫描实测（100px 粗体，`max_width` 944px）：

| 标题行数 | 标题墨迹底部 | 水印墨迹顶部 | 净空 |
|---|---|---|---|
| 1 行 | 433 | 560 | 127px |
| 3 行 | 553 | 560 | **7px** |
| 5 行（38 字） | — | — | **两条墨带已合并，直接重叠** |

同一次扫描测得约 **8~9 字/行**。因此：

- **舒适上限约 2 行 / 16~18 字**
- **3 行是灰区**（还没压上，但 7px 净空在视觉上已经贴住了）
- **≥4 行必压穿** y=576 的水印

**TS 原版同样如此**（标题容器垂直居中、水印绝对定位，互不感知），所以这是**忠实移植
而非移植缺陷**——但 TS 版一样会在长标题下出问题，不是"照抄就没事"。

**接真实标题时必须二选一：限制标题长度，或让水印在标题超长时下沉。**
**这是下一份计划（ffmpeg 合成与 CLI）的直接输入。**

> **2026-09-04 补记**：`cover` 水印默认不再画（见规格 §8.6），所以「压穿水印」
> 这个失败形式在默认配置下**不会发生**了——标题过长只会自己越界。但配了
> `--watermark-cover` 的用户仍会踩到原样的问题，这条不算销账。

### 记一笔：Outro 底部留白（2026-09-04）

去掉 Outro 底行的工具推广之后，`logo + 品牌名` 之下空出一块——原先那一行占着
的位置现在是白底。**布局没有跟着重排**（本次改动的范围是「画不画」，不是
「怎么排」），实测导出帧上品牌名之后到画面底部约有 200px 空白。

配了 `--watermark-cover` 时那块会重新被填上，所以这只影响默认配置。要不要把
`logo + 品牌名` 整体下移居中，是个纯视觉决定，留给看过成片的人定。

---

## ffmpeg 合成子系统

ffmpeg 合成子系统（`docs/superpowers/plans/2026-09-03-ffmpeg-compose.md`，
Task 0–8）端到端验收与终审修复波留下的账。

### 已销账

- **`amix` 把立体声下混到单声道时用功率保持（≈0.707）而非算术平均，BGM / 两段
  音效比规格字面值响约 3dB**（原「值得做」第 1 条，裁定 R-T8-6 曾判不修）。
  **终审修复波 I1 已修**：根因比原来记的更大一层——四路素材格式各异，`amix` 的
  格式协商被最低的那一路（TTS 24kHz 单声道）拉走，成片音轨整个是 24kHz 单声道，
  三路立体声素材同时**丢掉了 12kHz 以上的全部频段和立体声像**，+3dB 只是同一件
  事的一半。修法是每路进 `amix` 前各接一次
  `aformat=sample_rates=48000:channel_layouts=stereo`（**不是**在输出侧写
  `-ar`/`-ac`——实测那条路不会经 `amix` 反向传播，只会把 ffprobe 读数伪造成
  正确的）。复测见 `docs/ffmpeg-pipeline.md` §9.5：三路立体声落到规格字面值的
  ±0.06 dB 内。**残留的一半**（偏差移到单声道 TTS 上）随后由裁定 R-F9 修掉，
  见下一条。
- **`-stream_loop -1` 从未被真正触发过**（终审修复波 I2）。已用 200 秒静音把
  成片撑到 210 秒跨过两个循环点实测通过，见 `docs/ffmpeg-pipeline.md` §9.6。
- **单声道 TTS 上混成立体声时被衰减 3.01 dB，`volume=1` 名不副实**（原「值得做」
  第 1 条，裁定 R-F9）。**已修**：TTS 那一路从
  `aformat=sample_rates=48000:channel_layouts=stereo` 改为
  `aformat=sample_rates=48000,pan=stereo|c0=c0|c1=c0`——先只重采样（保持单声道），
  再用 `pan` 以单位增益把 c0 复制到两个声道，绕开 swresample 的功率保持上混。
  **保留 `sample_rates=48000` 是有意的**：只写 `pan` 也能跑通，但那一路就不再自己
  声明采样率，I1 修的「协商被最低的一路拉走」就少一道明示的防线；实测这个滤镜的
  代价为零（TTS 那一路只做一次 `mono 24k → mono 48k`，`amix` 仍是 48000/stereo，
  BGM 依旧零转换）。复测见 `docs/ffmpeg-pipeline.md` §9.7：成片 TTS 窗对源的
  `mean_volume`/`max_volume` 两个读数逐字相同（−24.1 / −4.4 dB），±0.00 dB。

（另有两条本子系统自己发现并当场修掉、从未在本文件挂过账的：「写帧中途出错仍
留下冻结帧 mp4」由 `cleanup_output_on_failure` + e2e 抽帧比对堵住；「临时目录名
只用 pid」由 `unique_tmp_audio_dir` 的 pid+uuid 修掉。这里只作记录，本文件里
没有对应条目要删。）

### 值得做

#### 1. 族 A：清理代码写在可能被跳过的位置（三处同根因）

- **生产代码**：`src/main.rs` 的 `compose_video_with_runner` 里，`create_dir_all`
  之后、`cleanup_tmp_and_propagate` 之前有两个 `?` 出口
  （`write_embedded_audio_checked` 与 `FrameSource::new`）。走这两条路时临时目录
  连同两个 mp3（约 120 KB）泄漏在 `/tmp`。
- **测试**：`run_with_timeout` 在被测闭包 panic 后清理不可达。
- **测试**：注入的 `runner` 闭包 panic 会穿过 `compose_video_with_runner`，
  同样跳过 `cleanup_tmp_and_propagate`。

**修法**：一个持有临时目录路径、`Drop` 里删目录的 guard，三处一起解决——把
「清理」从控制流的某一行挪到作用域上，就不存在「哪条路径漏了」这个问题。

**为什么可以推迟**：泄漏量小（约 120 KB/次）、只发生在已经失败的运行上，且
`/tmp` 由系统清理；测试侧的两处只影响测试机的临时文件卫生。

#### 2. 族 B：CLI / 环境变量层覆盖不足（两处）

1. **四个素材路径函数的「空白视同未设置」语义无覆盖**：`bg_video_path` /
   `bgm_path` / `title_json_path` / `video_output_path` 都经 `non_empty_env`
   处理（`BG_VIDEO="  "` 等价于不设置），但没有一条测试覆盖这个分支——把
   `non_empty_env` 换成裸 `std::env::var().ok()` 的变异**存活**。
2. **`Commands::Make` 这个 match arm 自身零覆盖**：把 `tts_artifact_paths(&outdir)`
   换成写死路径不会有任何测试变红。`Commands::Render` 有 `compose_video_with_runner`
   一层可注入执行器兜着，`Make` 没有对应的接缝。

**为什么可以推迟**：两处都是「参数装配」层，错了会立刻在第一次真实运行里以
「文件不存在」的形式炸出来，不是静默坏片。**修法**：(1) 照 `tests/config_env.rs`
既有的串行化环境变量夹具补四条；(2) 把 `Make` 分支体也提炼成一个可注入
`run_tts` 的函数，或至少给 `tts_artifact_paths` 补一条独立断言。

#### 3. 反预乘慢路径有 6× 优化空间

`src/render/frame.rs` 的 `unpremultiply_into` 逐像素调
`PremultipliedColorU8::demultiply()`，而它**只对 `alpha == 255` 短路**；
`alpha == 0`（Content 段的透明底，占绝大多数像素）会走三次 f64 除零。

三行改动（`alpha == 0` 时直接写四个 0），**release 下输出逐位相同**，已实测
600 帧 **1.6s → 0.26s**。

**为什么可以推迟**：反预乘不是端到端瓶颈（`panda render` 实测 51 fps，
tiny-skia 渲染与 libx264 编码并行吃掉约 3.8 个核）；受影响最明显的是
`frame.rs` 里两条走全时间轴的单测。

#### 4. `make` 在整条 TTS 跑完之后才校验素材存在

`check_render_inputs_exist` 在 `run_tts` 之后（它在 `compose_video_with_runner`
里）。`panda make --bg` 打错一个字，要先付一整轮 Edge TTS 网络往返（实测约 10s，
长文稿更久）才报错；`panda render` 因为不跑 TTS，是立刻报错。

**修法**：把 bg/bgm 的存在性检查提到 `run_tts` 之前。**为什么可以推迟**：报错
文案本身是对的，只是来得晚。

#### 5. 取整口径不一致，且没有一条测试用非整秒的 `content_frames`

`src/ffmpeg.rs` 里 `adelay` 的毫秒用 `.round() as i64`，`afade` 的 `st` 与 `-t`
用 `{:.3}`。两者在整秒输入下结果相同，而**现有测试的 `content_frames` 全是
30 的倍数**——把 `.round()` 换成截断不会有任何测试变红。

**修法**：补一条 `content_frames` 非 30 倍数的用例（例如 `A = 10.01` →
`content_frames = 361` → Outro 起点 `16.0333…s`），把口径钉死。

#### 6. `run_render_does_not_panic_when_ffmpeg_exits_early` 经七次变异零响应

实证零鉴别力的测试比没有测试更糟：占测试计数、给虚假信心、还要花时间起一个
假 ffmpeg 子进程。**修法**：要么删，要么改成断言 broken-pipe 这一条具体路径。
注意 `run_render_reports_write_failure_even_when_ffmpeg_exits_zero` 已经在测更
强的性质（ffmpeg 退出码 0 但写帧失败时错误不能被吞），**直接删可能更干净**。

### 可以不做

#### 1. `overlay` 合成会让个别帧的可见内容晚一帧出现（约 33ms），根因已找到，不是 h264 编码伪影

Task 8 端到端报告 §4.2 记录过一个异常：Intro 逐字打出动画理论上（也经
渲染器实测确认）应在全局帧 24/33/41/50/**58**/**67**/75 首次出现新字符，
但成片的相邻帧差尖峰有两处晚了一帧，落在 **59**/**68**。修复轮 2 复现并
定位了根因（方法：固定同一标题「端到端验收标题」，跳过网络 TTS，用静音
音频 + 极简 VTT 复现同一条 Intro 时间轴，四组对照实验）：

1. **Rust 侧写出的原始 RGBA 帧流本身完全正确**：直接把
   `FrameSource::write_rgba_frames` 的输出落盘（不经过 ffmpeg），160×90
   降采样后逐帧算平均绝对差，峰值精确落在 24/33/41/50/**58**/**67**/75，
   和渲染器/理论值完全吻合，没有错位。
2. **不是 h264 有损编码的伪影**：把生产 `-crf 23` 换成无损的 `-crf 0`
   重新编码同一条帧流（背景视频 + `overlay` 一并保留），峰值**依然**落在
   59/68——如果是编码/去块滤波抹平了差异，无损编码下不该复现；它复现了，
   说明编码质量不是原因。
3. **只有接了 `overlay` 才会错位**：把同一条原始 RGBA 帧流跳过背景视频
   合成、直接编码（`-vf format=yuv420p` 之后 `-c:v libx264 -crf 23`），
   峰值精确回到 24/33/41/50/**58**/**67**/75，无错位。
4. **结论**：一帧的错位是 `overlay` 滤镜把 stdin 的 rawvideo 帧流与循环
   播放的背景视频（AV1 素材本身帧率不规则，§4.2 已记录过它按约 5 帧周期
   自带重复帧）做时间戳对齐时引入的，具体是 ffmpeg/`overlay` 内部哪一步
   的时间戳配对逻辑导致，**没有继续往 ffmpeg 内部深挖**。

**为什么可以不做**：受影响的两处（本次复现里是第 5/6 次字符出现）本身
在渲染器里就与相邻帧几乎没有视觉差异（同一秒内打字机继续打字，光标闪烁
之外没有其它变化），一帧≈33ms 的错位肉眼不可辨；且不会累积——后续帧
（含 67→75 这一段）都精确对齐，说明它是局部的、自我纠正的，不是整条
时间轴的系统性漂移。真正要花力气的是往 ffmpeg/`overlay` 内部去确认具体
机制，性价比不高。**若未来发现累积性的帧错位、或错位在视觉上变得可辨**
（比如背景换成帧率更不规则的素材、或字符逐帧变化更密集），需要重新评估。
