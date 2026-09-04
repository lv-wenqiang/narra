use anyhow::{Context, Result};

pub const FONT: &[u8] = include_bytes!("../assets/dingliesongtypeface.ttf");
pub const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");
pub const INTRO_MP3: &[u8] = include_bytes!("../assets/intro.mp3");
pub const INTRO_TYPEWRITER_MP3: &[u8] = include_bytes!("../assets/intro_typewriter.mp3");

/// 解码内嵌 logo 为 RGBA8，返回 (像素, 宽, 高)。
pub fn logo_rgba() -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(LOGO_PNG).context("解码内嵌 logo.png 失败")?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

/// 把用户自备的图标文件光栅化成 `size×size` 的 RGBA8，返回 (像素, 宽, 高)。
///
/// **按扩展名分流，不嗅探文件头**：`.svg` 走 `resvg`（矢量，按目标尺寸直接
/// 渲染，任意尺寸都清晰），其余走 `image` 解码后缩放。扩展名写错时报「解析
/// 失败」并点名那个文件，比默默按另一种格式猜要好——用户给的是自己的文件，
/// 猜错的后果是水印上出现一块看不懂的东西而没有任何提示。
///
/// **不重新着色**：图标原样保留自己的颜色，水印预设的 alpha 由调用方作为
/// 整体不透明度施加。用户给的多半是自家彩色 logo，按水印色重新着色会把它
/// 拍成一块单色剪影。
pub fn load_icon(path: &std::path::Path, size: u32) -> Result<(Vec<u8>, u32, u32)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let bytes =
        std::fs::read(path).with_context(|| format!("读取图标文件失败：{}", path.display()))?;

    if ext == "svg" {
        let opt = resvg::usvg::Options::default();
        let tree = resvg::usvg::Tree::from_data(&bytes, &opt)
            .with_context(|| format!("解析 SVG 图标失败：{}", path.display()))?;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).context("创建图标画布失败")?;
        let svg_size = tree.size();
        let transform = resvg::tiny_skia::Transform::from_scale(
            size as f32 / svg_size.width(),
            size as f32 / svg_size.height(),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        return Ok((pixmap.data().to_vec(), size, size));
    }

    if ext == "png" {
        let img = image::load_from_memory(&bytes)
            .with_context(|| format!("解码 PNG 图标失败：{}", path.display()))?;
        let scaled = image::imageops::resize(
            &img.to_rgba8(),
            size,
            size,
            image::imageops::FilterType::Lanczos3,
        );
        return Ok((scaled.into_raw(), size, size));
    }

    anyhow::bail!(
        "不认识的图标格式 `.{ext}`：{}（支持 .svg 与 .png）",
        path.display()
    )
}

/// 把两段内嵌音效写进 `dir`，返回 `(intro.mp3, intro_typewriter.mp3)` 的路径。
///
/// ffmpeg 的四路音频里有两路是内嵌资源，而 stdin 已经被帧流占用，无法再从
/// 管道喂第二、第三份数据；所以运行时落成真实文件是最简单可靠的做法。
/// 调用方负责选一个临时目录并在结束后清理。
pub fn write_embedded_audio(
    dir: &std::path::Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let intro = dir.join("intro.mp3");
    let typewriter = dir.join("intro_typewriter.mp3");
    std::fs::write(&intro, INTRO_MP3)
        .with_context(|| format!("写入内嵌片尾音效失败：{}", intro.display()))?;
    std::fs::write(&typewriter, INTRO_TYPEWRITER_MP3)
        .with_context(|| format!("写入内嵌打字机音效失败：{}", typewriter.display()))?;
    Ok((intro, typewriter))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_are_non_empty() {
        assert!(FONT.len() > 1_000_000, "字体过小：{} 字节", FONT.len());
        assert!(
            LOGO_PNG.len() > 100_000,
            "logo 过小：{} 字节",
            LOGO_PNG.len()
        );
    }

    #[test]
    fn font_parses_as_truetype() {
        // sfnt version 应为 0x00010000（TrueType）
        assert_eq!(&FONT[..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn logo_decodes_to_rgba() {
        let (px, w, h) = logo_rgba().unwrap();
        assert!(w > 100 && h > 100, "logo 尺寸异常：{w}x{h}");
        assert_eq!(px.len(), (w * h * 4) as usize);
        // 不能是全透明
        assert!(px.chunks(4).any(|p| p[3] > 0), "logo 全透明");
    }

    /// `load_icon` 按扩展名分流，两条路都要能光栅化到请求的尺寸。
    ///
    /// **SVG 走 resvg 按目标尺寸渲染**（矢量，任意尺寸都清晰）；**位图走
    /// image 再缩放**。分流依据是扩展名而不是嗅探文件头：用户给的是自己的
    /// 文件，扩展名写错时报「解析失败」比默默按另一种格式猜要好。
    #[test]
    fn load_icon_rasterizes_svg_and_png_at_the_requested_size() {
        let dir = std::env::temp_dir().join(format!("panda_icon_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // 一个最小的实心方块 SVG。
        let svg = dir.join("mark.svg");
        std::fs::write(
            &svg,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#336699"/></svg>"##,
        )
        .unwrap();

        // 一个 4x4 的不透明 PNG。
        let png = dir.join("mark.png");
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0x33, 0x66, 0x99, 0xff]));
        img.save(&png).unwrap();

        for (label, path) in [("svg", &svg), ("png", &png)] {
            let (px, w, h) =
                load_icon(path, 32).unwrap_or_else(|e| panic!("{label} 应能加载：{e}"));
            assert_eq!((w, h), (32, 32), "{label} 应光栅化到请求尺寸");
            assert_eq!(px.len(), 32 * 32 * 4, "{label} 像素数据长度应为 w*h*4");
            assert!(px.chunks_exact(4).any(|p| p[3] > 0), "{label} 不应全透明");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 文件不存在、或扩展名不认识时报错而不是静默画个空图标。
    #[test]
    fn load_icon_reports_missing_files_and_unknown_extensions() {
        let missing = std::path::Path::new("/nonexistent-icon-xyz.png");
        let err = load_icon(missing, 32).unwrap_err().to_string();
        assert!(
            err.contains("nonexistent-icon-xyz.png"),
            "报错应点名那个文件：{err}"
        );

        let dir = std::env::temp_dir().join(format!("panda_icon_ext_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let weird = dir.join("mark.txt");
        std::fs::write(&weird, b"not an image").unwrap();
        assert!(
            load_icon(&weird, 32).is_err(),
            "不认识的扩展名应报错，而不是猜格式"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn embedded_audio_is_non_empty_mp3() {
        // ID3v2 头是 "ID3"，裸 MPEG 帧头是 0xFF 0xFB/0xF3/0xF2。两者都算合法 mp3 开头。
        for (name, bytes) in [("intro", INTRO_MP3), ("typewriter", INTRO_TYPEWRITER_MP3)] {
            assert!(
                bytes.len() > 10_000,
                "{name} 太小，可能没复制成功：{}",
                bytes.len()
            );
            let ok = bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0);
            assert!(ok, "{name} 开头不像 mp3：{:02X?}", &bytes[..4]);
        }
    }

    #[test]
    fn write_embedded_audio_produces_two_readable_files() {
        let dir = std::env::temp_dir().join(format!("panda_assets_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (intro, typewriter) = write_embedded_audio(&dir).unwrap();

        assert_eq!(
            std::fs::read(&intro).unwrap(),
            INTRO_MP3,
            "写出的内容应与内嵌字节一致"
        );
        assert_eq!(std::fs::read(&typewriter).unwrap(), INTRO_TYPEWRITER_MP3);
        assert_ne!(intro, typewriter, "两个文件不能是同一个路径");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_embedded_audio_is_idempotent() {
        // render 与 make 可能在同一个目录下先后调用，重复写不应报错。
        let dir = std::env::temp_dir().join(format!("panda_assets_idem_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = write_embedded_audio(&dir).unwrap();
        let second = write_embedded_audio(&dir).unwrap();
        assert_eq!(first, second);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 把「常量」与「它自称来自的那个文件」绑定死。
    ///
    /// 此前三条测试对「两个 `include_bytes!` 路径写反」完全无感：两个文件都是
    /// 合法 mp3、都远超长度下限，而另两条测试是拿写出的文件跟**同一个**常量比，
    /// 自指恒真。审查用真实变异（对调两个路径）实测确认 7 条全过。
    ///
    /// 这里用编译期绝对路径重新读一遍磁盘上的文件，逐字节比对——对「有人改了
    /// include 路径」这个唯一现实的回退变异精确响应，且不依赖测试进程的 CWD。
    #[test]
    fn each_constant_holds_the_file_it_claims_to_come_from() {
        let intro_on_disk =
            std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/intro.mp3")).unwrap();
        let typewriter_on_disk = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/intro_typewriter.mp3"
        ))
        .unwrap();

        assert_eq!(
            INTRO_MP3,
            intro_on_disk.as_slice(),
            "INTRO_MP3 应内嵌 assets/intro.mp3"
        );
        assert_eq!(
            INTRO_TYPEWRITER_MP3,
            typewriter_on_disk.as_slice(),
            "INTRO_TYPEWRITER_MP3 应内嵌 assets/intro_typewriter.mp3"
        );
    }

    /// 尺寸关系哨兵：片尾音效（2.486s、44.1kHz）明显大于打字机音效（3.157s、24kHz）。
    /// 比上面那条弱，但失败信息更直白，能一眼看出是不是两者搞反了。
    #[test]
    fn intro_is_substantially_larger_than_the_typewriter_clip() {
        assert!(
            INTRO_MP3.len() > INTRO_TYPEWRITER_MP3.len(),
            "片尾音效应大于打字机音效；若两者接近或反了，多半是 include_bytes! 路径写反：\
             intro={} typewriter={}",
            INTRO_MP3.len(),
            INTRO_TYPEWRITER_MP3.len()
        );
    }
}
