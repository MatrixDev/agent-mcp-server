use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::anyhow;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone)]
pub struct PathResolver {
    home_dir: Arc<Path>,
    current_dir: Arc<Path>,
}

impl PathResolver {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new(current_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let home_dir = std::env::home_dir().ok_or_else(|| anyhow!("home directory not found"))?;
        let current_dir = current_dir.as_ref().to_path_buf();

        Ok(Self {
            home_dir: Arc::from(home_dir),
            current_dir: Arc::from(current_dir),
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn resolve(&self, path: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
        let absolute = self.make_absolute(path.as_ref());
        self.resolve_symlinks(&absolute).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn make_absolute(&self, path: &Path) -> PathBuf {
        if let Ok(rest) = path.strip_prefix("~") {
            self.home_dir.join(rest)
        } else if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.current_dir.join(path)
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn resolve_symlinks(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let mut resolved = PathBuf::new();
        for component in path.components() {
            match component {
                Component::RootDir => {
                    resolved.push(component);
                }
                Component::Prefix(e) => {
                    return Err(anyhow!("prefixes are not supported: {e:?}"));
                }
                Component::CurDir => {
                    continue;
                }
                Component::ParentDir => {
                    resolved.pop();
                }
                Component::Normal(e) => {
                    let candidate = resolved.join(e);
                    resolved = tokio::fs::canonicalize(&candidate).await.unwrap_or(candidate);
                }
            }
        }

        Ok(resolved)
    }
}
