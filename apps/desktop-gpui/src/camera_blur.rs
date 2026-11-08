//! Live background blur for the camera preview bubble -- the gpui counterpart
//! of the Tauri preview's blur path (`src-tauri/src/camera.rs:1328-1546`).
//!
//! The Tauri native preview renders every frame through its own wgpu device:
//! the camera IOSurface is imported as a texture, `cap_camera_effects::
//! BlurProcessor` segments + blurs it, and the output lands in the preview
//! surface. This app paints CVPixelBuffers directly (`paint_surface_fitted`),
//! so the same pipeline is bent into surface-in / surface-out form:
//!
//! 1. [`crate::camera_window`]'s `FrameConverter` hands over the converted
//!    BGRA IOSurface pixel buffer (already downscaled to [`BLUR_MAX_DIMS`] and
//!    mirrored when the toolbar says so).
//! 2. The buffer's IOSurface is imported as a `Bgra8Unorm` wgpu texture
//!    through `cap-rendering`'s `iosurface_texture` seam -- the exact import
//!    the Tauri `NativeFrameConverter` uses (`camera_native.rs:301-310`).
//!    Imports are cached per ring slot (keyed by pixel-buffer pointer +
//!    ring generation), so the steady state creates no textures.
//! 3. The same pixel buffer goes to Apple Vision person segmentation on its
//!    own thread, and `BlurProcessor::process_into_encoder` composites with
//!    the newest finished mask, refined against this frame by the guided
//!    filter. Segmentation never blocks a preview frame.
//! 4. `cap_rendering::RgbaToBgraSurfaceConverter` blits the RGBA output into
//!    a BGRA IOSurface-backed CVPixelBuffer from its own ring, and
//!    `PendingSurface::wait` blocks until the GPU has finished writing it, so
//!    the buffer handed back to the window is always complete when painted.
//!
//! Everything runs on one dedicated `camera-blur` thread (the Tauri preview
//! renders on a dedicated thread too, `camera.rs:466-490`), with the ~10ms
//! Neural Engine segmentation on a second one, so neither touches the UI
//! thread.
//! The channel back to the window is how "what you see is what records" holds:
//! this is the same `BlurProcessor` the editor/export camera layer runs
//! (`cap-rendering/src/lib.rs:5742`), fed by the same `BackgroundBlurMode`
//! the recording bridge writes into `project-config.json`.
//!
//! Failure degrades exactly like Tauri's `ensure_blur_processor`
//! (`camera.rs:1486-1519`): if the device, the ONNX runtime or the model
//! cannot be brought up, the worker exits, the window notices the closed
//! channel and keeps painting raw frames -- the toggle still cycles and
//! persists, nothing crashes.

#![cfg(target_os = "macos")]

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use cap_rendering::{
    RgbaToBgraSurfaceConverter,
    iosurface_texture::{IOSurfaceTextureCache, import_metal_texture_to_wgpu},
};
use cidre::{arc, cv};

/// With an effect on, the preview processes at most 720p: enough for a crisp
/// cutout edge in the largest bubble on a Retina display, while the guided
/// filter and composite stay around a millisecond of GPU time. The
/// `FrameConverter` applies this cap during its hardware conversion, so the
/// scale costs nothing extra.
pub const BLUR_MAX_DIMS: (usize, usize) = (1280, 720);

/// `CAMERA_PREVIEW_BLUR_INFERENCE_INTERVAL` (`camera.rs:50`).
const BLUR_INFERENCE_INTERVAL: Duration = Duration::from_millis(33);

/// `LOW_SPEC_PREVIEW_RAM_THRESHOLD_BYTES` (`camera.rs:66`): machines at or
/// under 8GB never spin up the ONNX/wgpu blur processor -- the heaviest cost
/// in the preview. The toggle still cycles and persists; it just takes no
/// visual effect, exactly like `ensure_blur_processor`'s low-spec early
/// return (`camera.rs:1491-1498`).
const LOW_SPEC_RAM_THRESHOLD_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// The Tauri check reads total RAM once at startup (`init_preview_profile`,
/// `camera.rs:75-87`, via sysinfo); `hw.memsize` is the same number without
/// the dependency.
pub fn is_low_spec_preview() -> bool {
    static LOW_SPEC: OnceLock<bool> = OnceLock::new();
    *LOW_SPEC.get_or_init(|| {
        let mut bytes: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        let result = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                &mut bytes as *mut u64 as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        let low_spec = result == 0 && bytes > 0 && bytes <= LOW_SPEC_RAM_THRESHOLD_BYTES;
        if low_spec {
            tracing::info!(
                total_ram_mb = bytes / 1_048_576,
                "low-spec camera preview profile: background blur preview disabled"
            );
        }
        low_spec
    })
}

/// A CVPixelBuffer crossing the worker boundary. CF objects have atomic
/// refcounts; same contract as `FrameConverter` and cap-rendering's
/// `SurfaceFrame`.
pub struct SendPixelBuf(pub arc::R<cv::PixelBuf>);
unsafe impl Send for SendPixelBuf {}

pub struct BlurJob {
    /// Converted BGRA IOSurface pixel buffer from the window's ring.
    pub buffer: SendPixelBuf,
    pub width: u32,
    pub height: u32,
    /// The `FrameConverter` ring generation the buffer belongs to; a rebuilt
    /// ring invalidates the worker's imported-texture cache so a recycled
    /// allocation address cannot alias a stale import.
    pub ring_generation: u64,
    pub mode: Option<cap_camera_effects::BlurMode>,
    pub skin_smoothing: bool,
}

pub struct BlurOutput {
    pub mode: Option<cap_camera_effects::BlurMode>,
    pub skin_smoothing: bool,
    pub cutout: Option<std::sync::Arc<gpui::RenderImage>>,
    /// Blurred BGRA IOSurface pixel buffer, GPU work complete.
    pub buffer: SendPixelBuf,
    pub width: u32,
    pub height: u32,
}

/// The worker loop. Runs until the job sender is dropped (blur switched off,
/// window closed) or an unrecoverable error; either way every GPU/ONNX
/// resource is dropped on exit, the same release-on-off behaviour as
/// `release_blur_resources` (`camera.rs:1477-1484`).
pub fn run(jobs: flume::Receiver<BlurJob>, results: flume::Sender<BlurOutput>) {
    // Current-thread runtime for `request_adapter` and the timed blocking
    // wait inside `PendingSurface::wait`.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::warn!("camera blur worker runtime failed: {error}");
            return;
        }
    };

    let Some((device, queue)) = shared_device(&runtime) else {
        return;
    };

    let mut worker = match Worker::new(device, queue) {
        Ok(worker) => worker,
        Err(error) => {
            tracing::warn!("camera blur preview unavailable: {error:#}");
            return;
        }
    };

    while let Ok(mut job) = jobs.recv() {
        // Latest-wins, like the Tauri render loop draining `camera_rx`
        // (`camera.rs:1018-1020`).
        while let Ok(newer) = jobs.try_recv() {
            job = newer;
        }
        match worker.process(&runtime, job) {
            Ok(output) => {
                if results.send(output).is_err() {
                    return;
                }
            }
            Err(error) => {
                tracing::warn!("camera blur preview stopped: {error:#}");
                return;
            }
        }
    }
}

/// One device + queue for the app's blur lifetime, latched like Tauri's
/// `blur_processor_init_attempted`: a failed bring-up is not retried on every
/// toggle. The Tauri preview keeps its device for the window's lifetime
/// (`camera.rs:573-583`, `LowPower`, downlevel limits); only the ONNX/blur
/// resources are per-enable.
fn shared_device(runtime: &tokio::runtime::Runtime) -> Option<(wgpu::Device, wgpu::Queue)> {
    static DEVICE: OnceLock<Option<(wgpu::Device, wgpu::Queue)>> = OnceLock::new();
    DEVICE
        .get_or_init(|| {
            runtime.block_on(async {
                let instance = cap_rendering::create_wgpu_instance_sync();
                let adapter = instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::LowPower,
                        force_fallback_adapter: cap_rendering::force_software_wgpu_adapter(),
                        compatible_surface: None,
                    })
                    .await
                    .map_err(|error| {
                        tracing::warn!("camera blur adapter unavailable: {error}");
                    })
                    .ok()?;
                let (device, queue) = adapter
                    .request_device(&wgpu::DeviceDescriptor {
                        label: Some("Camera Preview Blur"),
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                            .using_resolution(adapter.limits()),
                        memory_hints: Default::default(),
                        trace: wgpu::Trace::Off,
                    })
                    .await
                    .map_err(|error| {
                        tracing::warn!("camera blur device unavailable: {error}");
                    })
                    .ok()?;
                cap_rendering::log_uncaptured_errors(&device, "camera-preview-blur");
                Some((device, queue))
            })
        })
        .clone()
}

struct Worker {
    device: wgpu::Device,
    queue: wgpu::Queue,
    cache: IOSurfaceTextureCache,
    processor: Option<cap_camera_effects::BlurProcessor>,
    smoother: Option<cap_camera_effects::SkinSmoother>,
    converter: RgbaToBgraSurfaceConverter,
    /// Imported input textures keyed by pixel-buffer pointer. The converter
    /// ring is four fixed buffers, so four entries cover the steady state and
    /// nothing is created per frame (stricter than the Tauri path, which
    /// re-imports each frame, `camera_native.rs:301-310`).
    imported: Vec<(usize, wgpu::Texture)>,
    imported_generation: u64,
    frame_number: u32,
}

impl Worker {
    fn new(device: wgpu::Device, queue: wgpu::Queue) -> anyhow::Result<Self> {
        let converter =
            RgbaToBgraSurfaceConverter::new(&device).map_err(|error| anyhow!("{error}"))?;
        let cache = IOSurfaceTextureCache::new().context("Metal device unavailable")?;
        tracing::info!("camera blur preview initialized");
        Ok(Self {
            device,
            queue,
            cache,
            processor: None,
            smoother: None,
            converter,
            imported: Vec::new(),
            imported_generation: 0,
            frame_number: 0,
        })
    }

    fn ensure_processor(&mut self) -> anyhow::Result<&mut cap_camera_effects::BlurProcessor> {
        if self.processor.is_none() {
            let mut processor = cap_camera_effects::BlurProcessor::new(
                &self.device,
                wgpu::TextureFormat::Rgba8Unorm,
            )
            .context("blur processor init")?;
            processor.set_inference_interval(BLUR_INFERENCE_INTERVAL);
            self.processor = Some(processor);
        }
        Ok(self.processor.as_mut().expect("just ensured"))
    }

    fn process(
        &mut self,
        runtime: &tokio::runtime::Runtime,
        job: BlurJob,
    ) -> anyhow::Result<BlurOutput> {
        let mode = job.mode;
        let skin_smoothing = job.skin_smoothing;
        let (width, height) = (job.width, job.height);
        let texture = self.input_texture(&job)?.clone();

        if mode.is_some() {
            self.ensure_processor()?;
        } else if skin_smoothing {
            // Only brightening needs the mask, so smoothing carries on without it.
            if let Err(error) = self.ensure_processor() {
                tracing::warn!("camera brightening unavailable: {error:#}");
            }
        } else {
            self.processor = None;
        }
        if !skin_smoothing {
            self.smoother = None;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Camera Preview Blur"),
            });
        if skin_smoothing {
            let person_mask = self.processor.as_mut().and_then(|processor| {
                processor.set_segmentation_source(job.buffer.0.clone());
                processor.update_person_mask(&self.device, &self.queue, &texture)
            });
            self.smoother
                .get_or_insert_with(|| cap_camera_effects::SkinSmoother::new(&self.device))
                .process_into_encoder(
                    &self.device,
                    &self.queue,
                    &texture,
                    &mut encoder,
                    cap_camera_effects::DEFAULT_SKIN_SMOOTHING,
                    person_mask,
                );
        }
        let smoothed = self
            .smoother
            .as_ref()
            .and_then(cap_camera_effects::SkinSmoother::output_texture);
        let output = if let (Some(mode), Some(processor)) = (mode, self.processor.as_mut()) {
            // Vision takes the converted IOSurface directly (no GPU readback),
            // and segmentation runs beside rendering instead of blocking it.
            processor.set_segmentation_source(job.buffer.0.clone());
            processor.process_into_encoder(
                &self.device,
                &self.queue,
                smoothed.unwrap_or(&texture),
                &mut encoder,
                mode,
            );
            processor
                .process_returning_output()
                .context("blur output missing")?
        } else {
            smoothed.context("skin smoothing output missing")?
        };

        self.frame_number = self.frame_number.wrapping_add(1);
        let pending = runtime
            .block_on(self.converter.encode(
                &self.device,
                &mut encoder,
                output,
                width,
                height,
                self.frame_number,
                30,
            ))
            .map_err(|error| anyhow!("{error}"))?;
        let submission = self.queue.submit(std::iter::once(encoder.finish()));

        let frame = runtime
            .block_on(pending.wait(&self.device, submission))
            .map_err(|error| anyhow!("{error}"))?;
        let cutout = if mode == Some(cap_camera_effects::BlurMode::Remove) {
            use core_foundation::base::TCFType as _;
            use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};
            let raw = &*frame.pixel_buffer as *const cv::PixelBuf as CVPixelBufferRef;
            let buffer = unsafe { CVPixelBuffer::wrap_under_get_rule(raw) };
            Some(
                crate::camera_window::snapshot_cutout(&buffer)
                    .context("cutout preview pixels unavailable")?,
            )
        } else {
            None
        };
        Ok(BlurOutput {
            mode,
            skin_smoothing,
            cutout,
            buffer: SendPixelBuf(frame.pixel_buffer),
            width: frame.width,
            height: frame.height,
        })
    }

    fn input_texture(&mut self, job: &BlurJob) -> anyhow::Result<&wgpu::Texture> {
        if self.imported_generation != job.ring_generation {
            self.imported.clear();
            self.imported_generation = job.ring_generation;
        }
        let key = &*job.buffer.0 as *const cv::PixelBuf as usize;
        if let Some(index) = self.imported.iter().position(|(k, _)| *k == key) {
            return Ok(&self.imported[index].1);
        }
        let io_surface = job
            .buffer
            .0
            .io_surf()
            .context("pixel buffer has no IOSurface")?;
        // A `Bgra8Unorm` binding samples as RGBA-ordered floats, so the blur
        // pipelines (all `Float { filterable: true }` layouts) and the 256px
        // segmentation downsample read correct channels with no extra
        // conversion pass.
        let metal = self
            .cache
            .create_bgra_texture(io_surface, job.width, job.height)
            .map_err(|error| anyhow!("{error}"))?;
        let texture = import_metal_texture_to_wgpu(
            &self.device,
            &metal,
            wgpu::TextureFormat::Bgra8Unorm,
            job.width,
            job.height,
            Some("Camera Blur Input"),
        )
        .map_err(|error| anyhow!("{error}"))?;
        self.imported.push((key, texture));
        Ok(&self.imported.last().expect("just pushed").1)
    }
}

#[cfg(test)]
mod bench;

#[cfg(test)]
mod tests {
    use super::*;
    use cap_camera_effects::BlurMode;
    use cidre::cf;
    use core_foundation::base::TCFType as _;
    use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};
    use std::time::Instant;

    fn with_pixels<T>(
        buffer: &cv::PixelBuf,
        height: usize,
        read: impl FnOnce(&mut [u8], usize) -> T,
    ) -> T {
        let raw = buffer as *const cv::PixelBuf as CVPixelBufferRef;
        let wrapped = unsafe { CVPixelBuffer::wrap_under_get_rule(raw) };
        assert_eq!(wrapped.lock_base_address(0), 0);
        let stride = wrapped.get_bytes_per_row();
        let pixels = unsafe {
            std::slice::from_raw_parts_mut(wrapped.get_base_address().cast::<u8>(), stride * height)
        };
        let result = read(pixels, stride);
        assert_eq!(wrapped.unlock_base_address(0), 0);
        result
    }

    fn roughness(buffer: &cv::PixelBuf, height: usize) -> f64 {
        with_pixels(buffer, height, |pixels, stride| {
            let row = &pixels[stride * (height / 2)..][..stride];
            let green: Vec<f64> = row
                .chunks_exact(4)
                .skip(64)
                .take(512)
                .map(|pixel| f64::from(pixel[1]))
                .collect();
            let mean = green.iter().sum::<f64>() / green.len() as f64;
            green.iter().map(|value| (value - mean).abs()).sum::<f64>() / green.len() as f64
        })
    }

    #[test]
    #[ignore = "requires a working native WGPU adapter"]
    fn segmentation_session_is_released_once_every_effect_is_off() {
        let (width, height) = BLUR_MAX_DIMS;
        let properties = cf::Dictionary::new();
        let attributes = cf::Dictionary::with_keys_values(
            &[
                cv::pixel_buffer::keys::io_surf_props().as_ref(),
                cv::pixel_buffer::keys::metal_compatibility().as_ref(),
            ],
            &[properties.as_ref(), cf::Boolean::value_true().as_ref()],
        )
        .unwrap();
        let buffer =
            cv::PixelBuf::new(width, height, cv::PixelFormat::_32_BGRA, Some(&attributes)).unwrap();
        with_pixels(&buffer, height, |pixels, stride| {
            for (y, row) in pixels.chunks_exact_mut(stride).enumerate() {
                for (x, pixel) in row.chunks_exact_mut(4).take(width).enumerate() {
                    let speck = if (x * 7 + y * 13) % 5 == 0 { 0 } else { 14 };
                    pixel.copy_from_slice(&[121 + speck, 146 + speck, 191 + speck, 255]);
                }
            }
        });
        let before = roughness(&buffer, height);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let (device, queue) = shared_device(&runtime).unwrap();
        let mut worker = Worker::new(device, queue).unwrap();
        let job = |mode, skin_smoothing| BlurJob {
            buffer: SendPixelBuf(buffer.clone()),
            width: width as u32,
            height: height as u32,
            ring_generation: 1,
            mode,
            skin_smoothing,
        };

        let output = worker.process(&runtime, job(None, true)).unwrap();
        assert!(worker.processor.is_some());
        assert!(roughness(&output.buffer.0, height) < before * 0.8);

        let output = worker
            .process(&runtime, job(Some(BlurMode::Light), true))
            .unwrap();
        assert!(worker.processor.is_some() && worker.smoother.is_some());
        assert_eq!(output.mode, Some(BlurMode::Light));
        assert!(output.skin_smoothing);

        worker
            .process(&runtime, job(Some(BlurMode::Light), false))
            .unwrap();
        assert!(worker.smoother.is_none());
        worker.process(&runtime, job(None, false)).ok();
        assert!(worker.processor.is_none());
    }

    #[test]
    #[ignore = "requires CAP_CAMERA_EFFECT_TEST_FRAME containing a 1280x720 RGBA webcam frame"]
    fn native_cutout_preserves_alpha_through_iosurface_and_preview_image() {
        let pixels = std::fs::read(std::env::var("CAP_CAMERA_EFFECT_TEST_FRAME").unwrap()).unwrap();
        let (width, height) = BLUR_MAX_DIMS;
        assert_eq!(pixels.len(), width * height * 4);
        let properties = cf::Dictionary::new();
        let attributes = cf::Dictionary::with_keys_values(
            &[
                cv::pixel_buffer::keys::io_surf_props().as_ref(),
                cv::pixel_buffer::keys::metal_compatibility().as_ref(),
            ],
            &[properties.as_ref(), cf::Boolean::value_true().as_ref()],
        )
        .unwrap();
        let buffer =
            cv::PixelBuf::new(width, height, cv::PixelFormat::_32_BGRA, Some(&attributes)).unwrap();
        let raw = &*buffer as *const cv::PixelBuf as CVPixelBufferRef;
        let wrapped = unsafe { CVPixelBuffer::wrap_under_get_rule(raw) };
        assert_eq!(wrapped.lock_base_address(0), 0);
        let stride = wrapped.get_bytes_per_row();
        let destination = unsafe {
            std::slice::from_raw_parts_mut(wrapped.get_base_address().cast::<u8>(), stride * height)
        };
        for (source, target) in pixels
            .chunks_exact(width * 4)
            .zip(destination.chunks_exact_mut(stride))
        {
            for (source, target) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
                target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
            }
        }
        assert_eq!(wrapped.unlock_base_address(0), 0);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let (device, queue) = shared_device(&runtime).unwrap();
        let mut worker = Worker::new(device, queue).unwrap();
        for mode in [
            BlurMode::Remove,
            BlurMode::Heavy,
            BlurMode::Light,
            BlurMode::Remove,
        ] {
            let mut timings = Vec::new();
            for _ in 0..30 {
                let start = Instant::now();
                let output = worker
                    .process(
                        &runtime,
                        BlurJob {
                            buffer: SendPixelBuf(buffer.clone()),
                            width: width as u32,
                            height: height as u32,
                            ring_generation: 1,
                            mode: Some(mode),
                            skin_smoothing: false,
                        },
                    )
                    .unwrap();
                timings.push(start.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(output.mode, Some(mode));
                if mode == BlurMode::Remove {
                    let image = output.cutout.unwrap();
                    let bytes = image.as_bytes(0).unwrap();
                    assert!(bytes.chunks_exact(4).any(|pixel| pixel[3] == 0));
                    assert!(bytes.chunks_exact(4).any(|pixel| pixel[3] == 255));
                } else {
                    assert!(output.cutout.is_none());
                }
                std::thread::sleep(Duration::from_millis(33).saturating_sub(start.elapsed()));
            }
            timings.sort_by(f64::total_cmp);
            eprintln!(
                "native {mode:?} preview worker: p50={:.2}ms p95={:.2}ms",
                timings[15], timings[28]
            );
        }
        assert_eq!(worker.imported.len(), 1);
    }
}
