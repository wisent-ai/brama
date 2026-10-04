//! `multipart/form-data` bodies for the provider calls that take files: a
//! voice cloned from recordings, a picture edited from the images it is
//! handed. The form is written here rather than by the HTTP client, so the
//! client keeps the small feature set every other provider call uses.

use crate::types::{GatewayRefusal, Refusal};

/// One form being written: its boundary and the bytes so far.
pub(super) struct Form {
    boundary: String,
    body: Vec<u8>,
}

impl Form {
    pub(super) fn new() -> Self {
        Self {
            boundary: format!("brama-{}", uuid::Uuid::new_v4().simple()),
            body: Vec::new(),
        }
    }

    /// One text field.
    pub(super) fn text(&mut self, name: &str, value: &str) -> Result<(), Refusal> {
        self.part(name, None, None, value.as_bytes())
    }

    /// One file field.
    pub(super) fn file(
        &mut self,
        name: &str,
        filename: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<(), Refusal> {
        self.part(name, Some(filename), Some(content_type), bytes)
    }

    /// The `Content-Type` header value and the finished body.
    pub(super) fn finish(mut self) -> (String, Vec<u8>) {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (
            format!("multipart/form-data; boundary={}", self.boundary),
            self.body,
        )
    }

    /// One part. A header value that could end its own line or quote is
    /// refused, because it would rewrite the form.
    fn part(
        &mut self,
        name: &str,
        filename: Option<&str>,
        content_type: Option<&str>,
        value: &[u8],
    ) -> Result<(), Refusal> {
        for header in filename.into_iter().chain(content_type) {
            if header.is_empty() || header.contains(['\r', '\n', '"']) {
                return Err(Refusal::gateway(
                    GatewayRefusal::InvalidRequest,
                    format!(
                        "invalid_request: `{}` cannot be written into a form header",
                        header.escape_debug()
                    ),
                ));
            }
        }
        let body = &mut self.body;
        body.extend_from_slice(format!("--{}\r\n", self.boundary).as_bytes());
        let disposition = match filename {
            Some(filename) => format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n"
            ),
            None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n"),
        };
        body.extend_from_slice(disposition.as_bytes());
        if let Some(content_type) = content_type {
            body.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(value);
        body.extend_from_slice(b"\r\n");
        Ok(())
    }
}
