use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

pub fn inspect_overlay_image(encoded: &[u8]) -> Result<(&'static str, u32, u32), String> {
    use image::ImageDecoder as _;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let extension = match reader.format() {
        Some(image::ImageFormat::Png) => "png",
        Some(image::ImageFormat::Jpeg) => "jpg",
        Some(image::ImageFormat::WebP) => "webp",
        Some(image::ImageFormat::Gif) => "gif",
        Some(image::ImageFormat::Bmp) => "bmp",
        _ => return Err("Choose a PNG, JPEG, WebP, GIF or BMP image".into()),
    };
    reader.limits(image_limits());
    let mut decoder = reader.into_decoder().map_err(|error| {
        format!("Cannot decode image (maximum 32,768 pixels per side): {error}")
    })?;
    validate_dimensions(&decoder)?;
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("Cannot decode image: {error}"))?;
    use image::metadata::Orientation;
    let (width, height) = match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (decoded.height(), decoded.width()),
        _ => (decoded.width(), decoded.height()),
    };
    Ok((extension, width, height))
}

pub struct OverlayImage {
    bytes: Vec<u8>,
    extension: &'static str,
    pub width: u32,
    pub height: u32,
    pub name: String,
}

impl OverlayImage {
    const MAX_BYTES: u64 = 64 * 1024 * 1024;

    pub fn from_file(source: &Path) -> Result<Self, String> {
        let file = fs::File::open(source).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > Self::MAX_BYTES {
            return Err("Choose an image file of at most 64 MiB".into());
        }
        let mut bytes = Vec::new();
        file.take(Self::MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let name = source
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Image");
        Self::from_bytes(bytes, name.into())
    }

    pub fn from_clipboard(bytes: Vec<u8>, name: String) -> Result<Self, String> {
        if image::guess_format(&bytes).ok() != Some(image::ImageFormat::Tiff) {
            return Self::from_bytes(bytes, name);
        }
        if bytes.len() as u64 > Self::MAX_BYTES {
            return Err("Image files must be 64 MiB or smaller".into());
        }
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Tiff);
        reader.limits(image_limits());
        let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
        use image::ImageDecoder as _;
        validate_dimensions(&decoder)?;
        let orientation = decoder.orientation().map_err(|error| error.to_string())?;
        let mut decoded =
            image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
        decoded.apply_orientation(orientation);
        let (width, height) = (decoded.width(), decoded.height());
        let mut bytes = std::io::Cursor::new(Vec::new());
        decoded
            .write_to(&mut bytes, image::ImageFormat::Png)
            .map_err(|error| error.to_string())?;
        let bytes = bytes.into_inner();
        if bytes.len() as u64 > Self::MAX_BYTES {
            return Err("Image files must be 64 MiB or smaller".into());
        }
        Ok(Self {
            bytes,
            extension: "png",
            width,
            height,
            name,
        })
    }

    pub fn from_bytes(bytes: Vec<u8>, name: String) -> Result<Self, String> {
        if bytes.len() as u64 > Self::MAX_BYTES {
            return Err("Image files must be 64 MiB or smaller".into());
        }
        let (extension, width, height) = inspect_overlay_image(&bytes)?;
        Ok(Self {
            bytes,
            extension,
            width,
            height,
            name,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn relative_path(&self, id: uuid::Uuid) -> String {
        format!("content/images/{id}.{}", self.extension)
    }

    pub fn write_to_project(&self, project: &Path, id: uuid::Uuid) -> Result<bool, String> {
        let project = project.canonicalize().map_err(|error| error.to_string())?;
        let mut directory = project.clone();
        for part in ["content", "images"] {
            directory.push(part);
            match fs::create_dir(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.to_string()),
            }
            if !fs::symlink_metadata(&directory)
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                return Err("The project image folder must be a local directory".into());
            }
        }
        let destination = project.join(self.relative_path(id));
        let mut file =
            tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
        file.write_all(&self.bytes)
            .and_then(|()| file.as_file().sync_all())
            .map_err(|error| error.to_string())?;
        match file.persist_noclobber(&destination) {
            Ok(_) => Ok(true),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata =
                    fs::symlink_metadata(&destination).map_err(|error| error.to_string())?;
                if metadata.is_file()
                    && metadata.len() == self.bytes.len() as u64
                    && fs::read(&destination).map_err(|error| error.to_string())? == self.bytes
                {
                    Ok(false)
                } else {
                    Err("The project image already exists with different contents".into())
                }
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

fn image_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(32_768);
    limits.max_image_height = Some(32_768);
    limits
}

fn validate_dimensions(decoder: &impl image::ImageDecoder) -> Result<(), String> {
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > 16_777_216
        || decoder.total_bytes() > 128 * 1024 * 1024
    {
        return Err("Images must have at most 16,777,216 pixels (32,768 per side) and decode to at most 128 MiB".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_tiff_is_persisted_as_png_with_its_pixels() {
        let pixels = image::RgbaImage::from_pixel(4, 2, image::Rgba([220, 40, 120, 128]));
        let mut source = std::io::Cursor::new(Vec::new());
        pixels
            .write_to(&mut source, image::ImageFormat::Tiff)
            .unwrap();
        let image = OverlayImage::from_clipboard(source.into_inner(), "Paste".into()).unwrap();
        let project = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4();
        assert!(image.write_to_project(project.path(), id).unwrap());
        let path = project.path().join(image.relative_path(id));
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(image::open(path).unwrap().into_rgba8(), pixels);
    }
}
