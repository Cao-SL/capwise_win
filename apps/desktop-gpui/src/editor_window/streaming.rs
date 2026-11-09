use std::sync::Arc;

use anyhow::Result;
use gpui::{DevicePixels, StreamingImage, StreamingImageFormat, size};

use cap_rendering::RenderedFrame;

/// The editor preview on Windows. `RenderedFrame` is already row-padded RGBA
/// straight out of wgpu, and the D3D11 stream atlas accepts an RGBA texture
/// with an arbitrary row pitch, so neither the un-padding nor the BGRA swap
/// that `frame_image` does is needed here.
///
/// One `StreamingImage` is held for the life of the editor: each window keeps
/// a single texture and calls UpdateSubresource into it, instead of packing a
/// fresh `RenderImage` into the sprite atlas every frame.
pub(crate) struct PreviewStream {
    image: StreamingImage,
    spare: Option<Vec<u8>>,
}

impl PreviewStream {
    pub fn new() -> Self {
        Self {
            image: StreamingImage::new(StreamingImageFormat::Rgba8),
            spare: None,
        }
    }

    pub fn image(&self) -> &StreamingImage {
        &self.image
    }

    /// Dropping every handle releases the window's stream texture on its next
    /// draw, so the pump needs no explicit teardown.
    pub fn push(&mut self, frame: RenderedFrame) -> Result<()> {
        let dimensions = size(
            DevicePixels(frame.width as i32),
            DevicePixels(frame.height as i32),
        );
        let stride = frame.padded_bytes_per_row as usize;
        let buffer = take_buffer(frame.data, &mut self.spare);
        if let Some(previous) = self.image.set_frame(dimensions, stride, buffer)? {
            self.spare = Some(previous);
        }
        Ok(())
    }
}

/// Prefer the renderer's own buffer. `set_frame` hands the previous one back
/// once paint is done with it, so a frame whose Arc is still shared (a scrub
/// snapshot, for instance) can reuse that allocation instead of cloning.
fn take_buffer(data: Arc<Vec<u8>>, spare: &mut Option<Vec<u8>>) -> Vec<u8> {
    match Arc::try_unwrap(data) {
        Ok(owned) => owned,
        Err(shared) => {
            let mut buffer = spare.take().unwrap_or_default();
            buffer.clear();
            buffer.extend_from_slice(&shared);
            buffer
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unique_render_buffer_is_taken_without_copying() {
        let mut spare = Some(vec![9u8; 64]);
        let data = Arc::new(vec![1u8, 2, 3, 4]);
        let buffer = take_buffer(data, &mut spare);
        assert_eq!(buffer, vec![1, 2, 3, 4]);
        assert_eq!(buffer.capacity(), 4);
    }

    #[test]
    fn a_shared_render_buffer_reuses_the_spare_allocation() {
        let mut spare = Some(Vec::with_capacity(32));
        let spare_ptr = spare.as_ref().unwrap().as_ptr();
        let data = Arc::new(vec![7u8; 16]);
        let shared = Arc::clone(&data);
        let buffer = take_buffer(data, &mut spare);
        assert_eq!(buffer, vec![7u8; 16]);
        assert_eq!(buffer.as_ptr(), spare_ptr);
        assert_eq!(buffer.capacity(), 32);
        assert_eq!(shared.len(), 16);
        assert!(spare.is_none());
    }

    #[test]
    fn a_shared_buffer_still_works_without_a_spare() {
        let mut spare = None;
        let data = Arc::new(vec![5u8; 8]);
        let shared = Arc::clone(&data);
        let buffer = take_buffer(data, &mut spare);
        assert_eq!(buffer, vec![5u8; 8]);
        assert_eq!(shared.len(), 8);
    }
}
