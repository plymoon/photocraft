//! `import overcast_engine`: composite() and stroke() for the Python core.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use engine::{Error, PngDepth, StackSpec, StrokeSpec, Target};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

fn err(e: Error) -> PyErr {
    PyValueError::new_err(e.to_string())
}

fn depth(bits: u8) -> PyResult<PngDepth> {
    match bits {
        8 => Ok(PngDepth::Eight),
        16 => Ok(PngDepth::Sixteen),
        _ => Err(PyValueError::new_err(format!("bit_depth must be 8 or 16, got {bits}"))),
    }
}

fn parse<T: serde::de::DeserializeOwned>(json: &str) -> PyResult<T> {
    serde_json::from_str(json).map_err(|e| PyValueError::new_err(format!("invalid JSON: {e}")))
}

/// Flatten a stack (JSON `StackSpec`) at `size`² and return PNG bytes. Runs without the GIL.
#[pyfunction]
#[pyo3(signature = (stack_json, size, bit_depth = 8))]
fn composite<'py>(py: Python<'py>, stack_json: &str, size: u32, bit_depth: u8) -> PyResult<Bound<'py, PyBytes>> {
    let d = depth(bit_depth)?;
    let spec: StackSpec = parse(stack_json)?;
    let png = py.allow_threads(|| engine::composite(&spec, size).and_then(|img| img.to_png(d))).map_err(err)?;
    Ok(PyBytes::new(py, &png))
}

/// `(png_bytes, (x0, y0, x1, y1), tiles)`.
type StrokeOut<'py> = (Bound<'py, PyBytes>, (i32, i32, i32, i32), usize);

/// Rasterize one stroke (JSON `StrokeSpec`) onto a transparent `size`² surface and return
/// `(png_bytes, (x0, y0, x1, y1), tiles)`.
#[pyfunction]
#[pyo3(signature = (stroke_json, size, bit_depth = 8))]
fn stroke<'py>(py: Python<'py>, stroke_json: &str, size: u32, bit_depth: u8) -> PyResult<StrokeOut<'py>> {
    let d = depth(bit_depth)?;
    let s: StrokeSpec = parse(stroke_json)?;
    let (png, r, tiles) = py
        .allow_threads(|| -> Result<_, Error> {
            let mut st = engine::Stack::new(size, engine::Depth::U16)?;
            let layer = st.push_layer(&serde_json::from_str(r#"{"type":"paint"}"#).map_err(|e| Error::Json(e.to_string()))?)?;
            let rep = st.stroke(layer, Target::Content, &s)?;
            Ok((st.composite().to_png(d)?, rep.bounds, rep.tiles))
        })
        .map_err(err)?;
    Ok((PyBytes::new(py, &png), (r[0], r[1], r[2], r[3]), tiles))
}

/// A live stack kept in Rust memory: add strokes incrementally, composite on demand.
#[pyclass(unsendable)]
struct Stack {
    inner: engine::Stack,
}

#[pymethods]
impl Stack {
    #[new]
    fn new(stack_json: &str, size: u32) -> PyResult<Self> {
        Ok(Self { inner: engine::Stack::from_json(stack_json, size).map_err(err)? })
    }

    /// Paint a stroke into layer `layer` (`target` = "content" or "mask"); returns
    /// `((x0, y0, x1, y1), tiles)`.
    #[pyo3(signature = (layer, stroke_json, target = "content"))]
    fn stroke(&mut self, layer: usize, stroke_json: &str, target: &str) -> PyResult<((i32, i32, i32, i32), usize)> {
        let t = match target {
            "content" => Target::Content,
            "mask" => Target::Mask,
            _ => return Err(PyValueError::new_err(format!("target must be 'content' or 'mask', got {target:?}"))),
        };
        let s: StrokeSpec = parse(stroke_json)?;
        let r = self.inner.stroke(layer, t, &s).map_err(err)?;
        Ok(((r.bounds[0], r.bounds[1], r.bounds[2], r.bounds[3]), r.tiles))
    }

    #[pyo3(signature = (bit_depth = 8))]
    fn composite<'py>(&self, py: Python<'py>, bit_depth: u8) -> PyResult<Bound<'py, PyBytes>> {
        let png = self.inner.composite().to_png(depth(bit_depth)?).map_err(err)?;
        Ok(PyBytes::new(py, &png))
    }

    fn tile_count(&self, layer: usize) -> Option<usize> {
        self.inner.tile_count(layer)
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }
}

#[pymodule]
fn overcast_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(composite, m)?)?;
    m.add_function(wrap_pyfunction!(stroke, m)?)?;
    m.add_class::<Stack>()?;
    Ok(())
}
