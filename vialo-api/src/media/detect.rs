//! Format detection and the upload allowlist

use std::{fs::File, path::Path};

use super::{models::MediaType, process::SourceFormat};

/// Enough for infer's OOXML scan, which reads ~6KB past the first zip entry
pub const SNIFF_LEN: usize = 16 * 1024;

const DOCUMENTS: &[&str] = &[
    "application/pdf",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.oasis.opendocument.text",
    "application/vnd.oasis.opendocument.spreadsheet",
    "application/vnd.oasis.opendocument.presentation",
];

const OOXML: &[(&str, &str, &str)] = &[
    (
        "word/",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docx",
    ),
    (
        "xl/",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsx",
    ),
    (
        "ppt/",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pptx",
    ),
];

pub struct Detected {
    pub media_type: MediaType,
    pub mime: &'static str,
    pub extension: &'static str,
    /// Set for images
    pub source: Option<SourceFormat>,
}

pub fn detect(head: &[u8]) -> Option<Detected> {
    let kind = infer::get(head)?;
    let mime = kind.mime_type();
    let (media_type, source) = match SourceFormat::from_mime(mime) {
        // Heif needs libheif
        Some(source) if source != SourceFormat::Heif || cfg!(feature = "media-heif") => {
            (MediaType::Image, Some(source))
        }
        Some(_) => return None,
        None if DOCUMENTS.contains(&mime) || mime == "application/zip" => {
            (MediaType::Document, None)
        }
        None => return None,
    };
    Some(Detected {
        media_type,
        mime,
        extension: kind.extension(),
        source,
    })
}

/// infer misses some OOXML zip layouts.
/// This function turns what might look like a zip into its OOXML type or rejects it
/// Sync file I/O and a full zip directory parse, so run off the async runtime
pub fn ooxml_from_zip(path: &Path, detected: Detected) -> Option<Detected> {
    let zip = zip::ZipArchive::new(File::open(path).ok()?).ok()?;
    let names: Vec<String> = zip
        .file_names()
        .filter_map(Result::ok)
        .map(|n| n.into_owned())
        .collect();
    if !names.iter().any(|n| n == "[Content_Types].xml") {
        return None;
    }
    OOXML
        .iter()
        .find(|(prefix, ..)| names.iter().any(|n| n.starts_with(prefix)))
        .map(|&(_, mime, extension)| Detected {
            mime,
            extension,
            ..detected
        })
}
