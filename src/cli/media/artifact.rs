//! Writing a rendered artifact to the path the operator named.
//!
//! One image goes to that exact path; several are numbered beside it, so a
//! request for four images does not leave three of them nowhere while the
//! command reports success.

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use base64::Engine;

/// The PNG file signature's first bytes: https://www.w3.org/TR/png/#5PNG-file-signature
const PNG_SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G'];
/// A JPEG stream opens with SOI (FF D8) and a marker (FF):
/// https://www.w3.org/Graphics/JPEG/itu-t81.pdf (B.1.1.3, B.2.1)
const JPEG_SIGNATURE: &[u8] = &[0xFF, 0xD8, 0xFF];

pub(super) fn write_artifact(path: &str, index: usize, total: usize, encoded: &str) {
    let destination = match numbered(path, index, total) {
        Ok(destination) => destination,
        Err(error) => crate::cli::refused(error),
    };
    let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(bytes) => bytes,
        Err(error) => crate::cli::refused(format!("the provider's image is not base64: {error}")),
    };
    if let Err(error) = std::fs::write(&destination, &bytes) {
        crate::cli::refused(format!("cannot write {}: {error}", destination.display()));
    }
    println!(
        "[{index}] {} bytes written to {}",
        bytes.len(),
        destination.display()
    );
}

/// One input image file as a `data:` URL. The image type is read from the
/// file's own signature, so a misnamed file is described by what it holds.
pub(super) fn data_url(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    // A WebP file is a RIFF container: "RIFF", its size as a little-endian
    // u32, then "WEBP" (https://developers.google.com/speed/webp/docs/riff_container).
    let webp = bytes
        .strip_prefix(b"RIFF")
        .and_then(|rest| rest.get(std::mem::size_of::<u32>()..))
        .is_some_and(|rest| rest.starts_with(b"WEBP"));
    let mime_type = if bytes.starts_with(PNG_SIGNATURE) {
        "image/png"
    } else if bytes.starts_with(JPEG_SIGNATURE) {
        "image/jpeg"
    } else if webp {
        "image/webp"
    } else {
        return Err(format!("{path} is not a PNG, JPEG or WebP image"));
    };
    Ok(format!(
        "data:{mime_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// Write one artifact to the exact path asked for; number the rest so nothing
/// overwrites its predecessor. A path that names no file is refused.
fn numbered(path: &str, index: usize, total: usize) -> Result<PathBuf, String> {
    if NonZeroUsize::new(total) == Some(NonZeroUsize::MIN) {
        return Ok(PathBuf::from(path));
    }
    let path = Path::new(path);
    let stem = path
        .file_stem()
        .ok_or_else(|| format!("--output {} names no file to number the images after", path.display()))?;
    let mut name = OsString::from(stem);
    name.push(format!("-{index}"));
    if let Some(extension) = path.extension() {
        name.push(".");
        name.push(extension);
    }
    Ok(path.with_file_name(name))
}
