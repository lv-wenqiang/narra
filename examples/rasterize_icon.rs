//! 临时工具：把 SVG 光栅化成 PNG。用法：rasterize_icon <in.svg> <out.png> <size>
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (src, dst, size) = (&a[1], &a[2], a[3].parse::<u32>()?);
    let (rgba, w, h) = narra::assets::load_icon(std::path::Path::new(src), size)?;
    image::save_buffer(dst, &rgba, w, h, image::ColorType::Rgba8)?;
    println!("  写出 {dst} ({w}x{h})");
    Ok(())
}
