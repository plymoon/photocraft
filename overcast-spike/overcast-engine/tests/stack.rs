//! Tests for the spike's own code: spec parsing, stroke translation, sparse storage, masks,
//! adjustments, resolution independence, PNG output and graceful failure.

use overcast_engine::{Depth, Error, LayerKind, LayerSpec, PngDepth, Stack, StackSpec, StrokeSpec, Target, Tip};
use photocraft_color::BlendMode;

fn layer(kind: LayerKind) -> LayerSpec {
    LayerSpec { name: String::new(), enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind, mask: None }
}

fn fill(c: [f32; 4]) -> LayerSpec {
    layer(LayerKind::Fill { color: c })
}

fn px(img: &overcast_engine::Image, x: u32, y: u32) -> [f32; 4] {
    img.px[(y * img.width + x) as usize]
}

#[test]
fn fill_layer_composites_to_its_colour() {
    let spec = StackSpec { depth: Depth::U8, layers: vec![fill([0.2, 0.4, 0.6, 1.0])] };
    let img = overcast_engine::composite(&spec, 64).unwrap();
    assert_eq!((img.width, img.height), (64, 64));
    let p = px(&img, 10, 50);
    for (a, b) in p.iter().zip([0.2, 0.4, 0.6, 1.0]) {
        assert!((a - b).abs() < 1.0 / 255.0, "{p:?}");
    }
}

#[test]
fn stroke_allocates_only_touched_tiles() {
    // 2048² = 64 tiles; a short stroke in one corner touches one or two.
    let mut st = Stack::new(2048, Depth::U16).unwrap();
    let l = st.push_layer(&layer(LayerKind::Paint { strokes: vec![] })).unwrap();
    assert_eq!(st.tile_count(l), Some(0));
    let rep = st.stroke(l, Target::Content, &StrokeSpec::through(0.01, &[[0.02, 0.02], [0.08, 0.05]])).unwrap();
    assert!(rep.tiles >= 1 && rep.tiles <= 2, "{rep:?}");
    assert!(rep.bounds[0] < rep.bounds[2] && rep.bounds[2] <= 256, "{rep:?}");
}

#[test]
fn soft_tip_is_softer_than_hard_tip() {
    // One dab, so build-up along a line doesn't saturate the soft falloff.
    let line = [[0.5, 0.5]];
    let render = |hardness: f32| {
        let mut s = StrokeSpec::through(0.2, &line);
        s.hardness = hardness;
        s.color = [1.0, 1.0, 1.0, 1.0];
        let spec = StackSpec { depth: Depth::F32, layers: vec![fill([0.0, 0.0, 0.0, 1.0]), layer(LayerKind::Paint { strokes: vec![s] })] };
        overcast_engine::composite(&spec, 256).unwrap()
    };
    let (hard, soft) = (render(1.0), render(0.0));
    // 20 px from the dab's centre (radius 25.6 px): the hard tip is solid there, the soft one is fading.
    let (h, s) = (px(&hard, 128, 108)[0], px(&soft, 128, 108)[0]);
    assert!(h > 0.95, "hard {h}");
    assert!(s < 0.6 && s > 0.0, "soft {s}");
}

#[test]
fn square_tip_rotates_with_angle() {
    let render = |angle: f32| {
        let mut s = StrokeSpec::through(0.5, &[[0.5, 0.5]]);
        s.tip = Tip::Square;
        s.angle = angle;
        s.color = [1.0, 1.0, 1.0, 1.0];
        let mut st = Stack::new(128, Depth::U8).unwrap();
        let l = st.push_layer(&layer(LayerKind::Paint { strokes: vec![] })).unwrap();
        st.stroke(l, Target::Content, &s).unwrap();
        st.composite()
    };
    let (straight, rotated) = (render(0.0), render(45.0));
    // Near the square's corner (64 ± 30): covered unrotated, empty when turned 45°.
    assert!(px(&straight, 36, 36)[3] > 0.9);
    assert!(px(&rotated, 36, 36)[3] < 0.1);
    // The rotated diamond reaches further along the axis than the straight square.
    assert!(px(&straight, 64, 22)[3] < 0.1);
    assert!(px(&rotated, 64, 22)[3] > 0.9);
}

#[test]
fn custom_alpha_tip_paints_its_stamp() {
    // A ring stamp: the centre of the dab stays empty.
    let n = 32u32;
    let data: Vec<f32> = (0..n * n)
        .map(|i| {
            let (x, y) = ((i % n) as f32 - 15.5, (i / n) as f32 - 15.5);
            let d = (x * x + y * y).sqrt();
            if (10.0..15.0).contains(&d) { 1.0 } else { 0.0 }
        })
        .collect();
    let mut s = StrokeSpec::through(64.0, &[[64.0, 64.0]]);
    s.space = overcast_engine::Space::Pixel;
    s.tip = Tip::Alpha { width: n, height: n, data };
    let mut st = Stack::new(128, Depth::U8).unwrap();
    let l = st.push_layer(&layer(LayerKind::Paint { strokes: vec![] })).unwrap();
    st.stroke(l, Target::Content, &s).unwrap();
    let img = st.composite();
    assert!(px(&img, 64, 64)[3] < 0.05, "centre {:?}", px(&img, 64, 64));
    assert!(px(&img, 64 + 25, 64)[3] > 0.9, "ring {:?}", px(&img, 89, 64));
}

#[test]
fn hue_saturation_applies_only_inside_its_mask() {
    let mut reveal = StrokeSpec::through(0.3, &[[0.75, 0.25], [0.75, 0.75]]);
    reveal.color = [1.0, 1.0, 1.0, 1.0];
    let mut hs = layer(LayerKind::HueSaturation { hue: 120.0, saturation: 0.0, lightness: 0.0 });
    hs.mask = Some(vec![reveal]);
    let spec = StackSpec { depth: Depth::U16, layers: vec![fill([0.8, 0.2, 0.2, 1.0]), hs] };
    let st = Stack::from_spec(&spec, 256).unwrap();
    assert!(st.mask_tile_count(1).unwrap() >= 1);
    let img = st.composite();
    let outside = px(&img, 32, 128);
    let inside = px(&img, 192, 128);
    assert!(outside[0] > 0.7 && outside[1] < 0.3, "outside {outside:?}");
    // Red rotated by +120° is green.
    assert!(inside[1] > 0.7 && inside[0] < 0.3, "inside {inside:?}");
}

#[test]
fn levels_and_blend_mode_and_opacity() {
    let mut multiply = fill([0.5, 0.5, 0.5, 1.0]);
    multiply.blend = BlendMode::Multiply;
    multiply.opacity = 0.5;
    let spec = StackSpec { depth: Depth::F32, layers: vec![fill([0.8, 0.8, 0.8, 1.0]), multiply] };
    let p = px(&overcast_engine::composite(&spec, 16).unwrap(), 8, 8);
    // 0.8 · (1 - 0.5) + (0.8 · 0.5) · 0.5 = 0.6
    assert!((p[0] - 0.6).abs() < 0.01, "{p:?}");

    let lv = layer(LayerKind::Levels { in_black: 0.0, in_white: 0.5, gamma: 1.0 });
    let spec = StackSpec { depth: Depth::F32, layers: vec![fill([0.25, 0.25, 0.25, 1.0]), lv] };
    let p = px(&overcast_engine::composite(&spec, 16).unwrap(), 8, 8);
    assert!((p[0] - 0.5).abs() < 0.02, "{p:?}");
}

#[test]
fn disabled_layer_is_skipped() {
    let mut top = fill([1.0, 0.0, 0.0, 1.0]);
    top.enabled = false;
    let spec = StackSpec { depth: Depth::U8, layers: vec![fill([0.0, 0.0, 1.0, 1.0]), top] };
    let p = px(&overcast_engine::composite(&spec, 8).unwrap(), 4, 4);
    assert!(p[2] > 0.99 && p[0] < 0.01);
}

#[test]
fn uv_strokes_rerasterize_at_any_resolution() {
    let mut s = StrokeSpec::through(0.08, &[[0.1, 0.2], [0.5, 0.8], [0.9, 0.3]]);
    s.hardness = 0.5;
    s.color = [1.0, 1.0, 1.0, 1.0];
    let spec = StackSpec { depth: Depth::F32, layers: vec![fill([0.0, 0.0, 0.0, 1.0]), layer(LayerKind::Paint { strokes: vec![s] })] };
    let small = overcast_engine::composite(&spec, 128).unwrap();
    let big = overcast_engine::composite(&spec, 512).unwrap().downsample(4);
    let mean = small.px.iter().zip(&big.px).map(|(a, b)| (a[0] - b[0]).abs()).sum::<f32>() / small.px.len() as f32;
    assert!(mean < 0.02, "mean abs difference {mean}");
}

#[test]
fn strokes_replay_deterministically_with_scatter() {
    let mut s = StrokeSpec::through(0.05, &[[0.1, 0.5], [0.9, 0.5]]);
    s.scatter = 2.0;
    s.seed = 7;
    let spec = StackSpec { depth: Depth::U8, layers: vec![layer(LayerKind::Paint { strokes: vec![s.clone()] })] };
    let a = overcast_engine::composite(&spec, 128).unwrap();
    let b = overcast_engine::composite(&spec, 128).unwrap();
    assert_eq!(a, b);
    s.seed = 8;
    let spec2 = StackSpec { depth: Depth::U8, layers: vec![layer(LayerKind::Paint { strokes: vec![s] })] };
    assert_ne!(a, overcast_engine::composite(&spec2, 128).unwrap());
}

#[test]
fn json_spec_round_trips_and_parses_defaults() {
    let json = r#"{
        "depth": "f32",
        "layers": [
            {"type": "fill", "color": [1, 1, 1, 1]},
            {"type": "paint", "blend": "Multiply", "opacity": 0.8,
             "strokes": [{"size": 0.1, "tip": {"shape": "square"}, "angle": 30, "points": [[0.2, 0.2, 1], [0.8, 0.8, 0.5]]}]},
            {"type": "hue_saturation", "hue": 90, "mask": [{"size": 0.4, "hardness": 0, "color": [1,1,1,1], "points": [[0.5, 0.5, 1]]}]}
        ]
    }"#;
    let spec: StackSpec = serde_json::from_str(json).unwrap();
    assert_eq!(spec.depth, Depth::F32);
    assert_eq!(spec.layers[1].blend, BlendMode::Multiply);
    assert_eq!(spec.layers[1].kind, LayerKind::Paint { strokes: match &spec.layers[1].kind { LayerKind::Paint { strokes } => strokes.clone(), _ => vec![] } });
    let back: StackSpec = serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
    assert_eq!(spec, back);
    let st = Stack::from_json(json, 64).unwrap();
    assert_eq!(st.len(), 3);
}

#[test]
fn png_output_8_and_16_bit() {
    let spec = StackSpec { depth: Depth::U16, layers: vec![fill([0.5, 0.25, 1.0, 1.0])] };
    let img = overcast_engine::composite(&spec, 32).unwrap();
    for (d, bits) in [(PngDepth::Eight, png::BitDepth::Eight), (PngDepth::Sixteen, png::BitDepth::Sixteen)] {
        let bytes = img.to_png(d).unwrap();
        let dec = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        assert_eq!(dec.info().bit_depth, bits);
        assert_eq!((dec.info().width, dec.info().height), (32, 32));
    }
}

#[test]
fn bad_input_returns_errors_not_panics() {
    assert_eq!(Stack::new(0, Depth::U8).unwrap_err(), Error::InvalidSize(0));
    assert!(matches!(Stack::new(1 << 20, Depth::U8), Err(Error::InvalidSize(_))));
    assert!(matches!(Stack::from_json("{not json", 16), Err(Error::Json(_))));

    let mut st = Stack::new(64, Depth::U8).unwrap();
    let f = st.push_layer(&fill([1.0; 4])).unwrap();
    let good = StrokeSpec::through(0.1, &[[0.5, 0.5]]);
    assert!(matches!(st.stroke(f, Target::Content, &good), Err(Error::NotPaintable { .. })));
    assert!(matches!(st.stroke(99, Target::Content, &good), Err(Error::NoSuchLayer(99))));
    // A fill can carry a mask, though.
    assert!(st.stroke(f, Target::Mask, &good).is_ok());

    let p = st.push_layer(&layer(LayerKind::Paint { strokes: vec![] })).unwrap();
    let mut bad = good.clone();
    bad.points.clear();
    assert!(matches!(st.stroke(p, Target::Content, &bad), Err(Error::InvalidStroke(_))));
    let mut bad = good.clone();
    bad.points = vec![[f32::NAN, 0.0, 1.0]];
    assert!(matches!(st.stroke(p, Target::Content, &bad), Err(Error::InvalidStroke(_))));
    let mut bad = good.clone();
    bad.size = -1.0;
    assert!(matches!(st.stroke(p, Target::Content, &bad), Err(Error::InvalidStroke(_))));
    let mut bad = good.clone();
    bad.size = f32::INFINITY;
    assert!(matches!(st.stroke(p, Target::Content, &bad), Err(Error::InvalidStroke(_))));
    let mut bad = good.clone();
    bad.tip = Tip::Alpha { width: 4, height: 4, data: vec![1.0; 3] };
    assert!(matches!(st.stroke(p, Target::Content, &bad), Err(Error::InvalidStroke(_))));
    let mut bad = good.clone();
    bad.tip = Tip::Alpha { width: 0, height: 4, data: vec![] };
    assert!(matches!(st.stroke(p, Target::Content, &bad), Err(Error::InvalidStroke(_))));
    // A huge brush is clamped to PhotoCraft's maximum instead of allocating without bound.
    let mut big = good.clone();
    big.size = 1e9;
    assert!(st.stroke(p, Target::Content, &big).is_ok());
    // Out-of-range layer values are clamped or rejected.
    assert!(matches!(st.push_layer(&layer(LayerKind::Levels { in_black: 0.9, in_white: 0.1, gamma: 1.0 })), Err(Error::InvalidLayer(_))));
    let mut nan = fill([f32::NAN; 4]);
    nan.opacity = f32::NAN;
    assert!(matches!(st.push_layer(&nan), Err(Error::InvalidLayer(_))));
    nan.opacity = 1.0;
    assert!(st.push_layer(&nan).is_ok());
    let _ = st.composite();
}
