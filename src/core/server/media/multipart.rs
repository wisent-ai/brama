//! Reading a `multipart/form-data` body (RFC 7578) into its parts. The one
//! endpoint that takes a form, the OpenAI image edit contract, is read here,
//! so the server keeps the small axum feature set every JSON route uses.

/// One part of the form.
pub(super) struct Part<'a> {
    pub(super) name: String,
    pub(super) filename: Option<String>,
    pub(super) content_type: Option<String>,
    pub(super) body: &'a [u8],
}

/// The boundary a `multipart/form-data` content type names.
pub(super) fn boundary(content_type: &str) -> Option<&str> {
    let mut parameters = content_type.split(';');
    if !parameters
        .next()?
        .trim()
        .eq_ignore_ascii_case("multipart/form-data")
    {
        return None;
    }
    parameters.find_map(|parameter| {
        let (key, value) = parameter.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("boundary")
            .then(|| value.trim().trim_matches('"'))
            .filter(|value| !value.is_empty())
    })
}

/// Every part of the body, in order. A body the boundary does not frame is
/// refused with what was wrong with it, never read as an empty form.
pub(super) fn parts<'a>(body: &'a [u8], boundary: &str) -> Result<Vec<Part<'a>>, String> {
    let opening = format!("--{boundary}");
    let delimiter = format!("\r\n--{boundary}");
    let malformed = |what: &str| format!("the multipart form {what}");
    let start = find(body, opening.as_bytes(), 0)
        .ok_or_else(|| malformed("does not contain its boundary"))?;
    let mut cursor = start + opening.len();
    let mut parts = Vec::new();
    loop {
        if body[cursor..].starts_with(b"--") {
            return Ok(parts);
        }
        if !body[cursor..].starts_with(b"\r\n") {
            return Err(malformed("has a boundary not followed by a line break"));
        }
        cursor += 2;
        let headers_end = find(body, b"\r\n\r\n", cursor)
            .ok_or_else(|| malformed("has a part whose headers never end"))?;
        let headers = std::str::from_utf8(&body[cursor..headers_end])
            .map_err(|_| malformed("has part headers that are not UTF-8"))?;
        let content_start = headers_end + 4;
        let content_end = find(body, delimiter.as_bytes(), content_start)
            .ok_or_else(|| malformed("is not closed by its boundary"))?;
        parts.push(part(headers, &body[content_start..content_end])?);
        cursor = content_end + delimiter.len();
    }
}

fn part<'a>(headers: &str, body: &'a [u8]) -> Result<Part<'a>, String> {
    let mut name = None;
    let mut filename = None;
    let mut content_type = None;
    for line in headers.split("\r\n") {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("content-type") {
            content_type = Some(value.trim().to_string());
        } else if key.trim().eq_ignore_ascii_case("content-disposition") {
            for parameter in value.split(';').skip(1) {
                let Some((key, value)) = parameter.split_once('=') else {
                    continue;
                };
                let value = value.trim().trim_matches('"').to_string();
                match key.trim() {
                    "name" => name = Some(value),
                    "filename" => filename = Some(value),
                    _ => {}
                }
            }
        }
    }
    Ok(Part {
        name: name.ok_or("a multipart part names no field in its Content-Disposition")?,
        filename,
        content_type,
        body,
    })
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| from + position)
}
