# 素材来源与授权

这份文档服务一件具体的事：**在发片之前，能查到每一样素材是哪来的、能不能用。**

先破一个常见误解：**「无版权素材」基本不存在**。绝大多数素材都有版权，区别只在
授权条款给了你什么。真正要确认的是三件事：

1. **能不能商用**——注意「商用」的定义各站不同，接广告、带货、甚至只是开了创作
   激励，都可能被算作商用。
2. **要不要署名**——CC-BY 类授权必须署名，漏了就是违约，哪怕素材是免费下的。
3. **能不能再分发**——这条最容易踩。把字体编进二进制、把素材打包进仓库，都属于
   再分发，很多「免费可商用」的授权其实不允许。

---

> **换音效素材时必须核对声道数。** 2026-09-05 实测发现：`assets/intro.mp3`
> 是单声道，走当时四路共用的 `channel_layouts=stereo` 会吃到 swresample 的
> 功率保持上混，比规格轻 3 dB，而**没有任何测试会变红**（断言看的是滤镜图
> 字符串，声道数不在里面）。滤镜图现在按 `ffprobe` 探到的声道数逐路选写法
> （见 `docs/ffmpeg-pipeline.md` §14），换素材因此不再改变电平——但换之前
> 仍应知道自己换的是几声道。


## 1. 本项目自带的素材

| 素材 | 文件 | 授权 | 是否随二进制分发 |
|---|---|---|---|
| 正文字体 | `assets/LXGWWenKaiLite-Regular.ttf` | **SIL OFL 1.1**（见 `assets/LICENSE-LXGWWenKai.txt`） | 是（`include_bytes!`） |
| Logo | `assets/logo.png`（源 `assets/logo.svg`） | **ISC**（Lucide，见 `assets/LICENSE-lucide.txt`） | 是 |
| 片尾音效 | `assets/intro.mp3` | **CC0**（Freesound #406243） | 是 |
| 打字机音效 | `assets/intro_typewriter.mp3` | **CC0**（Freesound #455044） | 是 |
| 背景视频 | `public/video/0.mp4` | 用户自备 | 否（`.gitignore`） |
| 背景音乐 | `public/bgm/0.mp3` | 用户自备 | 否（`.gitignore`） |

### 字体：霞鹜文楷 Lite

- 项目主页：https://github.com/lxgw/LxgwWenKai
- Lite 变体：https://github.com/lxgw/LxgwWenKai-Lite
- 授权：SIL Open Font License 1.1 —— 明确允许商用、嵌入、修改与再分发
- **OFL 的硬性要求**：分发字体时必须附带许可证原文，本仓库放在
  `assets/LICENSE-LXGWWenKai.txt`。**删掉它就构成违约**，改动打包方式时注意。
- 保留字体名：`霞鹜`/`霞鶩`/`落霞孤鹜`/`落霞孤鶩`/`LXGW`。若你修改字体源码后再
  分发，不能继续用这些名字（作者对「重编译未改源码」和「子集化/转 WOFF 供网页
  使用」给了额外豁免，细则见许可证原文）。

> **历史记录**：本项目此前内嵌的是 `dingliesongtypeface.ttf`，其 name 表中只有
> 「Copyright reserved by MiaoKeming」，**没有任何授权声明**。因该字体随二进制
> 分发、且原文件进了公开版本控制，于 2026-09-05 替换为霞鹜文楷 Lite。附带收益：
> 字形覆盖从 7505 个码位增至 25598 个（CJK 基本汉字 32.2% → 87.8%）。

**体积账**（2026-09-05 实测，发布 crates.io 前量的）：

| | |
|---|---|
| 字体原始 | 13.23 MiB（25,985 个字形，`glyf` 表 12.57 MiB，平均每字形约 507 字节）|
| gzip -9 后 | 7.81 MiB —— 占整个 `.crate`（8.44 MiB）的 **93%** |
| crates.io 上限 | 10 MiB，余量 1.56 MiB |
| release 二进制 | 26.67 MiB（strip 后 23.80）|

**结论：不换、也不子集化。** 同族里 Lite 已经是最小的——官方 Regular 25.6 MB、
GB 变体 25.8 MB（GB 是覆盖更全，不是更小）。自己子集化到常用 3500 字能把 `.crate`
压到 2 MiB 量级（按每字形 507 字节估算，**未实测**），OFL 也明确豁免了子集化，
但对这个工具是坏交易：它吃的是**任意**口播文稿，而渲染器刻意禁用了系统字体回退、
且不检测缺字形——三者叠加，子集化等于把「任何文稿都能出片」换成「少数文稿悄悄出
豆腐块」：不报错、能播放，要人眼看到成片才发现。

真撞上 10 MiB 那天的顺序：① **zstd 预压缩内嵌 blob**、启动解压（实测 zstd -19 =
5.94 MiB，比 gzip 少 1.87 MiB，**字形一个不少**，代价是多一个依赖与启动解压）；
② 子集化，**且同时补上缺字形检测**；③ 不内嵌、强制 `--font`（毁掉开箱即用）。

### Logo：Lucide `feather`

- 图标集：https://lucide.dev （源码 https://github.com/lucide-icons/lucide ）
- 授权：**ISC** —— 极宽松，允许商用、修改、再分发，只需保留版权声明
  （原文在 `assets/LICENSE-lucide.txt`）
- 改动：把 `stroke="currentColor"` 固化为墨色 `#171717`，viewBox 四周各加 2 单位
  留白。源文件保留为 `assets/logo.svg`
- `assets/logo.png` 是它光栅化到 512×512 的产物。要换图或改尺寸：

  ```bash
  cargo run --release --example rasterize_icon -- assets/logo.svg assets/logo.png 512
  ```

  内嵌 logo 走 PNG 路径（`logo_rgba()`），外部 `--logo` 才支持直接给 SVG。
  512 是够用的尺寸：logo 最大显示于横版片尾，`outro_logo_size` 216 × 1.5 = 324px。

### 两段音效：Freesound CC0

两段都取自 [freesound.org](https://freesound.org) 上标记 **Creative Commons 0** 的
条目。CC0 等同于放弃著作权、进入公有领域：允许商用、修改、**再分发**，且**不要求
署名**——最后两点正是内嵌素材需要的（理由见下）。

| 用途 | 文件 | 来源 | 作者 | 原始时长 |
|---|---|---|---|---|
| 打字机 | `assets/intro_typewriter.mp3` | [Freesound #455044](https://freesound.org/s/455044/) | escritor1 | 27.75s |
| 片尾「叮」 | `assets/intro.mp3` | [Freesound #406243](https://freesound.org/s/406243/) | stubb | 1.056s |

两者同源于打字机，片头打字、片尾换行铃，听感上是一套。

**加工方式**（原始文件未入库，按下述步骤可从源头复现）：

```bash
# 打字机：从 27.75s 的录音里截取 8.0~11.157s，两端各 5ms 淡入淡出防爆音
ffmpeg -ss 8.0 -t 3.157 -i 455044__escritor1__typewriter-typing.wav \
  -af "afade=t=in:st=0:d=0.005,afade=t=out:st=3.152:d=0.005" \
  -codec:a libmp3lame -b:a 128k -ar 48000 -ac 2 intro_typewriter.mp3

# 片尾「叮」：原样转码，尾部本就自然归零，只补 10ms 淡出
ffmpeg -i 406243__stubb__typewriter-ding_near_mono.wav \
  -af "afade=t=out:st=1.046:d=0.010" \
  -codec:a libmp3lame -b:a 128k -ar 48000 -ac 1 intro.mp3
```

**为什么截 8.0~11.157s**：逐 0.5s 量过整段录音的 RMS，稳定连打的区段是 1.0~5.5s、
7.0~12.5s、13.5~16.0s、23.0~26.0s。8.0s 起的七个窗口 RMS 全在 -38~-41dB 之间、
没有停顿缺口，是最匀的一段。3.157s 这个长度沿用替换前的时长，使改动只涉及素材本身。

**采样率取 48kHz** 而非此前的 44.1kHz：滤镜图的 `MIX_FORMAT` 本就把所有支路统一到
48kHz（`src/ffmpeg.rs`），源文件也是 48kHz，直接对齐可少一次重采样。

> **历史记录**：2026-09-05 这两段曾短暂用过 `tools/gen_sfx.py` 的合成音，原因是
> 当时误判「音效站不可达」（见下）。该脚本仍保留在仓库里可供参考，但**已不是**
> 内嵌音效的来源。

#### ⚠️ 内嵌素材的授权门槛比自用素材高

这是本文档开头列的第三条（「能不能再分发」）在本项目里的具体落点，值得单独说：

| 用途 | 素材去向 | 授权要求 |
|---|---|---|
| `--bg` / `--bgm` | 只进你自己的成片 | 「免费商用」即可 |
| `assets/` 下的内嵌素材 | `include_bytes!` 进二进制，随构建产物分发；原文件还在公开仓库里 | **必须明确允许再分发** |

多数免费音效站（Mixkit、Pixabay 等）的条款是「可用于你的项目」，同时**禁止把素材
本身作为独立文件再分发**。把 mp3 编进一个发布到 GitHub 的二进制，很难说不算再分发。

所以 §4 那张表对 `--bgm` 完全适用，对 `assets/` 下这三项则不然。内嵌素材要找的是
CC0 / OFL / MIT / ISC 这类**明确授予再分发权**的授权——本项目现在的字体（OFL）、
logo（ISC）、音效（CC0）都满足，这不是巧合。

**CC-BY 也允许再分发**，技术上可用，但署名义务会传染到成片：你发布的每一支视频都
包含那段素材，严格讲每支都要带署名。对持续产出的管线这是一条永久的运营负担，
所以本项目选 CC0。

#### 一次判断错误的更正（2026-09-05）

替换素材时我判定「音效站在本机网络环境下不可达」，并据此改用合成音、写进了提交
信息和本文档。**这个判断是错的。** 起因是拿一条**猜测的** CDN 直链去探测，失败后
就外推为整站不可达，没有从页面里取真实链接复核。当天实测：

| 站点 | 实测 | 说明 |
|---|---|---|
| `mixkit.co` | **HTTP 200** | 可达；从页面取真实直链后，`assets.mixkit.co` 上的 mp3 **下载成功** |
| `freesound.org` | **HTTP 200** | 可达；网页搜索结果页即含条目信息，**无需 API key 即可检索** |
| `pixabay.com` / `cdn.pixabay.com` | **HTTP 403** | 确实被代理拒绝 |

三站里只有 Pixabay 真的不可达。更正后即从 Freesound 取到了 CC0 素材替换合成音。
**教训与本仓库对变异验证的那条一样：探测失败时先确认探测本身有效，再下结论。**

---

## 2. 免费可商用的中文字体

以下都允许商用与嵌入分发，是可以直接替换 `assets/` 里那份的候选。**OFL 类授权最稳**
——条款白纸黑字，且明确覆盖嵌入场景。

| 字体 | 风格 | 授权 | 地址 |
|---|---|---|---|
| **霞鹜文楷** | 楷体 | SIL OFL 1.1 | https://github.com/lxgw/LxgwWenKai |
| **思源黑体** (Source Han Sans) | 黑体 | SIL OFL 1.1 | https://github.com/adobe-fonts/source-han-sans |
| **思源宋体** (Source Han Serif) | 宋体 | SIL OFL 1.1 | https://github.com/adobe-fonts/source-han-serif |
| **落霞孤鹜** (LXGW Bright) | 宋体 | SIL OFL 1.1 | https://github.com/lxgw/LxgwBright |
| **得意黑** (Smiley Sans) | 黑体/标题 | SIL OFL 1.1 | https://github.com/atelier-anchor/smiley-sans |
| **未来荧黑** (Glow Sans) | 黑体 | SIL OFL 1.1 | https://github.com/welai/glow-sans |
| **寒蝉系列** | 多种 | 各版本不同，**逐款看** | https://github.com/GuiWonder |
| **阿里巴巴普惠体** | 黑体 | 免费商用（官方声明） | https://fonts.alibabagroup.com |
| **HarmonyOS Sans** | 黑体 | 免费商用（官方声明） | https://developer.harmonyos.com/cn/design/resource |
| **站酷系列**（快乐体/小薇/庆科黄油等） | 多种 | 免费商用（官方声明） | https://www.zcool.com.cn/special/zcoolfonts/ |
| **MiSans** | 黑体 | 免费商用（官方声明） | https://hyperos.mi.com/font |

**选型提示**：

- **要嵌入二进制就优先选 OFL**。厂商自定义的「免费商用」声明多数没有明确提到嵌入
  和再分发，法务上不如 OFL 干净。
- **注意字重和体积**。中文字体动辄一二十 MB，思源系列全字重包上百 MB。本项目用
  合成粗体（不依赖 Bold 字体文件），所以单一 Regular 字重就够。
- **注意字形覆盖**。只覆盖 GB2312（6763 字）的字体遇到罕用字会出豆腐块。可以用
  `docs/` 里记的办法解析 cmap 表实测，别只看宣传。

---

## 3. 免费视频素材站

用于 `--bg` 的循环背景视频。

| 站点 | 授权要点 | 地址 |
|---|---|---|
| **Pexels** | 免费商用、无需署名。禁止原样转售素材本身；禁止用可辨识人物暗示背书 | https://www.pexels.com/videos/ |
| **Pixabay** | 免费商用、无需署名，条款与 Pexels 相近，量大 | https://pixabay.com/videos/ |
| **Mixkit** | 免费商用、无需署名（Envato 旗下）。禁止把素材本身作为主要卖点再分发 | https://mixkit.co/free-stock-video/ |
| **Coverr** | 专做网页/视频背景，正对本项目用途 | https://coverr.co |
| **NASA** | 美国政府作品属公有领域。科技/星空/地球类背景很好用 | https://images.nasa.gov |
| **Videvo** | **混合站**：部分免费需署名、部分付费。必须逐条看授权标签 | https://www.videvo.net |
| **Internet Archive** | **混合站**：公有领域与各类授权混杂，逐条看 | https://archive.org |

前四个最适合本项目——都是免署名商用，拿来即用。

**⚠️ 对国内素材站的提醒**：多数国内站走「免费下载、商用另需授权」的模式，下载页
显眼处写着「免费」，商用授权藏在会员条款里，事后追责的案例不少。用之前务必找到
明确的授权页面，别只看下载按钮旁边那行字。

---

## 4. 免费音乐素材站

用于 `--bgm` 的背景音乐。

| 站点 | 授权要点 | 地址 |
|---|---|---|
| **Pixabay Music** | 免费商用、无需署名，最省心 | https://pixabay.com/music/ |
| **YouTube 音频库** | 免费，部分曲目需署名。**可用于 YouTube 之外的平台** | https://studio.youtube.com （需登录，创作者工具内） |
| **Musopen** | 古典乐，公有领域录音 | https://musopen.org |
| **Uppbeat** | 免费档需署名，付费档免署名 | https://uppbeat.io |
| **Free Music Archive** | **混合 CC 授权，逐曲看**。别整站当免费 | https://freemusicarchive.org |
| **Freesound** | 学术机构运营，**逐条标注具体 CC 授权**（含 CC0）。下载需注册；申请 API key 后可程序化取用 | https://freesound.org |
| **ccMixter** | 混合 CC 授权，逐曲看 | http://ccmixter.org |
| **Incompetech** (Kevin MacLeod) | CC-BY，**必须署名**。质量好但见下方警告 | https://incompetech.com/music/royalty-free/ |
| **Bensound** | 免费档需署名 | https://www.bensound.com |

### ⚠️ 名字撞车：`freesound.org` 不是 `freesound.cn`

上表里的 **Freesound 指 `freesound.org`**——西班牙 Universitat Pompeu Fabra 运营的
音频库，每个条目**逐条标注具体的 CC 授权**，能按 CC0 过滤。这是本文档推荐它的唯一
理由：CC0 明确允许再分发，是内嵌素材少数几种可用的授权之一。

**`freesound.cn` 是另一家完全不相干的商业站**（「FREESOUND 飞声无版权音乐库」，
后端 API 在 `freesound-api.gongyier.com`），只是名字撞了。2026-09-05 实测：

- 它的宣传语是**「无版权音乐库」**——正是本文档开头破的那个说法，没有素材是「无版权」的
- 站点是纯前端渲染，**条款正文取不到**，因此本文档**不对它的授权模式做任何判断**
- 就算它的条款允许商用，那也只是「你自己用」这一档；`assets/` 下的内嵌素材要的是
  **明确的再分发授权**，而国内素材站几乎从不授予这个

要用 Freesound，认准 **`.org`**。

### ⚠️ 最容易被忽略的坑：合规 ≠ 不被平台判侵权

**「授权上合规」和「不被平台的版权识别系统拦下」是两回事。**

即使你用的是 CC0 音乐，也可能被第三方抢先注册进版权识别库（Content ID 及各平台的
同类系统）。结果是：你在抖音／B站／视频号／YouTube 发片时收到版权提示、被限流、
或者收益被划走——**你在法律上没有问题，但片子已经被限了**，申诉要时间。

Incompetech 这类被海量使用的曲库尤其容易撞上。

两个规避方向：

- **发哪个平台就用哪个平台自带的曲库**。站内使用最安全，代价是只能站内用。
- **付费订阅**（Epidemic Sound、Artlist、Soundstripe 等）。它们提供授权凭证，
  通常还提供版权识别的白名单／申诉通道。

判断标准很简单：**自用或小范围分享，免费站完全够**；一旦要商业化或规模分发，
付费订阅省下的申诉时间远比订阅费值钱。

---

## 5. 换素材的操作方式

背景视频和背景音乐都是路径参数，换素材零成本：

```bash
cargo run --release -- render \
    --audio output/tts/audio.mp3 \
    --vtt   output/tts/audio.vtt \
    --bg    /path/to/your/background.mp4 \
    --bgm   /path/to/your/music.mp3
```

也可以走环境变量 `BG_VIDEO` / `BGM_FILE`。完整的配置表见 `README.md`。

换内嵌字体则需要改 `src/assets.rs` 的 `include_bytes!` 路径并重新编译，同时注意：

- 附上新字体的许可证文件
- 逐字节回归门禁（`tests/canvas_baseline.rs`）会全红，这是**预期行为**。按该文件
  里写明的流程处理：删掉 `tests/baseline/` 重新生成，并在提交信息里说明理由
- 字形度量相关的测试常量（如 `COVER_WM_INK_WIDTH_PX`）需要重新实测。**重新实测
  之后要验证判据没有因此失去鉴别力**——改动被测参数，测试仍须变红

换 logo 或音效同理。2026-09-05 那次替换连带动了三条判据，都不是「把数字改到通过」：

- `embedded_assets_are_non_empty` 里 `LOGO_PNG.len() > 100_000` 的门槛是照旧那张
  1.33MB 位图定的，新 logo 只有 20.5KB。**换成了解码判据**
  `logo_decodes_to_the_expected_size_and_has_ink`——校验解码尺寸与非透明像素数，
  不依赖体积。已验证截断、尺寸不同步、全透明三种变异都能抓住
- `outro_logo_diameter_...` 用墨迹纵向跨度代替直径，前提是旧 logo 为填满方框的
  圆形。羽毛是斜向线稿（跨度 175/216），**判据改为形状无关**：满尺寸占比落在
  55%~100%，起始缩放用两帧跨度之比判定
- 音效尺寸哨兵的方向翻转了（新素材同档编码，时长长的更大），断言随之改写并重新
  验证仍能抓住 `include_bytes!` 路径对调

---

## 6. 免责

本文档整理的是**截至 2026-09-05** 各站的授权条款要点。**这些站改条款不算罕见**，
正式发布前请以各站官网当前版本为准。本工具不对素材来源做任何检查或担保。
