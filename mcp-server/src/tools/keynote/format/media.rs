//! Media (images) embedded in the package: `Data/` entries listed by the package metadata and
//! counted per object in each component's info.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

use super::document::KeynoteDocument;
use super::package::component_info;
use super::package::component_path;
use super::proto::Message;

////////////////////////////////////////////////////////////////////////////////
/// TSP.PackageMetadata.datas
const METADATA_DATAS: u32 = 4;

/// TSP.DataInfo
mod data_info {
    pub const IDENTIFIER: u32 = 1;
    pub const DIGEST: u32 = 2;
    pub const PREFERRED_FILE_NAME: u32 = 3;
    pub const FILE_NAME: u32 = 4;
    pub const MATERIALIZED_LENGTH: u32 = 18;
}

/// TSP.ComponentDataReference
mod data_reference {
    pub const DATA: u32 = 1;
    pub const OBJECTS: u32 = 2;
    pub const OBJECT: u32 = 1;
    pub const COUNT: u32 = 2;
}

////////////////////////////////////////////////////////////////////////////////
/// An image file ready to embed
#[derive(Debug, Clone)]
pub struct ImageFile {
    /// what Keynote displays: the file itself, or a PNG rendition of an SVG
    pub bytes: Vec<u8>,
    pub file_name: String,
    /// the SVG source when `bytes` is its rendition
    pub svg: Option<Vec<u8>>,
    /// pixel (or point) size
    pub width: f32,
    pub height: f32,
}

impl ImageFile {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("failed to read {path:?}"))?;
        let file_name = path
            .file_name()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".to_string());
        Self::from_bytes(bytes, &file_name)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn from_bytes(bytes: Vec<u8>, file_name: &str) -> Result<Self> {
        let extension = file_name.rsplit_once('.').map(|e| e.1.to_lowercase()).unwrap_or_default();
        match extension.as_str() {
            "svg" => {
                let options = resvg::usvg::Options::default();
                let tree = resvg::usvg::Tree::from_data(&bytes, &options)
                    .map_err(|e| anyhow!("{file_name} is not a valid SVG: {e}"))?;
                let size = tree.size();
                // twice the size keeps the rendition sharp on retina displays
                let scale = 2.0;
                let width = (size.width() * scale).ceil() as u32;
                let height = (size.height() * scale).ceil() as u32;
                let mut pixmap = resvg::tiny_skia::Pixmap::new(width.max(1), height.max(1))
                    .ok_or_else(|| anyhow!("{file_name} is too large to render"))?;
                resvg::render(
                    &tree,
                    resvg::tiny_skia::Transform::from_scale(scale, scale),
                    &mut pixmap.as_mut(),
                );
                let png = pixmap.encode_png().map_err(|e| anyhow!("failed to encode {file_name}: {e}"))?;
                let stem = file_name.rsplit_once('.').map(|e| e.0).unwrap_or(file_name);
                Ok(Self {
                    bytes: png,
                    file_name: format!("{stem}.png"),
                    svg: Some(bytes),
                    width: size.width(),
                    height: size.height(),
                })
            }
            "pdf" => {
                let (width, height) = pdf_size(&bytes).ok_or_else(|| anyhow!("{file_name}: no page size found"))?;
                Ok(Self {
                    bytes,
                    file_name: file_name.to_string(),
                    svg: None,
                    width,
                    height,
                })
            }
            _ => {
                let size = imagesize::blob_size(&bytes)
                    .map_err(|e| anyhow!("{file_name} is not a supported image (png, jpg, gif, heic, tiff, svg, pdf): {e}"))?;
                Ok(Self {
                    bytes,
                    file_name: file_name.to_string(),
                    svg: None,
                    width: size.width as f32,
                    height: size.height as f32,
                })
            }
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Size of the first page's MediaBox
fn pdf_size(bytes: &[u8]) -> Option<(f32, f32)> {
    let text = String::from_utf8_lossy(bytes);
    let pattern = regex::Regex::new(r"/MediaBox\s*\[\s*([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\s*\]").ok()?;
    let captures = pattern.captures(&text)?;
    let value = |index: usize| captures.get(index)?.as_str().parse::<f32>().ok();
    let (x0, y0, x1, y1) = (value(1)?, value(2)?, value(3)?, value(4)?);
    Some(((x1 - x0).abs(), (y1 - y0).abs()))
}

////////////////////////////////////////////////////////////////////////////////
/// A data entry of the package
#[derive(Debug, Clone)]
pub struct DataEntry {
    pub identifier: u64,
    /// entry name in the zip, eg "Data/photo-123.png"
    pub path: Option<String>,
    pub preferred_name: String,
}

impl KeynoteDocument {
    ////////////////////////////////////////////////////////////////////////////////
    /// Every data entry the package metadata lists
    pub(super) fn data_entries(&self) -> Result<Vec<DataEntry>> {
        let metadata = self.package.metadata()?;
        let mut entries = Vec::new();
        for info in metadata.messages(METADATA_DATAS)? {
            let Some(identifier) = info.varint(data_info::IDENTIFIER) else {
                continue;
            };
            let file = info.string(data_info::FILE_NAME).filter(|e| !e.is_empty());
            let preferred_name = info.string(data_info::PREFERRED_FILE_NAME).unwrap_or_default();
            // theme resources without a file name are stored under their preferred one
            let path = file
                .or_else(|| Some(preferred_name.clone()).filter(|e| !e.is_empty()))
                .map(|e| format!("Data/{e}"))
                .filter(|e| self.package.has_file(e));
            entries.push(DataEntry {
                identifier,
                path,
                preferred_name,
            });
        }
        Ok(entries)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn data_entry(&self, identifier: u64) -> Result<Option<DataEntry>> {
        Ok(self.data_entries()?.into_iter().find(|e| e.identifier == identifier))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Contents of a data entry
    pub fn data_bytes(&self, identifier: u64) -> Result<Option<(String, Vec<u8>)>> {
        let Some(entry) = self.data_entry(identifier)? else {
            return Ok(None);
        };
        let Some(path) = entry.path else {
            return Ok(None);
        };
        Ok(self.package.read_file(&path)?.map(|e| (entry.preferred_name, e)))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Stores a file in the package and returns its data id; identical files are stored once
    pub(super) fn add_data(&mut self, bytes: Vec<u8>, preferred_name: &str) -> Result<u64> {
        let digest = sha1_smol::Sha1::from(&bytes).digest().bytes().to_vec();
        let metadata = self.package.metadata()?;
        let infos = metadata.messages(METADATA_DATAS)?;
        for info in &infos {
            if info.bytes(data_info::DIGEST) == Some(digest.as_slice())
                && let Some(identifier) = info.varint(data_info::IDENTIFIER)
                && self.data_bytes(identifier)?.is_some()
            {
                return Ok(identifier);
            }
        }

        let identifier = infos
            .iter()
            .filter_map(|e| e.varint(data_info::IDENTIFIER))
            .max()
            .unwrap_or(0)
            + 1;

        let preferred_name = sanitize_file_name(preferred_name);
        let (stem, extension) = match preferred_name.rsplit_once('.') {
            Some((stem, extension)) => (stem.to_string(), format!(".{extension}")),
            None => (preferred_name.clone(), String::new()),
        };
        let file_name = format!("{stem}-{identifier}{extension}");
        let path = format!("Data/{file_name}");
        if self.package.has_file(&path) {
            bail!("the package already has a {path} entry");
        }

        let mut info = Message::default();
        info.set_varint(data_info::IDENTIFIER, identifier);
        info.set_bytes(data_info::DIGEST, digest);
        info.set_bytes(data_info::PREFERRED_FILE_NAME, preferred_name.into_bytes());
        info.set_bytes(data_info::FILE_NAME, file_name.into_bytes());
        info.set_varint(data_info::MATERIALIZED_LENGTH, bytes.len() as u64);

        self.package.set_file(&path, bytes);
        self.package.update_metadata(|e| {
            e.push_message(METADATA_DATAS, &info);
            Ok(())
        })?;
        Ok(identifier)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Records in the object's component info that it uses a data entry
    pub(super) fn note_data_use(&mut self, object: u64, data: u64) -> Result<()> {
        let component = self.package.component_of(object)?.to_string();
        let mut infos = self.package.component_infos()?;
        let Some(info) = infos
            .iter_mut()
            .find(|e| component_path(e).as_deref() == Some(component.as_str()))
        else {
            return Ok(());
        };

        let mut entries = info.messages(component_info::DATA_REFERENCES)?;
        let index = match entries.iter().position(|e| e.varint(data_reference::DATA) == Some(data)) {
            Some(index) => index,
            None => {
                let mut entry = Message::default();
                entry.set_varint(data_reference::DATA, data);
                entries.push(entry);
                entries.len() - 1
            }
        };

        let entry = &mut entries[index];
        let mut objects = entry.messages(data_reference::OBJECTS)?;
        match objects.iter_mut().find(|e| e.varint(data_reference::OBJECT) == Some(object)) {
            Some(usage) => {
                let count = usage.varint(data_reference::COUNT).unwrap_or(0);
                usage.set_varint(data_reference::COUNT, count + 1);
            }
            None => {
                let mut usage = Message::default();
                usage.set_varint(data_reference::OBJECT, object);
                usage.set_varint(data_reference::COUNT, 1);
                objects.push(usage);
            }
        }
        entry.set_messages(data_reference::OBJECTS, &objects);
        info.set_messages(component_info::DATA_REFERENCES, &entries);
        self.package.set_component_infos(&infos)
    }
}

////////////////////////////////////////////////////////////////////////////////
fn sanitize_file_name(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|e| if e.is_alphanumeric() || matches!(e, '.' | '-' | '_') { e } else { '-' })
        .collect();
    let name = name.trim_matches(['-', '.']).to_string();
    if name.is_empty() { "image".to_string() } else { name }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_image_sizes() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100" fill="red"/></svg>"#;
        let image = ImageFile::from_bytes(svg.to_vec(), "chart.svg").unwrap();
        assert_eq!((image.width, image.height), (200.0, 100.0));
        assert_eq!(image.file_name, "chart.png");
        assert!(image.svg.is_some());

        let png = ImageFile::from_bytes(image.bytes.clone(), "chart.png").unwrap();
        assert_eq!((png.width, png.height), (400.0, 200.0));

        assert_eq!(pdf_size(b"<< /Type /Page /MediaBox [0 0 612 792] >>"), Some((612.0, 792.0)));
        assert!(ImageFile::from_bytes(b"nope".to_vec(), "x.png").is_err());
        assert_eq!(sanitize_file_name("my photo (1).png"), "my-photo--1-.png");
    }
}
