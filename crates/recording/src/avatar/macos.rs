use super::{AvatarKind, Pose, paint::Portrait};
use crate::NativeCameraFrame;
use anyhow::{Context, anyhow};
use cidre::{arc, cf, cm, cv, ns, vn, vt};
use objc::{msg_send, sel, sel_impl};
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn CVPixelBufferGetBaseAddress(buffer: &cv::PixelBuf) -> *mut u8;
    fn CVPixelBufferGetBytesPerRow(buffer: &cv::PixelBuf) -> usize;
}

struct Job {
    frame: NativeCameraFrame,
    kind: AvatarKind,
}

pub(crate) struct AvatarWorker {
    input: flume::Sender<Job>,
    output: flume::Receiver<anyhow::Result<NativeCameraFrame>>,
}

impl AvatarWorker {
    pub fn new() -> anyhow::Result<Self> {
        let (input, jobs) = flume::bounded::<Job>(1);
        let (results, output) = flume::bounded(1);
        std::thread::Builder::new()
            .name("cartoon-camera".into())
            .spawn(move || {
                let mut renderer = Renderer::new();
                while let Ok(job) = jobs.recv() {
                    let result = objc::rc::autoreleasepool(|| match &mut renderer {
                        Ok(renderer) => renderer.render(job),
                        Err(error) => Err(anyhow!("{error:#}")),
                    });
                    if results.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { input, output })
    }

    pub async fn render(
        &self,
        frame: NativeCameraFrame,
        kind: AvatarKind,
    ) -> anyhow::Result<NativeCameraFrame> {
        self.input.send_async(Job { frame, kind }).await?;
        self.output.recv_async().await?
    }
}

struct Request(*mut objc::runtime::Object);
impl Drop for Request {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.0, release];
        }
    }
}

struct Renderer {
    portrait: Portrait,
    pose: Pose,
    target: Pose,
    start: Instant,
    last_tracking: Option<Instant>,
    last_face: Option<Instant>,
    tracking_active: bool,
    handler: arc::R<vn::SequenceRequestHandler>,
    transfer: arc::R<vt::PixelTransferSession>,
}

impl Renderer {
    fn new() -> anyhow::Result<Self> {
        let mut transfer = vt::PixelTransferSession::new().map_err(|e| anyhow!("{e:?}"))?;
        transfer.set_realtime(true).map_err(|e| anyhow!("{e:?}"))?;
        Ok(Self {
            portrait: Portrait::new(),
            pose: Pose::default(),
            target: Pose::default(),
            start: Instant::now(),
            last_tracking: None,
            last_face: None,
            tracking_active: false,
            handler: vn::SequenceRequestHandler::new(),
            transfer,
        })
    }

    fn track(&self, pixel: &cv::PixelBuf) -> anyhow::Result<Option<Pose>> {
        let class = objc::runtime::Class::get("VNDetectFaceLandmarksRequest")
            .context("Face tracking is unavailable")?;
        let raw: *mut objc::runtime::Object = unsafe { msg_send![class, new] };
        anyhow::ensure!(!raw.is_null(), "Cannot initialize face tracking");
        let request = Request(raw);
        let requests =
            ns::Array::<vn::Request>::from_slice(&[unsafe { &*request.0.cast::<vn::Request>() }]);
        self.handler
            .perform_on_cv_pixel_buf(&requests, pixel)
            .map_err(|e| anyhow!("Face tracking failed: {e:?}"))?;
        let results: *const ns::Array<vn::FaceObservation> =
            unsafe { msg_send![request.0, results] };
        let Some(results) = (unsafe { results.as_ref() }) else {
            return Ok(None);
        };
        let Some(face) = results.iter().max_by(|a, b| {
            let a = a.bounding_box().size;
            let b = b.bounding_box().size;
            (a.width * a.height).total_cmp(&(b.width * b.height))
        }) else {
            return Ok(None);
        };
        let Some(landmarks) = face.landmarks() else {
            return Ok(None);
        };
        fn opening(
            region: Option<arc::R<vn::FaceLandmarkRegion2d>>,
            aspect: f64,
            fallback: f32,
        ) -> f32 {
            let Some(region) = region else {
                return fallback;
            };
            let points = region.normalized_points();
            if points.len() < 3 {
                return fallback;
            }
            let (mut min_x, mut min_y, mut max_x, mut max_y) =
                (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for point in points {
                min_x = min_x.min(point.x);
                max_x = max_x.max(point.x);
                min_y = min_y.min(point.y);
                max_y = max_y.max(point.y);
            }
            ((max_y - min_y) / ((max_x - min_x) * aspect).max(0.001)) as f32
        }
        let bounds = face.bounding_box().size;
        let aspect =
            bounds.width * pixel.width() as f64 / (bounds.height * pixel.height() as f64).max(1.);
        Ok(Some(Pose {
            yaw: face.yaw().map_or(0., |v| v.as_f64() as f32 / 0.7),
            pitch: face.pitch().map_or(0., |v| v.as_f64() as f32 / 0.5),
            roll: face.roll().map_or(0., |v| -v.as_f64() as f32),
            left_eye: ((opening(landmarks.left_eye(), aspect, 0.25) - 0.08) / 0.19).clamp(0., 1.),
            right_eye: ((opening(landmarks.right_eye(), aspect, 0.25) - 0.08) / 0.19).clamp(0., 1.),
            mouth: ((opening(landmarks.inner_lips(), aspect, 0.) - 0.025) / 0.55).clamp(0., 1.),
            smile: 0.,
        }))
    }

    fn render(&mut self, job: Job) -> anyhow::Result<NativeCameraFrame> {
        let input = job
            .frame
            .sample_buf
            .image_buf()
            .context("Camera frame has no image")?;
        let now = Instant::now();
        if self
            .last_tracking
            .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_millis(66))
        {
            self.last_tracking = Some(now);
            match self.track(input) {
                Ok(Some(pose)) => {
                    if !self.tracking_active {
                        tracing::info!("Cartoon camera face tracking active");
                        self.tracking_active = true;
                    }
                    self.target = pose;
                    self.last_face = Some(now);
                }
                Ok(None) => {}
                Err(error) => tracing::debug!("{error:#}"),
            }
        }
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        if self
            .last_face
            .is_none_or(|last| now.saturating_duration_since(last) > Duration::from_millis(600))
        {
            if self.tracking_active {
                tracing::info!("Cartoon camera holding neutral pose without a tracked face");
                self.tracking_active = false;
            }
            let blink = if elapsed % 4.3 < 0.16 { 0. } else { 1. };
            self.target = Pose {
                left_eye: blink,
                right_eye: blink,
                ..Pose::default()
            };
        }
        self.pose.approach(self.target, 0.4);
        self.portrait.draw(job.kind, self.pose, elapsed);
        let width = input.width();
        let height = input.height();
        let small_width = (640. * width as f32 / height as f32)
            .round()
            .clamp(1., 2560.) as u32;
        let mut canvas =
            tiny_skia::Pixmap::new(small_width, 640).context("Invalid camera dimensions")?;
        let color = self
            .portrait
            .pixels
            .pixel(0, 0)
            .context("Missing portrait")?
            .demultiply();
        canvas.fill(tiny_skia::Color::from_rgba8(
            color.red(),
            color.green(),
            color.blue(),
            255,
        ));
        canvas.draw_pixmap(
            0,
            0,
            self.portrait.pixels.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::from_translate((small_width as f32 - 640.) / 2., 0.),
            None,
        );
        let mut bgra = pixel_buffer(small_width as usize, 640, cv::PixelFormat::_32_BGRA)?;
        unsafe {
            bgra.lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT)
                .result()
                .map_err(|e| anyhow!("{e:?}"))?;
        }
        let stride = unsafe { CVPixelBufferGetBytesPerRow(&bgra) };
        let address = unsafe { CVPixelBufferGetBaseAddress(&bgra) };
        if address.is_null() {
            unsafe {
                bgra.unlock_lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT);
            }
            anyhow::bail!("Avatar buffer is not addressable");
        }
        let pixels = unsafe { std::slice::from_raw_parts_mut(address, stride * 640) };
        for (row, rgba) in pixels
            .chunks_mut(stride)
            .zip(canvas.data().chunks_exact(small_width as usize * 4))
        {
            for (to, from) in row.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
                to.copy_from_slice(&[from[2], from[1], from[0], 255]);
            }
        }
        unsafe {
            bgra.unlock_lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT);
        }
        let output = pixel_buffer(width, height, input.pixel_format())?;
        self.transfer
            .transfer(&bgra, &output)
            .map_err(|e| anyhow!("Avatar conversion failed: {e:?}"))?;
        let desc = cm::FormatDesc::with_image_buf(&output).map_err(|e| anyhow!("{e:?}"))?;
        let timing = job
            .frame
            .sample_buf
            .timing_info(0)
            .map_err(|e| anyhow!("{e:?}"))?;
        let sample_buf =
            cm::SampleBuf::with_image_buf(&output, true, None, std::ptr::null(), &desc, &timing)
                .map_err(|e| anyhow!("{e:?}"))?;
        Ok(NativeCameraFrame {
            sample_buf,
            timestamp: job.frame.timestamp,
        })
    }
}

fn pixel_buffer(
    width: usize,
    height: usize,
    format: cv::PixelFormat,
) -> anyhow::Result<arc::R<cv::PixelBuf>> {
    let props = cf::Dictionary::new();
    let keys: [&cf::Type; 2] = [
        cv::pixel_buffer::keys::io_surf_props().as_ref(),
        cv::pixel_buffer::keys::metal_compatibility().as_ref(),
    ];
    let values: [&cf::Type; 2] = [props.as_ref(), cf::Boolean::value_true().as_ref()];
    let attrs =
        cf::Dictionary::with_keys_values(&keys, &values).context("Avatar buffer attributes")?;
    cv::PixelBuf::new(width, height, format, Some(&attrs))
        .map_err(|e| anyhow!("Avatar pixel buffer: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn camera_pixels_are_replaced_before_delivery_and_keep_timing() {
        let mut input = pixel_buffer(640, 360, cv::PixelFormat::_32_BGRA).unwrap();
        unsafe {
            input
                .lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT)
                .result()
                .unwrap();
            let stride = CVPixelBufferGetBytesPerRow(&input);
            let data =
                std::slice::from_raw_parts_mut(CVPixelBufferGetBaseAddress(&input), stride * 360);
            for pixel in data.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[255, 0, 255, 255]);
            }
            input
                .unlock_lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT)
                .result()
                .unwrap();
        }
        let timing = cm::SampleTimingInfo {
            duration: cm::Time::new(1, 30),
            pts: cm::Time::new(10, 30),
            dts: cm::Time::invalid(),
        };
        let desc = cm::FormatDesc::with_image_buf(&input).unwrap();
        let sample =
            cm::SampleBuf::with_image_buf(&input, true, None, std::ptr::null(), &desc, &timing)
                .unwrap();
        let frame = NativeCameraFrame {
            sample_buf: sample,
            timestamp: cap_timestamp::Timestamp::Instant(Instant::now()),
        };
        let worker = AvatarWorker::new().unwrap();
        for kind in [AvatarKind::Cat, AvatarKind::Bear, AvatarKind::Fox] {
            let result = worker.render(frame.clone(), kind).await.unwrap();
            assert_eq!(result.sample_buf.pts(), frame.sample_buf.pts());
            assert_eq!(result.sample_buf.image_buf().unwrap().width(), 640);
            assert_eq!(result.sample_buf.image_buf().unwrap().height(), 360);
            let ffmpeg = cap_camera_ffmpeg::sample_buf_as_ffmpeg(&result.sample_buf).unwrap();
            assert!(
                !ffmpeg
                    .data(0)
                    .chunks_exact(4)
                    .any(|pixel| pixel == [255, 0, 255, 255])
            );
        }
    }
}
