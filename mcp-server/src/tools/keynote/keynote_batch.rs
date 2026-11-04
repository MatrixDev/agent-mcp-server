use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::permissions::PermissionsGroup;
use crate::tools::keynote::format::document::KeynoteDocument;
use crate::tools::keynote::operations::{
    AddCodeParams, AddImageParams, AddLineParams, AddShapeParams, AddTextBoxParams, DeleteObjectsParams,
    EditObjectParams,
};
use crate::tools::keynote::params::Images;
use crate::tools::keynote::{
    CheckedSlide, NewSlideSpec, SlideContentParams, add_slides, checked_slide, resolve_existing, run_blocking,
};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteBatchTool {
    /// path to the *.key file
    path: String,
    /// operations applied in order and saved together; when one fails nothing is saved
    operations: Vec<BatchItem>,
    /// apply everything and report the result without saving
    dry_run: Option<bool>,
}

////////////////////////////////////////////////////////////////////////////////
/// One operation; `op` picks it, the other fields are those of the tool of the same name
/// (keynote_add_image, keynote_edit_slide, …) without `path`
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BatchItem {
    /// name for the object an add operation creates; later operations use it as "$name" in
    /// ids, build targets and z orders ("above:$name")
    #[serde(rename = "ref")]
    reference: Option<String>,
    #[serde(flatten)]
    operation: BatchOperation,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BatchOperation {
    AddSlides {
        slides: Vec<NewSlideSpec>,
        /// 1-based position of the first new slide, defaults to the end
        position: Option<u32>,
    },
    EditSlide {
        slide: u32,
        #[serde(flatten)]
        content: SlideContentParams,
    },
    MoveSlide {
        from: u32,
        to: u32,
    },
    DeleteSlides {
        slides: Vec<u32>,
    },
    AddImage(AddImageParams),
    AddShape(AddShapeParams),
    AddTextBox(AddTextBoxParams),
    AddLine(AddLineParams),
    AddCode(AddCodeParams),
    EditObject(EditObjectParams),
    DeleteObjects(DeleteObjectsParams),
}

impl BatchOperation {
    ////////////////////////////////////////////////////////////////////////////////
    fn name(&self) -> &'static str {
        match self {
            BatchOperation::AddSlides { .. } => "add_slides",
            BatchOperation::EditSlide { .. } => "edit_slide",
            BatchOperation::MoveSlide { .. } => "move_slide",
            BatchOperation::DeleteSlides { .. } => "delete_slides",
            BatchOperation::AddImage(_) => "add_image",
            BatchOperation::AddShape(_) => "add_shape",
            BatchOperation::AddTextBox(_) => "add_text_box",
            BatchOperation::AddLine(_) => "add_line",
            BatchOperation::AddCode(_) => "add_code",
            BatchOperation::EditObject(_) => "edit_object",
            BatchOperation::DeleteObjects(_) => "delete_objects",
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn image_paths(&self) -> Vec<String> {
        match self {
            BatchOperation::AddSlides { slides, .. } => slides.iter().flat_map(|e| e.content.image_paths()).collect(),
            BatchOperation::EditSlide { content, .. } => content.image_paths(),
            BatchOperation::AddImage(params) => params.image_paths(),
            _ => Vec::new(),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn text_paths(&self) -> Vec<String> {
        match self {
            BatchOperation::AddCode(params) => params.text_paths(),
            _ => Vec::new(),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Applies the operation, returns the object it created or changed and the slides it touched
    fn apply(&self, document: &mut KeynoteDocument, images: &Images) -> Result<(Option<u64>, Vec<usize>)> {
        let identifier = match self {
            BatchOperation::AddSlides { slides, position } => {
                let position = match position {
                    Some(e) => *e as usize,
                    None => document.slide_count()? + 1,
                };
                add_slides(document, slides, position, images)?;
                return Ok((None, (position..position + slides.len()).collect()));
            }
            BatchOperation::EditSlide { slide, content } => {
                document.edit_slide(*slide as usize, &content.content(images)?)?;
                return Ok((None, vec![*slide as usize]));
            }
            BatchOperation::MoveSlide { from, to } => {
                let count = document.slide_count()?;
                if !(1..=count).contains(&(*from as usize)) {
                    bail!("slide {from} is out of range, the presentation has {count} slides");
                }
                document.move_slide(*from as usize, *to as usize)?;
                return Ok((None, vec![(*to as usize).clamp(1, count)]));
            }
            BatchOperation::DeleteSlides { slides } => {
                let mut numbers: Vec<usize> = slides.iter().map(|e| *e as usize).collect();
                numbers.sort_unstable();
                numbers.dedup();
                for number in numbers.iter().rev() {
                    document.delete_slide(*number)?;
                }
                return Ok((None, Vec::new()));
            }
            BatchOperation::DeleteObjects(params) => return Ok((None, params.apply(document, images)?)),
            BatchOperation::AddImage(params) => params.apply(document, images)?,
            BatchOperation::AddShape(params) => params.apply(document, images)?,
            BatchOperation::AddTextBox(params) => params.apply(document, images)?,
            BatchOperation::AddLine(params) => params.apply(document, images)?,
            BatchOperation::AddCode(params) => params.apply(document, images)?,
            BatchOperation::EditObject(params) => params.apply(document, images)?,
        };
        Ok((Some(identifier), vec![document.slide_with_object(identifier)?]))
    }
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Serialize)]
struct OperationResult {
    /// 1-based position in `operations`
    index: usize,
    op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    reference: Option<String>,
}

#[derive(Debug, Serialize)]
struct BatchResult {
    saved: bool,
    operations: Vec<OperationResult>,
    /// the slides the operations touched, as they are now
    slides: Vec<CheckedSlide>,
}

////////////////////////////////////////////////////////////////////////////////
/// Replaces "$name" (alone, or after "above:" / "below:") with "id:N" in every string
fn substitute_references(value: &mut Value, references: &HashMap<String, u64>) -> Result<()> {
    match value {
        Value::String(text) => {
            let (prefix, name) = match text.split_once('$') {
                Some((prefix, name)) if prefix.is_empty() || prefix == "above:" || prefix == "below:" => {
                    (prefix.to_string(), name.to_string())
                }
                _ => return Ok(()),
            };
            if name.is_empty() || !name.chars().all(|e| e.is_alphanumeric() || e == '_' || e == '-') {
                return Ok(());
            }
            let id = references
                .get(&name)
                .ok_or_else(|| anyhow!("unknown ref ${name}, refs are named by earlier add operations"))?;
            *text = format!("{prefix}id:{id}");
        }
        Value::Array(items) => {
            for item in items {
                substitute_references(item, references)?;
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                // text and code are content, not ids
                if matches!(key.as_str(), "text" | "markdown" | "code" | "notes" | "title" | "body" | "runs") {
                    continue;
                }
                substitute_references(item, references)?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl KeynoteBatchTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let path = resolve_existing(context, &self.path, PermissionsGroup::FsWrite).await?;
        let images: Vec<String> = self.operations.iter().flat_map(|e| e.operation.image_paths()).collect();
        let texts: Vec<String> = self.operations.iter().flat_map(|e| e.operation.text_paths()).collect();
        let images = Images::load(context, images, texts).await?;
        let dry_run = self.dry_run.unwrap_or(false);
        let operations = self.operations;

        run_blocking(move || {
            let mut document = KeynoteDocument::open(&path)?;
            let mut references: HashMap<String, u64> = HashMap::new();
            let mut results = Vec::new();
            let mut touched: Vec<usize> = Vec::new();

            for (index, item) in operations.iter().enumerate() {
                let context = || format!("operation {} ({}), nothing was saved", index + 1, item.operation.name());
                let mut value = serde_json::to_value(&item.operation).with_context(context)?;
                substitute_references(&mut value, &references).with_context(context)?;
                let operation: BatchOperation = serde_json::from_value(value).with_context(context)?;

                // slide numbers shift when slides come and go
                match &operation {
                    BatchOperation::DeleteSlides { .. } | BatchOperation::MoveSlide { .. } => touched.clear(),
                    BatchOperation::AddSlides { position: Some(position), slides } => {
                        for number in &mut touched {
                            if *number >= *position as usize {
                                *number += slides.len();
                            }
                        }
                    }
                    _ => {}
                }

                let (id, slides) = operation.apply(&mut document, &images).with_context(context)?;
                if let (Some(name), Some(id)) = (&item.reference, id) {
                    references.insert(name.trim_start_matches('$').to_string(), id);
                }
                touched.extend(slides);
                results.push(OperationResult {
                    index: index + 1,
                    op: operation.name(),
                    id: id.map(|e| format!("id:{e}")),
                    reference: item.reference.clone(),
                });
            }

            if !dry_run {
                document.save(&path)?;
            }
            touched.sort_unstable();
            touched.dedup();
            let count = document.slide_count()?;
            let slides = touched
                .into_iter()
                .filter(|e| (1..=count).contains(e))
                .map(|e| checked_slide(&document, e))
                .collect::<Result<Vec<_>>>()?;
            Ok(BatchResult {
                saved: !dry_run,
                operations: results,
                slides,
            })
        })
        .await
    }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_operations_and_substitutes_refs() {
        let items: Vec<BatchItem> = serde_json::from_str(
            r##"[
                {"op": "add_shape", "ref": "tile", "slide": 2, "shape": "rounded_rect",
                 "frame": {"x": 96, "y": 290, "w": 400, "h": 200}, "fill": "#23222A"},
                {"op": "edit_object", "id": "$tile", "z": "above:$tile", "text": "costs $tile"},
                {"op": "edit_slide", "slide": 2, "background": "#17161B",
                 "builds": [{"target": "$tile", "effect": "apple:dissolve"}]},
                {"op": "delete_slides", "slides": [3]}
            ]"##,
        )
        .unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].reference.as_deref(), Some("tile"));
        assert!(matches!(items[0].operation, BatchOperation::AddShape(_)));

        let references = HashMap::from([("tile".to_string(), 42)]);
        let mut value = serde_json::to_value(&items[1].operation).unwrap();
        substitute_references(&mut value, &references).unwrap();
        let BatchOperation::EditObject(edit) = serde_json::from_value(value).unwrap() else {
            panic!("not an edit");
        };
        assert_eq!(edit.id, "id:42");
        assert_eq!(edit.z.as_deref(), Some("above:id:42"));

        let mut value = serde_json::to_value(&items[2].operation).unwrap();
        substitute_references(&mut value, &references).unwrap();
        assert_eq!(value["builds"][0]["target"], "id:42");

        let mut unknown = serde_json::json!({"id": "$missing"});
        assert!(substitute_references(&mut unknown, &references).is_err());
    }
}
