use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::permissions::PermissionsGroup;
use crate::tools::keynote::format::document::KeynoteDocument;
use crate::tools::keynote::{resolve_existing, run_blocking};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteDeleteSlidesTool {
    /// path to the *.key file
    path: String,
    /// 1-based numbers of the slides to delete, as numbered before the deletion
    slides: Vec<u32>,
}

impl KeynoteDeleteSlidesTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let path = resolve_existing(context, &self.path, PermissionsGroup::FsWrite).await?;
        run_blocking(move || {
            let mut document = KeynoteDocument::open(&path)?;

            // delete from the back so the numbers still to go stay valid
            let mut numbers: Vec<usize> = self.slides.iter().map(|e| *e as usize).collect();
            numbers.sort_unstable();
            numbers.dedup();
            let count = document.slide_count()?;
            if let Some(number) = numbers.iter().find(|e| !(1..=count).contains(*e)) {
                anyhow::bail!("slide {number} is out of range, the presentation has {count} slides");
            }
            for number in numbers.into_iter().rev() {
                document.delete_slide(number)?;
            }

            document.save(&path)?;
            document.info()
        })
        .await
    }
}
