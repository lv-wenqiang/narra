use anyhow::{Context, Result};

pub const FONT: &[u8] = include_bytes!("../assets/dingliesongtypeface.ttf");
pub const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");
pub const GITHUB_MARK_SVG: &[u8] = include_bytes!("../assets/github-mark.svg");

/// 解码内嵌 logo 为 RGBA8，返回 (像素, 宽, 高)。
pub fn logo_rgba() -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(LOGO_PNG).context("解码内嵌 logo.png 失败")?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

/// 把内嵌的 GitHub 图标 SVG 光栅化为 size×size 的 RGBA8。
/// 图标是单色的，调用方会按水印预设重新着色，故此处填充色不重要。
pub fn github_mark_rgba(size: u32) -> Result<(Vec<u8>, u32, u32)> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(GITHUB_MARK_SVG, &opt)
        .context("解析内嵌 github-mark.svg 失败")?;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .context("创建图标画布失败")?;

    let svg_size = tree.size();
    let transform = resvg::tiny_skia::Transform::from_scale(
        size as f32 / svg_size.width(),
        size as f32 / svg_size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    Ok((pixmap.data().to_vec(), size, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_are_non_empty() {
        assert!(FONT.len() > 1_000_000, "字体过小：{} 字节", FONT.len());
        assert!(LOGO_PNG.len() > 100_000, "logo 过小：{} 字节", LOGO_PNG.len());
        assert!(GITHUB_MARK_SVG.len() > 500, "svg 过小：{} 字节", GITHUB_MARK_SVG.len());
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

    #[test]
    fn github_mark_rasterizes_at_requested_size() {
        let (px, w, h) = github_mark_rgba(32).unwrap();
        assert_eq!((w, h), (32, 32));
        assert_eq!(px.len(), 32 * 32 * 4);
        assert!(px.chunks(4).any(|p| p[3] > 0), "图标全透明");
    }
}
