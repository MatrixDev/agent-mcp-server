use std::path::Path;

////////////////////////////////////////////////////////////////////////////////
const DEFAULT_MIME_TYPE: &str = "application/octet-stream";

////////////////////////////////////////////////////////////////////////////////
pub fn mime_type(path: &Path, bytes: &[u8]) -> String {
    if let Some(mime) = mime_guess::from_path(path).first() {
        return mime.essence_str().to_owned();
    }

    let sniffed = infer::get(bytes).map_or(DEFAULT_MIME_TYPE, |e| e.mime_type());
    sniffed.to_owned()
}
