//! Tests against two presentations saved by Keynote 14 at the repository root:
//!
//! - Rust.key, 67 slides full of transitions and builds: parsing (text, layouts, transitions,
//!   builds), lossless round trips, and edits that must keep the package consistent
//! - template.key, a theme with a single empty slide: slides made from layouts, transitions
//!   and builds written from scratch

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::format::document::{
    BuildInfo, BuildSpec, BuildStep, BuildTarget, KeynoteDocument, LayoutInfo, SlideContent, SlideInfo,
    TransitionInfo, TransitionSpec,
};
use super::format::iwa::{decode_iwa, decompress, encode_iwa};
use super::format::package::{KeyPackage, component_info, component_path};
use super::format::proto::{Message, ReferenceAction, reference, rewrite_references};
use super::keynote_create::create_presentation;
use super::params::{Images, TextParams};
use super::{BuildParams, NewSlideSpec, SlideContentParams, TransitionParams};

////////////////////////////////////////////////////////////////////////////////
const SLIDE_COUNT: usize = 67;
const TYPE_SLIDE_NODE: u32 = 4;
const TYPE_SLIDE: u32 = 5;
const TYPE_BUILD: u32 = 8;
const TYPE_BUILD_CHUNK: u32 = 153;
/// component of slide 5, the one with three image builds and a code build
const SLIDE_5_COMPONENT: &str = "Index/Slide-2762576.iwa";
const TYPE_STORAGE: u32 = 2001;
const LAYOUTS: [&str; 10] = [
    "Title",
    "Title & Photo",
    "Agenda",
    "Section",
    "Section & Subtitle",
    "Title & Photo Alt",
    "Title & Bullets",
    "Title & Bullets 2H",
    "Title & Bullets 2V",
    "Blank",
];
const TEMPLATE_LAYOUTS: [&str; 8] = [
    "Title",
    "Title & Photo",
    "Agenda",
    "Section",
    "Title & Photo Alt",
    "Title & Bullets",
    "Title & Bullets 2H",
    "Blank",
];

/// KN.SlideNodeArchive fields tracking animations
const NODE_HAS_TRANSITION: u32 = 7;
const NODE_BUILD_EVENT_COUNT: u32 = 15;
const NODE_HAS_EXPLICIT_BUILDS: u32 = 20;

////////////////////////////////////////////////////////////////////////////////
// Helpers
////////////////////////////////////////////////////////////////////////////////

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../Rust.key")
}

fn template() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../template.key")
}

fn temporary(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("mdev-keynote-fixture-{:016x}", rand::random::<u64>()));
    std::fs::create_dir_all(&folder).unwrap();
    folder.join(name)
}

/// Every zip entry with its raw bytes, in archive order
fn zip_entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        entries.push((entry.name().to_string(), bytes));
    }
    entries
}

fn type_counts(package: &KeyPackage) -> BTreeMap<u32, usize> {
    let mut counts = BTreeMap::new();
    for object in package.objects() {
        *counts.entry(object.message_type()).or_default() += 1;
    }
    counts
}

fn slide(document: &KeynoteDocument, number: usize) -> SlideInfo {
    document.info().unwrap().slides.swap_remove(number - 1)
}

/// Slide info without its number and object ids, to compare a slide with its copy
fn content(slide: &SlideInfo) -> String {
    fn strip_ids(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.remove("id");
                map.values_mut().for_each(strip_ids);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip_ids),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(slide).unwrap();
    value.as_object_mut().unwrap().remove("slide");
    strip_ids(&mut value);
    value.to_string()
}

/// Builds without the ids of their targets, to compare with expected ones
fn without_ids(builds: &[BuildInfo]) -> Vec<BuildInfo> {
    let mut builds = builds.to_vec();
    for build in &mut builds {
        build.target.id = None;
    }
    builds
}

fn build(target: &str, text: Option<&str>, effect: &str, delivery: &str, duration: f64) -> BuildInfo {
    BuildInfo {
        target: BuildTarget {
            id: None,
            kind: target.to_string(),
            text: text.map(str::to_string),
        },
        kind: Some("In".to_string()),
        effect: Some(effect.to_string()),
        delivery: Some(delivery.to_string()),
        duration: Some(duration),
    }
}

fn step(build: usize, automatic: bool, duration: f64) -> BuildStep {
    BuildStep {
        build,
        automatic,
        delay: Some(0.0),
        duration: Some(duration),
    }
}

fn spec(layout: Option<&str>, duplicate: Option<u32>, title: Option<&str>, body: Option<&str>) -> NewSlideSpec {
    NewSlideSpec {
        layout: layout.map(str::to_string),
        duplicate,
        content: SlideContentParams {
            title: title.map(|e| TextParams::Plain(e.to_string())),
            body: body.map(|e| TextParams::Plain(e.to_string())),
            ..Default::default()
        },
    }
}

fn layout_names(layouts: &[LayoutInfo]) -> Vec<&str> {
    layouts.iter().map(|e| e.name.as_str()).collect()
}


/// Ids of every reference-shaped value in an object's payloads, listed in the header or not
fn embedded_references(package: &KeyPackage, identifier: u64) -> HashSet<u64> {
    let mut found = HashSet::new();
    for payload in package.object(identifier).unwrap().payloads() {
        let mut message = Message::decode(payload).unwrap();
        rewrite_references(&mut message, &mut |e| {
            found.insert(e);
            ReferenceAction::Keep
        });
    }
    found
}

/// The first reference of `field` in every object of a type within one component
fn references_of_type(package: &KeyPackage, component: &str, message_type: u32, field: u32) -> Vec<(u64, u64)> {
    package
        .component_objects(component)
        .filter(|e| e.message_type() == message_type)
        .map(|e| {
            (
                e.identifier(),
                reference(&e.message().unwrap(), field).unwrap().unwrap(),
            )
        })
        .collect()
}

////////////////////////////////////////////////////////////////////////////////
/// Package invariants an edit must not break, measured against the untouched fixture since
/// Keynote itself leaves a few loose ends (dangling references, components without info)
struct Baseline {
    dangling: HashSet<u64>,
    components_without_info: HashSet<String>,
}

impl Baseline {
    fn of(package: &KeyPackage) -> Self {
        let infos: HashSet<String> = package
            .component_infos()
            .unwrap()
            .iter()
            .filter_map(component_path)
            .collect();
        Self {
            dangling: dangling_references(package),
            components_without_info: package
                .component_names()
                .filter(|e| !infos.contains(*e))
                .map(str::to_string)
                .collect(),
        }
    }

    fn assert_consistent(&self, package: &KeyPackage) {
        // every object lives in exactly one component
        let mut seen = HashSet::new();
        for object in package.objects() {
            assert!(
                seen.insert(object.identifier()),
                "object {} is stored twice",
                object.identifier()
            );
        }

        // new ids never collide with the ones Keynote hands out next
        let metadata = package.metadata().unwrap();
        let last = metadata.varint(1).unwrap();
        assert!(
            seen.iter().all(|e| *e <= last),
            "last_object_identifier {last} is behind the ids in use"
        );

        // no new dangling references
        let dangling: Vec<_> = dangling_references(package)
            .difference(&self.dangling)
            .copied()
            .collect();
        assert!(dangling.is_empty(), "dangling references: {dangling:?}");

        // component infos and components match up
        let infos = package.component_infos().unwrap();
        let mut paths = HashSet::new();
        for info in &infos {
            let path = component_path(info).unwrap();
            assert!(package.has_component(&path), "component info without component: {path}");
            assert!(paths.insert(path.clone()), "component listed twice: {path}");
        }
        for component in package.component_names() {
            assert!(
                paths.contains(component) || self.components_without_info.contains(component),
                "component without info: {component}"
            );
        }

        // external references name the component their object lives in
        let component_ids: HashMap<String, u64> = infos
            .iter()
            .map(|e| {
                (
                    component_path(e).unwrap(),
                    e.varint(component_info::IDENTIFIER).unwrap(),
                )
            })
            .collect();
        for info in &infos {
            for external in info.messages(component_info::EXTERNAL_REFERENCES).unwrap() {
                let (Some(component), Some(object)) = (external.varint(1), external.varint(2)) else {
                    continue;
                };
                let Ok(path) = package.component_of(object) else {
                    continue;
                };
                assert_eq!(
                    component_ids.get(path),
                    Some(&component),
                    "external reference to {object} names component {component}, it lives in {path}"
                );
            }
        }
    }
}

fn dangling_references(package: &KeyPackage) -> HashSet<u64> {
    let references = package.objects().flat_map(|e| e.object_references());
    references.filter(|e| !package.contains(*e)).collect()
}

////////////////////////////////////////////////////////////////////////////////
// Parsing
////////////////////////////////////////////////////////////////////////////////

#[test]
fn decodes_every_component_losslessly() {
    let mut objects = 0;
    let mut components = 0;
    for (name, bytes) in zip_entries(&fixture()) {
        if !name.starts_with("Index/") {
            continue;
        }
        components += 1;

        let decoded = decode_iwa(&bytes).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        for object in &decoded {
            for payload in object.payloads() {
                let message =
                    Message::decode(payload).unwrap_or_else(|e| panic!("{name} #{}: {e:#}", object.identifier()));
                assert_eq!(
                    &message.encode(),
                    payload,
                    "{name} #{} doesn't re-encode",
                    object.identifier()
                );
            }
        }
        objects += decoded.len();

        // the whole object stream, archive headers included, survives a decode/encode cycle
        let reencoded = encode_iwa(&decoded).unwrap();
        assert_eq!(
            decompress(&reencoded).unwrap(),
            decompress(&bytes).unwrap(),
            "{name} stream changed"
        );
    }

    assert_eq!(components, 84);
    assert!(objects > 2000, "only {objects} objects decoded");
}

#[test]
fn indexes_every_object() {
    let package = KeyPackage::open(&fixture()).unwrap();
    let counts = type_counts(&package);

    // 67 slides plus 10 layouts share the slide node and slide archive types
    assert_eq!(counts[&TYPE_SLIDE_NODE], SLIDE_COUNT + LAYOUTS.len());
    assert_eq!(counts[&TYPE_SLIDE], SLIDE_COUNT + LAYOUTS.len());
    assert_eq!(counts[&TYPE_BUILD], 43);
    assert_eq!(counts[&TYPE_BUILD_CHUNK], 63);
    assert_eq!(counts[&TYPE_STORAGE], 316);

    let infos = package.component_infos().unwrap();
    assert!(
        infos
            .iter()
            .any(|e| component_path(e).as_deref() == Some("Index/Document.iwa"))
    );
    assert!(
        infos
            .iter()
            .any(|e| component_path(e).as_deref() == Some("Index/Slide.iwa"))
    );
}

#[test]
fn describes_the_presentation() {
    let info = KeynoteDocument::open(&fixture()).unwrap().info().unwrap();

    assert_eq!((info.width, info.height), (Some(1920.0), Some(1080.0)));
    assert_eq!(layout_names(&info.layouts), LAYOUTS);
    assert_eq!(info.slides.len(), SLIDE_COUNT);
    assert!(info.slides.iter().enumerate().all(|(index, e)| e.slide == index + 1));
    assert!(info.slides.iter().all(|e| !e.skipped && e.notes.is_none()));
    assert!(
        info.slides
            .iter()
            .all(|e| e.layout.as_deref().is_some_and(|e| LAYOUTS.contains(&e)))
    );

    let first = &info.slides[0];
    assert_eq!(first.layout.as_deref(), Some("Title"));
    assert_eq!(first.title.as_deref(), Some("Into to the Rust"));
    assert_eq!(first.body.as_deref(), Some("The most beloved language in the World!"));

    let agenda = &info.slides[1];
    assert_eq!(agenda.layout.as_deref(), Some("Agenda"));
    let agenda_body = agenda.body.as_deref().unwrap();
    assert_eq!(agenda_body.lines().count(), 8);
    assert!(agenda_body.starts_with("Problems of Today\nMemory & Thread safety guaranties\n"));
    assert!(agenda_body.ends_with("Statistics about the Ferris"));

    assert_eq!(info.slides[3].layout.as_deref(), Some("Section"));
    assert_eq!(info.slides[3].title.as_deref(), Some("Memory Safety"));
    assert_eq!(info.slides[3].body.as_deref(), Some(""));

    // non-ASCII text, emoji included
    let ownership = &info.slides[44];
    assert_eq!(ownership.title.as_deref(), Some("Borrow checker - Ownership"));
    assert!(
        ownership
            .body
            .as_deref()
            .unwrap()
            .starts_with("let string1 = \"🦀\".to_string();")
    );
    assert!(info.slides[42].body.as_deref().unwrap().contains("‘static and ‘a"));

    let last = &info.slides[SLIDE_COUNT - 1];
    assert_eq!(last.layout.as_deref(), Some("Title & Bullets 2H"));
    assert_eq!(last.title.as_deref(), Some("And There Is More…"));
    assert_eq!(
        last.body.as_deref(),
        Some("Procedural Macros\nUnit Tests\nBenchmarking\nAsync/Await")
    );
}

#[test]
fn describes_transitions() {
    let info = KeynoteDocument::open(&fixture()).unwrap().info().unwrap();
    let transition = |number: usize| info.slides[number - 1].transition.clone();
    let expected = |effect: &str, duration: f64| {
        Some(TransitionInfo {
            effect: effect.to_string(),
            duration: Some(duration),
            delay: Some(0.5),
            automatic: false,
        })
    };

    // "none" means the transition was removed
    for number in [1, 2, 3, 65, 66, 67] {
        assert_eq!(transition(number), None, "slide {number}");
    }
    assert_eq!(transition(4), expected("apple:magic-move-implied-motion-path", 1.0));
    // stored as a float widened to double
    assert_eq!(
        transition(5),
        expected("apple:magic-move-implied-motion-path", 0.7f32 as f64)
    );
    assert_eq!(transition(7), expected("apple:fade-and-move", 1.0));
    assert_eq!(transition(8), expected("apple:dissolve", 1.0));

    let effects: HashSet<String> = info
        .slides
        .iter()
        .filter_map(|e| Some(e.transition.as_ref()?.effect.clone()))
        .collect();
    for effect in [
        "apple:magic-move-implied-motion-path",
        "apple:fade-and-move",
        "apple:dissolve",
    ] {
        assert!(effects.contains(effect), "{effect} missing from {effects:?}");
    }
}

#[test]
fn describes_builds() {
    let info = KeynoteDocument::open(&fixture()).unwrap().info().unwrap();

    // every build and build chunk in the file is reachable from its slide
    let builds: usize = info.slides.iter().map(|e| e.builds.len()).sum();
    let steps: usize = info.slides.iter().map(|e| e.build_order.len()).sum();
    assert_eq!((builds, steps), (43, 63));
    assert!(
        info.slides
            .iter()
            .flat_map(|e| &e.builds)
            .all(|e| e.target.kind != "missing")
    );
    for slide in &info.slides {
        assert!(
            slide
                .build_order
                .iter()
                .all(|e| (1..=slide.builds.len()).contains(&e.build))
        );
    }

    // three images and the code listing, played code first, the last two images automatically
    let code = "int value;\nstd::cout << \"value = \" << value;\n\n";
    let slide5 = &info.slides[4];
    assert_eq!(
        without_ids(&slide5.builds),
        vec![
            build("image", None, "apple:dissolve", "All at Once", 1.0),
            build("image", None, "apple:dissolve", "All at Once", 1.0),
            build("image", None, "apple:dissolve", "All at Once", 1.0),
            build(
                "body",
                Some(code),
                "apple:dissolve character",
                "All at Once",
                0.7000000000000001
            ),
        ]
    );
    assert_eq!(
        slide5.build_order,
        vec![
            step(4, false, 0.7),
            step(2, false, 1.0),
            step(3, true, 1.0),
            step(1, true, 1.0)
        ]
    );

    // a custom effect on a free text box
    let slide42 = &info.slides[41];
    assert_eq!(
        without_ids(&slide42.builds),
        vec![build(
            "text box",
            Some("It has no Mercy"),
            "com.apple.iWork.Keynote.FromDarkness",
            "All at Once",
            1.0
        )]
    );
    assert_eq!(slide42.build_order, vec![step(1, false, 1.0)]);

    // bullet by bullet: one step per paragraph
    let slide43 = &info.slides[42];
    assert_eq!(slide43.builds.len(), 1);
    assert_eq!(slide43.builds[0].target.kind, "body");
    assert_eq!(slide43.builds[0].delivery.as_deref(), Some("By Highlighted Bullet"));
    assert_eq!(slide43.build_order, vec![step(1, false, 1.0); 3]);

    let slide44 = &info.slides[43];
    assert_eq!(slide44.builds[0].delivery.as_deref(), Some("By Highlighted Bullet"));
    assert_eq!(slide44.build_order.len(), 4);

    // slides without animations
    assert!(info.slides[0].builds.is_empty() && info.slides[0].build_order.is_empty());
}

////////////////////////////////////////////////////////////////////////////////
// Saving and editing
////////////////////////////////////////////////////////////////////////////////

#[test]
fn saving_unchanged_copies_every_entry() {
    let saved = temporary("unchanged.key");
    let document = KeynoteDocument::open(&fixture()).unwrap();
    document.save(&saved).unwrap();

    assert_eq!(zip_entries(&saved), zip_entries(&fixture()));
    let reopened = KeynoteDocument::open(&saved).unwrap();
    assert_eq!(
        serde_json::to_string(&reopened.info().unwrap()).unwrap(),
        serde_json::to_string(&document.info().unwrap()).unwrap()
    );
}

#[test]
fn edits_text_keeping_the_rest() {
    let original = KeynoteDocument::open(&fixture()).unwrap();
    let baseline = Baseline::of(original.package());
    let before = original.info().unwrap();

    let mut document = KeynoteDocument::open(&fixture()).unwrap();
    let changes = SlideContent {
        title: Some("Problems of Tomorrow".to_string()),
        body: Some("Memory safety\nThread safety 🧵\nNew paragraph".to_string()),
        notes: Some("Mention CVE numbers".to_string()),
        skipped: Some(true),
        ..Default::default()
    };
    document.edit_slide(3, &changes).unwrap();

    let saved = temporary("edited.key");
    document.save(&saved).unwrap();
    let document = KeynoteDocument::open(&saved).unwrap();
    baseline.assert_consistent(document.package());

    let after = document.info().unwrap();
    let edited = &after.slides[2];
    assert_eq!(edited.title.as_deref(), Some("Problems of Tomorrow"));
    assert_eq!(
        edited.body.as_deref(),
        Some("Memory safety\nThread safety 🧵\nNew paragraph")
    );
    assert_eq!(edited.notes.as_deref(), Some("Mention CVE numbers"));
    assert!(edited.skipped);
    assert_eq!(edited.layout, before.slides[2].layout);
    assert_eq!(edited.transition, before.slides[2].transition);

    for (index, (after, before)) in after.slides.iter().zip(&before.slides).enumerate() {
        if index != 2 {
            assert_eq!(content(after), content(before), "slide {} changed", index + 1);
        }
    }

    // only the edited slide's component and the document component were rewritten
    let changed: Vec<_> = zip_entries(&saved)
        .into_iter()
        .zip(zip_entries(&fixture()))
        .filter(|(after, before)| after != before)
        .map(|(after, _)| after.0)
        .collect();
    assert_eq!(changed.len(), 2, "{changed:?}");
    assert!(changed.contains(&"Index/Document.iwa".to_string()));
}

#[test]
fn edits_animated_text_keeping_its_build() {
    let mut document = KeynoteDocument::open(&fixture()).unwrap();
    let before = slide(&document, 43);

    let body = "Ownership\nBorrowing\nLifetimes\nTraits";
    let content = SlideContent {
        body: Some(body.to_string()),
        ..Default::default()
    };
    document.edit_slide(43, &content).unwrap();

    let after = slide(&document, 43);
    assert_eq!(after.body.as_deref(), Some(body));
    assert_eq!(after.builds.len(), 1);
    assert_eq!(after.builds[0].target.text.as_deref(), Some(body));
    assert_eq!(after.builds[0].delivery, before.builds[0].delivery);
    assert_eq!(after.build_order, before.build_order);
}

#[test]
fn duplicates_an_animated_slide() {
    let original = KeynoteDocument::open(&fixture()).unwrap();
    let baseline = Baseline::of(original.package());
    let source = slide(&original, 5);

    let mut document = KeynoteDocument::open(&fixture()).unwrap();
    document.duplicate_slide(5, 6).unwrap();
    let saved = temporary("duplicated.key");
    document.save(&saved).unwrap();

    let document = KeynoteDocument::open(&saved).unwrap();
    let package = document.package();
    baseline.assert_consistent(package);

    let info = document.info().unwrap();
    assert_eq!(info.slides.len(), SLIDE_COUNT + 1);
    assert_eq!(
        content(&info.slides[5]),
        content(&source),
        "the copy differs from its source"
    );
    assert_eq!(content(&info.slides[4]), content(&source), "the source changed");
    assert_eq!(
        content(&info.slides[6]),
        content(&slide(&original, 6)),
        "later slides didn't shift"
    );

    // builds and chunks were copied into a new component and point at the copies, not at
    // the source slide's objects (these references are missing from the archive headers)
    let counts = type_counts(package);
    assert_eq!(counts[&TYPE_BUILD], 43 + 4);
    assert_eq!(counts[&TYPE_BUILD_CHUNK], 63 + 4);

    let source_component = SLIDE_5_COMPONENT;
    let copy_components: Vec<&str> = package
        .component_names()
        .filter(|e| original.package().component_objects(e).next().is_none())
        .collect();
    assert_eq!(copy_components.len(), 1, "{copy_components:?}");
    let copy_component = copy_components[0];

    let copy_objects: HashSet<u64> = package
        .component_objects(copy_component)
        .map(|e| e.identifier())
        .collect();
    let source_objects: HashSet<u64> = package
        .component_objects(source_component)
        .map(|e| e.identifier())
        .collect();
    assert_eq!(copy_objects.len(), source_objects.len());

    let builds = references_of_type(package, copy_component, TYPE_BUILD, 1);
    let chunks = references_of_type(package, copy_component, TYPE_BUILD_CHUNK, 1);
    assert_eq!((builds.len(), chunks.len()), (4, 4));
    assert!(
        builds.iter().all(|(_, drawable)| copy_objects.contains(drawable)),
        "{builds:?}"
    );
    assert!(
        chunks.iter().all(|(_, build)| builds.iter().any(|(e, _)| e == build)),
        "{chunks:?}"
    );

    // nothing in the copy points back into the source slide
    for object in &copy_objects {
        let leaked: Vec<_> = embedded_references(package, *object)
            .intersection(&source_objects)
            .copied()
            .collect();
        assert!(leaked.is_empty(), "object {object} still references {leaked:?}");
    }
}

#[test]
fn moves_slides() {
    let original = KeynoteDocument::open(&fixture()).unwrap();
    let before = original.info().unwrap();

    let mut document = KeynoteDocument::open(&fixture()).unwrap();
    document.move_slide(1, SLIDE_COUNT).unwrap();
    document.move_slide(10, 2).unwrap();

    let after = document.info().unwrap();
    assert_eq!(content(&after.slides[SLIDE_COUNT - 1]), content(&before.slides[0]));
    assert_eq!(content(&after.slides[1]), content(&before.slides[10]));
    assert_eq!(content(&after.slides[0]), content(&before.slides[1]));
}

#[test]
fn deletes_an_animated_slide() {
    let original = KeynoteDocument::open(&fixture()).unwrap();
    let baseline = Baseline::of(original.package());
    let before = original.info().unwrap();

    let source_component = SLIDE_5_COMPONENT;
    let removed: HashSet<u64> = original
        .package()
        .component_objects(source_component)
        .map(|e| e.identifier())
        .collect();

    let mut document = KeynoteDocument::open(&fixture()).unwrap();
    document.delete_slide(5).unwrap();
    let saved = temporary("deleted.key");
    document.save(&saved).unwrap();

    let document = KeynoteDocument::open(&saved).unwrap();
    let package = document.package();
    baseline.assert_consistent(package);

    let after = document.info().unwrap();
    assert_eq!(after.slides.len(), SLIDE_COUNT - 1);
    assert_eq!(content(&after.slides[4]), content(&before.slides[5]));

    assert!(!package.has_component(source_component));
    assert!(!zip_entries(&saved).iter().any(|(name, _)| name == source_component));
    assert!(removed.iter().all(|e| !package.contains(*e)));

    let counts = type_counts(package);
    assert_eq!(counts[&TYPE_BUILD], 43 - 4);
    assert_eq!(counts[&TYPE_BUILD_CHUNK], 63 - 4);

    // no leftover reference to the deleted objects anywhere, listed in headers or not
    for object in package.objects() {
        let leaked: Vec<_> = embedded_references(package, object.identifier())
            .intersection(&removed)
            .copied()
            .collect();
        assert!(
            leaked.is_empty(),
            "object {} still references {leaked:?}",
            object.identifier()
        );
    }
}

#[test]
fn creates_a_presentation_from_it() {
    let original = KeynoteDocument::open(&fixture()).unwrap();
    let baseline = Baseline::of(original.package());
    let animated = slide(&original, 5);

    let saved = temporary("created.key");
    let slides = [
        spec(Some("Title"), None, Some("Hello, Keynote"), Some("Made by an agent")),
        spec(Some("Title & Bullets"), None, Some("Why"), Some("Fast\nSafe\nFun")),
        spec(None, Some(5), None, None),
        spec(Some("Section"), None, Some("The End"), None),
    ];
    let info = create_presentation(&fixture(), &saved, &slides, &Images::default()).unwrap();
    assert_eq!(info.slides.len(), 4);

    let document = KeynoteDocument::open(&saved).unwrap();
    baseline.assert_consistent(document.package());
    let reopened = document.info().unwrap();
    assert_eq!(
        serde_json::to_string(&reopened).unwrap(),
        serde_json::to_string(&info).unwrap()
    );

    let [title, bullets, copy, section] = &reopened.slides[..] else {
        unreachable!()
    };
    assert_eq!(title.layout.as_deref(), Some("Title"));
    assert_eq!(title.title.as_deref(), Some("Hello, Keynote"));
    assert_eq!(title.body.as_deref(), Some("Made by an agent"));

    assert_eq!(bullets.layout.as_deref(), Some("Title & Bullets"));
    assert_eq!(bullets.body.as_deref(), Some("Fast\nSafe\nFun"));

    // a duplicate keeps everything, animations included
    assert_eq!(content(copy), content(&animated));

    // a layout copy keeps the source slide's transition and builds, only the text is cleared
    assert_eq!(section.layout.as_deref(), Some("Section"));
    assert_eq!(section.title.as_deref(), Some("The End"));
    assert_eq!(section.body.as_deref(), Some(""));

    // the template's own slides are gone, layouts and media stay
    let package = document.package();
    assert_eq!(layout_names(&reopened.layouts), LAYOUTS);
    assert_eq!(type_counts(package)[&TYPE_SLIDE_NODE], 4 + LAYOUTS.len());
    assert!(
        package
            .component_names()
            .filter(|e| e.starts_with("Index/Slide"))
            .count()
            == 4
    );

    let identifier = zip_entries(&saved)
        .into_iter()
        .find(|(name, _)| name == "Metadata/DocumentIdentifier")
        .unwrap()
        .1;
    let template_identifier = zip_entries(&fixture())
        .into_iter()
        .find(|(name, _)| name == "Metadata/DocumentIdentifier")
        .unwrap()
        .1;
    assert_ne!(identifier, template_identifier);
}

////////////////////////////////////////////////////////////////////////////////
// Layouts and animations written from scratch (template.key)
////////////////////////////////////////////////////////////////////////////////

fn transition(effect: &str, duration: f64, automatic: bool) -> TransitionSpec {
    TransitionSpec {
        effect: effect.to_string(),
        duration,
        delay: 0.0,
        automatic,
    }
}

fn build_spec(target: &str, effect: &str, delivery: &str, automatic: bool) -> BuildSpec {
    BuildSpec {
        target: target.to_string(),
        kind: "In".to_string(),
        effect: effect.to_string(),
        delivery: delivery.to_string(),
        duration: 0.5,
        delay: 0.0,
        automatic,
    }
}

/// Template with one slide per layout appended after its own slide
fn template_with_every_layout() -> KeynoteDocument {
    let mut document = KeynoteDocument::open(&template()).unwrap();
    for (index, layout) in TEMPLATE_LAYOUTS.iter().enumerate() {
        document.new_slide_from_layout(layout, index + 2).unwrap();
    }
    document
}

#[test]
fn template_has_no_animations() {
    let info = KeynoteDocument::open(&template()).unwrap().info().unwrap();
    assert_eq!(layout_names(&info.layouts), TEMPLATE_LAYOUTS);
    assert_eq!(info.slides.len(), 1);
    assert!(
        info.slides
            .iter()
            .all(|e| e.transition.is_none() && e.builds.is_empty())
    );
}

#[test]
fn makes_slides_from_every_layout() {
    let original = KeynoteDocument::open(&template()).unwrap();
    let baseline = Baseline::of(original.package());
    let layout_objects: HashSet<u64> = original.package().objects().map(|e| e.identifier()).collect();

    let document = template_with_every_layout();
    let saved = temporary("layouts.key");
    document.save(&saved).unwrap();

    let document = KeynoteDocument::open(&saved).unwrap();
    baseline.assert_consistent(document.package());

    let info = document.info().unwrap();
    assert_eq!(info.slides.len(), 1 + TEMPLATE_LAYOUTS.len());
    for (slide, layout) in info.slides[1..].iter().zip(TEMPLATE_LAYOUTS) {
        assert_eq!(slide.layout.as_deref(), Some(layout));
        // the layout's sample text is gone, the placeholders stay
        assert!(slide.title.as_deref().is_none_or(str::is_empty), "{layout}: {slide:?}");
        assert!(slide.body.as_deref().is_none_or(str::is_empty), "{layout}: {slide:?}");
        assert!(
            slide.transition.is_none() && slide.builds.is_empty(),
            "{layout}: {slide:?}"
        );
    }
    let bullets = &info.slides[1 + TEMPLATE_LAYOUTS.iter().position(|e| *e == "Title & Bullets").unwrap()];
    assert!(bullets.title.is_some() && bullets.body.is_some());

    // each slide has objects of its own: none of its placeholders or storages is the layout's
    for number in 2..=info.slides.len() {
        let slide = document.slide_message(number);
        for field in [5u32, 6] {
            if let Some(placeholder) = reference(&slide, field).unwrap() {
                assert!(
                    !layout_objects.contains(&placeholder),
                    "slide {number} shares placeholder {placeholder}"
                );
            }
        }
        let owned: Vec<u64> = document
            .package()
            .object(reference(&slide, 27).unwrap().unwrap())
            .unwrap()
            .object_references();
        assert!(
            owned.iter().all(|e| !layout_objects.contains(e)),
            "slide {number} shares its notes"
        );
        // layout-only fields are gone, the link to the layout is there
        assert!(!slide.has(10) && !slide.has(31) && !slide.has(35));
        assert!(reference(&slide, 17).unwrap().is_some());
    }

    // layouts are untouched
    let reopened_layouts = document.info().unwrap().layouts;
    assert_eq!(layout_names(&reopened_layouts), TEMPLATE_LAYOUTS);
    // and say which placeholders they show
    let blank = reopened_layouts.iter().find(|e| e.name == "Blank").unwrap();
    assert!(blank.placeholders.is_empty(), "{blank:?}");
    let section = reopened_layouts.iter().find(|e| e.name == "Section").unwrap();
    assert!(section.placeholders.contains(&"title".to_string()), "{section:?}");
    assert!(!section.placeholders.contains(&"body".to_string()), "{section:?}");
    let photo = reopened_layouts.iter().find(|e| e.name == "Title & Photo").unwrap();
    assert!(photo.placeholders.contains(&"media".to_string()), "{photo:?}");
}

#[test]
fn edits_text_of_slides_made_from_layouts() {
    let mut document = template_with_every_layout();
    let number = 2 + TEMPLATE_LAYOUTS.iter().position(|e| *e == "Title & Bullets").unwrap();
    let changes = SlideContent {
        title: Some("Memory safety".to_string()),
        body: Some("Ownership\nBorrowing\nLifetimes".to_string()),
        notes: Some("No garbage collector".to_string()),
        ..Default::default()
    };
    document.edit_slide(number, &changes).unwrap();

    let slide = slide(&document, number);
    assert_eq!(slide.title.as_deref(), Some("Memory safety"));
    assert_eq!(slide.body.as_deref(), Some("Ownership\nBorrowing\nLifetimes"));
    assert_eq!(slide.notes.as_deref(), Some("No garbage collector"));
    assert_eq!(
        slide.objects.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["title", "body"]
    );
}

#[test]
fn sets_and_removes_transitions() {
    let mut document = template_with_every_layout();
    let magic = transition("apple:magic-move-implied-motion-path", 1.5, false);
    let automatic = transition("apple:dissolve", 0.7, true);
    document
        .edit_slide(
            1,
            &SlideContent {
                transition: Some(magic),
                ..Default::default()
            },
        )
        .unwrap();
    document
        .edit_slide(
            2,
            &SlideContent {
                transition: Some(automatic),
                ..Default::default()
            },
        )
        .unwrap();

    let saved = temporary("transitions.key");
    document.save(&saved).unwrap();
    let mut document = KeynoteDocument::open(&saved).unwrap();

    assert_eq!(
        slide(&document, 1).transition,
        Some(TransitionInfo {
            effect: "apple:magic-move-implied-motion-path".to_string(),
            duration: Some(1.5),
            delay: Some(0.0),
            automatic: false,
        })
    );
    assert_eq!(
        slide(&document, 2).transition.map(|e| (e.effect, e.automatic)),
        Some(("apple:dissolve".to_string(), true))
    );
    assert_eq!(slide(&document, 3).transition, None);
    assert_eq!(document.node_message(1).bool(NODE_HAS_TRANSITION), Some(true));

    document
        .edit_slide(
            1,
            &SlideContent {
                transition: Some(transition("none", 1.0, false)),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(slide(&document, 1).transition, None);
    assert_eq!(document.node_message(1).bool(NODE_HAS_TRANSITION), Some(false));
}

#[test]
fn writes_builds_from_scratch() {
    let original = KeynoteDocument::open(&template()).unwrap();
    let baseline = Baseline::of(original.package());

    let mut document = template_with_every_layout();
    let number = 2 + TEMPLATE_LAYOUTS.iter().position(|e| *e == "Title & Bullets").unwrap();
    let changes = SlideContent {
        title: Some("Thread safety".to_string()),
        body: Some("Send\nSync\n\nNo data races".to_string()),
        builds: Some(vec![
            build_spec("title", "apple:dissolve", "All at Once", true),
            build_spec("body", "apple:dissolve character", "By Bullet", false),
        ]),
        ..Default::default()
    };
    document.edit_slide(number, &changes).unwrap();

    let saved = temporary("builds.key");
    document.save(&saved).unwrap();
    let document = KeynoteDocument::open(&saved).unwrap();
    baseline.assert_consistent(document.package());

    let slide_info = slide(&document, number);
    let expected = |target: &str, text: &str, effect: &str, delivery: &str| BuildInfo {
        target: BuildTarget {
            id: None,
            kind: target.to_string(),
            text: Some(text.to_string()),
        },
        kind: Some("In".to_string()),
        effect: Some(effect.to_string()),
        delivery: Some(delivery.to_string()),
        duration: Some(0.5),
    };
    assert_eq!(
        without_ids(&slide_info.builds),
        vec![
            expected("title", "Thread safety", "apple:dissolve", "All at Once"),
            expected(
                "body",
                "Send\nSync\n\nNo data races",
                "apple:dissolve character",
                "By Bullet"
            ),
        ]
    );
    // the title comes in by itself, then one click per non-empty paragraph
    assert_eq!(
        slide_info.build_order,
        vec![
            step(1, true, 0.5),
            step(2, false, 0.5),
            step(2, false, 0.5),
            step(2, false, 0.5)
        ]
    );

    let node = document.node_message(number);
    assert_eq!(node.varint(NODE_BUILD_EVENT_COUNT), Some(3));
    assert_eq!(node.bool(NODE_HAS_EXPLICIT_BUILDS), Some(true));

    // builds point at the slide's own placeholders, chunks at their build and share its id
    let package = document.package();
    let slide_message = document.slide_message(number);
    let builds: Vec<u64> = super::format::proto::references(&slide_message, 2).unwrap();
    let chunks: Vec<u64> = super::format::proto::references(&slide_message, 43).unwrap();
    assert_eq!((builds.len(), chunks.len()), (2, 4));
    assert_eq!(
        reference(&package.message(builds[0]).unwrap(), 1).unwrap(),
        reference(&slide_message, 5).unwrap()
    );
    assert_eq!(
        reference(&package.message(builds[1]).unwrap(), 1).unwrap(),
        reference(&slide_message, 6).unwrap()
    );
    let chunk_messages: Vec<Message> = chunks.iter().map(|e| package.message(*e).unwrap()).collect();
    assert_eq!(reference(&chunk_messages[0], 1).unwrap(), Some(builds[0]));
    assert!(
        chunk_messages[1..]
            .iter()
            .all(|e| reference(e, 1).unwrap() == Some(builds[1]))
    );
    let body_ids: Vec<_> = chunk_messages[1..].iter().map(|e| e.message(8).unwrap()).collect();
    assert!(body_ids.windows(2).all(|e| e[0] == e[1]));
    assert_ne!(chunk_messages[0].message(8).unwrap(), body_ids[0]);
    let chunk_numbers: Vec<_> = chunk_messages[1..]
        .iter()
        .map(|e| e.message(7).unwrap().unwrap().varint(2).unwrap())
        .collect();
    assert_eq!(chunk_numbers, [1, 2, 3]);
    // builds and chunks live next to the slide
    let component = package
        .component_of(reference(&slide_message, 5).unwrap().unwrap())
        .unwrap();
    assert!(
        builds
            .iter()
            .chain(&chunks)
            .all(|e| package.component_of(*e).unwrap() == component)
    );
}

#[test]
fn replaces_and_removes_builds() {
    let mut document = template_with_every_layout();
    let number = 2 + TEMPLATE_LAYOUTS.iter().position(|e| *e == "Title & Bullets").unwrap();
    let first = SlideContent {
        body: Some("One\nTwo".to_string()),
        builds: Some(vec![build_spec("body", "apple:dissolve", "By Bullet", false)]),
        ..Default::default()
    };
    document.edit_slide(number, &first).unwrap();
    let first_objects: Vec<u64> = {
        let slide = document.slide_message(number);
        let mut ids = super::format::proto::references(&slide, 2).unwrap();
        ids.extend(super::format::proto::references(&slide, 43).unwrap());
        ids
    };
    assert_eq!(first_objects.len(), 3);

    let second = SlideContent {
        builds: Some(vec![build_spec(
            "object 1",
            "com.apple.iWork.Keynote.FromDarkness",
            "All at Once",
            false,
        )]),
        ..Default::default()
    };
    document.edit_slide(number, &second).unwrap();
    let info = slide(&document, number);
    assert_eq!(info.builds.len(), 1);
    assert_eq!(info.builds[0].target.kind, "title");
    assert_eq!(info.build_order.len(), 1);
    assert!(
        first_objects.iter().all(|e| !document.package().contains(*e)),
        "old builds left behind"
    );

    document
        .edit_slide(
            number,
            &SlideContent {
                builds: Some(Vec::new()),
                ..Default::default()
            },
        )
        .unwrap();
    let info = slide(&document, number);
    assert!(info.builds.is_empty() && info.build_order.is_empty());
    let node = document.node_message(number);
    assert_eq!(node.varint(NODE_BUILD_EVENT_COUNT), Some(0));
    assert_eq!(node.bool(NODE_HAS_EXPLICIT_BUILDS), Some(false));
}

#[test]
fn rejects_unknown_build_targets_without_changes() {
    let mut document = template_with_every_layout();
    let number = 2 + TEMPLATE_LAYOUTS.iter().position(|e| *e == "Blank").unwrap();
    let before = content(&slide(&document, number));

    for target in ["title", "object 1", "picture"] {
        let changes = SlideContent {
            builds: Some(vec![build_spec(target, "apple:dissolve", "All at Once", false)]),
            ..Default::default()
        };
        assert!(document.edit_slide(number, &changes).is_err(), "{target} accepted");
    }
    assert_eq!(content(&slide(&document, number)), before);
    assert!(document.new_slide_from_layout("Photo Wall", 2).is_err());
}

#[test]
fn copies_get_their_own_build_ids() {
    let mut document = KeynoteDocument::open(&fixture()).unwrap();
    document.duplicate_slide(43, 44).unwrap();

    let chunk_ids = |number: usize| -> Vec<Message> {
        let slide = document.slide_message(number);
        let chunks = super::format::proto::references(&slide, 43).unwrap();
        chunks
            .iter()
            .map(|e| document.package().message(*e).unwrap().message(8).unwrap().unwrap())
            .collect()
    };
    let (source, copy) = (chunk_ids(43), chunk_ids(44));
    assert_eq!(source.len(), 3);
    assert_eq!(copy.len(), 3);
    assert!(
        copy.windows(2).all(|e| e[0] == e[1]),
        "chunks of one build share its id"
    );
    assert_ne!(source[0], copy[0]);
    assert_eq!(content(&slide(&document, 43)), content(&slide(&document, 44)));
}

#[test]
fn edits_animations_of_a_real_presentation() {
    let original = KeynoteDocument::open(&fixture()).unwrap();
    let baseline = Baseline::of(original.package());

    let mut document = KeynoteDocument::open(&fixture()).unwrap();
    let changes = SlideContent {
        transition: Some(transition("apple:fade-and-move", 2.0, false)),
        builds: Some(vec![
            build_spec("body", "apple:dissolve", "All at Once", false),
            build_spec("object 3", "apple:dissolve", "All at Once", true),
        ]),
        ..Default::default()
    };
    document.edit_slide(5, &changes).unwrap();

    let saved = temporary("reanimated.key");
    document.save(&saved).unwrap();
    let document = KeynoteDocument::open(&saved).unwrap();
    baseline.assert_consistent(document.package());

    let slide5 = slide(&document, 5);
    assert_eq!(
        slide5.transition.as_ref().map(|e| e.effect.as_str()),
        Some("apple:fade-and-move")
    );
    assert_eq!(slide5.builds.len(), 2);
    assert_eq!(slide5.builds[0].target.kind, "body");
    assert_eq!(slide5.builds[1].target.id, Some(slide5.objects[2].id));
    assert_eq!(slide5.builds[1].target.kind, slide5.objects[2].kind);
    assert_eq!(slide5.build_order, vec![step(1, false, 0.5), step(2, true, 0.5)]);

    let counts = type_counts(document.package());
    assert_eq!(counts[&TYPE_BUILD], 43 - 4 + 2);
    assert_eq!(counts[&TYPE_BUILD_CHUNK], 63 - 4 + 2);
}

#[test]
fn creates_an_animated_presentation_from_the_template() {
    let original = KeynoteDocument::open(&template()).unwrap();
    let baseline = Baseline::of(original.package());

    let fade = TransitionParams {
        effect: "apple:dissolve".to_string(),
        duration: Some(0.8),
        delay: None,
        automatic: None,
    };
    let by_bullet = BuildParams {
        target: "body".to_string(),
        kind: None,
        effect: Some("apple:dissolve character".to_string()),
        delivery: Some("By Bullet".to_string()),
        duration: None,
        delay: None,
        automatic: None,
    };
    let slides = [
        NewSlideSpec {
            layout: Some("Title".to_string()),
            content: SlideContentParams {
                title: Some(TextParams::Plain("Rust Safety".to_string())),
                transition: Some(fade.clone()),
                ..Default::default()
            },
            ..Default::default()
        },
        NewSlideSpec {
            layout: Some("Title & Bullets".to_string()),
            content: SlideContentParams {
                title: Some(TextParams::Plain("Ownership".to_string())),
                body: Some(TextParams::Plain("One owner\nMoves\nDrops".to_string())),
                transition: Some(fade.clone()),
                builds: Some(vec![by_bullet.clone()]),
                ..Default::default()
            },
            ..Default::default()
        },
        NewSlideSpec {
            duplicate: Some(1),
            ..Default::default()
        },
    ];

    let saved = temporary("animated.key");
    let info = create_presentation(&template(), &saved, &slides, &Images::default()).unwrap();

    let document = KeynoteDocument::open(&saved).unwrap();
    baseline.assert_consistent(document.package());
    assert_eq!(
        serde_json::to_string(&document.info().unwrap()).unwrap(),
        serde_json::to_string(&info).unwrap()
    );

    assert_eq!(info.slides.len(), 3);
    assert_eq!(info.slides[0].title.as_deref(), Some("Rust Safety"));
    assert_eq!(info.slides[1].layout.as_deref(), Some("Title & Bullets"));
    assert_eq!(info.slides[1].build_order.len(), 3);
    assert!(
        info.slides
            .iter()
            .take(2)
            .all(|e| e.transition.as_ref().is_some_and(|e| e.duration == Some(0.8)))
    );
    // "duplicate" refers to the template's own (empty) slide
    assert_eq!(info.slides[2].layout.as_deref(), Some("Title"));
    assert_eq!(info.slides[2].title.as_deref(), Some(""));
}
