//! Keynote package: a zip holding `Index/*.iwa` object components next to media (`Data/`),
//! metadata (`Metadata/`) and previews.
//!
//! Every component is decoded up front so objects can be looked up and cross-referenced by id.
//! Saving re-encodes only the components that changed; every other entry is copied from the
//! source zip byte for byte.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::iwa::{IwaObject, decode_iwa, encode_iwa};
use super::proto::{Message, ReferenceAction, rewrite_references};

////////////////////////////////////////////////////////////////////////////////
pub const TYPE_PACKAGE_METADATA: u32 = 11006;

////////////////////////////////////////////////////////////////////////////////
mod package_metadata {
    pub const LAST_OBJECT_IDENTIFIER: u32 = 1;
    pub const COMPONENTS: u32 = 3;
}

////////////////////////////////////////////////////////////////////////////////
pub mod component_info {
    pub const IDENTIFIER: u32 = 1;
    pub const PREFERRED_LOCATOR: u32 = 2;
    pub const LOCATOR: u32 = 3;
    pub const EXTERNAL_REFERENCES: u32 = 6;
    pub const DATA_REFERENCES: u32 = 7;
    pub const OBJECT_UUID_MAP_ENTRIES: u32 = 11;
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Default)]
struct Component {
    objects: Vec<IwaObject>,
    dirty: bool,
}

////////////////////////////////////////////////////////////////////////////////
pub struct KeyPackage {
    source: PathBuf,
    /// entry names of the source zip, in their original order
    entries: Vec<String>,
    /// decoded `Index/*.iwa` components by entry name
    components: BTreeMap<String, Component>,
    /// entry name of the component holding each object
    locations: HashMap<u64, String>,
    /// entries replacing or adding to the source ones
    files: BTreeMap<String, Vec<u8>>,
    /// source entries left out when saving
    removed: HashSet<String>,
}

impl KeyPackage {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn open(path: &Path) -> Result<Self> {
        if path.is_dir() {
            bail!("{path:?} is a package folder, only single-file .key presentations are supported");
        }

        let file = File::open(path).with_context(|| format!("failed to open {path:?}"))?;
        let mut archive = ZipArchive::new(file).with_context(|| format!("{path:?} is not a Keynote file"))?;

        let mut entries = Vec::new();
        let mut components = BTreeMap::new();
        let mut locations = HashMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name().to_string();
            entries.push(name.clone());
            if !is_component(&name) {
                continue;
            }

            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            let objects = decode_iwa(&bytes).with_context(|| format!("failed to decode {name}"))?;
            for object in &objects {
                locations.insert(object.identifier(), name.clone());
            }
            components.insert(name, Component { objects, dirty: false });
        }

        if components.is_empty() {
            bail!("{path:?} has no Index/*.iwa entries, it is not a Keynote 6+ file");
        }

        Ok(Self {
            source: path.to_path_buf(),
            entries,
            components,
            locations,
            files: Default::default(),
            removed: Default::default(),
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Writes the package to `path` through a temporary file, so a failure leaves it untouched
    pub fn save(&self, path: &Path) -> Result<()> {
        let folder = path.parent().ok_or_else(|| anyhow!("{path:?} has no parent folder"))?;
        let temporary = folder.join(format!(".{}.tmp-{:016x}", file_name(path), rand::random::<u64>()));

        let result = self.write(&temporary).and_then(|_| {
            std::fs::rename(&temporary, path).with_context(|| format!("failed to move the saved file to {path:?}"))
        });
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn write(&self, path: &Path) -> Result<()> {
        let source = File::open(&self.source).with_context(|| format!("failed to reopen {:?}", self.source))?;
        let mut source = ZipArchive::new(source)?;
        let target = File::create(path).with_context(|| format!("failed to create {path:?}"))?;
        let mut target = ZipWriter::new(target);

        // Keynote stores entries uncompressed, the IWA payloads are compressed already
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut written = HashSet::new();

        for (index, name) in self.entries.iter().enumerate() {
            if self.removed.contains(name) {
                continue;
            }
            written.insert(name.as_str());

            if let Some(bytes) = self.replacement(name)? {
                target.start_file(name.as_str(), options)?;
                target.write_all(&bytes)?;
            } else {
                target.raw_copy_file(source.by_index_raw(index)?)?;
            }
        }

        let added = self.components.keys().chain(self.files.keys());
        for name in added {
            if written.insert(name.as_str()) {
                let bytes = self
                    .replacement(name)?
                    .ok_or_else(|| anyhow!("{name} has no contents"))?;
                target.start_file(name.as_str(), options)?;
                target.write_all(&bytes)?;
            }
        }

        target.finish()?.sync_all()?;
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn replacement(&self, name: &str) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.files.get(name) {
            return Ok(Some(bytes.clone()));
        }
        match self.components.get(name) {
            Some(component) if component.dirty => Ok(Some(encode_iwa(&component.objects)?)),
            _ => Ok(None),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Objects
    ////////////////////////////////////////////////////////////////////////////////

    pub fn object(&self, identifier: u64) -> Result<&IwaObject> {
        let component = self.component_of(identifier)?;
        let objects = &self.components[component].objects;
        objects
            .iter()
            .find(|e| e.identifier() == identifier)
            .ok_or_else(|| anyhow!("object {identifier} is missing"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn message(&self, identifier: u64) -> Result<Message> {
        self.object(identifier)?
            .message()
            .with_context(|| format!("failed to decode object {identifier}"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn contains(&self, identifier: u64) -> bool {
        self.locations.contains_key(&identifier)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn component_of(&self, identifier: u64) -> Result<&str> {
        let component = self.locations.get(&identifier);
        component
            .map(String::as_str)
            .ok_or_else(|| anyhow!("object {identifier} is missing"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn objects(&self) -> impl Iterator<Item = &IwaObject> {
        self.components.values().flat_map(|e| e.objects.iter())
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn component_objects(&self, component: &str) -> impl Iterator<Item = &IwaObject> {
        self.components
            .get(component)
            .into_iter()
            .flat_map(|e| e.objects.iter())
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn find_type(&self, message_type: u32) -> Vec<u64> {
        let objects = self.objects().filter(|e| e.message_type() == message_type);
        objects.map(IwaObject::identifier).collect()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn find_single(&self, message_type: u32) -> Result<u64> {
        match self.find_type(message_type).as_slice() {
            [identifier] => Ok(*identifier),
            [] => bail!("no object of type {message_type} found"),
            found => bail!("{} objects of type {message_type} found, expected one", found.len()),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Every object referencing each object, by id
    pub fn referrers(&self) -> HashMap<u64, Vec<u64>> {
        let mut referrers: HashMap<u64, Vec<u64>> = HashMap::new();
        for object in self.objects() {
            for reference in object.object_references() {
                referrers.entry(reference).or_default().push(object.identifier());
            }
        }
        referrers
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Edits an object message. References the edit removed are dropped from the object's
    /// reference list and `added` references that ended up in the message are appended to it.
    pub fn update(
        &mut self,
        identifier: u64,
        added: &[u64],
        edit: impl FnOnce(&mut Message) -> Result<()>,
    ) -> Result<()> {
        let object = self.object_mut(identifier)?;
        let mut message = object.message()?;
        let found_before = collect_references(&message);
        edit(&mut message)?;
        let found_after = collect_references(&message);

        let mut references: Vec<_> = object.message_references();
        references.retain(|e| found_after.contains(e) || !found_before.contains(e));
        for reference in added {
            if found_after.contains(reference) && !references.contains(reference) {
                references.push(*reference);
            }
        }
        object.set_message(&message, &references)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn object_mut(&mut self, identifier: u64) -> Result<&mut IwaObject> {
        let name = self
            .locations
            .get(&identifier)
            .ok_or_else(|| anyhow!("object {identifier} is missing"))?;
        let component = self
            .components
            .get_mut(name)
            .ok_or_else(|| anyhow!("component {name} is missing"))?;
        component.dirty = true;
        component
            .objects
            .iter_mut()
            .find(|e| e.identifier() == identifier)
            .ok_or_else(|| anyhow!("object {identifier} is missing"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Adds an object to a component, creating the component when it doesn't exist yet
    pub fn insert(&mut self, component: &str, object: IwaObject) -> Result<()> {
        let identifier = object.identifier();
        if self.locations.contains_key(&identifier) {
            bail!("object {identifier} already exists");
        }

        let entry = self.components.entry(component.to_string()).or_default();
        entry.objects.push(object);
        entry.dirty = true;
        self.removed.remove(component);
        self.locations.insert(identifier, component.to_string());
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn remove(&mut self, identifier: u64) -> Result<IwaObject> {
        let name = self
            .locations
            .remove(&identifier)
            .ok_or_else(|| anyhow!("object {identifier} is missing"))?;
        let component = self
            .components
            .get_mut(&name)
            .ok_or_else(|| anyhow!("component {name} is missing"))?;
        let index = component.objects.iter().position(|e| e.identifier() == identifier);
        let index = index.ok_or_else(|| anyhow!("object {identifier} is missing"))?;
        component.dirty = true;
        Ok(component.objects.remove(index))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn has_component(&self, component: &str) -> bool {
        self.components.contains_key(component)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Drops a component that has no objects left
    pub fn remove_component(&mut self, component: &str) -> Result<()> {
        if let Some(entry) = self.components.get(component)
            && !entry.objects.is_empty()
        {
            bail!("component {component} still holds {} objects", entry.objects.len());
        }
        self.components.remove(component);
        self.removed.insert(component.to_string());
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn set_file(&mut self, name: &str, bytes: Vec<u8>) {
        self.files.insert(name.to_string(), bytes);
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[cfg(test)]
    pub fn component_names(&self) -> impl Iterator<Item = &str> {
        self.components.keys().map(String::as_str)
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[cfg(test)]
    pub fn file_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn has_file(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e == name) || self.files.contains_key(name)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Contents of a non-component entry, None when there is no such entry
    pub fn read_file(&self, name: &str) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.files.get(name) {
            return Ok(Some(bytes.clone()));
        }
        if self.removed.contains(name) || !self.entries.iter().any(|e| e == name) {
            return Ok(None);
        }
        let source = File::open(&self.source).with_context(|| format!("failed to reopen {:?}", self.source))?;
        let mut archive = ZipArchive::new(source)?;
        let mut entry = archive.by_name(name)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Package metadata
    ////////////////////////////////////////////////////////////////////////////////

    pub fn metadata(&self) -> Result<Message> {
        self.message(self.find_single(TYPE_PACKAGE_METADATA)?)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn update_metadata(&mut self, edit: impl FnOnce(&mut Message) -> Result<()>) -> Result<()> {
        let identifier = self.find_single(TYPE_PACKAGE_METADATA)?;
        self.update(identifier, &[], edit)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn last_object_identifier(&self) -> Result<u64> {
        let metadata = self.metadata()?;
        let last = metadata
            .varint(package_metadata::LAST_OBJECT_IDENTIFIER)
            .unwrap_or_default();
        // never trust the counter alone, a stale one would hand out ids already in use
        Ok(self.locations.keys().copied().max().unwrap_or_default().max(last))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn set_last_object_identifier(&mut self, identifier: u64) -> Result<()> {
        self.update_metadata(|e| {
            e.set_varint(package_metadata::LAST_OBJECT_IDENTIFIER, identifier);
            Ok(())
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn component_infos(&self) -> Result<Vec<Message>> {
        self.metadata()?.messages(package_metadata::COMPONENTS)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn set_component_infos(&mut self, infos: &[Message]) -> Result<()> {
        self.update_metadata(|e| {
            e.set_messages(package_metadata::COMPONENTS, infos);
            Ok(())
        })
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Zip entry name of the component a TSP.ComponentInfo describes
pub fn component_path(info: &Message) -> Option<String> {
    let locator = info.string(component_info::LOCATOR);
    let locator = locator.or_else(|| info.string(component_info::PREFERRED_LOCATOR))?;
    Some(format!("Index/{locator}.iwa"))
}

////////////////////////////////////////////////////////////////////////////////
fn collect_references(message: &Message) -> HashSet<u64> {
    let mut found = HashSet::new();
    let mut message = message.clone();
    rewrite_references(&mut message, &mut |e| {
        found.insert(e);
        ReferenceAction::Keep
    });
    found
}

////////////////////////////////////////////////////////////////////////////////
fn is_component(name: &str) -> bool {
    name.starts_with("Index/") && name.ends_with(".iwa")
}

////////////////////////////////////////////////////////////////////////////////
fn file_name(path: &Path) -> String {
    let name = path.file_name().map(|e| e.to_string_lossy().into_owned());
    name.unwrap_or_else(|| "keynote".to_string())
}
