//! Writing a rendered artifact to the path the operator named.
//!
//! One image goes to that exact path; several are numbered beside it, so a
//! request for four images does not leave three of them nowhere while the
//! command reports success.

use std::path::{Path, PathBuf};

use base64::Engine;

pub(super) fn write_artifact(path: &str, index: usize, total: usize, encoded: &str) {
    let destination = numbered(path, index, total);
    let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("the provider's image is not base64: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = std::fs::write(&destination, &bytes) {
        eprintln!("cannot write {}: {error}", destination.display());
        std::process::exit(1);
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
    let mime_type = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
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
/// overwrites its predecessor.
fn numbered(path: &str, index: usize, total: usize) -> PathBuf {
    if total <= 1 {
        return PathBuf::from(path);
    }
    let path = Path::new(path);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    let extension = path.extension().and_then(|value| value.to_str());
    let name = match extension {
        Some(extension) => format!("{stem}-{index}.{extension}"),
        None => format!("{stem}-{index}"),
    };
    path.with_file_name(name)
}
