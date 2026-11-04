//! Keynote file format (Keynote 6 and later): a zip of Snappy-compressed protobuf object
//! archives. Apple doesn't document it; field numbers come from the schemas psobot/keynote-parser
//! extracted from Keynote 14.5.

pub mod document;
pub mod drawing;
pub(super) mod iwa;
pub mod media;
pub mod objects;
pub(super) mod package;
pub(super) mod proto;
pub mod render;
pub mod rich_text;
pub mod style;
mod text;
