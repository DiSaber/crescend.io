//! Host-uploaded prompt images: validation, re-encoding and storage on the data volume.
//!
//! Uploads are decoded and written again from pixels, which drops every metadata
//! block (EXIF location, camera details, comments). Files live beside the database
//! in `game-images/<lobby_id>/<image_id>.<ext>`; rows in `game_images` own them.
use image::{
    DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, codecs::jpeg::JpegEncoder,
};
use std::{io::Cursor, path::PathBuf};

/// Largest accepted upload, in bytes.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Largest accepted width or height, in pixels.
pub const MAX_IMAGE_SIDE: u32 = 4096;
/// Images a lobby may hold at once.
pub const MAX_IMAGES_PER_LOBBY: i64 = 30;
/// Quality for re-encoded JPEGs.
const JPEG_QUALITY: u8 = 85;

#[derive(Debug, PartialEq, Eq)]
pub enum ImageError {
    TooLarge,
    Type,
    Dimensions,
    Invalid,
}

#[derive(Debug)]
pub struct Processed {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub width: u32,
    pub height: u32,
}

pub fn extension(content_type: &str) -> &'static str {
    if content_type == "image/png" {
        "png"
    } else {
        "jpg"
    }
}

/// Accepts JPEG, PNG or WebP within the byte and pixel limits, applies the EXIF
/// orientation, and re-encodes: PNG when the image has transparency, else JPEG.
pub fn process(input: &[u8]) -> Result<Processed, ImageError> {
    if input.len() > MAX_IMAGE_BYTES {
        return Err(ImageError::TooLarge);
    }
    let format = image::guess_format(input).map_err(|_| ImageError::Type)?;
    if !matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP
    ) {
        return Err(ImageError::Type);
    }
    let mut reader = ImageReader::with_format(Cursor::new(input), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(decode_error)?;
    let (width, height) = decoder.dimensions();
    if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        return Err(ImageError::Dimensions);
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(decode_error)?;
    decoded.apply_orientation(orientation);
    let mut bytes = Vec::new();
    let content_type = if decoded.color().has_alpha() {
        decoded
            .to_rgba8()
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .map_err(|_| ImageError::Invalid)?;
        "image/png"
    } else {
        decoded
            .to_rgb8()
            .write_with_encoder(JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY))
            .map_err(|_| ImageError::Invalid)?;
        "image/jpeg"
    };
    Ok(Processed {
        bytes,
        content_type,
        width: decoded.width(),
        height: decoded.height(),
    })
}

fn decode_error(error: image::ImageError) -> ImageError {
    match error {
        image::ImageError::Limits(_) => ImageError::Dimensions,
        image::ImageError::Unsupported(_) => ImageError::Type,
        _ => ImageError::Invalid,
    }
}

/// `game-images` next to the main database file (the backend's data volume).
pub async fn root(database: &crate::database::Database) -> Result<PathBuf, sqlx::Error> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as("PRAGMA database_list")
        .fetch_all(&database.db_pool)
        .await?;
    let file = rows
        .into_iter()
        .find(|(_, name, _)| name == "main")
        .map(|(_, _, file)| file)
        .unwrap_or_default();
    let parent = std::path::Path::new(&file)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    Ok(parent.join("game-images"))
}

pub fn lobby_dir(root: &std::path::Path, lobby: &str) -> PathBuf {
    root.join(lobby)
}

pub fn path(root: &std::path::Path, lobby: &str, image: &str, content_type: &str) -> PathBuf {
    lobby_dir(root, lobby).join(format!("{image}.{}", extension(content_type)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    pub(crate) fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        RgbImage::from_pixel(width, height, Rgb([200, 30, 30]))
            .write_with_encoder(JpegEncoder::new_with_quality(&mut bytes, 90))
            .unwrap();
        bytes
    }

    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        RgbaImage::from_pixel(width, height, Rgba([0, 0, 255, 128]))
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// A JPEG carrying an APP1 EXIF block with a GPS marker string and orientation 6.
    pub(crate) fn jpeg_with_exif(width: u32, height: u32) -> Vec<u8> {
        let plain = jpeg(width, height);
        let mut tiff = b"II*\0\x08\0\0\0".to_vec();
        tiff.extend_from_slice(&1u16.to_le_bytes());
        // Tag 0x0112 Orientation, SHORT, count 1, value 6 (rotate 90 clockwise).
        tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(b"GPS-SECRET-LOCATION");
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut out = plain[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&plain[2..]);
        out
    }

    #[test]
    fn reencodes_and_strips_metadata_while_applying_orientation() {
        let input = jpeg_with_exif(40, 20);
        assert!(input.windows(10).any(|w| w == b"GPS-SECRET"));
        let processed = process(&input).unwrap();
        assert_eq!(processed.content_type, "image/jpeg");
        assert_eq!(
            (processed.width, processed.height),
            (20, 40),
            "rotated upright"
        );
        assert!(!processed.bytes.windows(10).any(|w| w == b"GPS-SECRET"));
        assert!(!processed.bytes.windows(4).any(|w| w == b"Exif"));
        assert_eq!(
            image::guess_format(&processed.bytes).unwrap(),
            ImageFormat::Jpeg
        );
    }

    #[test]
    fn transparent_images_stay_png_and_webp_is_accepted() {
        let processed = process(&png(8, 8)).unwrap();
        assert_eq!(processed.content_type, "image/png");
        let mut webp = Vec::new();
        RgbaImage::from_pixel(6, 4, Rgba([1, 2, 3, 255]))
            .write_to(&mut Cursor::new(&mut webp), ImageFormat::WebP)
            .unwrap();
        let processed = process(&webp).unwrap();
        assert_eq!((processed.width, processed.height), (6, 4));
    }

    #[test]
    fn rejects_other_types_oversized_files_huge_dimensions_and_garbage() {
        assert_eq!(process(b"GIF89a....").unwrap_err(), ImageError::Type);
        assert_eq!(process(b"not an image").unwrap_err(), ImageError::Type);
        assert_eq!(
            process(&vec![0xFF; MAX_IMAGE_BYTES + 1]).unwrap_err(),
            ImageError::TooLarge
        );
        assert_eq!(
            process(&png(MAX_IMAGE_SIDE + 1, 1)).unwrap_err(),
            ImageError::Dimensions
        );
        let mut truncated = jpeg(64, 64);
        truncated.truncate(40);
        assert_eq!(process(&truncated).unwrap_err(), ImageError::Invalid);
    }
}
