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

### 已销账

- **画布尺寸与帧率同时存在于两个文件、互不引用**（原「值得做」第 3 条，
  帧渲染子系统补充部分）。两半现在都已收敛：

  - **尺寸半**：`docs/superpowers/plans/2026-09-04-canvas-metrics-refactor.md`
    收敛为单一的值类型 `render::canvas::Canvas`（`BASE`/`scale()`/`w_f32()`/
    `h_f32()`），`Metrics::for_canvas` 接管全部 44 个字段（1 个 `canvas` +
    27 个按 `scale` 缩放的长度量纲字段 + 2 个语义字段 + 14 个宽/高直接派生
    的位置/容器尺寸字段；44 是总字段数，**不是 44 个字段都是宽/高派生量**，
    真正由宽/高直接派生的只有 14 个），`Painter` / `FrameSource` /
    `RenderInputs` 都改成显式持有 `Canvas`。中间步骤曾让 `timeline::WIDTH`/`HEIGHT` 从 `Canvas::BASE`
    派生（而不是直接删除）作为过渡；等到 `draw.rs` 的 `CANVAS_W`/`CANVAS_H`
    也全部改用 `Metrics` 之后，这两个 `pub` 重导出在 `src/` 与 `tests/` 里
    已经没有任何读者，只是靠 `pub` 躲过了 `dead_code` 检查——发现后直接
    删除，见 `src/render/timeline.rs` 的删除记录。变异验证：把
    `Canvas::BASE` 改错一格（`1280`→`1281`），
    `render::canvas::tests::base_matches_the_pre_refactor_hardcoded_size`
    与 `cargo test --test canvas_baseline` 两侧同时变红。
  - **帧率半**（终审修复波补做，源码文本钉住已在 Task 4 补全）：`src/render/timeline.rs` 的
    `pub const FPS: u32 = 30` 与 `src/render/draw.rs` 曾各写一份独立的
    `const FPS`，互不引用——只改 `timeline::FPS` 会让 `layout()`/ffmpeg 的
    `-r`/`INTRO_START_SECS`/`CONTENT_START_SECS` 跟着变，而 `draw.rs` 里
    打字机、光标闪烁、Outro 各阶段的动画窗口仍按旧帧率的常量走，动画整体
    变速甚至错位，且没有任何测试把两者摆在一起比。现在
    `draw::FPS`（`pub(crate)`）直接写成
    `crate::render::timeline::FPS as f64`，从源码结构上排除再分叉的可能；
    `src/ffmpeg.rs` 新增跨模块测试
    `draw_fps_derives_from_the_timeline_single_source_of_truth` 钉住这个
    关系，写法与风格同本文件旁边 `segment_starts_match_the_audio_delays_in_the_filter_graph`
    一致。变异验证：暂时把 `draw::FPS` 改回独立字面量 `30.0`、把
    `timeline::FPS` 改成 `60`，该测试从 `ok` 变 `FAILED`
    （`left: 30.0, right: 60.0`）；两处都改回后重新变绿。
    
    **源码文本层补全**（Task 4）：上述运行期断言虽然守住了「两处取值分歧」，
    但若有人把 `draw::FPS` 的推导改回字面量 `30.0`、保持数值不变，断言仍会
    通过（因为 `30.0 == 30 as f64`）——这正是双真相源重新长出来的方式。
    新增 `tests/fps_single_source.rs` 以源码文本比对补这个盲点：
    `draw_fps_is_written_as_a_derivation_not_a_literal` 保证 `draw.rs` 里
    出现的是 `const FPS: f64 = crate::render::timeline::FPS as f64;`，
    `draw_rs_has_no_hardcoded_frame_rate` 保证不出现写死的 `const FPS: f64 = 30.0;` 或
    `const FPS: f64 = 30f64;`。变异验证对比结果：同一变异（把推导改成 `30.0`）下，
    源码文本测试两条全红 ❌，而原先的运行期断言仍然通过 ✓——这正是 F-4 记录的
    那个盲区，也是新增源码文本测试存在的理由。

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

（画布尺寸/帧率的双真相源部分已销账，见上面「已销账」。）

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
- **`Painter::new()` 把 2048² 的 logo 解码了两遍**（原「值得做」第 3 条）。
  logo 改成可由 `--logo` 覆盖时顺手做掉：新增 `LogoSource`（解码 + 预乘一次），
  36px 与 216px 两个尺寸各从它缩一次。原实测单次解码 18ms、两次 36ms，占
  `new()` 总耗时 132ms 的四分之一还多。**条目里附带的「换 256px 预缩版省
  1.3MB 二进制」没做**：内嵌那张现在只是默认值，用户可以给自己的文件，把
  默认图换小是独立的一件事。**这条建议现在确定不做**：计划 B 的 1920×1080
  横版模式需要一枚 324px 的 Outro logo，256px 的预缩版不够用；内嵌原图
  2048² 在 BASE（36px/216px）与计划 B（324px）两档尺寸下都够缩，换成更小
  的预缩版反而会在计划 B 落地时不够用。

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

### 记一笔：Outro 的留白是对称的，不是缺口（2026-09-04，已核实）

去掉 Outro 底行的工具推广之后，一度记成「`logo + 品牌名` 之下空出约 200px」。
**实测推翻了这个说法**（逐像素扫描墨迹包围盒，`draw_outro` 第 60 帧）：

| | 墨迹 y 范围 | 上留白 | 下留白 |
|---|---|---|---|
| 默认（无水印） | 194–520 | **194px** | **199px** |
| 配了 `--watermark-cover` | 194–588 | 194px | 131px |

默认状态**上下留白差 5px，基本对称**——`draw_outro` 里那一组本来就是
`group_top = CANVAS_H / 2.0 - group_h / 2.0`，垂直居中于画布中心。水印原先是
往下半部**加**东西、让构图偏下；去掉它反而**恢复了对称**，而不是留下缺口。

**结论：无需处理**，渲染代码不动。这条留在这里是为了记住那个错误的判断从哪
来的——「少了一个元素」与「构图失衡」是两回事，前者不蕴含后者，凭肉眼印象
下结论之前应该先量一遍。

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
- **族 A：清理代码写在可能被跳过的位置（三处同根因）**（原「值得做」第 1 条）。
  新增 `src/tmp.rs` 的 `TempPath` 作用域守卫，`Drop` 里删路径，三处一起解决：
  生产路径的 `compose_video_with_runner`（`create_dir_all` 与清理之间夹着两个
  `?` 出口，走那两条时临时目录连同两个 mp3 约 120 KB 泄漏在 `/tmp`）、
  `write_fake_ffmpeg` 交出的脚本、以及三条假 ffmpeg 测试的产物目录（它们经
  `run_with_timeout` 跑闭包，闭包 panic 或超时时 `run_with_timeout` 自己
  `panic!`，写在测试尾部的清理执行不到——**越是抓到了 bug，越会留下垃圾文件**）。
  新增 `compose_video_cleans_up_the_tmp_dir_on_every_exit_path` 覆盖三条出口
  （成功 / Err / **runner panic**），判据是让注入的 runner 从
  `render_inputs.intro` 捞出临时目录路径，返回后断言那个具体路径不存在——
  确定性，不靠扫 `/tmp` 找残留。
- **族 B：四个素材路径的「空白视同未设置」语义无覆盖**（原「值得做」第 2 条）。
  照 `tests/config_env.rs` 既有的串行化环境变量夹具补了
  `blank_material_env_vars_are_treated_as_unset`。变异验证：把 `non_empty_env`
  换成裸 `std::env::var().ok()` → 该文件 9 条全红（此前该变异存活）。
- **反预乘慢路径**（原「值得做」第 3 条）。`unpremultiply_into` 对
  `alpha == 0` 直接写四个 0——`demultiply()` 只对 `alpha == 255` 短路，而
  Content 段是透明底，绝大多数像素正是这一支。**本机实测 600 帧 1.49s →
  0.38s，3.89×**（欠账本原记的是 6× / 1.6s → 0.26s，这里记实测值）。等价性由
  `unpremultiply_fast_path_is_byte_identical_to_plain_demultiply` 保证：拿四段
  各一帧真实渲染帧，对着未优化的参考实现逐字节比对。
- **取整口径不一致**（原「值得做」第 5 条）。补了两条非整秒 `content_frames`
  的用例：`content_frames=362` → Outro 起点 `16.0666…s`，四舍五入 16067 /
  截断 16066，能区分；另一条钉住 `afade` 的 `st` 保留三位小数不被取整。
  变异验证：`.round()` 换截断 → 1 条红；`st` 也按秒取整 → 4 条红。
- **`run_render_does_not_panic_when_ffmpeg_exits_early` 经七次变异零响应**
  （原「值得做」第 6 条）。**已删**。它与
  `run_render_reports_ffmpeg_stderr_verbatim_when_it_exits_nonzero` 用同一套坏
  输入、走同一条代码路径（ffmpeg 立刻退出 → 写端撞上已关闭的管道），但只断言
  `result.is_err()`；后者断言错误必须原样透出 ffmpeg 的 stderr，严格更强，且
  写端若真的 panic 或挂死，它一样会 panic 或超时。职责已写进后者的文档注释。
- **`Commands::Make` 这个 match arm 自身零覆盖**（原「族 B」第 2 小条）与
  **`make` 在整条 TTS 跑完之后才校验素材存在**（原「值得做」第 4 条）。
  **两条一起销**：`panda make` 连同它那层胶水移到了仓库根的 `justfile`。
  那层胶水（跑 TTS → 拼产物路径 → 转手调用合成）在二进制里既没有可注入的
  接缝、也没有一条测试，而它调用的每一段单独都已经有测试；搬到 just 之后它
  不在二进制里了，「零覆盖」自然消失。校验时机同时修好：配方在跑 TTS **之前**
  先检查 bg/bgm 存在，`--bg` 打错一个字不必先付一整轮 Edge TTS 网络往返。
  `justfile` 里镜像的默认值由 `tests/justfile_defaults.rs` 与 `src/config.rs`
  逐条比对，改一边不改另一边会变红。
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

（本子系统的欠账已全部销清，见上面「已销账」。）

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

---

## 素材与授权

### 已销账

#### 1. `logo.png` 与两段音效的来源与授权没有记录 —— 2026-09-05 销

三项都是 `include_bytes!` 编进二进制、随构建产物分发的，此前来源与授权均无记录，
与替换前的字体处境相同。已全部替换为来源明确的素材：

- **logo**：Lucide `feather`（**ISC**），源 SVG 与许可证一并入库；`logo.png` 是它
  光栅化到 512×512 的产物，1.33MB → 20.5KB
- **两段音效**：取自 Freesound 上标记 **CC0** 的条目——打字机 [#455044](https://freesound.org/s/455044/)
  （escritor1），片尾「叮」[#406243](https://freesound.org/s/406243/)（stubb）。两者同源于
  打字机，片头打字、片尾换行铃，听感上成一套

逐项授权与复现方式见 `docs/assets-and-licensing.md` §1。

**连带动到的三条判据**（都不是「把数字改到通过」，各自重新验证了鉴别力）：
`LOGO_PNG.len() > 100_000` 的体积门槛换成解码判据；`outro_logo_diameter_...` 的
墨迹跨度判据改为形状无关；音效尺寸哨兵方向翻转后重写。详见该文档「换素材的操作
方式」一节。

**听感问题已解决**：起初用的是 `tools/gen_sfx.py` 的合成音，因为当时误判「音效站
不可达」；复核推翻该判断后，改用 Freesound 的 CC0 真实录音，由人工挑选试听确认。

**选 CC0 而非 CC-BY 的理由**：两者都允许再分发（这是内嵌素材的硬门槛），但 CC-BY 的
署名义务会传染到成片——每发一支视频都要带署名，对持续产出的管线是永久负担。
