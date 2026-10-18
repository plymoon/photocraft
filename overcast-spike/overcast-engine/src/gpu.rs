//! Optional GPU path: PhotoCraft's wgpu compositor on the same document the CPU path flattens.

use photocraft_gpu::{Compositor, render_to_vec};

use crate::{Error, Image, Result, Stack};

/// A headless wgpu device plus PhotoCraft's compositor.
pub struct Gpu {
    pub adapter_info: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    comp: Compositor,
}

impl Gpu {
    /// Open the default adapter headlessly. `Ok(None)` when the machine has no usable adapter.
    pub fn open() -> Result<Option<Self>> {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            return Ok(None);
        };
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| Error::Gpu(e.to_string()))?;
        let comp = Compositor::try_new_with_format(&device, Compositor::preferred_acc_format(&adapter)).map_err(|e| Error::Gpu(e.to_string()))?;
        Ok(Some(Self { adapter_info: format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type), device, queue, comp }))
    }

    /// Flatten the stack on the GPU (same straight-alpha RGBA as [`Stack::composite`]).
    pub fn composite(&mut self, stack: &Stack) -> Result<Image> {
        let doc = stack.document();
        self.comp.supports(doc).map_err(|e| Error::Gpu(e.to_string()))?;
        let r = doc.bounds();
        let px = render_to_vec(&mut self.comp, &self.device, &self.queue, doc, r).map_err(|e| Error::Gpu(e.to_string()))?;
        Ok(Image { width: r.width(), height: r.height(), px })
    }
}
