//! The serde model of an Overcast layer stack: plain data, resolution-independent.
//!
//! Stroke coordinates and brush sizes are in **UV units** (0..1 across the surface) by default,
//! so the same stack re-rasterizes at 512², 2048² or 8192². `Space::Pixel` takes them as-is.

use photocraft_color::BlendMode;
use serde::{Deserialize, Serialize};

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}
fn default_spacing() -> f32 {
    0.1
}
fn black() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}
fn normal() -> BlendMode {
    BlendMode::Normal
}

/// Storage depth of the stack's pixel layers and masks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Depth {
    U8,
    #[default]
    U16,
    F32,
}

/// A whole stack, bottom layer first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StackSpec {
    #[serde(default)]
    pub depth: Depth,
    pub layers: Vec<LayerSpec>,
}

/// One layer: common properties plus its kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "one")]
    pub opacity: f32,
    /// Any of PhotoCraft's blend modes by name (`"Normal"`, `"Multiply"`, `"Overlay"`, …).
    #[serde(default = "normal")]
    pub blend: BlendMode,
    #[serde(flatten)]
    pub kind: LayerKind,
    /// Pixel mask built from paint strokes on a hide-all mask (`None` = no mask).
    #[serde(default)]
    pub mask: Option<Vec<StrokeSpec>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerKind {
    /// Solid fill over the whole surface (straight RGBA, 0..1).
    Fill { color: [f32; 4] },
    /// Paint layer: strokes stored as data and rasterized into a sparse tiled surface.
    Paint {
        #[serde(default)]
        strokes: Vec<StrokeSpec>,
    },
    /// Hue/Saturation adjustment: hue in degrees (-180..180), saturation and lightness -100..100.
    HueSaturation {
        #[serde(default)]
        hue: f32,
        #[serde(default)]
        saturation: f32,
        #[serde(default)]
        lightness: f32,
    },
    /// Master Levels: input black/white and gamma (0..1 inputs, gamma 0.1..9.99).
    Levels {
        #[serde(default)]
        in_black: f32,
        #[serde(default = "one")]
        in_white: f32,
        #[serde(default = "one")]
        gamma: f32,
    },
}

/// Coordinate space of a stroke's points and size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Space {
    /// 0..1 across the surface (Overcast's UV space).
    #[default]
    Uv,
    /// Surface pixels.
    Pixel,
}

/// Brush tip.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Tip {
    /// Computed round/elliptical tip (hardness, roundness, angle).
    #[default]
    Round,
    /// Anti-aliased square (a sampled tip, so `angle` rotates it).
    Square,
    /// Custom alpha stamp: `width × height` row-major coverage values 0..1.
    Alpha { width: u32, height: u32, data: Vec<f32> },
}

/// A brush stroke as data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeSpec {
    #[serde(default)]
    pub tip: Tip,
    /// Diameter (UV units or pixels, per `space`).
    pub size: f32,
    #[serde(default = "one")]
    pub hardness: f32,
    /// Tip rotation in degrees, counter-clockwise.
    #[serde(default)]
    pub angle: f32,
    #[serde(default = "one")]
    pub roundness: f32,
    /// Dab spacing as a fraction of the diameter.
    #[serde(default = "default_spacing")]
    pub spacing: f32,
    #[serde(default = "black")]
    pub color: [f32; 4],
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default = "one")]
    pub flow: f32,
    #[serde(default)]
    pub erase: bool,
    /// Pen pressure scales the tip size.
    #[serde(default)]
    pub pressure_size: bool,
    /// Scatter amount (fraction of the diameter); jitter is seeded, so strokes replay exactly.
    #[serde(default)]
    pub scatter: f32,
    #[serde(default)]
    pub seed: u64,
    #[serde(default)]
    pub space: Space,
    /// `[x, y, pressure]` samples.
    pub points: Vec<[f32; 3]>,
}

impl StrokeSpec {
    /// A stroke with default brush settings through `points` (UV space, full pressure).
    pub fn through(size: f32, points: &[[f32; 2]]) -> Self {
        Self {
            tip: Tip::Round,
            size,
            hardness: 1.0,
            angle: 0.0,
            roundness: 1.0,
            spacing: 0.1,
            color: black(),
            opacity: 1.0,
            flow: 1.0,
            erase: false,
            pressure_size: false,
            scatter: 0.0,
            seed: 0,
            space: Space::Uv,
            points: points.iter().map(|p| [p[0], p[1], 1.0]).collect(),
        }
    }
}
