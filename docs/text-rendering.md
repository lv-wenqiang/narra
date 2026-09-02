# CJK 排版 + 字形轮廓描边 —— 实测记录

> 本文档记录的是 **2026-09-02 在本仓库 `examples/text_probe.rs` 中实际跑通** 的写法，
> 已对照实际安装到的 crate 版本源码（`~/.cargo/registry/src/*/`）逐一核实，
> 不是照抄 `task-0-brief.md` 里"仅为说明意图"的伪代码。Task 4 实现时应直接照抄本文档。

## 结论

**路线可行：`cosmic-text` 负责排版，`ttf-parser` 从字体原始字节里取字形轮廓，
转成 `tiny-skia::Path` 后描边+填充。** brief Step 5 的排查阶梯用到了第 3 条
（描边宽度需要在变换到像素空间之后再应用），其余没用到。

验证结果（`cargo run --example text_probe`）：
- 主探针文本「熊猫智研社 Test 123」：14 个字形，其中 12 个取到非空轮廓（2 个是空格，
  正常没有轮廓）；近纯白像素 10121 个、近纯黑像素 8073 个（描边和填充都生效）；
  非透明前景包围盒 651×71px（9 个可见字符，70px 字号，在预期的 500~800px 量级内）。
- 肉眼核对 `text_probe.png`：白字黑边的「熊猫智研社 Test 123」正确显示，中文笔画
  清晰可辨，不是豆腐块方框；描边均匀包裹字形，没有断裂或自相交伪影。
- **豆腐块专项核对**：另外分别渲染「熊猫」（`text_probe_cjk_a.png`）和「智研」
  （`text_probe_cjk_b.png`），逐字节比较两张同尺寸图片的像素数据——22800 字节不同
  （满打满算的 RGBA 图总字节数是 300×220×4=264000，约 8.6% 的像素不同，且集中在
  字形形状差异处），两张图各自都有黑/白像素。如果字体没被正确识别、两者都退化成
  `.notdef` 方框，这两张图会因为字形宽度、外形完全相同而逐字节相同——而实测并不
  相同，证明确实取到了「熊」「猫」「智」「研」各自不同的字形轮廓，不是同一个豆腐块
  被复制了四次。

## crate 版本

以 `Cargo.lock` 实际解析结果为准：

| crate | 版本 |
|---|---|
| `cosmic-text` | 0.19.0 |
| `ttf-parser` | 0.25.1 |
| `tiny-skia` | 0.12.0（依赖的 `tiny-skia-path` 也是 0.12.0） |
| `image` | 0.25.10（文字描边探针没直接用到，保留给 `assets::logo_rgba()` 用，见下方一节） |

`Cargo.toml` 通过 `cargo add tiny-skia cosmic-text ttf-parser image` 添加，未指定精确版本号
（走的是 `^` 语义化范围），后续任务如果要锁定这些版本，直接沿用现有 `Cargo.lock` 即可。

## 字体加载

字体文件：`assets/dingliesongtypeface.ttf`（从 `../panda-video-ts/public/fonts/` 复制，
5.6MB）。

```rust
const FONT_BYTES: &[u8] = include_bytes!("../assets/dingliesongtypeface.ttf");

let mut font_system = cosmic_text::FontSystem::new();
font_system.db_mut().load_font_data(FONT_BYTES.to_vec());
```

`FontSystem::db_mut()` 返回 `&mut fontdb::Database`；`fontdb::Database::load_font_data`
的签名是 `pub fn load_font_data(&mut self, data: Vec<u8>)`（`fontdb-0.23.0/src/lib.rs:195`），
接受的是拥有所有权的 `Vec<u8>`，不是借用。

## ⚠️ 系统字体静默回退陷阱（代码审查发现，务必照做规避方案）

**`FontSystem::new()` 会把运行机器上装的所有系统字体也注册进同一个 `fontdb::Database`，
且 `Shaping::Advanced` 在内嵌字体不覆盖某个字符时会静默回退到这些系统字体——不报错，
也不画内嵌字体自己的 `.notdef`。**

源码位置：`FontSystem::new()` → `new_with_fonts(empty)`
（`cosmic-text-0.19.0/src/font/system.rs:191-192`）→ `load_fonts(&mut db, empty)`
→ `db.load_system_fonts()`（`src/font/system.rs:433-438`，`std` feature 下）。这一行
和 `load_font_data(FONT_BYTES.to_vec())` 一样，都是把字体塞进*同一个* `db` 里，
之后 `cosmic-text` 的字体匹配逻辑（`FontMatchKey`）分不清哪些是"我特意内嵌的"、
哪些是"系统顺带装进来的"，缺字时会在整个 `db` 范围内挑一个能覆盖该字符的字体。

**为什么这对本项目是致命的**：项目的全局约束是"所有文字只用内嵌的
`dingliesongtypeface`，不内嵌第二套字体"。这个回退行为意味着**渲染结果长什么样
取决于运行机器上装了什么系统字体**——本机和别人的机器可能不一致，CI 环境和生产
环境也可能不一致，而且是静默的，没有任何报错或日志提示。真实文稿里出现内嵌字体
没覆盖的字符（生僻符号、某些全角标点、非中文字符）时就会撞上，观感上是"字体突然
变了"，排查起来毫无线索。

**实测复现**（`examples/text_probe.rs` 的 `resolve_glyphs()`；本机是 x86_64 Linux /
NixOS 环境）：用 `FontSystem::new()` + `load_font_data(FONT_BYTES)` 排版内嵌字体大概率
不覆盖的阿拉伯字母 `ا`（U+0627），`cargo run --example text_probe` 的实际输出：

```
系统字体回退核对：用内嵌字体大概率不覆盖的字符 "ا" (U+0627) 探测……
  1) FontSystem::new()（会 load_system_fonts）+ load_font_data：
     glyph_id=1365, font_id=ID(InnerId(1v1)), post_script_name=Some("DejaVuSans")
     [复现成功] 落到了内嵌字体（family=Some("dingliesongtypeface")）以外的字体上——
     cosmic-text 静默回退到了系统字体，而不是内嵌字体自己的 .notdef。
```

`glyph_id=1365` 是一个非零、有意义的字形 id，`post_script_name` 是
`"DejaVuSans"`——不是我们指定的 `family = "dingliesongtypeface"`，也不是 `.notdef`
（`glyph_id=0`）。证实了回退确实发生了。（本机装的系统字体恰好也是 DejaVu Sans，
和代码审查者在 Nix 环境下复现时看到的一致，但**这只是巧合**——换一台机器，回退
去的字体名字会不同，行为本身不会变。）

**规避方案（已实测跑通）**：不要用 `FontSystem::new()`，改成自己构造一个**从未调用
过 `load_system_fonts()`** 的 `fontdb::Database`，只塞内嵌字体，再用
`FontSystem::new_with_locale_and_db` 包起来：

```rust
fn font_system_without_system_fonts(font_bytes: &[u8]) -> cosmic_text::FontSystem {
    let mut db = cosmic_text::fontdb::Database::new();
    db.load_font_data(font_bytes.to_vec());
    cosmic_text::FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}
```

- `FontSystem::new_with_locale_and_db(locale: String, db: fontdb::Database) -> Self`
  （`cosmic-text-0.19.0/src/font/system.rs:276-278`）直接委托给
  `new_with_locale_and_db_and_fallback(locale, db, PlatformFallback)`，**完全不经过**
  `new_with_fonts()`/`load_fonts()`，所以不会调用 `load_system_fonts()`。
- `locale` 参数传字面量 `"en-US"` 就够：locale 只影响"缺字时按脚本/语言挑哪个系统
  回退字体"的排序（`Fallbacks::new`），既然我们的 `db` 里压根没有系统字体可回退，
  这个值不影响任何观察到的行为。
- `cosmic_text::fontdb` 是 `cosmic-text` 对 `fontdb` crate 的 re-export
  （`pub use fontdb;`，`src/font/system.rs:14`），不需要单独把 `fontdb` 加进
  `Cargo.toml` 依赖。

**验证规避方案生效**（同一个探针，同一个字符，换成上面这个构造函数）：

```
  2) font_system_without_system_fonts()（跳过 load_system_fonts）+ 同一个字符：
     glyph_id=0, font_id=ID(InnerId(1v1)), post_script_name=Some("dingliesongtypeface")
     [PASS] 落回了内嵌字体自己的 .notdef（glyph_id=0），没有回退到别的字体——规避方案有效。
```

`glyph_id=0` 就是 `.notdef`（内嵌字体自己的占位符），`post_script_name` 变回了
`"dingliesongtypeface"`——确认不再触碰任何系统字体。**Task 4 必须用
`FontSystem::new_with_locale_and_db`（或等价的、跳过 `load_system_fonts` 的构造方式）
来创建生产环境用的 `FontSystem`，不能直接用 `FontSystem::new()`。**

## 字体内部家族名（关键：解决"中文显示为豆腐块"的第一道检查）

用 `ttf-parser` 读字体的 `name` 表拿真实家族名，不要凭空猜测或用文件名代替：

```rust
fn family_name_from_ttf(data: &[u8]) -> Result<String> {
    let face = ttf_parser::Face::parse(data, 0)?;
    let mut fallback = None;
    for name in face.names() {
        if name.name_id == ttf_parser::name_id::FAMILY {
            if let Some(s) = name.to_string() {
                if name.is_unicode() && name.platform_id == ttf_parser::PlatformId::Windows {
                    return Ok(s);
                }
                fallback.get_or_insert(s);
            }
        }
    }
    fallback.ok_or_else(|| anyhow!("字体 name 表里没有可解码的 FAMILY(name_id=1) 记录"))
}
```

- `Face::names() -> name::Names<'_>` 实现了 `IntoIterator<Item = name::Name<'_>>`
  （`ttf-parser-0.25.1/src/lib.rs:1396`、`src/tables/name.rs:254`）。
- `name_id::FAMILY == 1`（`src/tables/name.rs:17`）。
- `Name::to_string() -> Option<String>` 只支持 Unicode 编码（Unicode 平台 / Windows 平台
  + Unicode BMP or Symbol），非 Unicode 编码返回 `None`（`src/tables/name.rs:145`）。
- **实测结果**：`assets/dingliesongtypeface.ttf` 的 FAMILY 记录解码为字符串
  `"dingliesongtypeface"`（英文，没有中文家族名记录）。

排版时用这个真实家族名去匹配，而不是猜测的名字：

```rust
let family = family_name_from_ttf(FONT_BYTES)?; // "dingliesongtypeface"
let attrs = cosmic_text::Attrs::new()
    .family(cosmic_text::Family::Name(&family))
    .weight(cosmic_text::Weight::BOLD);
```

> 注意：这份测试字体只有一个字重，`Weight::BOLD` 对它没有实际的"加粗"效果
> （`cosmic-text` 本身不做合成粗体/fake bold；`Weight` 只影响多字重字体族里选哪个
> 字面）。Task 4 如果需要真正的粗体视觉效果，要么找带独立 Bold 字面的字体，要么
> 用更粗的描边宽度或额外描边圈数来模拟。

## 排版调用（`cosmic_text::Buffer`）

```rust
let metrics = cosmic_text::Metrics::new(70.0, 84.0); // 70px 字号，84px 行高
let mut buffer = cosmic_text::Buffer::new(&mut font_system, metrics);
buffer.set_size(Some(canvas_w as f32), Some(canvas_h as f32));

buffer.set_text(text, &attrs, cosmic_text::Shaping::Advanced, None);
buffer.shape_until_scroll(&mut font_system, false);

for run in buffer.layout_runs() {
    for glyph in run.glyphs {
        // ...
    }
}
```

要点（与 brief 里"仅为说明意图"的伪代码的实际差异）：

- `Buffer::set_text` 的签名是
  `(&mut self, text: &str, attrs: &Attrs, shaping: Shaping, alignment: Option<Align>)`
  ——**不带 `font_system` 参数**（`cosmic-text-0.19.0/src/buffer.rs:934`）。真正需要
  `font_system` 的是排版本身：显式调用 `Buffer::shape_until_scroll(&mut self, font_system,
  prune: bool)`（`src/buffer.rs:571`），或者让 `Buffer::layout_runs(&mut self)`（在
  `BorrowedWithFontSystem` 包装类型上）自动调用；本探针用的是前者，直接在裸
  `Buffer` 上调用。
- `Buffer::layout_runs(&self) -> LayoutRunIter<'_>` 本身不需要 `font_system`
  （`src/buffer.rs:1134`），前提是已经 `shape_until_scroll` 过。
- `Shaping::Advanced` 支持复杂文字排版和字体回退；本探针的场景（CJK + 拉丁混排）
  用 `Advanced`。

## 拿到排版结果后取字形轮廓（核心难点，路线 1：cosmic-text 排版 + ttf-parser 取轮廓）

`LayoutGlyph`（`run.glyphs` 的元素类型）**不含轮廓**，只有版式信息：

```rust
pub struct LayoutGlyph {
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub font_size: f32,
    pub font_weight: fontdb::Weight,
    pub x: f32, pub y: f32, // логical 坐标，见 physical() 方法
    // ...
}
```

（`cosmic-text-0.19.0/src/layout.rs:16-63`）

拿物理像素位置和一个便于查找字体的 cache key，用法与 `cosmic-text` 自己的
`Buffer::draw()`/`Buffer::render()` 完全一致（`src/buffer.rs:1621,1641`）：

```rust
let physical = glyph.physical((40.0, run.line_y + 40.0), 1.0);
// physical.cache_key.{font_id, glyph_id, font_size_bits, font_weight}
// physical.{x, y}: i32，笔位置的整数像素坐标
```

**踩坑点**：`physical()` 的 `offset` 参数第二个分量必须是 `run.line_y`（该行基线相对
buffer 顶部的像素 y 坐标），不能自己瞎填一个常量——`LayoutGlyph.y` 只是行内的微小
垂直修正量（连字/上下标之类），不包含基线位置。最初写成 `(40.0, 60.0)` 时所有字形
挤在画布最顶端、彼此重叠，包围盒高度只有 57px（应有的 ~69px）。

用 `font_id` + `font_weight` 拿到真正的字体对象和它的原始字节，再交给
`ttf-parser` 解析、取轮廓：

```rust
let font = font_system
    .get_font(cache_key.font_id, cache_key.font_weight)
    .ok_or_else(|| anyhow!("找不到 font_id={:?} 对应的已加载字体", cache_key.font_id))?;

let face = ttf_parser::Face::parse(font.data(), 0)?;
let units_per_em = face.units_per_em() as f32;
let font_size = f32::from_bits(cache_key.font_size_bits);
let scale = font_size / units_per_em;
```

- `FontSystem::get_font(&mut self, id: fontdb::ID, weight: fontdb::Weight) -> Option<Arc<Font>>`
  （`cosmic-text-0.19.0/src/font/system.rs:302`）。
- `cosmic_text::Font::data(&self) -> &[u8]` 返回字体原始字节
  （`src/font/mod.rs`，`Font { data: FontData, .. }` 的访问器）。
- `ttf_parser::Face::parse(data: &[u8], index: u32) -> Result<Face, FaceParsingError>`
  （`index` 是字体集合内的字号，普通单字体 `.ttf` 传 `0`）。
- `CacheKey` 的字段 `font_id`、`glyph_id`、`font_size_bits`（`f32::to_bits`/
  `f32::from_bits` 存取）、`font_weight`、`flags` 全部是 `pub`
  （`cosmic-text-0.19.0/src/glyph_cache.rs:19-34`），不需要额外的 getter。

取轮廓：实现 `ttf_parser::OutlineBuilder` trait，把回调转发给
`tiny_skia::PathBuilder`：

```rust
struct SkiaOutline(tiny_skia::PathBuilder);

impl ttf_parser::OutlineBuilder for SkiaOutline {
    fn move_to(&mut self, x: f32, y: f32) { self.0.move_to(x, y); }
    fn line_to(&mut self, x: f32, y: f32) { self.0.line_to(x, y); }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) { self.0.quad_to(x1, y1, x, y); }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) { self.0.close(); }
}

let mut outline = SkiaOutline(tiny_skia::PathBuilder::new());
let bbox = face.outline_glyph(ttf_parser::GlyphId(cache_key.glyph_id), &mut outline);
if bbox.is_none() {
    continue; // 空格等无墨字形没有轮廓，是正常情况
}
let font_space_path = outline.0.finish().unwrap(); // 字体 unitsPerEm 坐标系，y 轴向上
```

- `ttf_parser::OutlineBuilder` 的 `curve_to(x1,y1,x2,y2,x,y)`
  （`ttf-parser-0.25.1/src/lib.rs:577-593`）参数顺序与
  `tiny_skia_path::PathBuilder::cubic_to(x1,y1,x2,y2,x,y)`
  （`tiny-skia-path-0.12.0/src/path_builder.rs:223`）完全一致，直接透传即可，
  不需要转换三次贝塞尔/二次贝塞尔的参数顺序。
- `Face::outline_glyph(&self, glyph_id: GlyphId, builder: &mut dyn OutlineBuilder) ->
  Option<Rect>`（`ttf-parser-0.25.1/src/lib.rs:2125`）返回值是字体单位下的紧致包围盒，
  `None` 表示没有轮廓（空格/占位字形）。

## 构造 `tiny_skia::Path` 并变换到像素空间

`PathBuilder::finish(self) -> Option<Path>`（`tiny-skia-path-0.12.0/src/path_builder.rs:411`）
把累积的命令转成不可变 `Path`。ttf-parser 给出的坐标是字体 unitsPerEm 空间、
y 轴向上、原点在字形基线；需要变换成像素空间（y 轴向下）：

```rust
let transform = tiny_skia::Transform::from_row(
    scale, 0.0,
    0.0, -scale,
    physical.x as f32, physical.y as f32,
);
let path = font_space_path.transform(transform).unwrap();
```

`Transform::from_row(sx, ky, kx, sy, tx, ty)` 对应
`x' = sx*x + kx*y + tx`，`y' = ky*x + sy*y + ty`
（`tiny-skia-path-0.12.0/src/transform.rs:52`）。

## 描边 + 填充（**关键踩坑点**）

```rust
pixmap.stroke_path(&path, &black_paint, &stroke, tiny_skia::Transform::identity(), None);
pixmap.fill_path(&path, &white_paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
```

其中：

```rust
let stroke = tiny_skia::Stroke { width: 6.0, ..Default::default() };

let mut black_paint = tiny_skia::Paint::default();
black_paint.set_color_rgba8(0, 0, 0, 255);
black_paint.anti_alias = true;

let mut white_paint = tiny_skia::Paint::default();
white_paint.set_color_rgba8(255, 255, 255, 255);
white_paint.anti_alias = true;
```

**踩坑（对应 brief Step 5 排查阶梯第 3 条 "尝试先 `path.transform()` 归一化再描边"，
实测确实是这条救了场）**：`PixmapMut::stroke_path`/`fill_path` 签名是
`(path, paint, stroke_or_fill_rule, transform, mask)`，那个 `transform` 参数**只是把
已经生成好的描边/填充几何整体搬过去**，`Stroke.width` 本身是在传入的 `path` 的
**局部坐标系**里解释的。如果像 brief 伪代码那样直接把字体单位空间的 `Path`
（坐标动辄几百到几千，因为 unitsPerEm 常见是 1000 或 2048）连同缩放用的 `transform`
一起传给 `stroke_path`，`width: 6.0` 会先在字体单位空间里被当成"6 个字体单位"那么
细的线（`scale = 70/2048 ≈ 0.034`，换算到像素只有约 0.2px），实际渲染出来这条描边
几乎看不见——第一次跑探针时复现了这个问题：`near_black` 像素数恒为 0，肉眼看只有
白色填充、没有黑边。

修复方式：**先用 `Path::transform(transform)` 把路径本身变换到像素空间**（这一步
的 `transform` 就是上面"构造 Path"一节算出来的缩放+翻转+平移矩阵），再用
`Transform::identity()` 调 `stroke_path`/`fill_path`。这样 `Stroke.width = 6.0`
才是最终画面里真正的 6 像素。

`Path::transform(&self, ts: Transform) -> Option<Path>`
（返回 `Option`，退化变换如缩放为 0 时返回 `None`）。

## 合成粗体（协调者拍板方案，已实测验证生效）

`assets/dingliesongtypeface.ttf` 用 `ttf-parser` 核实过：`weight: Normal`、
`is_bold: false`、`is_variable: false`（没有 `fvar` 可变字重轴）、只有 1 个 face、
Subfamily 是 `"Regular"`、Full name 是 `"鼎猎宋刻体"`——**只有一个静态字重，没有
真正的 Bold 字面**。`cosmic-text` 本身也不做合成粗体（fake bold）：`Attrs::weight`
只用来在多字重字体族里挑字面，对单字重字体没有任何加粗效果。规格要求四个段落的
文字都是粗体，所以描边/填充这一步必须自己合成粗体，否则整体观感偏细。

**做法**：拿到像素空间的 `path`（前一节"构造 Path"算出来的那个）之后，按顺序画三遍：

```rust
let bold_w = font_size * 0.03; // font_size 是这个字形的实际像素字号（这里是 70.0）

// 1) 描边色（黑）描边：宽度是 stroke_w+bold_w；bold=false 时就是原始 stroke_w。
let outer_stroke_width = if bold { stroke.width + bold_w } else { stroke.width };
let outer_stroke = Stroke { width: outer_stroke_width, ..stroke.clone() };
pixmap.stroke_path(&path, &black_paint, &outer_stroke, Transform::identity(), None);

// 2) 仅 bold=true 时，再用**填充色**（白）描边一次，宽度 bold_w。
//    这一步把内部的白色实体向外挤宽 bold_w/2，是真正产生"变粗"视觉效果的一步；
//    因为黑色描边（第 1 步）的总宽度比它多出 stroke_w，缩回去之后黑色轮廓
//    仍然完整地包在加粗后的白色字形外侧，宽度还是原始的 stroke_w。
if bold {
    let bold_stroke = Stroke { width: bold_w, ..stroke.clone() };
    pixmap.stroke_path(&path, &white_paint, &bold_stroke, Transform::identity(), None);
}

// 3) 填充色（白）填充路径内部，补上笔画本身较粗、两侧描边圈不满的区域。
pixmap.fill_path(&path, &white_paint, FillRule::Winding, Transform::identity(), None);
```

`bold=false` 时完全退化成原来的两步画法（先黑描边、后白填充），不会影响非粗体的
渲染结果。

**验证**（`examples/text_probe.rs`，`main_text` 换成较短的"熊猫智研社"、
canvas 500×220、其余参数不变，`bold=true` 和 `bold=false` 各渲染一次，统计
`analyze()` 里"近纯白 + 近纯黑"像素数——用这个而不是简单数 `alpha > 128` 的像素，
是因为画布背景本身是半透明灰底（alpha=180 > 128），直接数 alpha 会把整张画布都
算成"墨迹"，两个版本量出来的数字会一样，等于没测——已实测踩到这个坑，改用
`analyze()` 的口径后才测出真实差异）：

```
合成粗体核对：对比同一段文字 bold=true / bold=false 的墨迹量……
  bold=false 墨迹像素数：11088
  bold=true  墨迹像素数：13464
  比例：1.214
  [PASS] 加粗版墨迹明显更多（1.214 倍），且未超过 2.5 倍上限
```

加粗版比非加粗版墨迹多约 21.4%，肉眼比对 `text_probe.png`（主探针文本用的就是
`bold=true`）也能看出笔画明显更饱满，同时黑色描边依然完整、均匀，没有断裂。

## 背景 / 画布

```rust
let mut pixmap = tiny_skia::Pixmap::new(canvas_w, canvas_h).unwrap();
pixmap.fill(tiny_skia::Color::from_rgba8(128, 128, 128, 180)); // 半透明灰底
```

`tiny_skia::Pixmap::save_png(&self, path) -> Result<(), png::EncodingError>`
直接存 PNG，不需要额外引入 `image` crate。

## `image` crate：保留，不是探针本身用到的

brief Step 1 让 `cargo add` 时一并加了 `image`，探针里最终没有用它渲染文字——
`tiny_skia::Pixmap` 自带 `save_png`，像素级校验也是直接读 `Pixmap::pixels()`
（`&[tiny_skia::PremultipliedColorU8]`）算的，不需要额外解码 PNG 来验证文字这条路径。

**结论（协调者已拍板）：保留这个依赖。** 下一个任务里 `assets::logo_rgba()` 需要
`image` 来解码 `logo.png` 得到 RGBA 位图，不是可有可无的依赖——只是跟本文档记录的
"文字描边"这条路径没有关系。

## 像素级验收方法（探针里怎么做的）

`tiny_skia::PremultipliedColorU8`（`pixmap.pixels()` 的元素类型）存的是预乘 alpha 的
RGBA。判断"近纯白/近纯黑"前要先反预乘：

```rust
let unmul = pixel.demultiply(); // -> tiny_skia::ColorU8
let (r, g, b, a) = (unmul.red(), unmul.green(), unmul.blue(), pixel.alpha());
```

- `PremultipliedColorU8::demultiply(&self) -> ColorU8`、
  `PremultipliedColorU8::alpha(self) -> u8`（`tiny-skia-0.12.0/src/color.rs:100-179`）。
- `ColorU8::red()/green()/blue()/alpha() -> u8`（`src/color.rs:33-56`）。

包围盒统计方法：排除掉"看起来还是背景灰底"的像素（RGB 都落在 100~156 区间且
alpha=255），剩下的不透明像素算包围盒。

**豆腐块专项判定**：分别渲染两段不同的 CJK 文本到同尺寸画布，取
`Pixmap::data(&self) -> &[u8]`（预乘后的原始 RGBA 字节，`src/pixmap.rs:230`）逐字节
比较。如果两次渲染逐字节相同，说明两段文本被渲染成了同一个占位形状（`.notdef` 方框），
即"豆腐块"；只有像素分布确实不同，才能说明各自取到了正确的字形轮廓。仅检查包围盒
宽度是不够的——`.notdef` 方框本身也有固定宽度、也有黑白像素，包围盒检查一样能通过。

## 完整可运行示例

见 `examples/text_probe.rs`。运行：

```bash
cargo run --example text_probe
```

产出 `text_probe.png`（主探针图）、`text_probe_cjk_a.png`（"熊猫"）、
`text_probe_cjk_b.png`（"智研"），均已加入 `.gitignore`（渲染产物不入库，只有生成
它们的代码入库）。
