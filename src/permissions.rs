use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};

use anyhow::anyhow;

use crate::path_resolver::PathResolver;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug)]
pub struct Permissions {
    path_resolver: PathResolver,
    fs: HashMap<PermissionsPair, Vec<PathBuf>>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PermissionsKind {
    Deny,
    Allow,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PermissionsGroup {
    FsRead,
    FsWrite,
}

impl Permissions {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new(path_resolver: PathResolver) -> Self {
        Permissions {
            path_resolver: path_resolver.clone(),
            fs: Default::default(),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn register_fs_entry(
        &mut self,
        group: PermissionsGroup,
        kind: PermissionsKind,
        path: impl AsRef<Path>,
    ) -> anyhow::Result<()> {
        let path = self.path_resolver.resolve(path).await?;
        self.fs.entry(PermissionsPair(group, kind)).or_default().push(path);
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn check(&self, group: PermissionsGroup, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = self.path_resolver.resolve(path).await?;

        if let Some(entries) = self.fs.get(&PermissionsPair(group, PermissionsKind::Deny)) {
            if entries.iter().any(|e| path.starts_with(e)) {
                return Err(anyhow!("permission to file {path:?} is denied"));
            }
        }

        if let Some(entries) = self.fs.get(&PermissionsPair(group, PermissionsKind::Allow)) {
            if entries.iter().any(|e| path.starts_with(e)) {
                return Ok(());
            }
        }

        Err(anyhow!("permission to file {path:?} was not granted"))
    }
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct PermissionsPair(PermissionsGroup, PermissionsKind);

impl Debug for PermissionsPair {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} / {:?}", self.0, self.1)
    }
}
