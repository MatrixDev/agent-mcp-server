use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use glob::MatchOptions;

use crate::path_resolver::PathResolver;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug)]
pub struct Permissions {
    path_resolver: PathResolver,
    fs: HashMap<PermissionsPair, Vec<glob::Pattern>>,
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
        let (prefix, suffix) = Self::split_on_first_wildcard(path);
        let resolved = self.path_resolver.resolve(prefix).await?;
        let path = if suffix.as_os_str().is_empty() {
            resolved
        } else {
            resolved.join(suffix)
        };
        let pattern = glob::Pattern::new(path.to_string_lossy().as_ref())?;
        self.fs.entry(PermissionsPair(group, kind)).or_default().push(pattern);
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn check(&self, group: PermissionsGroup, path: impl AsRef<Path>) -> anyhow::Result<()> {
        const MATCH_OPTIONS: MatchOptions = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };

        let path = self.path_resolver.resolve(path).await?;

        if let Some(entries) = self.fs.get(&PermissionsPair(group, PermissionsKind::Deny)) {
            if entries.iter().any(|e| e.matches_path_with(&path, MATCH_OPTIONS)) {
                return Err(anyhow!("permission to file {path:?} is denied"));
            }
        }

        if let Some(entries) = self.fs.get(&PermissionsPair(group, PermissionsKind::Allow)) {
            if entries.iter().any(|e| e.matches_path_with(&path, MATCH_OPTIONS)) {
                return Ok(());
            }
        }

        Err(anyhow!("permission to file {path:?} was not granted"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn split_on_first_wildcard(raw: impl AsRef<Path>) -> (PathBuf, PathBuf) {
        let (mut base, mut tail) = (PathBuf::new(), PathBuf::new());

        let mut components = raw.as_ref().components();
        while let Some(component) = components.next() {
            if component.as_os_str().to_string_lossy().contains(['*', '?', '[']) {
                tail.push(component);
                break;
            }
            base.push(component);
        }
        tail.extend(components);

        (base, tail)
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
