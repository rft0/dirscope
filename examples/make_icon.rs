//! Regenerates the app icon files from `assets/icon.svg`:
//!   assets/windows/dirscope.ico  (16–256 px, embedded into the .exe by build.rs)
//!   assets/linux/dirscope.png    (512 px, for the .desktop entry)
//!
//! Run with: cargo run --example make_icon

use std::fs;

use resvg::{tiny_skia, usvg};

const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

fn render(tree: &usvg::Tree, size: u32) -> Vec<u8> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).unwrap();
    let scale = size as f32 / tree.size().width();
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().unwrap()
}

/// Builds an .ico whose entries are PNG-compressed images (supported since Vista).
fn build_ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());

    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in images {
        let dim = if *size >= 256 { 0 } else { *size as u8 }; // 0 means 256
        out.extend_from_slice(&[dim, dim, 0, 0]); // width, height, palette, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // color planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}

fn main() {
    let svg = fs::read("assets/icon.svg").expect("run from the project root");
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).unwrap();

    let images: Vec<_> = ICO_SIZES.iter().map(|&s| (s, render(&tree, s))).collect();
    fs::create_dir_all("assets/windows").unwrap();
    fs::write("assets/windows/dirscope.ico", build_ico(&images)).unwrap();

    fs::create_dir_all("assets/linux").unwrap();
    fs::write("assets/linux/dirscope.png", render(&tree, 512)).unwrap();

    println!("wrote assets/windows/dirscope.ico and assets/linux/dirscope.png");
}
