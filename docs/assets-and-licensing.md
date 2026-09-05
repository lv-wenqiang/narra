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

## 1. 本项目自带的素材

| 素材 | 文件 | 授权 | 是否随二进制分发 |
|---|---|---|---|
| 正文字体 | `assets/LXGWWenKaiLite-Regular.ttf` | **SIL OFL 1.1**（见 `assets/LICENSE-LXGWWenKai.txt`） | 是（`include_bytes!`） |
| Logo | `assets/logo.png` | **未记录** ⚠️ | 是 |
| 片尾音效 | `assets/intro.mp3` | **未记录** ⚠️ | 是 |
| 打字机音效 | `assets/intro_typewriter.mp3` | **未记录** ⚠️ | 是 |
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

### ⚠️ 三项待补的来源记录

`logo.png` 和两段音效的来源与授权**没有记录**，而它们和字体一样是编进二进制分发的。
如果你打算公开发布成片或分发构建产物，这三项需要补上来源确认；来源不明的话，最省
事的做法是用下面列出的免费素材站替换掉。

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
| **ccMixter** | 混合 CC 授权，逐曲看 | http://ccmixter.org |
| **Incompetech** (Kevin MacLeod) | CC-BY，**必须署名**。质量好但见下方警告 | https://incompetech.com/music/royalty-free/ |
| **Bensound** | 免费档需署名 | https://www.bensound.com |

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

---

## 6. 免责

本文档整理的是**截至 2026-09-05** 各站的授权条款要点。**这些站改条款不算罕见**，
正式发布前请以各站官网当前版本为准。本工具不对素材来源做任何检查或担保。
