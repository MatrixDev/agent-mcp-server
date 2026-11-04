//! Rich slides built on template.key: backgrounds, styled text, shapes, lines, images, code,
//! object editing and deleting, builds on new objects and rendered previews. Every saved file
//! is reopened and checked for package consistency.

use std::path::{Path, PathBuf};

use super::format::document::{BuildSpec, KeynoteDocument, SlideContent};
use super::format::drawing::{Color, Dash, Fill, Frame, LineEnd, Outline, Shadow, Stroke};
use super::format::media::ImageFile;
use super::format::objects::{
    BackgroundSpec, CommonSpec, Fit, FramePatch, ImageSpec, LineSpec, ObjectEdit, ShapeSpec, ShapeStyleSpec,
    TextLayoutSpec, VerticalAlign, ZOrder,
};
use super::format::proto::references;
use super::format::render::svg_to_png;
use super::format::rich_text::{Paragraph, RunStyle, TextRun};
use super::operations::AddCodeParams;
use super::params::{Images, TextParams};

////////////////////////////////////////////////////////////////////////////////
fn template() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../template.key")
}

fn temporary(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("mdev-keynote-rich-{:016x}", rand::random::<u64>()));
    std::fs::create_dir_all(&folder).unwrap();
    folder.join(name)
}

fn color(text: &str) -> Color {
    Color::parse(text).unwrap()
}

fn frame(x: f32, y: f32, w: f32, h: f32) -> Frame {
    Frame { x, y, w, h }
}

/// A 300 × 200 orange PNG, made from an SVG
fn test_image() -> ImageFile {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="150" height="100"><rect width="150" height="100" fill="#F2762E"/><circle cx="75" cy="50" r="30" fill="#17161B"/></svg>"##;
    let rendition = ImageFile::from_bytes(svg.to_vec(), "logo.svg").unwrap();
    ImageFile::from_bytes(rendition.bytes, "photo.png").unwrap()
}

fn run(text: &str, style: RunStyle) -> TextRun {
    TextRun {
        text: text.to_string(),
        style,
    }
}

fn tile_text(number: &str, caption: &str, accent: Color) -> Vec<Paragraph> {
    vec![
        Paragraph {
            runs: vec![run(
                number,
                RunStyle {
                    size: Some(84.0),
                    bold: Some(true),
                    color: Some(accent),
                    ..Default::default()
                },
            )],
            ..Default::default()
        },
        Paragraph {
            runs: vec![run(
                caption,
                RunStyle {
                    size: Some(26.0),
                    color: Some(color("#A8A39A")),
                    ..Default::default()
                },
            )],
            space_before: Some(8.0),
            ..Default::default()
        },
    ]
}

/// Package invariants every saved file must keep (see tests.rs for the full baseline)
fn assert_consistent(document: &KeynoteDocument) {
    let package = document.package();
    let mut seen = std::collections::HashSet::new();
    for object in package.objects() {
        assert!(seen.insert(object.identifier()), "object {} stored twice", object.identifier());
    }
    let last = package.metadata().unwrap().varint(1).unwrap();
    assert!(seen.iter().all(|e| *e <= last), "last object id {last} is behind");

    // every reference the new objects make resolves
    let original = KeynoteDocument::open(&template()).unwrap();
    let known_dangling: std::collections::HashSet<u64> = original
        .package()
        .objects()
        .flat_map(|e| e.object_references())
        .filter(|e| !original.package().contains(*e))
        .collect();
    for object in package.objects() {
        for reference in object.object_references() {
            assert!(
                package.contains(reference) || known_dangling.contains(&reference),
                "object {} points at missing {reference}",
                object.identifier()
            );
        }
    }
}

/// The template's own slide followed by: 2 and 3 "Title & Bullets", 4 "Title & Photo", 5 "Blank"
fn template_with_slides() -> KeynoteDocument {
    let mut document = KeynoteDocument::open(&template()).unwrap();
    document.new_slide_from_layout("Title & Bullets", 2).unwrap();
    document.new_slide_from_layout("Title & Bullets", 3).unwrap();
    document.new_slide_from_layout("Title & Photo", 4).unwrap();
    document.new_slide_from_layout("Blank", 5).unwrap();
    document
}
#[test]
fn keeps_spaces_around_bold_markdown() {
    let mut document = template_with_slides();
    let text: TextParams = serde_json::from_str(r##"{"markdown": "are **memory** bugs", "size": 30}"##).unwrap();
    let identifier = document
        .add_shape(
            5,
            &ShapeSpec {
                outline: Outline::Rect,
                frame: frame(100.0, 100.0, 1000.0, 60.0),
                rotation: 0.0,
                text_box: true,
                preset: None,
                style: ShapeStyleSpec::default(),
                layout: TextLayoutSpec::default(),
                text: Some(text.paragraphs().unwrap()),
                common: CommonSpec::default(),
            },
        )
        .unwrap();
    let storage = document.drawable_storage(identifier).unwrap().unwrap();
    let message = document.package().message(storage).unwrap();
    let paragraphs = super::format::rich_text::stored_paragraphs(&message).unwrap();
    let pieces: Vec<String> = paragraphs[0]
        .runs
        .iter()
        .map(|(start, end, _)| {
            let units: Vec<u16> = paragraphs[0].text.encode_utf16().collect();
            String::from_utf16_lossy(&units[*start..*end])
        })
        .collect();
    assert_eq!(pieces.concat(), "are memory bugs", "{pieces:?} {:?}", paragraphs[0].runs);
    let svg = document.render_slide(5).unwrap().svg;
    let texts: Vec<&str> = svg.split("<text").skip(1).collect();
    assert!(texts.iter().any(|e| e.contains(">bugs<")), "{texts:?}");
    let x = |needle: &str| -> f32 {
        let text = texts.iter().find(|e| e.contains(needle)).unwrap();
        text.split("x=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap()
    };
    // the gap after the bold word holds the word and a space
    let gap = x(">bugs<") - x(">memory<");
    assert!(gap > 100.0, "{texts:?}");
}


////////////////////////////////////////////////////////////////////////////////
#[test]
fn builds_the_acceptance_slide() {
    let mut document = template_with_slides();
    let slide = 2;
    let accent = color("#F2762E");

    // 1. dark background, the title in the theme's placeholder, styled
    let content = SlideContent {
        background: Some(BackgroundSpec::Fill(Fill::Color(color("#17161B")))),
        title_rich: Some(vec![Paragraph {
            runs: vec![run(
                "The 70% problem",
                RunStyle {
                    color: Some(color("#F4EFE6")),
                    bold: Some(true),
                    ..Default::default()
                },
            )],
            ..Default::default()
        }]),
        show_body: Some(false),
        ..Default::default()
    };
    document.edit_slide(slide, &content).unwrap();

    // 2. header tag and the orange bar
    let tag = document
        .add_shape(
            slide,
            &ShapeSpec {
                outline: Outline::Rect,
                frame: frame(96.0, 60.0, 600.0, 40.0),
                rotation: 0.0,
                text_box: true,
                preset: None,
                style: ShapeStyleSpec::default(),
                layout: TextLayoutSpec::default(),
                text: Some(vec![Paragraph {
                    runs: vec![run(
                        "OPENING",
                        RunStyle {
                            size: Some(22.0),
                            color: Some(accent),
                            bold: Some(true),
                            ..Default::default()
                        },
                    )],
                    ..Default::default()
                }]),
                common: CommonSpec::default(),
            },
        )
        .unwrap();
    let bar = document
        .add_shape(
            slide,
            &ShapeSpec {
                outline: Outline::Rect,
                frame: frame(96.0, 110.0, 72.0, 6.0),
                rotation: 0.0,
                text_box: false,
                preset: None,
                style: ShapeStyleSpec {
                    fill: Some(Fill::Color(accent)),
                    stroke: Some(None),
                    ..Default::default()
                },
                layout: TextLayoutSpec::default(),
                text: None,
                common: CommonSpec::default(),
            },
        )
        .unwrap();

    // 3. four stat tiles
    let mut tiles = Vec::new();
    for (index, (number, caption, tint)) in [
        ("70%", "of Chrome's serious bugs", "#E4574C"),
        ("76%", "Android, 2019", "#F2C94C"),
        ("24%", "Android, 2024", "#6CC38B"),
        ("0", "memory bugs in Rust code", "#5DA9E9"),
    ]
    .iter()
    .enumerate()
    {
        let tile = document
            .add_shape(
                slide,
                &ShapeSpec {
                    outline: Outline::RoundedRect { radius: 16.0 },
                    frame: frame(96.0 + index as f32 * 440.0, 290.0, 400.0, 260.0),
                    rotation: 0.0,
                    text_box: false,
                    preset: None,
                    style: ShapeStyleSpec {
                        fill: Some(Fill::Color(color("#23222A"))),
                        stroke: Some(Some(Stroke {
                            color: color("#2E2D35"),
                            width: 2.0,
                            dash: Dash::Solid,
                        })),
                        shadow: Some(Some(Shadow {
                            color: color("#00000080"),
                            blur: 12.0,
                            dx: 0.0,
                            dy: 4.0,
                        })),
                        opacity: None,
                    },
                    layout: TextLayoutSpec {
                        inset: Some(24.0),
                        vertical_align: Some(VerticalAlign::Top),
                        ..Default::default()
                    },
                    text: Some(tile_text(number, caption, color(tint))),
                    common: CommonSpec::default(),
                },
            )
            .unwrap();
        tiles.push(tile);
    }

    // 4. a takeaway in italic markdown, with a link
    let takeaway = TextParams::Plain(String::new());
    let _ = takeaway;
    let takeaway: TextParams = serde_json::from_str(
        r##"{"markdown": "*Most* security bugs are **memory** bugs — see [the data](https://example.com)", "size": 30, "color": "#F4EFE6"}"##,
    )
    .unwrap();
    let takeaway = document
        .add_shape(
            slide,
            &ShapeSpec {
                outline: Outline::Rect,
                frame: frame(96.0, 600.0, 1728.0, 60.0),
                rotation: 0.0,
                text_box: true,
                preset: None,
                style: ShapeStyleSpec::default(),
                layout: TextLayoutSpec::default(),
                text: Some(takeaway.paragraphs().unwrap()),
                common: CommonSpec::default(),
            },
        )
        .unwrap();

    // 5. footer rule, an arrow and an image cropped to a circle-ish rounded frame
    let rule = document
        .add_line(
            slide,
            &LineSpec {
                from: (96.0, 980.0),
                to: (1824.0, 980.0),
                stroke: Stroke {
                    color: color("#2E2D35"),
                    width: 2.0,
                    dash: Dash::Solid,
                },
                head: LineEnd::None,
                tail: LineEnd::None,
                opacity: None,
                shadow: None,
                common: CommonSpec::default(),
            },
        )
        .unwrap();
    let arrow = document
        .add_line(
            slide,
            &LineSpec {
                from: (96.0, 900.0),
                to: (600.0, 900.0),
                stroke: Stroke {
                    color: accent,
                    width: 4.0,
                    dash: Dash::Dash,
                },
                head: LineEnd::None,
                tail: LineEnd::FilledArrow,
                opacity: Some(0.8),
                shadow: None,
                common: CommonSpec::default(),
            },
        )
        .unwrap();
    let image = document
        .add_image(
            slide,
            &ImageSpec {
                file: test_image(),
                frame: Some(frame(1500.0, 700.0, 200.0, 200.0)),
                fit: Fit::Cover,
                rotation: 0.0,
                opacity: Some(0.9),
                corner_radius: Some(100.0),
                stroke: None,
                shadow: None,
                fill_placeholder: false,
                common: CommonSpec {
                    description: Some("an orange logo".to_string()),
                    ..Default::default()
                },
            },
        )
        .unwrap();

    // 6. builds on the new objects: title automatic, tiles one by one, takeaway after
    let mut builds = vec![BuildSpec {
        target: "title".to_string(),
        kind: "In".to_string(),
        effect: "apple:dissolve".to_string(),
        delivery: "All at Once".to_string(),
        duration: 0.5,
        delay: 0.0,
        automatic: true,
    }];
    for tile in &tiles {
        builds.push(BuildSpec {
            target: format!("id:{tile}"),
            kind: "In".to_string(),
            effect: "apple:pop".to_string(),
            delivery: "All at Once".to_string(),
            duration: 0.5,
            delay: 0.0,
            automatic: false,
        });
    }
    builds.push(BuildSpec {
        target: format!("id:{takeaway}"),
        kind: "In".to_string(),
        effect: "apple:dissolve".to_string(),
        delivery: "All at Once".to_string(),
        duration: 0.5,
        delay: 0.15,
        automatic: true,
    });
    document
        .edit_slide(
            slide,
            &SlideContent {
                builds: Some(builds),
                ..Default::default()
            },
        )
        .unwrap();

    let saved = temporary("acceptance.key");
    document.save(&saved).unwrap();
    let document = KeynoteDocument::open(&saved).unwrap();
    assert_consistent(&document);

    // everything reads back
    let info = document.slide(slide).unwrap();
    assert_eq!(info.background.as_deref(), Some("#17161B"));
    assert_eq!(info.title.as_deref(), Some("The 70% problem"));
    let kinds: Vec<(u64, &str)> = info.objects.iter().map(|e| (e.id, e.kind.as_str())).collect();
    assert!(!kinds.iter().any(|e| e.1 == "body"), "body still shown: {kinds:?}");
    for (id, kind) in [
        (tag, "text box"),
        (bar, "shape"),
        (tiles[0], "shape"),
        (takeaway, "text box"),
        (rule, "line"),
        (arrow, "line"),
        (image, "image"),
    ] {
        assert!(kinds.contains(&(id, kind)), "id:{id} {kind} missing from {kinds:?}");
    }
    let tile = info.objects.iter().find(|e| e.id == tiles[1]).unwrap();
    assert_eq!(tile.frame, Some(frame(536.0, 290.0, 400.0, 260.0)));
    assert_eq!(tile.fill.as_deref(), Some("#23222A"));
    assert_eq!(tile.stroke.as_deref(), Some("#2E2D35 2pt"));
    assert_eq!(tile.text.as_deref(), Some("76%\nAndroid, 2019"));
    let arrow_info = info.objects.iter().find(|e| e.id == arrow).unwrap();
    assert_eq!(arrow_info.stroke.as_deref(), Some("#F2762E 4pt dash"));
    assert_eq!(arrow_info.opacity, Some(0.8));
    let image_info = info.objects.iter().find(|e| e.id == image).unwrap();
    assert_eq!(image_info.image.as_deref(), Some("photo.png"));
    assert_eq!(image_info.description.as_deref(), Some("an orange logo"));
    assert_eq!(
        info.objects.iter().find(|e| e.id == takeaway).unwrap().text.as_deref(),
        Some("Most security bugs are memory bugs — see the data")
    );

    assert_eq!(info.builds.len(), 6);
    assert_eq!(info.builds[1].target.id, Some(tiles[0]));
    assert_eq!(info.build_order.len(), 6);
    assert_eq!(info.build_order.iter().filter(|e| !e.automatic).count(), 4);

    // identical styles are shared, not duplicated per tile
    let package = document.package();
    let style_of = |id: u64| {
        let message = package.message(id).unwrap();
        let shape = message.message(1).unwrap().unwrap();
        super::format::proto::reference(&shape, 2).unwrap().unwrap()
    };
    assert_eq!(style_of(tiles[0]), style_of(tiles[3]));
    assert_ne!(style_of(tiles[0]), style_of(bar));

    // the image's media is in the package and counted in its component
    let image_message = package.message(image).unwrap();
    let data = image_message.message(11).unwrap().unwrap().varint(1).unwrap();
    assert!(document.data_bytes(data).unwrap().is_some());
    assert!(package.object(image).unwrap().data_references().contains(&data));
    assert!(image_message.message(5).unwrap().is_some(), "cover crop needs a mask");

    // the preview draws all of it
    let rendered = document.render_slide(slide).unwrap();
    assert!(rendered.svg.contains("rgb(23,22,27)"), "background");
    assert!(rendered.svg.contains("76%"));
    assert!(rendered.svg.contains("data:image/png;base64,"));
    assert!(rendered.svg.contains("clip-path"));
    let png = svg_to_png(&rendered.svg, 960).unwrap();
    // kept in target/ for a look by eye
    let preview = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/keynote-acceptance");
    if std::fs::create_dir_all(&preview).is_ok() {
        let _ = std::fs::write(preview.join("slide.png"), &png);
        let _ = std::fs::write(preview.join("slide.svg"), &rendered.svg);
        let _ = std::fs::copy(&saved, preview.join("acceptance.key"));
    }
    let size = imagesize::blob_size(&png).unwrap();
    assert_eq!((size.width, size.height), (960, 540));
}

////////////////////////////////////////////////////////////////////////////////
#[test]
fn edits_moves_and_deletes_objects() {
    let mut document = template_with_slides();
    let slide = 5;
    let shape = |document: &mut KeynoteDocument, x: f32| {
        document
            .add_shape(
                slide,
                &ShapeSpec {
                    outline: Outline::Ellipse,
                    frame: frame(x, 100.0, 200.0, 200.0),
                    rotation: 0.0,
                    text_box: false,
                    preset: Some(2),
                    style: ShapeStyleSpec {
                        fill: Some(Fill::Gradient {
                            angle: 135.0,
                            stops: vec![(color("#17161B"), 0.0), (color("#2A1A12"), 1.0)],
                        }),
                        ..Default::default()
                    },
                    layout: TextLayoutSpec::default(),
                    text: Some(vec![Paragraph::plain("hi")]),
                    common: CommonSpec::default(),
                },
            )
            .unwrap()
    };
    let first = shape(&mut document, 100.0);
    let second = shape(&mut document, 400.0);

    // move, recolour, retext, restack and lock the first
    document
        .edit_object(
            first,
            &ObjectEdit {
                frame: FramePatch {
                    x: Some(700.0),
                    w: Some(300.0),
                    ..Default::default()
                },
                rotation: Some(15.0),
                z: Some(ZOrder::Front),
                style: ShapeStyleSpec {
                    fill: Some(Fill::Color(color("#5DA9E9"))),
                    opacity: Some(0.5),
                    ..Default::default()
                },
                text: Some(vec![Paragraph::plain("moved")]),
                locked: Some(true),
                ..Default::default()
            },
        )
        .unwrap();

    let info = document.slide(slide).unwrap();
    let ids: Vec<u64> = info.objects.iter().map(|e| e.id).collect();
    assert_eq!(&ids[ids.len() - 2..], [second, first], "first is in front");
    let moved = info.objects.iter().find(|e| e.id == first).unwrap();
    assert_eq!(moved.frame, Some(frame(700.0, 100.0, 300.0, 200.0)));
    assert!((moved.rotation - 15.0).abs() < 1e-3);
    assert_eq!(moved.fill.as_deref(), Some("#5DA9E9"));
    assert_eq!(moved.opacity, Some(0.5));
    assert_eq!(moved.text.as_deref(), Some("moved"));
    assert!(moved.locked);
    let untouched = info.objects.iter().find(|e| e.id == second).unwrap();
    assert!(untouched.fill.as_deref().unwrap().starts_with("gradient"));

    // a copy of the slide gets its own objects
    document.duplicate_slide(slide, slide + 1).unwrap();
    let copy = document.slide(slide + 1).unwrap();
    assert_eq!(copy.objects.len(), info.objects.len());
    assert!(copy.objects.iter().all(|e| !ids.contains(&e.id)));
    assert_eq!(copy.objects.last().unwrap().text.as_deref(), Some("moved"));

    // delete the second shape along with a build on it
    document
        .edit_slide(
            slide,
            &SlideContent {
                builds: Some(vec![BuildSpec {
                    target: format!("id:{second}"),
                    kind: "In".to_string(),
                    effect: "apple:dissolve".to_string(),
                    delivery: "All at Once".to_string(),
                    duration: 1.0,
                    delay: 0.0,
                    automatic: false,
                }]),
                ..Default::default()
            },
        )
        .unwrap();
    let storage = document.drawable_storage(second).unwrap().unwrap();
    document.delete_objects(&[second]).unwrap();
    assert!(!document.package().contains(second));
    assert!(!document.package().contains(storage));
    let info = document.slide(slide).unwrap();
    assert!(info.builds.is_empty() && info.build_order.is_empty());
    assert!(info.objects.iter().all(|e| e.id != second));

    // placeholders can only be hidden
    let title = document.slide(2).unwrap().objects[0].id;
    assert!(document.delete_objects(&[title]).is_err());

    // and the slide with objects goes away cleanly
    document.delete_slide(slide).unwrap();
    assert!(!document.package().contains(first));

    let saved = temporary("edited.key");
    document.save(&saved).unwrap();
    let document = KeynoteDocument::open(&saved).unwrap();
    assert_consistent(&document);
    assert_eq!(document.slide(slide).unwrap().objects.last().unwrap().text.as_deref(), Some("moved"));
}

////////////////////////////////////////////////////////////////////////////////
#[test]
fn fills_media_placeholders_and_slide_backgrounds() {
    let mut document = template_with_slides();
    let photo = 4;
    let before = document.slide(photo).unwrap();
    let placeholder = before.objects.iter().find(|e| e.kind == "media placeholder").unwrap().clone();

    let image = document
        .add_image(
            photo,
            &ImageSpec {
                file: test_image(),
                frame: None,
                fit: Fit::Cover,
                rotation: 0.0,
                opacity: None,
                corner_radius: None,
                stroke: None,
                shadow: None,
                fill_placeholder: true,
                common: CommonSpec::default(),
            },
        )
        .unwrap();
    let after = document.slide(photo).unwrap();
    assert!(after.objects.iter().all(|e| e.id != placeholder.id));
    let filled = after.objects.iter().find(|e| e.id == image).unwrap();
    assert_eq!(filled.kind, "media placeholder", "the new image took the Media tag");

    // an image background, then a gradient one, then back to the layout's
    document
        .set_background(
            photo,
            &BackgroundSpec::Image {
                file: test_image(),
                fit: Fit::Cover,
            },
        )
        .unwrap();
    assert_eq!(document.slide(photo).unwrap().background.as_deref(), Some("image photo.png"));
    document
        .set_background(
            photo,
            &BackgroundSpec::Fill(Fill::Gradient {
                angle: 90.0,
                stops: vec![(color("#000000"), 0.0), (color("#FFFFFF"), 1.0)],
            }),
        )
        .unwrap();
    assert!(document.slide(photo).unwrap().background.unwrap().starts_with("gradient 90°"));
    let layout_background = document.slide(5).unwrap().background;
    document.set_background(photo, &BackgroundSpec::Layout).unwrap();
    assert_eq!(document.slide(photo).unwrap().background, layout_background);

    // hiding and showing the title
    let hidden = SlideContent {
        show_title: Some(false),
        ..Default::default()
    };
    document.edit_slide(photo, &hidden).unwrap();
    assert!(document.slide(photo).unwrap().objects.iter().all(|e| e.kind != "title"));
    let shown = SlideContent {
        show_title: Some(true),
        title: Some("Back".to_string()),
        ..Default::default()
    };
    document.edit_slide(photo, &shown).unwrap();
    let info = document.slide(photo).unwrap();
    assert!(info.objects.iter().any(|e| e.kind == "title"));
    assert_eq!(info.title.as_deref(), Some("Back"));

    // a layout without a media placeholder says so
    let blank = ImageSpec {
        file: test_image(),
        frame: None,
        fit: Fit::Cover,
        rotation: 0.0,
        opacity: None,
        corner_radius: None,
        stroke: None,
        shadow: None,
        fill_placeholder: true,
        common: CommonSpec::default(),
    };
    assert!(document.add_image(5, &blank).is_err());

    let saved = temporary("photo.key");
    document.save(&saved).unwrap();
    let document = KeynoteDocument::open(&saved).unwrap();
    assert_consistent(&document);
    assert!(document.render_slide(photo).unwrap().svg.contains("data:image/png"));
}

////////////////////////////////////////////////////////////////////////////////
#[test]
fn writes_code_blocks_and_styled_body_text() {
    let mut document = template_with_slides();
    let code: AddCodeParams = serde_json::from_value(serde_json::json!({
        "slide": 3,
        "frame": {"x": 96, "y": 290, "w": 840, "h": 400},
        "code": "fn main() {\n    let v = vec![1, 2, 3];\n    println!(\"{v:?}\");\n}",
        "language": "rust",
        "highlight_lines": [2],
        "line_numbers": true
    }))
    .unwrap();
    let block = code.apply(&mut document, &Images::default()).unwrap();

    let body: TextParams = serde_json::from_value(serde_json::json!([
        {"text": "Ownership", "bold": true, "size": 40, "color": "#F2762E", "align": "center"},
        {"markdown": "- one owner\n  - moves\n- `Drop` runs at scope end", "size": 28},
        {"runs": [{"text": "H"}, {"text": "2", "baseline": "sub"}, {"text": "O, see "}, {"text": "docs", "link": "https://doc.rust-lang.org"}], "bullet": "none"}
    ]))
    .unwrap();
    let content = SlideContent {
        body_rich: Some(body.paragraphs().unwrap()),
        builds: Some(vec![BuildSpec {
            target: format!("id:{block}"),
            kind: "In".to_string(),
            effect: "apple:dissolve".to_string(),
            delivery: "By Bullet".to_string(),
            duration: 0.3,
            delay: 0.0,
            automatic: false,
        }]),
        ..Default::default()
    };
    document.edit_slide(3, &content).unwrap();

    let saved = temporary("code.key");
    document.save(&saved).unwrap();
    let document = KeynoteDocument::open(&saved).unwrap();
    assert_consistent(&document);

    let info = document.slide(3).unwrap();
    assert_eq!(
        info.body.as_deref(),
        Some("Ownership\none owner\nmoves\nDrop runs at scope end\nH2O, see docs")
    );
    let block_info = info.objects.iter().find(|e| e.id == block).unwrap();
    assert_eq!(block_info.kind, "text box");
    assert_eq!(block_info.fill.as_deref(), Some("#0F0E12"));
    assert!(block_info.text.as_deref().unwrap().starts_with("1  fn main() {"));
    // one build step per line of code
    assert_eq!(info.build_order.len(), 4);

    // the body has a hyperlink field and a list level, the block coloured runs
    let package = document.package();
    assert!(package.objects().any(|e| e.message_type() == 2032), "hyperlink field");
    let body_storage = {
        let slide = document.slide_message(3);
        let placeholder = super::format::proto::reference(&slide, 6).unwrap().unwrap();
        document.drawable_storage(placeholder).unwrap().unwrap()
    };
    let storage = package.message(body_storage).unwrap();
    let levels: Vec<u64> = storage
        .message(6)
        .unwrap()
        .unwrap()
        .messages(1)
        .unwrap()
        .iter()
        .map(|e| e.varint(2).unwrap())
        .collect();
    assert_eq!(levels, [0, 0, 1, 0, 0]);
    let char_runs = references(&storage.message(8).unwrap().unwrap(), 1).unwrap_or_default();
    let _ = char_runs;
    let block_storage = document.drawable_storage(block).unwrap().unwrap();
    let block_runs = package.message(block_storage).unwrap().message(8).unwrap().unwrap().messages(1).unwrap();
    assert!(block_runs.len() > 10, "{} runs", block_runs.len());

    // the preview wraps nothing in a wide box and finds no overflow
    let rendered = document.render_slide(3).unwrap();
    assert!(rendered.svg.contains("println!"));
    assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
}

////////////////////////////////////////////////////////////////////////////////
#[test]
fn warns_about_overflowing_text() {
    let mut document = template_with_slides();
    let long = "word ".repeat(200);
    let identifier = document
        .add_shape(
            5,
            &ShapeSpec {
                outline: Outline::Rect,
                frame: frame(100.0, 100.0, 300.0, 80.0),
                rotation: 0.0,
                text_box: false,
                preset: None,
                style: ShapeStyleSpec::default(),
                layout: TextLayoutSpec::default(),
                text: Some(vec![Paragraph::plain(&long)]),
                common: CommonSpec::default(),
            },
        )
        .unwrap();
    let warnings = document.text_warnings(5).unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains(&format!("id:{identifier}")));

    // shrinking to fit silences it
    document
        .edit_object(
            identifier,
            &ObjectEdit {
                layout: TextLayoutSpec {
                    shrink_to_fit: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    assert!(document.text_warnings(5).unwrap().is_empty());
}
