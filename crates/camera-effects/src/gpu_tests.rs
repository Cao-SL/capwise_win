use super::*;

async fn read_output(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Vec<u8> {
    let stride = (texture.width() * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(stride) * u64::from(texture.height()),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(texture.height()),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = tokio::sync::oneshot::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    device.poll(wgpu::PollType::Wait).unwrap();
    rx.await.unwrap().unwrap();
    let mapped = buffer.slice(..).get_mapped_range();
    mapped
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..texture.width() as usize * 4].iter().copied())
        .collect()
}

#[tokio::test]
#[ignore = "requires a GPU and ONNX Runtime"]
async fn effects_preserve_foreground_alpha_and_suppress_background_detail() {
    let instance = wgpu::Instance::default();
    let adapter = instance.request_adapter(&Default::default()).await.unwrap();
    let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
    let width = 640;
    let height = 360;
    let input = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let pixel = if x > width / 2 {
                [255, 0, 0, 255]
            } else {
                let value = if (x / 12 + y / 12) % 2 == 0 { 255 } else { 0 };
                [0, value, value, 255]
            };
            pixels.extend_from_slice(&pixel);
        }
    }
    queue.write_texture(
        input.as_image_copy(),
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        input.size(),
    );
    let mut processor = BlurProcessor::new(&device, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    processor.ensure_textures(&device, width, height);
    processor.set_frame_synchronous(true);
    processor.inference_requested = false;
    processor.mask_initialized = true;
    processor.mask_dimensions = (SEGMENTATION_SIZE, SEGMENTATION_SIZE);
    processor.mask_bytes = (0..SEGMENTATION_SIZE * SEGMENTATION_SIZE)
        .map(|index| {
            if index % SEGMENTATION_SIZE > SEGMENTATION_SIZE / 2 {
                255
            } else {
                0
            }
        })
        .collect();
    processor.mask_dirty = true;
    let mut blurred = Vec::new();
    for mode in [
        BlurMode::Remove,
        BlurMode::Light,
        BlurMode::Heavy,
        BlurMode::Remove,
    ] {
        let output = processor.process(&device, &queue, &input, mode);
        let actual = read_output(&device, &queue, output).await;
        let foreground = ((height / 2 * width + width * 3 / 4) * 4) as usize;
        assert_eq!(&actual[foreground..foreground + 4], &[255, 0, 0, 255]);
        for y in 20..height - 20 {
            for x in 20..width / 2 - 20 {
                let index = ((y * width + x) * 4) as usize;
                if mode == BlurMode::Remove {
                    assert_eq!(actual[index + 3], 0);
                } else {
                    assert_eq!(actual[index + 3], 255);
                    assert!(
                        actual[index] <= 2,
                        "foreground color leaked into the background: {}",
                        actual[index]
                    );
                }
            }
        }
        if mode != BlurMode::Remove {
            let mut detail = 0.0;
            for y in 40..height - 40 {
                for x in 40..width / 2 - 40 {
                    let index = ((y * width + x) * 4 + 1) as usize;
                    detail += (f64::from(actual[index]) - 127.5).powi(2);
                }
            }
            blurred.push(detail);
        }
    }
    assert!(
        blurred[1] < blurred[0] * 0.25,
        "Heavy should remove more detail than Light: {blurred:?}"
    );
    // A soft mask edge a few pixels off a hard colour edge: the guided
    // filter must pull the matte onto the colour edge and keep it sharp.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[40, 60, 200, 255]);
    }
    let colour_edge = width * 3 / 5;
    for row in pixels.chunks_exact_mut(width as usize * 4) {
        for pixel in row.chunks_exact_mut(4).skip(colour_edge as usize) {
            pixel.copy_from_slice(&[230, 180, 140, 255]);
        }
    }
    queue.write_texture(
        input.as_image_copy(),
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        input.size(),
    );
    let mask_edge = (colour_edge - 3) as f32 / width as f32;
    processor.mask_bytes = (0..SEGMENTATION_SIZE * SEGMENTATION_SIZE)
        .map(|index| {
            let x = (index % SEGMENTATION_SIZE) as f32 / SEGMENTATION_SIZE as f32;
            (((x - mask_edge) * 40.0 + 0.5).clamp(0.0, 1.0) * 255.0) as u8
        })
        .collect();
    processor.mask_dirty = true;
    let output = processor.process(&device, &queue, &input, BlurMode::Remove);
    let snapped = read_output(&device, &queue, output).await;
    for row in snapped
        .chunks_exact(width as usize * 4)
        .skip(40)
        .take(height as usize - 80)
    {
        let alpha: Vec<_> = row.chunks_exact(4).map(|pixel| pixel[3]).collect();
        let transition = alpha.iter().position(|&value| value >= 128).unwrap() as i32;
        assert!(
            (transition - colour_edge as i32).abs() <= 2,
            "matte edge at {transition}, colour edge at {colour_edge}"
        );
        let soft = alpha
            .iter()
            .filter(|&&value| value > 8 && value < 247)
            .count();
        // The mask ramps over 16 pixels; the matte must be far tighter.
        assert!(soft <= 6, "matte edge spread over {soft} pixels");
    }
    processor.frame_time = Some(1.0);
    processor.inference_requested = false;
    processor.set_frame_time(1.0);
    assert!(!processor.inference_requested);
    processor.set_frame_time(1.0 + 1.0 / 30.0);
    assert!(processor.inference_requested);
    let readback_dimensions = processor.segmenter.input_dimensions(width, height);
    let mut readback_pointer = None;
    for value in [80, 160] {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[value, value, value, 255]);
        }
        queue.write_texture(
            input.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            input.size(),
        );
        assert!(
            processor
                .readback_downsampled(&device, &queue, &input, true)
                .is_some()
        );
        assert_eq!(
            *readback_pointer.get_or_insert(processor.readback_pixels.as_ptr()),
            processor.readback_pixels.as_ptr()
        );
        assert_eq!(
            processor.readback_pixels.len(),
            (readback_dimensions.0 * readback_dimensions.1 * 4) as usize
        );
        assert!(
            processor
                .readback_pixels
                .chunks_exact(4)
                .all(|pixel| pixel == [value, value, value, 255])
        );
    }
    assert!(processor.mask_initialized);
    processor.set_frame_time(0.5);
    assert!(!processor.mask_initialized);
    assert_eq!(processor.mask_status.status, BlurMaskStatus::Pending);
    processor.ensure_textures(&device, 320, 180);
    assert!(!processor.mask_initialized);
    assert_eq!(processor.mask_status.status, BlurMaskStatus::Pending);
    assert!(processor.inference_requested);
}

mod skin_smoothing {
    use crate::SkinSmoother;

    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 128;

    async fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&Default::default()).await.ok()?;
        adapter.request_device(&Default::default()).await.ok()
    }

    fn smooth(device: &wgpu::Device, queue: &wgpu::Queue, pixels: &[u8], strength: f32) -> Vec<u8> {
        smooth_with_person(device, queue, pixels, strength, None)
    }

    fn smooth_with_person(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pixels: &[u8],
        strength: f32,
        person: Option<u8>,
    ) -> Vec<u8> {
        let person_view = person.map(|value| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                &[value],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(1),
                    rows_per_image: Some(1),
                },
                texture.size(),
            );
            texture.create_view(&Default::default())
        });
        let input = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            input.as_image_copy(),
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: Some(HEIGHT),
            },
            input.size(),
        );
        let mut smoother = SkinSmoother::new(device);
        let mut encoder = device.create_command_encoder(&Default::default());
        smoother.process_into_encoder(
            device,
            queue,
            &input,
            &mut encoder,
            strength,
            person_view.as_ref(),
        );
        let output = smoother.output_texture().unwrap();
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(WIDTH * 4) * u64::from(HEIGHT),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(WIDTH * 4),
                    rows_per_image: Some(HEIGHT),
                },
            },
            output.size(),
        );
        queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait).unwrap();
        slice.get_mapped_range().to_vec()
    }

    fn textured(base: [u8; 3]) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let speck = if (x * 7 + y * 13) % 5 == 0 { -8 } else { 2 };
                for channel in base {
                    pixels.push((i32::from(channel) + speck).clamp(0, 255) as u8);
                }
                pixels.push(255);
            }
        }
        pixels
    }

    fn mean_channels(pixels: &[u8]) -> [f64; 3] {
        let count = (pixels.len() / 4) as f64;
        let mut sums = [0.0; 3];
        for pixel in pixels.chunks_exact(4) {
            for channel in 0..3 {
                sums[channel] += f64::from(pixel[channel]);
            }
        }
        sums.map(|sum| sum / count)
    }

    #[tokio::test]
    async fn brightens_warm_pixels_only_where_the_person_mask_says() {
        let Some((device, queue)) = device().await else {
            return;
        };
        let skin = textured([170, 120, 95]);
        let without_mask = mean_channels(&smooth(&device, &queue, &skin, 1.0));
        let outside_person =
            mean_channels(&smooth_with_person(&device, &queue, &skin, 1.0, Some(0)));
        let on_person = mean_channels(&smooth_with_person(&device, &queue, &skin, 1.0, Some(255)));
        let original = mean_channels(&skin);

        for channel in 0..3 {
            assert!(
                (without_mask[channel] - original[channel]).abs() < 1.5,
                "{without_mask:?} {original:?}"
            );
            assert!(
                (outside_person[channel] - original[channel]).abs() < 1.5,
                "{outside_person:?} {original:?}"
            );
            assert!(
                on_person[channel] > original[channel] + 8.0,
                "{on_person:?} {original:?}"
            );
        }
        // Keeps the hue: the red lead over blue must not collapse into grey.
        assert!(on_person[0] - on_person[2] > (original[0] - original[2]) * 0.9);

        let grey = textured([120, 120, 120]);
        let grey_on_person =
            mean_channels(&smooth_with_person(&device, &queue, &grey, 1.0, Some(255)));
        assert!((grey_on_person[1] - mean_channels(&grey)[1]).abs() < 1.5);
    }

    #[tokio::test]
    async fn brightening_leaves_highlights_unclipped() {
        let Some((device, queue)) = device().await else {
            return;
        };
        let lit = textured([238, 190, 150]);
        let out = smooth_with_person(&device, &queue, &lit, 1.0, Some(255));
        assert!(out.chunks_exact(4).all(|pixel| pixel[0] < 252));
    }

    #[tokio::test]
    async fn levels_blotches_wider_than_the_fine_window() {
        let Some((device, queue)) = device().await else {
            return;
        };
        let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let wave = (f64::from(x) * std::f64::consts::TAU / 16.0).sin()
                    * (f64::from(y) * std::f64::consts::TAU / 16.0).sin();
                let blotch = (wave * 4.0).round() as i32;
                for channel in [190, 145, 120] {
                    pixels.push((channel + blotch) as u8);
                }
                pixels.push(255);
            }
        }
        let range = |pixels: &[u8]| {
            let row = (HEIGHT / 2 + 4) * WIDTH * 4;
            let green =
                (32..WIDTH as usize - 32).map(|x| i32::from(pixels[row as usize + x * 4 + 1]));
            green.clone().max().unwrap() - green.min().unwrap()
        };
        let smoothed = smooth(&device, &queue, &pixels, 1.0);
        assert!(
            range(&smoothed) * 2 <= range(&pixels),
            "{} -> {}",
            range(&pixels),
            range(&smoothed)
        );
    }

    fn roughness(pixels: &[u8]) -> f64 {
        let row = (HEIGHT / 2 * WIDTH * 4) as usize;
        let green: Vec<f64> = (16..WIDTH as usize - 16)
            .map(|x| f64::from(pixels[row + x * 4 + 1]))
            .collect();
        let mean = green.iter().sum::<f64>() / green.len() as f64;
        green.iter().map(|value| (value - mean).abs()).sum::<f64>() / green.len() as f64
    }

    #[tokio::test]
    async fn smooths_skin_texture_and_keeps_other_colors_and_alpha() {
        let Some((device, queue)) = device().await else {
            return;
        };
        let skin = textured([205, 160, 135]);
        let out = smooth(&device, &queue, &skin, 0.6);
        assert!(
            roughness(&out) < roughness(&skin) * 0.8,
            "{} vs {}",
            roughness(&out),
            roughness(&skin)
        );
        assert!(out.chunks(4).all(|pixel| pixel[3] == 255));

        let wall = textured([90, 130, 200]);
        let out = smooth(&device, &queue, &wall, 1.0);
        assert!((roughness(&out) - roughness(&wall)).abs() < 0.2);
    }

    #[tokio::test]
    async fn keeps_strong_edges_on_skin() {
        let Some((device, queue)) = device().await else {
            return;
        };
        let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
        for _ in 0..HEIGHT {
            for x in 0..WIDTH {
                let rgb: [u8; 3] = if x < WIDTH / 2 {
                    [205, 160, 135]
                } else {
                    [60, 35, 25]
                };
                pixels.extend_from_slice(&rgb);
                pixels.push(255);
            }
        }
        let out = smooth(&device, &queue, &pixels, 1.0);
        let row = (HEIGHT / 2 * WIDTH * 4) as usize;
        let green = |x: u32| i32::from(out[row + (x * 4) as usize + 1]);
        assert!(green(WIDTH / 2 - 2) - green(WIDTH / 2 + 1) > 100);
    }

    #[tokio::test]
    async fn zero_strength_is_identity() {
        let Some((device, queue)) = device().await else {
            return;
        };
        let skin = textured([205, 160, 135]);
        let out = smooth(&device, &queue, &skin, 0.0);
        assert!(
            out.iter()
                .zip(&skin)
                .all(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() <= 1)
        );
    }
}
