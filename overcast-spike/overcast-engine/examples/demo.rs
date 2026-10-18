//! The spike's demo stack on a 2048² sparse surface: a fill layer; a paint layer with a soft round
//! stroke and a rotated square-tip stroke; a Hue/Saturation layer limited by a painted mask.
//! Flattens with the CPU compositor (8- and 16-bit PNG) and, with `--features gpu` and an adapter,
//! the GPU compositor too.
//!
//! cargo run --release --example demo [--features gpu] -- [out_dir]

use std::time::Instant;

use overcast_engine::{Depth, LayerKind, LayerSpec, PngDepth, Stack, StackSpec, StrokeSpec, Tip};
use photocraft_color::BlendMode;

const SIZE: u32 = 2048;

fn layer(name: &str, kind: LayerKind) -> LayerSpec {
    LayerSpec { name: name.into(), enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind, mask: None }
}

/// The demo stack (UV space, resolution independent).
pub fn demo_spec() -> StackSpec {
    // Soft round: a wave across the upper half.
    let wave: Vec<[f32; 2]> = (0..=64).map(|i| i as f32 / 64.0).map(|t| [0.08 + 0.84 * t, 0.32 + 0.12 * (t * std::f32::consts::TAU * 1.5).sin()]).collect();
    let mut soft = StrokeSpec::through(0.09, &wave);
    soft.hardness = 0.0;
    soft.color = [0.10, 0.25, 0.75, 1.0];
    soft.flow = 0.6;
    // Square tip rotated 30°, spaced so the individual stamps show, with pen pressure on size.
    let mut square = StrokeSpec::through(0.06, &[]);
    square.tip = Tip::Square;
    square.angle = 30.0;
    square.spacing = 1.4;
    square.pressure_size = true;
    square.color = [0.15, 0.6, 0.2, 1.0];
    square.points = (0..=32).map(|i| i as f32 / 32.0).map(|t| [0.1 + 0.8 * t, 0.62 + 0.25 * t, 0.4 + 0.6 * (1.0 - (2.0 * t - 1.0).abs())]).collect();
    // Hue/Saturation masked to a soft vertical band on the right.
    let mut reveal = StrokeSpec::through(0.30, &[[0.70, 0.1], [0.70, 0.9]]);
    reveal.hardness = 0.2;
    reveal.color = [1.0, 1.0, 1.0, 1.0];
    let mut hs = layer("Hue/Sat (masked)", LayerKind::HueSaturation { hue: 150.0, saturation: 40.0, lightness: 0.0 });
    hs.mask = Some(vec![reveal]);
    StackSpec {
        depth: Depth::U16,
        layers: vec![
            layer("Fill", LayerKind::Fill { color: [0.85, 0.55, 0.35, 1.0] }),
            layer("Paint", LayerKind::Paint { strokes: vec![soft, square] }),
            hs,
        ],
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "out".into()));
    std::fs::create_dir_all(&out)?;
    let spec = demo_spec();
    std::fs::write(out.join("stack.json"), serde_json::to_string_pretty(&spec)?)?;

    let t = Instant::now();
    let stack = Stack::from_spec(&spec, SIZE)?;
    println!("build + rasterize strokes: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    let total = (SIZE / 256) * (SIZE / 256);
    println!(
        "sparse tiles: paint layer {}/{total}, hue/sat mask {}/{total}",
        stack.tile_count(1).unwrap_or(0),
        stack.mask_tile_count(2).unwrap_or(0)
    );

    let t = Instant::now();
    let cpu = stack.composite();
    println!("CPU composite {SIZE}²: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    std::fs::write(out.join("stack_cpu.png"), cpu.to_png(PngDepth::Eight)?)?;
    std::fs::write(out.join("stack_cpu16.png"), cpu.to_png(PngDepth::Sixteen)?)?;
    std::fs::write(out.join("stack_preview_512.png"), cpu.downsample(4).to_png(PngDepth::Eight)?)?;
    println!("wrote {}/stack_cpu.png, stack_cpu16.png, stack_preview_512.png", out.display());

    gpu(&stack, &cpu, &out)?;
    Ok(())
}

#[cfg(feature = "gpu")]
fn gpu(stack: &Stack, cpu: &overcast_engine::Image, out: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    match overcast_engine::gpu::Gpu::open()? {
        None => println!("GPU: no wgpu adapter on this machine; skipped (CPU result above is the reference)"),
        Some(mut g) => {
            println!("GPU adapter: {}", g.adapter_info);
            let t = Instant::now();
            let img = g.composite(stack)?;
            println!("GPU composite {SIZE}²: {:.1} ms (first frame, includes upload)", t.elapsed().as_secs_f64() * 1e3);
            let max = cpu.px.iter().zip(&img.px).flat_map(|(a, b)| (0..4).map(move |c| (a[c] - b[c]).abs())).fold(0.0f32, f32::max);
            println!("GPU vs CPU max abs difference: {max:.5} ({:.2}/255)", max * 255.0);
            std::fs::write(out.join("stack_gpu.png"), img.to_png(PngDepth::Eight)?)?;
        }
    }
    Ok(())
}

#[cfg(not(feature = "gpu"))]
fn gpu(_: &Stack, _: &overcast_engine::Image, _: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("GPU: built without --features gpu; skipped");
    Ok(())
}
