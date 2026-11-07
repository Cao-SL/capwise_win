use anyhow::ensure;
use cap_camera_effects::SkinSmoother;
use std::time::Instant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 6 || args.len() == 7,
        "usage: skin-smooth INPUT.rgba WIDTH HEIGHT STRENGTH OUTPUT.rgba [PERSON_MASK.r8]"
    );
    let width: u32 = args[2].parse()?;
    let height: u32 = args[3].parse()?;
    let strength: f32 = args[4].parse()?;
    let pixels = std::fs::read(&args[1])?;
    ensure!(
        pixels.len() == (width * height * 4) as usize,
        "invalid frame size"
    );

    let instance = wgpu::Instance::default();
    let adapter = instance.request_adapter(&Default::default()).await?;
    let (device, queue) = adapter.request_device(&Default::default()).await?;
    let input = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Skin smooth verification input"),
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

    let person_mask = args.get(6).map(|path| -> anyhow::Result<_> {
        let mask = std::fs::read(path)?;
        ensure!(mask.len() == (width * height) as usize, "invalid mask size");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Skin smooth verification mask"),
            size: input.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &mask,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            texture.size(),
        );
        Ok(texture.create_view(&Default::default()))
    });
    let person_mask = person_mask.transpose()?;

    let mut smoother = SkinSmoother::new(&device);
    let mut timings = Vec::new();
    for _ in 0..30 {
        let start = Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        smoother.process_into_encoder(
            &device,
            &queue,
            &input,
            &mut encoder,
            strength,
            person_mask.as_ref(),
        );
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::Wait)?;
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    println!("median_frame_ms: {:.2}", timings[timings.len() / 2]);

    let output = smoother.output_texture().expect("processed above");
    let row_bytes = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Skin smooth verification readback"),
        size: u64::from(row_bytes) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        output.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(height),
            },
        },
        output.size(),
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::Wait)?;
    let mapped = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity(pixels.len());
    for row in mapped.chunks(row_bytes as usize) {
        rgba.extend_from_slice(&row[..(width * 4) as usize]);
    }
    std::fs::write(&args[5], rgba)?;
    Ok(())
}
