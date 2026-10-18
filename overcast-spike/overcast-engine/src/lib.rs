//! Spike: Overcast's non-destructive layer stack on PhotoCraft's crates.
//!
//! The stack is Overcast's own data ([`StackSpec`]: fill, paint and adjustment layers, masks
//! built from strokes, strokes in UV space). [`Stack`] compiles it into a
//! [`photocraft_doc::Document`] whose pixel layers and masks are sparse copy-on-write 256² tiled
//! surfaces ([`photocraft_raster::Surface`]), rasterizes strokes with PhotoCraft's brush engine
//! ([`photocraft_paint`]) and flattens with the CPU reference compositor
//! ([`photocraft_compose::render`]); with the `gpu` feature also with the wgpu compositor
//! ([`gpu`]). Nothing here panics on bad input: every entry point validates and returns [`Error`].
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod spec;

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
pub mod gpu;

use std::fmt;

use photocraft_color::{BlendMode, Color, ColorMode, PixelFormat, SampleType};
use photocraft_doc::adjust::{HueRange, LevelsChannel};
use photocraft_doc::{Adjustment, Document, Fill, Layer, LayerContent, LayerId, LayerMask};
use photocraft_geom::{Rect, Size};
use photocraft_paint::{BrushSettings, Dynamic, GrayTile, MAX_BRUSH_SIZE, StrokePoint, TipShape};
use photocraft_raster::Surface;

pub use spec::{Depth, LayerKind, LayerSpec, Space, StackSpec, StrokeSpec, Tip};

/// Largest surface side accepted (PhotoCraft documents go to 300 000 px; Overcast UDIMs to 8K).
pub const MAX_SIZE: u32 = 16_384;
/// Most samples one stroke may carry.
pub const MAX_POINTS: usize = 200_000;
/// Largest custom alpha-tip side.
pub const MAX_TIP: u32 = 2048;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    InvalidSize(u32),
    NoSuchLayer(usize),
    /// The layer exists but has no surface of the requested kind (e.g. painting on a fill).
    NotPaintable { layer: usize, kind: &'static str },
    InvalidStroke(String),
    InvalidLayer(String),
    Json(String),
    Encode(String),
    Gpu(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidSize(s) => write!(f, "surface size {s} is outside 1..={MAX_SIZE}"),
            Error::NoSuchLayer(i) => write!(f, "no layer at index {i}"),
            Error::NotPaintable { layer, kind } => write!(f, "layer {layer} is a {kind} layer and has no pixels to paint"),
            Error::InvalidStroke(m) => write!(f, "invalid stroke: {m}"),
            Error::InvalidLayer(m) => write!(f, "invalid layer: {m}"),
            Error::Json(m) => write!(f, "invalid JSON: {m}"),
            Error::Encode(m) => write!(f, "PNG encoding failed: {m}"),
            Error::Gpu(m) => write!(f, "GPU compositor: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// What a stroke paints into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// The paint layer's pixels.
    #[default]
    Content,
    /// The layer's mask (created as hide-all on first use; paint white to reveal).
    Mask,
}

/// Result of rasterizing one stroke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrokeReport {
    /// Damaged rectangle `[x0, y0, x1, y1)` in surface pixels (empty when nothing was painted).
    pub bounds: [i32; 4],
    /// Allocated 256² tiles in the target surface after the stroke.
    pub tiles: usize,
}

/// A flattened image: straight-alpha RGBA, f32, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub px: Vec<[f32; 4]>,
}

/// PNG sample depth for [`Image::to_png`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngDepth {
    Eight,
    Sixteen,
}

impl Image {
    /// Encode as an RGBA PNG (8 or 16 bits per channel).
    pub fn to_png(&self, depth: PngDepth) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width, self.height);
            enc.set_color(png::ColorType::Rgba);
            let data: Vec<u8> = match depth {
                PngDepth::Eight => {
                    enc.set_depth(png::BitDepth::Eight);
                    self.px.iter().flat_map(|p| p.map(|v| quantize(v, 255.0) as u8)).collect()
                }
                PngDepth::Sixteen => {
                    enc.set_depth(png::BitDepth::Sixteen);
                    self.px.iter().flat_map(|p| p.map(|v| quantize(v, 65535.0) as u16)).flat_map(u16::to_be_bytes).collect()
                }
            };
            let mut w = enc.write_header().map_err(|e| Error::Encode(e.to_string()))?;
            w.write_image_data(&data).map_err(|e| Error::Encode(e.to_string()))?;
        }
        Ok(out)
    }

    /// Box-downsample by an integer factor (used to compare resolutions).
    pub fn downsample(&self, factor: u32) -> Image {
        let f = factor.max(1);
        let (w, h) = (self.width / f, self.height / f);
        let mut px = Vec::with_capacity(w as usize * h as usize);
        let n = (f * f) as f32;
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                for yy in 0..f {
                    for xx in 0..f {
                        let i = ((y * f + yy) * self.width + x * f + xx) as usize;
                        if let Some(p) = self.px.get(i) {
                            for c in 0..4 {
                                acc[c] += p[c];
                            }
                        }
                    }
                }
                px.push(acc.map(|v| v / n));
            }
        }
        Image { width: w, height: h, px }
    }
}

fn quantize(v: f32, max: f32) -> f32 {
    if v.is_finite() { (v.clamp(0.0, 1.0) * max).round() } else { 0.0 }
}

fn sample_type(d: Depth) -> SampleType {
    match d {
        Depth::U8 => SampleType::U8,
        Depth::U16 => SampleType::U16,
        Depth::F32 => SampleType::F32,
    }
}

fn finite(v: f32, what: &str) -> Result<f32> {
    if v.is_finite() { Ok(v) } else { Err(Error::InvalidStroke(format!("{what} must be finite"))) }
}

fn color4(c: [f32; 4]) -> [f32; 4] {
    c.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })
}

/// Translate a [`StrokeSpec`] into PhotoCraft brush settings and input samples for a `size`²
/// surface.
pub fn brush_for(s: &StrokeSpec, size: u32) -> Result<(BrushSettings, Vec<StrokePoint>)> {
    if s.points.is_empty() {
        return Err(Error::InvalidStroke("no points".into()));
    }
    if s.points.len() > MAX_POINTS {
        return Err(Error::InvalidStroke(format!("{} points (max {MAX_POINTS})", s.points.len())));
    }
    let scale = match s.space {
        Space::Uv => f64::from(size),
        Space::Pixel => 1.0,
    };
    let diameter = finite(s.size, "size")? * scale as f32;
    if !(diameter > 0.0) {
        return Err(Error::InvalidStroke("size must be positive".into()));
    }
    let tip = match &s.tip {
        Tip::Round => TipShape::Round,
        Tip::Square => TipShape::Sampled(GrayTile::from_fn(64, 64, |_, _| 1.0)),
        Tip::Alpha { width, height, data } => {
            if *width == 0 || *height == 0 || *width > MAX_TIP || *height > MAX_TIP {
                return Err(Error::InvalidStroke(format!("alpha tip {width}×{height} is outside 1..={MAX_TIP}")));
            }
            let n = *width as usize * *height as usize;
            if data.len() != n {
                return Err(Error::InvalidStroke(format!("alpha tip has {} values, expected {n}", data.len())));
            }
            let clean: Vec<f32> = data.iter().map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }).collect();
            TipShape::Sampled(GrayTile::from_f32(*width, *height, &clean))
        }
    };
    let mut b = BrushSettings {
        size: diameter.min(MAX_BRUSH_SIZE),
        hardness: finite(s.hardness, "hardness")?.clamp(0.0, 1.0),
        spacing: finite(s.spacing, "spacing")?.clamp(0.01, 10.0),
        opacity: finite(s.opacity, "opacity")?.clamp(0.0, 1.0),
        flow: finite(s.flow, "flow")?.clamp(0.0, 1.0),
        pressure_size: s.pressure_size,
        pressure_opacity: false,
        color: color4(s.color),
        erase: s.erase,
        tip,
        angle: finite(s.angle, "angle")?,
        roundness: finite(s.roundness, "roundness")?.clamp(0.01, 1.0),
        seed: s.seed,
        ..BrushSettings::default()
    };
    let scatter = finite(s.scatter, "scatter")?;
    if scatter > 0.0 {
        b.scattering.enabled = true;
        b.scattering.scatter = Dynamic::jitter(scatter.min(photocraft_paint::MAX_SCATTER));
    }
    let mut pts = Vec::with_capacity(s.points.len());
    for (i, p) in s.points.iter().enumerate() {
        let [x, y, pr] = *p;
        if !(x.is_finite() && y.is_finite() && pr.is_finite()) {
            return Err(Error::InvalidStroke(format!("point {i} is not finite")));
        }
        let mut sp = StrokePoint::new(f64::from(x) * scale, f64::from(y) * scale, pr.clamp(0.0, 1.0));
        sp.time = i as f64 * 8.0;
        pts.push(sp);
    }
    Ok((b, pts))
}

/// Rasterize one stroke into a sparse surface (any format: RGBA paint or gray mask) sized for a
/// `size`² canvas. Returns the damaged rectangle. Only touched 256² tiles are allocated.
pub fn stroke(surface: &mut Surface, s: &StrokeSpec, size: u32) -> Result<Rect> {
    check_size(size)?;
    let (brush, pts) = brush_for(s, size)?;
    Ok(photocraft_paint::render_stroke(surface, &brush, &pts, None, false, 1.0))
}

fn check_size(size: u32) -> Result<()> {
    if size == 0 || size > MAX_SIZE { Err(Error::InvalidSize(size)) } else { Ok(()) }
}

/// Flatten a stack spec at `size`² in one call.
pub fn composite(spec: &StackSpec, size: u32) -> Result<Image> {
    Ok(Stack::from_spec(spec, size)?.composite())
}

/// A live stack: the PhotoCraft document plus Overcast's layer order.
#[derive(Clone, Debug)]
pub struct Stack {
    doc: Document,
    ids: Vec<LayerId>,
    size: u32,
    depth: Depth,
}

impl Stack {
    pub fn new(size: u32, depth: Depth) -> Result<Self> {
        check_size(size)?;
        let doc = Document::new("overcast", Size::new(size, size), ColorMode::Rgb, sample_type(depth));
        Ok(Self { doc, ids: Vec::new(), size, depth })
    }

    /// Build a stack and rasterize every stroke in it at `size`².
    pub fn from_spec(spec: &StackSpec, size: u32) -> Result<Self> {
        let mut st = Self::new(size, spec.depth)?;
        for l in &spec.layers {
            st.push_layer(l)?;
        }
        Ok(st)
    }

    /// Parse a JSON [`StackSpec`] and build it.
    pub fn from_json(json: &str, size: u32) -> Result<Self> {
        let spec: StackSpec = serde_json::from_str(json).map_err(|e| Error::Json(e.to_string()))?;
        Self::from_spec(&spec, size)
    }

    pub fn size(&self) -> u32 {
        self.size
    }
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    /// The underlying PhotoCraft document (for the GPU compositor, export, inspection).
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Add a layer on top; returns its index (0 = bottom).
    pub fn push_layer(&mut self, l: &LayerSpec) -> Result<usize> {
        if !l.opacity.is_finite() {
            return Err(Error::InvalidLayer("opacity must be finite".into()));
        }
        let content = match &l.kind {
            LayerKind::Fill { color } => {
                let c = color4(*color);
                LayerContent::Fill(Fill::Solid(Color::rgba(c[0], c[1], c[2], c[3])))
            }
            LayerKind::Paint { .. } => LayerContent::Raster(Surface::new(self.doc.pixel_format())),
            LayerKind::HueSaturation { hue, saturation, lightness } => {
                let f = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
                LayerContent::Adjustment(Adjustment::HueSaturation {
                    hue: f(*hue, -180.0, 180.0),
                    saturation: f(*saturation, -100.0, 100.0),
                    lightness: f(*lightness, -100.0, 100.0),
                    colorize: false,
                    ranges: HueRange::defaults(),
                })
            }
            LayerKind::Levels { in_black, in_white, gamma } => {
                let (b, w, g) = (*in_black, *in_white, *gamma);
                if !(b.is_finite() && w.is_finite() && g.is_finite()) || !(0.0..1.0).contains(&b) || !(b < w && w <= 1.0) || !(0.1..=9.99).contains(&g) {
                    return Err(Error::InvalidLayer(format!("levels in_black {b}, in_white {w}, gamma {g} out of range")));
                }
                let master = LevelsChannel { in_black: b, in_white: w, gamma: g, ..LevelsChannel::default() };
                LayerContent::Adjustment(Adjustment::Levels {
                    master,
                    per_channel: std::array::from_fn(|_| LevelsChannel::default()),
                    space: Default::default(),
                    black: LevelsChannel::default(),
                })
            }
        };
        let mut layer = Layer::new(if l.name.is_empty() { format!("Layer {}", self.ids.len() + 1) } else { l.name.clone() }, content);
        layer.visible = l.enabled;
        layer.opacity = l.opacity.clamp(0.0, 1.0);
        layer.blend = if l.blend == BlendMode::PassThrough { BlendMode::Normal } else { l.blend };
        let id = layer.id;
        self.doc.layers.push(layer);
        self.ids.push(id);
        let index = self.ids.len() - 1;
        // Strokes go in after the layer exists, so a bad stroke leaves a well-formed (partial) stack.
        if let LayerKind::Paint { strokes } = &l.kind {
            for s in strokes {
                self.stroke(index, Target::Content, s)?;
            }
        }
        if let Some(mask) = &l.mask {
            self.ensure_mask(index)?;
            for s in mask {
                self.stroke(index, Target::Mask, s)?;
            }
        }
        Ok(index)
    }

    fn layer_mut(&mut self, index: usize) -> Result<&mut Layer> {
        let id = *self.ids.get(index).ok_or(Error::NoSuchLayer(index))?;
        self.doc.layer_mut(id).ok_or(Error::NoSuchLayer(index))
    }

    fn ensure_mask(&mut self, index: usize) -> Result<()> {
        let sample = sample_type(self.depth);
        let layer = self.layer_mut(index)?;
        if layer.mask.is_none() {
            let fmt = PixelFormat { mode: ColorMode::Grayscale, sample, alpha: false };
            layer.mask = Some(LayerMask { surface: Surface::with_default(fmt, &[0.0]), ..LayerMask::reveal_all() });
        }
        Ok(())
    }

    /// Rasterize a stroke into a layer's pixels or mask.
    pub fn stroke(&mut self, index: usize, target: Target, s: &StrokeSpec) -> Result<StrokeReport> {
        let size = self.size;
        let (brush, pts) = brush_for(s, size)?;
        if target == Target::Mask {
            self.ensure_mask(index)?;
        }
        let layer = self.layer_mut(index)?;
        let kind = layer.content.kind_name();
        let surface = match target {
            Target::Content => layer.surface_mut().ok_or(Error::NotPaintable { layer: index, kind })?,
            Target::Mask => &mut layer.mask.as_mut().ok_or(Error::NoSuchLayer(index))?.surface,
        };
        let r = photocraft_paint::render_stroke(surface, &brush, &pts, None, false, 1.0);
        Ok(StrokeReport { bounds: [r.x0, r.y0, r.x1, r.y1], tiles: surface.tile_count() })
    }

    /// Allocated tiles of a layer's pixels (`None` for layers without pixels).
    pub fn tile_count(&self, index: usize) -> Option<usize> {
        let id = *self.ids.get(index)?;
        self.doc.layer(id)?.surface().map(Surface::tile_count)
    }

    /// Allocated tiles of a layer's mask.
    pub fn mask_tile_count(&self, index: usize) -> Option<usize> {
        let id = *self.ids.get(index)?;
        self.doc.layer(id)?.mask.as_ref().map(|m| m.surface.tile_count())
    }

    /// Flatten with the CPU reference compositor.
    pub fn composite(&self) -> Image {
        self.composite_rect(self.doc.bounds())
    }

    /// Flatten one rectangle (e.g. a single 256² block or a UDIM sub-region).
    pub fn composite_rect(&self, rect: Rect) -> Image {
        let r = rect.intersect(&self.doc.bounds());
        let buf = photocraft_compose::render(&self.doc, r);
        Image { width: r.width(), height: r.height(), px: buf.px }
    }
}
