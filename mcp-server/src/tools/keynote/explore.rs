//! Temporary: dumps archives of the fixtures to learn their layout. Settings come from
//! `target/explore.cfg` (KEY=, TYPES=, IDS=, LIMIT= lines), output goes to `target/explore.txt`.
//! `cargo test -p mcp-server explore -- --ignored`

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::path::Path;

use super::format::package::KeyPackage;
use super::format::proto::{Message, Value};

fn dump(message: &Message, depth: usize, out: &mut String) {
    for field in &message.fields {
        let pad = "  ".repeat(depth);
        match &field.value {
            Value::Varint(e) => writeln!(out, "{pad}{}: {e}", field.number).unwrap(),
            Value::Fixed64(e) => writeln!(out, "{pad}{}: f64 {}", field.number, f64::from_bits(*e)).unwrap(),
            Value::Fixed32(e) => writeln!(out, "{pad}{}: f32 {}", field.number, f32::from_bits(*e)).unwrap(),
            Value::Bytes(bytes) => {
                let text = std::str::from_utf8(bytes).ok().filter(|e| {
                    !e.is_empty() && e.chars().all(|c| !c.is_control() || c == '\n' || c == '\u{2029}')
                });
                let nested = Message::decode(bytes).ok().filter(|e| !e.fields.is_empty());
                match (nested, text) {
                    (Some(nested), None) if depth < 14 => {
                        writeln!(out, "{pad}{} {{", field.number).unwrap();
                        dump(&nested, depth + 1, out);
                        writeln!(out, "{pad}}}").unwrap();
                    }
                    (_, Some(text)) => writeln!(out, "{pad}{}: {text:?}", field.number).unwrap(),
                    _ => writeln!(out, "{pad}{}: bytes[{}]", field.number, bytes.len()).unwrap(),
                }
            }
        }
    }
}

fn numbers<T: std::str::FromStr>(config: &HashMap<String, String>, key: &str) -> Vec<T> {
    let value = config.get(key).cloned().unwrap_or_default();
    value.split(',').filter_map(|e| e.trim().parse().ok()).collect()
}

#[test]
#[ignore]
fn explore_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let config = std::fs::read_to_string(root.join("target/explore.cfg")).unwrap_or_default();
    let config: HashMap<String, String> = config
        .lines()
        .filter_map(|e| e.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let file = config.get("KEY").cloned().unwrap_or_else(|| "Rust.key".to_string());
    let limit: usize = config.get("LIMIT").and_then(|e| e.parse().ok()).unwrap_or(3);

    let package = KeyPackage::open(&root.join(&file)).unwrap();
    let mut out = String::new();

    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for object in package.objects() {
        *counts.entry(object.message_type()).or_default() += 1;
    }
    writeln!(out, "TYPES {counts:?}").unwrap();
    let files: Vec<_> = package.file_names().filter(|e| !e.starts_with("Index/")).collect();
    writeln!(out, "FILES {files:?}").unwrap();

    for message_type in numbers::<u32>(&config, "TYPES") {
        for object in package.objects().filter(|e| e.message_type() == message_type).take(limit) {
            writeln!(
                out,
                "=== #{} type {message_type} in {} refs {:?}",
                object.identifier(),
                package.component_of(object.identifier()).unwrap(),
                object.object_references()
            )
            .unwrap();
            dump(&object.message().unwrap(), 1, &mut out);
        }
    }
    for id in numbers::<u64>(&config, "IDS") {
        if let Ok(object) = package.object(id) {
            writeln!(
                out,
                "=== #{id} type {} in {} refs {:?}",
                object.message_type(),
                package.component_of(id).unwrap(),
                object.object_references()
            )
            .unwrap();
            dump(&object.message().unwrap(), 1, &mut out);
        }
    }
    if config.contains_key("METADATA") {
        writeln!(out, "=== METADATA").unwrap();
        dump(&package.metadata().unwrap(), 1, &mut out);
    }
    std::fs::write(root.join("target/explore.txt"), out).unwrap();
}
