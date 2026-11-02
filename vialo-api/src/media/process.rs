//! Blocking decode/resize/encode. Run on the media rayon pool.

use std::{fmt, io::Cursor, path::Path};

#[cfg(feature = "media-heif")]
use image::{RgbImage, RgbaImage};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, imageops::FilterType};
use rayon::prelude::*;

use super::models::MediaContext;

/// Larger sources are rejected at upload
pub const MAX_PIXELS: u64 = 100_000_000;

const POST_WIDTHS: [u32; 3] = [480, 960, 1920];
const AVATAR_SIZES: [u32; 3] = [64, 128, 256];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Raster(ImageFormat),
    /// HEIC and AVIF, both via libheif
    Heif,
}

#[derive(Debug)]
pub enum ProbeError {
    Unsupported,
    Invalid(String),
    TooLarge,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbeError::Unsupported => write!(f, "unsupported format"),
            ProbeError::Invalid(e) => write!(f, "invalid image: {e}"),
            ProbeError::TooLarge => write!(f, "image exceeds {MAX_PIXELS} pixels"),
        }
    }
}

impl std::error::Error for ProbeError {}

impl SourceFormat {
    pub fn from_mime(mime: &str) -> Option<Self> {
        match mime {
            "image/jpeg" => Some(SourceFormat::Raster(ImageFormat::Jpeg)),
            "image/png" => Some(SourceFormat::Raster(ImageFormat::Png)),
            "image/webp" => Some(SourceFormat::Raster(ImageFormat::WebP)),
            "image/heif" | "image/avif" => Some(SourceFormat::Heif),
            _ => None,
        }
    }
}

fn check_size(width: u32, height: u32) -> Result<(u32, u32), ProbeError> {
    if width == 0 || height == 0 {
        return Err(ProbeError::Invalid("zero dimension".into()));
    }
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err(ProbeError::TooLarge);
    }
    Ok((width, height))
}

/// Reads only the header, to reject bombs before queueing
pub fn probe(path: &Path, format: SourceFormat) -> Result<(u32, u32), ProbeError> {
    match format {
        SourceFormat::Raster(format) => {
            let file = std::fs::File::open(path).map_err(|e| ProbeError::Invalid(e.to_string()))?;
            let (w, h) = ImageReader::with_format(std::io::BufReader::new(file), format)
                .into_dimensions()
                .map_err(|e| ProbeError::Invalid(e.to_string()))?;
            check_size(w, h)
        }
        #[cfg(feature = "media-heif")]
        SourceFormat::Heif => {
            let bytes = std::fs::read(path).map_err(|e| ProbeError::Invalid(e.to_string()))?;
            let ctx = libheif_rs::HeifContext::read_from_bytes(&bytes)
                .map_err(|e| ProbeError::Invalid(e.to_string()))?;
            let handle = ctx
                .primary_image_handle()
                .map_err(|e| ProbeError::Invalid(e.to_string()))?;
            check_size(handle.width(), handle.height())
        }
        #[cfg(not(feature = "media-heif"))]
        SourceFormat::Heif => Err(ProbeError::Unsupported),
    }
}

pub struct Variant {
    pub width: u32,
    pub avif: Vec<u8>,
}

pub struct Processed {
    pub width: u32,
    pub height: u32,
    pub variants: Vec<Variant>,
}

pub fn process(src: &[u8], context: MediaContext) -> anyhow::Result<Processed> {
    // Decode with orientation applied; metadata is dropped
    let format = infer::get(src).and_then(|t| SourceFormat::from_mime(t.mime_type()));
    let img = match format.ok_or(ProbeError::Unsupported)? {
        SourceFormat::Raster(format) => {
            let mut limits = Limits::default();
            limits.max_alloc = Some(MAX_PIXELS * 8);
            let mut reader = ImageReader::with_format(Cursor::new(src), format);
            reader.limits(limits);
            let mut decoder = reader.into_decoder()?;
            let orientation = decoder.orientation()?;
            let mut img = DynamicImage::from_decoder(decoder)?;
            img.apply_orientation(orientation);
            img
        }
        // libheif applies irot/imir itself
        #[cfg(feature = "media-heif")]
        SourceFormat::Heif => {
            use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

            let ctx = HeifContext::read_from_bytes(src)?;
            let handle = ctx.primary_image_handle()?;
            check_size(handle.width(), handle.height())?;
            let alpha = handle.has_alpha_channel();
            let chroma = if alpha { RgbChroma::Rgba } else { RgbChroma::Rgb };
            let decoded = LibHeif::new().decode(&handle, ColorSpace::Rgb(chroma), None)?;
            let planes = decoded.planes();
            let plane = planes
                .interleaved
                .ok_or_else(|| anyhow::anyhow!("libheif returned no interleaved plane"))?;

            // Rows are padded to `stride`
            let row = plane.width as usize * if alpha { 4 } else { 3 };
            let mut pixels = Vec::with_capacity(row * plane.height as usize);
            for line in plane.data.chunks(plane.stride).take(plane.height as usize) {
                pixels.extend_from_slice(&line[..row]);
            }

            let bad = || anyhow::anyhow!("libheif plane size mismatch");
            if alpha {
                DynamicImage::ImageRgba8(RgbaImage::from_raw(plane.width, plane.height, pixels).ok_or_else(bad)?)
            } else {
                DynamicImage::ImageRgb8(RgbImage::from_raw(plane.width, plane.height, pixels).ok_or_else(bad)?)
            }
        }
        #[cfg(not(feature = "media-heif"))]
        SourceFormat::Heif => return Err(ProbeError::Unsupported.into()),
    };
    let (width, height) = (img.width(), img.height());

    let (base, targets, quality, speed) = match context {
        MediaContext::Post => (img, &POST_WIDTHS[..], 70.0, 6),
        MediaContext::Avatar => {
            let side = width.min(height);
            let square = img.crop_imm((width - side) / 2, (height - side) / 2, side, side);
            (square, &AVATAR_SIZES[..], 75.0, 5)
        }
    };
    let (bw, bh) = (base.width(), base.height());

    // Targets clamped to the source; never upscales
    let mut widths: Vec<u32> = targets.iter().map(|t| (*t).min(bw)).collect();
    widths.dedup();

    let encoder = ravif::Encoder::new()
        .with_quality(quality)
        .with_alpha_quality(quality)
        .with_speed(speed);
    let variants = widths
        .into_par_iter()
        .map(|w| {
            let h = ((bh as u64 * w as u64 + bw as u64 / 2) / bw as u64).max(1) as u32;
            // Box prefilter down to 2x first: Lanczos straight from 48MP is far slower
            let resized = if (w, h) == (bw, bh) {
                base.clone()
            } else if bw > w * 4 {
                base.thumbnail_exact(w * 2, h * 2).resize_exact(w, h, FilterType::Lanczos3)
            } else {
                base.resize_exact(w, h, FilterType::Lanczos3)
            };
            // ravif leaves the alpha plane out when every pixel is opaque
            let rgba = resized.into_rgba8();
            let px: Vec<ravif::RGBA8> = rgba
                .as_raw()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&[r, g, b, a]| ravif::RGBA8::new(r, g, b, a))
                .collect();
            let avif = encoder.encode_rgba(ravif::Img::new(&px[..], w as usize, h as usize))?.avif_file;
            Ok(Variant { width: w, avif })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Processed { width, height, variants })
}
