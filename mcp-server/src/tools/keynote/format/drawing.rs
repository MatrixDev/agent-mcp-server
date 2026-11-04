//! Drawing primitives shared by slide objects and styles: colours, frames, paths, fills,
//! strokes and shadows, as TSP/TSD archives (field numbers from keynote-parser's Keynote 14.5
//! schemas).

use anyhow::{Result, anyhow, bail};
use kurbo::{BezPath, PathEl, Point, Shape};
use serde::Serialize;

use super::proto::{Message, Value};

////////////////////////////////////////////////////////////////////////////////
/// TSP.Color
mod color {
    pub const MODEL: u32 = 1;
    pub const R: u32 = 3;
    pub const G: u32 = 4;
    pub const B: u32 = 5;
    pub const A: u32 = 6;
    pub const C: u32 = 7;
    pub const M: u32 = 8;
    pub const Y: u32 = 9;
    pub const K: u32 = 10;
    pub const W: u32 = 11;
    pub const RGB_SPACE: u32 = 12;

    pub const MODEL_RGB: u64 = 1;
    pub const MODEL_CMYK: u64 = 2;
    pub const MODEL_WHITE: u64 = 3;
    pub const SPACE_SRGB: u64 = 1;
}

/// TSP.Point and TSP.Size
mod pair {
    pub const FIRST: u32 = 1;
    pub const SECOND: u32 = 2;
}

/// TSD.GeometryArchive
pub mod geometry {
    pub const POSITION: u32 = 1;
    pub const SIZE: u32 = 2;
    pub const FLAGS: u32 = 3;
    pub const ANGLE: u32 = 4;

    /// width and height are both fixed
    pub const FLAGS_FIXED: u64 = 3;
}

/// TSP.Path
mod path {
    pub const ELEMENTS: u32 = 1;
    pub const ELEMENT_TYPE: u32 = 1;
    pub const ELEMENT_POINTS: u32 = 2;

    pub const MOVE_TO: u64 = 1;
    pub const LINE_TO: u64 = 2;
    pub const QUAD_TO: u64 = 3;
    pub const CURVE_TO: u64 = 4;
    pub const CLOSE: u64 = 5;
}

/// TSD.PathSourceArchive
pub mod path_source {
    pub const HORIZONTAL_FLIP: u32 = 1;
    pub const VERTICAL_FLIP: u32 = 2;
    pub const POINT: u32 = 3;
    pub const SCALAR: u32 = 4;
    pub const BEZIER: u32 = 5;
    pub const CALLOUT: u32 = 6;
    pub const CONNECTION_LINE: u32 = 7;
    pub const EDITABLE_BEZIER: u32 = 8;
}

/// TSD.BezierPathSourceArchive
mod bezier_source {
    pub const NATURAL_SIZE: u32 = 2;
    pub const PATH: u32 = 3;
}

/// TSD.ScalarPathSourceArchive
mod scalar_source {
    pub const TYPE: u32 = 1;
    pub const SCALAR: u32 = 2;

    pub const ROUNDED_RECTANGLE: u64 = 0;
    pub const REGULAR_POLYGON: u64 = 1;
    pub const CHEVRON: u64 = 2;
}

/// TSD.PointPathSourceArchive
mod point_source {
    pub const TYPE: u32 = 1;
    pub const POINT: u32 = 2;

    pub const LEFT_ARROW: u64 = 0;
    pub const RIGHT_ARROW: u64 = 1;
    pub const DOUBLE_ARROW: u64 = 10;
    pub const STAR: u64 = 100;
    pub const PLUS: u64 = 200;
}

/// TSD.CalloutPathSourceArchive
mod callout_source {
    pub const CORNER_RADIUS: u32 = 4;
}

/// TSD.EditableBezierPathSourceArchive
mod editable_source {
    pub const SUBPATHS: u32 = 1;
    pub const NATURAL_SIZE: u32 = 2;
    pub const NODES: u32 = 1;
    pub const CLOSED: u32 = 2;
    pub const IN_CONTROL: u32 = 1;
    pub const NODE: u32 = 2;
    pub const OUT_CONTROL: u32 = 3;
}

/// TSD.FillArchive
pub mod fill {
    pub const COLOR: u32 = 1;
    pub const GRADIENT: u32 = 2;
    pub const IMAGE: u32 = 3;
}

/// TSD.GradientArchive
mod gradient {
    pub const TYPE: u32 = 1;
    pub const STOPS: u32 = 2;
    pub const OPACITY: u32 = 3;
    pub const ADVANCED: u32 = 4;
    pub const ANGLE: u32 = 5;
    pub const STOP_COLOR: u32 = 1;
    pub const STOP_FRACTION: u32 = 2;
    pub const STOP_INFLECTION: u32 = 3;
    pub const ANGLE_DEGREES: u32 = 2;

    pub const LINEAR: u64 = 0;
}

/// TSD.ImageFillArchive
pub mod image_fill {
    pub const TECHNIQUE: u32 = 2;
    pub const FILL_SIZE: u32 = 4;
    pub const IMAGE_DATA: u32 = 6;

    pub const NATURAL_SIZE: u64 = 0;
    pub const STRETCH: u64 = 1;
    pub const SCALE_TO_FILL: u64 = 3;
    pub const SCALE_TO_FIT: u64 = 4;
}

/// TSD.StrokeArchive
mod stroke {
    pub const COLOR: u32 = 1;
    pub const WIDTH: u32 = 2;
    pub const CAP: u32 = 3;
    pub const JOIN: u32 = 4;
    pub const MITER_LIMIT: u32 = 5;
    pub const PATTERN: u32 = 6;
    pub const PATTERN_TYPE: u32 = 1;
    pub const PATTERN_PHASE: u32 = 2;
    pub const PATTERN_COUNT: u32 = 3;
    pub const PATTERN_LENGTHS: u32 = 4;

    pub const PATTERN_DASHED: u64 = 0;
    pub const PATTERN_SOLID: u64 = 1;
    pub const PATTERN_EMPTY: u64 = 2;
    /// Keynote always writes six lengths
    pub const PATTERN_SLOTS: usize = 6;
}

/// TSD.ShadowArchive
mod shadow {
    pub const COLOR: u32 = 1;
    pub const ANGLE: u32 = 2;
    pub const OFFSET: u32 = 3;
    pub const RADIUS: u32 = 4;
    pub const OPACITY: u32 = 5;
    pub const ENABLED: u32 = 6;
    pub const TYPE: u32 = 7;
}

/// TSD.LineEndArchive
mod line_end {
    pub const PATH: u32 = 1;
    pub const JOIN: u32 = 2;
    pub const END_POINT: u32 = 3;
    pub const FILLED: u32 = 4;
    pub const IDENTIFIER: u32 = 5;
}

////////////////////////////////////////////////////////////////////////////////
// Colours
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);

    ////////////////////////////////////////////////////////////////////////////////
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// "#RGB", "#RRGGBB" or "#RRGGBBAA"
    pub fn parse(text: &str) -> Result<Self> {
        let hex = text.trim().trim_start_matches('#');
        let digits: Vec<u8> = hex
            .chars()
            .map(|e| e.to_digit(16).map(|e| e as u8))
            .collect::<Option<_>>()
            .ok_or_else(|| anyhow!("invalid colour {text:?}, use #RRGGBB or #RRGGBBAA"))?;

        let channel = |index: usize| f32::from(digits[index] * 16 + digits[index + 1]) / 255.0;
        match digits.len() {
            3 => Ok(Self::rgb(
                f32::from(digits[0] * 17) / 255.0,
                f32::from(digits[1] * 17) / 255.0,
                f32::from(digits[2] * 17) / 255.0,
            )),
            6 => Ok(Self::rgb(channel(0), channel(2), channel(4))),
            8 => Ok(Self {
                a: channel(6),
                ..Self::rgb(channel(0), channel(2), channel(4))
            }),
            _ => bail!("invalid colour {text:?}, use #RRGGBB or #RRGGBBAA"),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// "#RRGGBB", with "AA" appended when not opaque
    pub fn hex(&self) -> String {
        let byte = |e: f32| (e.clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut text = format!("#{:02X}{:02X}{:02X}", byte(self.r), byte(self.g), byte(self.b));
        if byte(self.a) != 255 {
            text.push_str(&format!("{:02X}", byte(self.a)));
        }
        text
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// TSP.Color in sRGB, the way Keynote writes it
    pub fn message(&self) -> Message {
        let mut message = Message::default();
        message.set_varint(color::MODEL, color::MODEL_RGB);
        message.set(color::R, float(self.r));
        message.set(color::G, float(self.g));
        message.set(color::B, float(self.b));
        message.set(color::A, float(self.a));
        message.set_varint(color::RGB_SPACE, color::SPACE_SRGB);
        message
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn from_message(message: &Message) -> Option<Self> {
        let get = |number| message.float(number).unwrap_or(0.0);
        let a = message.float(color::A).unwrap_or(1.0);
        match message.varint(color::MODEL)? {
            color::MODEL_RGB => Some(Self {
                a,
                ..Self::rgb(get(color::R), get(color::G), get(color::B))
            }),
            color::MODEL_WHITE => Some(Self {
                a,
                ..Self::rgb(get(color::W), get(color::W), get(color::W))
            }),
            color::MODEL_CMYK => {
                let k = get(color::K);
                Some(Self {
                    a,
                    ..Self::rgb(
                        (1.0 - get(color::C)) * (1.0 - k),
                        (1.0 - get(color::M)) * (1.0 - k),
                        (1.0 - get(color::Y)) * (1.0 - k),
                    )
                })
            }
            _ => None,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
pub fn float(value: f32) -> Value {
    Value::Fixed32(value.to_bits())
}

////////////////////////////////////////////////////////////////////////////////
/// TSP.Point or TSP.Size
pub fn pair_message(first: f32, second: f32) -> Message {
    let mut message = Message::default();
    message.set(pair::FIRST, float(first));
    message.set(pair::SECOND, float(second));
    message
}

////////////////////////////////////////////////////////////////////////////////
pub fn pair_of(message: &Message) -> (f32, f32) {
    (
        message.float(pair::FIRST).unwrap_or(0.0),
        message.float(pair::SECOND).unwrap_or(0.0),
    )
}

////////////////////////////////////////////////////////////////////////////////
// Geometry
////////////////////////////////////////////////////////////////////////////////

/// Rectangle in slide points, origin top-left
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Frame {
    ////////////////////////////////////////////////////////////////////////////////
    /// TSD.GeometryArchive; `rotation` in degrees clockwise
    pub fn geometry_message(&self, rotation: f32) -> Message {
        let mut message = Message::default();
        message.set_message(geometry::POSITION, &pair_message(self.x, self.y));
        message.set_message(geometry::SIZE, &pair_message(self.w, self.h));
        message.set_varint(geometry::FLAGS, geometry::FLAGS_FIXED);
        message.set(geometry::ANGLE, float(keynote_angle(rotation)));
        message
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Frame and clockwise rotation of a TSD.GeometryArchive
    pub fn from_geometry(message: &Message) -> Result<(Self, f32)> {
        let (x, y) = message.message(geometry::POSITION)?.as_ref().map(pair_of).unwrap_or_default();
        let (w, h) = message.message(geometry::SIZE)?.as_ref().map(pair_of).unwrap_or_default();
        let angle = message.float(geometry::ANGLE).unwrap_or(0.0);
        Ok((Self { x, y, w, h }, keynote_angle(angle)))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Keynote stores rotation counter-clockwise, the tools speak clockwise (like SVG and CSS);
/// the conversion is its own inverse
pub fn keynote_angle(degrees: f32) -> f32 {
    let angle = (360.0 - degrees).rem_euclid(360.0);
    if angle.abs() < 1e-4 || (angle - 360.0).abs() < 1e-4 { 0.0 } else { angle }
}

////////////////////////////////////////////////////////////////////////////////
// Paths
////////////////////////////////////////////////////////////////////////////////

/// Outline kinds of new shapes
#[derive(Debug, Clone, PartialEq)]
pub enum Outline {
    Rect,
    RoundedRect { radius: f32 },
    Ellipse,
    Triangle,
    RightTriangle,
    Diamond,
    Pentagon,
    Hexagon,
    Polygon { sides: u32 },
    Star { points: u32, inner: f32 },
    Arrow { double: bool },
    Chevron,
    Callout { radius: f32 },
    Plus,
    /// SVG path data in its own coordinates, scaled to the frame
    Path(String),
}

////////////////////////////////////////////////////////////////////////////////
/// The outline as a path in a `w` × `h` box
pub fn outline_path(outline: &Outline, w: f64, h: f64) -> Result<BezPath> {
    let polygon = |points: &[(f64, f64)]| {
        let mut path = BezPath::new();
        for (index, (x, y)) in points.iter().enumerate() {
            let point = Point::new(x * w, y * h);
            if index == 0 { path.move_to(point) } else { path.line_to(point) }
        }
        path.close_path();
        path
    };
    let regular = |sides: u32, inner: Option<f64>| {
        let count = if inner.is_some() { sides * 2 } else { sides };
        let points: Vec<(f64, f64)> = (0..count)
            .map(|index| {
                let angle = -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * index as f64 / count as f64;
                let radius = match inner {
                    Some(inner) if index % 2 == 1 => inner,
                    _ => 1.0,
                };
                (0.5 + 0.5 * radius * angle.cos(), 0.5 + 0.5 * radius * angle.sin())
            })
            .collect();
        polygon(&points)
    };

    let path = match outline {
        Outline::Rect => polygon(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
        Outline::RoundedRect { radius } => {
            let radius = f64::from(*radius).min(w / 2.0).min(h / 2.0).max(0.0);
            kurbo::RoundedRect::new(0.0, 0.0, w, h, radius).to_path(0.01)
        }
        Outline::Ellipse => kurbo::Ellipse::from_rect(kurbo::Rect::new(0.0, 0.0, w, h)).to_path(0.01),
        Outline::Triangle => polygon(&[(0.5, 0.0), (1.0, 1.0), (0.0, 1.0)]),
        Outline::RightTriangle => polygon(&[(0.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
        Outline::Diamond => polygon(&[(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)]),
        Outline::Pentagon => regular(5, None),
        Outline::Hexagon => regular(6, None),
        Outline::Polygon { sides } => {
            if *sides < 3 {
                bail!("a polygon needs at least 3 sides");
            }
            regular(*sides, None)
        }
        Outline::Star { points, inner } => {
            if *points < 3 {
                bail!("a star needs at least 3 points");
            }
            regular(*points, Some(f64::from(inner.clamp(0.05, 0.95))))
        }
        Outline::Arrow { double: false } => polygon(&[
            (0.0, 0.3),
            (0.65, 0.3),
            (0.65, 0.0),
            (1.0, 0.5),
            (0.65, 1.0),
            (0.65, 0.7),
            (0.0, 0.7),
        ]),
        Outline::Arrow { double: true } => polygon(&[
            (0.0, 0.5),
            (0.3, 0.0),
            (0.3, 0.3),
            (0.7, 0.3),
            (0.7, 0.0),
            (1.0, 0.5),
            (0.7, 1.0),
            (0.7, 0.7),
            (0.3, 0.7),
            (0.3, 1.0),
        ]),
        Outline::Chevron => polygon(&[(0.0, 0.0), (0.75, 0.0), (1.0, 0.5), (0.75, 1.0), (0.0, 1.0), (0.25, 0.5)]),
        Outline::Plus => polygon(&[
            (0.35, 0.0),
            (0.65, 0.0),
            (0.65, 0.35),
            (1.0, 0.35),
            (1.0, 0.65),
            (0.65, 0.65),
            (0.65, 1.0),
            (0.35, 1.0),
            (0.35, 0.65),
            (0.0, 0.65),
            (0.0, 0.35),
            (0.35, 0.35),
        ]),
        Outline::Callout { radius } => {
            // speech bubble: a rounded body over the top 80%, a tail at the bottom left
            let body = h * 0.8;
            let radius = f64::from(*radius).min(w / 2.0).min(body / 2.0).max(0.0);
            let mut path = kurbo::RoundedRect::new(0.0, 0.0, w, body, radius).to_path(0.01);
            path.move_to((w * 0.2, body - 0.5));
            path.line_to((w * 0.15, h));
            path.line_to((w * 0.35, body - 0.5));
            path.close_path();
            path
        }
        Outline::Path(data) => {
            let path = BezPath::from_svg(data).map_err(|e| anyhow!("invalid SVG path data: {e}"))?;
            let bounds = path.bounding_box();
            if bounds.width() <= 0.0 && bounds.height() <= 0.0 {
                bail!("the SVG path is empty");
            }
            let sx = if bounds.width() > 0.0 { w / bounds.width() } else { 1.0 };
            let sy = if bounds.height() > 0.0 { h / bounds.height() } else { 1.0 };
            kurbo::Affine::scale_non_uniform(sx, sy) * kurbo::Affine::translate((-bounds.x0, -bounds.y0)) * path
        }
    };
    Ok(path)
}

////////////////////////////////////////////////////////////////////////////////
/// Bounding box of SVG path data, to place a path shape where its coordinates say
pub fn svg_path_bounds(data: &str) -> Result<Frame> {
    let path = BezPath::from_svg(data).map_err(|e| anyhow!("invalid SVG path data: {e}"))?;
    let bounds = path.bounding_box();
    Ok(Frame {
        x: bounds.x0 as f32,
        y: bounds.y0 as f32,
        w: bounds.width() as f32,
        h: bounds.height() as f32,
    })
}

////////////////////////////////////////////////////////////////////////////////
/// TSP.Path
pub fn path_message(path: &BezPath) -> Message {
    let point = |e: Point| pair_message(e.x as f32, e.y as f32);
    let mut message = Message::default();
    for element in path.elements() {
        let mut entry = Message::default();
        let (kind, points) = match *element {
            PathEl::MoveTo(a) => (path::MOVE_TO, vec![a]),
            PathEl::LineTo(a) => (path::LINE_TO, vec![a]),
            PathEl::QuadTo(a, b) => (path::QUAD_TO, vec![a, b]),
            PathEl::CurveTo(a, b, c) => (path::CURVE_TO, vec![a, b, c]),
            PathEl::ClosePath => (path::CLOSE, vec![]),
        };
        entry.set_varint(path::ELEMENT_TYPE, kind);
        for e in points {
            entry.push_message(path::ELEMENT_POINTS, &point(e));
        }
        message.push_message(path::ELEMENTS, &entry);
    }
    message
}

////////////////////////////////////////////////////////////////////////////////
pub fn path_from_message(message: &Message) -> Result<BezPath> {
    let mut path = BezPath::new();
    for element in message.messages(path::ELEMENTS)? {
        let points: Vec<Point> = element
            .messages(path::ELEMENT_POINTS)?
            .iter()
            .map(|e| {
                let (x, y) = pair_of(e);
                Point::new(f64::from(x), f64::from(y))
            })
            .collect();
        match (element.varint(path::ELEMENT_TYPE), points.as_slice()) {
            (Some(path::MOVE_TO), [a, ..]) => path.move_to(*a),
            (Some(path::LINE_TO), [a, ..]) => path.line_to(*a),
            (Some(path::QUAD_TO), [a, b, ..]) => path.quad_to(*a, *b),
            (Some(path::CURVE_TO), [a, b, c, ..]) => path.curve_to(*a, *b, *c),
            (Some(path::CLOSE), _) => path.close_path(),
            _ => {}
        }
    }
    Ok(path)
}

////////////////////////////////////////////////////////////////////////////////
/// TSD.PathSourceArchive holding a bezier path in a `size` box, like Keynote writes for
/// text boxes and placeholders
pub fn bezier_path_source(path: &BezPath, w: f32, h: f32) -> Message {
    let mut bezier = Message::default();
    bezier.set_message(bezier_source::NATURAL_SIZE, &pair_message(w, h));
    bezier.set_message(bezier_source::PATH, &path_message(path));

    let mut source = Message::default();
    source.set_bool(path_source::HORIZONTAL_FLIP, false);
    source.set_bool(path_source::VERTICAL_FLIP, false);
    source.set_message(path_source::BEZIER, &bezier);
    source
}

////////////////////////////////////////////////////////////////////////////////
/// Outline of a TSD.PathSourceArchive scaled to a `w` × `h` frame, None when the source is of
/// a kind the renderer doesn't know
pub fn path_source_outline(source: &Message, w: f64, h: f64) -> Result<Option<BezPath>> {
    let scale = |path: BezPath, natural: Option<Message>| {
        let (nw, nh) = natural.as_ref().map(pair_of).unwrap_or((w as f32, h as f32));
        let sx = if nw > 0.0 { w / f64::from(nw) } else { 1.0 };
        let sy = if nh > 0.0 { h / f64::from(nh) } else { 1.0 };
        kurbo::Affine::scale_non_uniform(sx, sy) * path
    };

    let mut outline = None;
    if let Some(bezier) = source.message(path_source::BEZIER)? {
        let path = bezier.message(bezier_source::PATH)?.unwrap_or_default();
        outline = Some(scale(path_from_message(&path)?, bezier.message(bezier_source::NATURAL_SIZE)?));
    } else if let Some(connection) = source.message(path_source::CONNECTION_LINE)? {
        let bezier = connection.message(1)?.unwrap_or_default();
        let path = bezier.message(bezier_source::PATH)?.unwrap_or_default();
        outline = Some(scale(path_from_message(&path)?, bezier.message(bezier_source::NATURAL_SIZE)?));
    } else if let Some(scalar) = source.message(path_source::SCALAR)? {
        let value = f64::from(scalar.float(scalar_source::SCALAR).unwrap_or(0.0));
        let shape = match scalar.varint(scalar_source::TYPE).unwrap_or(scalar_source::ROUNDED_RECTANGLE) {
            scalar_source::ROUNDED_RECTANGLE => Outline::RoundedRect { radius: value as f32 },
            scalar_source::REGULAR_POLYGON => Outline::Polygon {
                sides: (value.round() as u32).max(3),
            },
            scalar_source::CHEVRON => Outline::Chevron,
            _ => Outline::Rect,
        };
        outline = Some(outline_path(&shape, w, h)?);
    } else if let Some(point) = source.message(path_source::POINT)? {
        let shape = match point.varint(point_source::TYPE) {
            Some(point_source::STAR) => {
                let (points, inner) = point.message(point_source::POINT)?.as_ref().map(pair_of).unwrap_or((5.0, 0.5));
                Outline::Star {
                    points: (points.round() as u32).max(3),
                    inner,
                }
            }
            Some(point_source::PLUS) => Outline::Plus,
            Some(point_source::DOUBLE_ARROW) => Outline::Arrow { double: true },
            Some(point_source::LEFT_ARROW | point_source::RIGHT_ARROW) => Outline::Arrow { double: false },
            _ => Outline::Rect,
        };
        outline = Some(outline_path(&shape, w, h)?);
    } else if let Some(callout) = source.message(path_source::CALLOUT)? {
        let radius = callout.float(callout_source::CORNER_RADIUS).unwrap_or(0.0);
        outline = Some(outline_path(&Outline::Callout { radius }, w, h)?);
    } else if let Some(editable) = source.message(path_source::EDITABLE_BEZIER)? {
        let mut path = BezPath::new();
        let get = |node: &Message, field| -> Result<Point> {
            let (x, y) = node.message(field)?.as_ref().map(pair_of).unwrap_or_default();
            Ok(Point::new(f64::from(x), f64::from(y)))
        };
        for subpath in editable.messages(editable_source::SUBPATHS)? {
            let nodes = subpath.messages(editable_source::NODES)?;
            for (index, node) in nodes.iter().enumerate() {
                let point = get(node, editable_source::NODE)?;
                if index == 0 {
                    path.move_to(point);
                    continue;
                }
                let previous = get(&nodes[index - 1], editable_source::OUT_CONTROL)?;
                path.curve_to(previous, get(node, editable_source::IN_CONTROL)?, point);
            }
            if subpath.bool(editable_source::CLOSED).unwrap_or(false) {
                if let (Some(first), Some(last)) = (nodes.first(), nodes.last()) {
                    path.curve_to(
                        get(last, editable_source::OUT_CONTROL)?,
                        get(first, editable_source::IN_CONTROL)?,
                        get(first, editable_source::NODE)?,
                    );
                }
                path.close_path();
            }
        }
        outline = Some(scale(path, editable.message(editable_source::NATURAL_SIZE)?));
    }

    let flip_x = source.bool(path_source::HORIZONTAL_FLIP).unwrap_or(false);
    let flip_y = source.bool(path_source::VERTICAL_FLIP).unwrap_or(false);
    Ok(outline.map(|path| {
        let flip = kurbo::Affine::new([
            if flip_x { -1.0 } else { 1.0 },
            0.0,
            0.0,
            if flip_y { -1.0 } else { 1.0 },
            if flip_x { w } else { 0.0 },
            if flip_y { h } else { 0.0 },
        ]);
        flip * path
    }))
}

////////////////////////////////////////////////////////////////////////////////
// Fills
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    None,
    Color(Color),
    /// linear gradient, `angle` in degrees clockwise from left-to-right
    Gradient { angle: f32, stops: Vec<(Color, f32)> },
    Image { data: u64, technique: u64, size: (f32, f32) },
}

impl Fill {
    ////////////////////////////////////////////////////////////////////////////////
    /// TSD.FillArchive; "no fill" is an empty one
    pub fn message(&self) -> Message {
        let mut message = Message::default();
        match self {
            Fill::None => {}
            Fill::Color(color) => message.set_message(fill::COLOR, &color.message()),
            Fill::Gradient { angle, stops } => {
                let mut archive = Message::default();
                archive.set_varint(gradient::TYPE, gradient::LINEAR);
                for (color, fraction) in stops {
                    let mut stop = Message::default();
                    stop.set_message(gradient::STOP_COLOR, &color.message());
                    stop.set(gradient::STOP_FRACTION, float(*fraction));
                    stop.set(gradient::STOP_INFLECTION, float(0.5));
                    archive.push_message(gradient::STOPS, &stop);
                }
                archive.set(gradient::OPACITY, float(1.0));
                archive.set_bool(gradient::ADVANCED, stops.len() > 2);
                let mut angle_archive = Message::default();
                angle_archive.set(gradient::ANGLE_DEGREES, float(keynote_angle(*angle)));
                archive.set_message(gradient::ANGLE, &angle_archive);
                message.set_message(fill::GRADIENT, &archive);
            }
            Fill::Image { data, technique, size } => {
                let mut reference = Message::default();
                reference.set_varint(1, *data);
                let mut archive = Message::default();
                archive.set_varint(image_fill::TECHNIQUE, *technique);
                archive.set_message(image_fill::FILL_SIZE, &pair_message(size.0, size.1));
                archive.set_message(image_fill::IMAGE_DATA, &reference);
                message.set_message(fill::IMAGE, &archive);
            }
        }
        message
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn from_message(message: &Message) -> Result<Self> {
        if let Some(color) = message.message(fill::COLOR)?.as_ref().and_then(Color::from_message) {
            return Ok(Fill::Color(color));
        }
        if let Some(archive) = message.message(fill::GRADIENT)? {
            let mut stops = Vec::new();
            for stop in archive.messages(gradient::STOPS)? {
                let color = stop.message(gradient::STOP_COLOR)?.as_ref().and_then(Color::from_message);
                stops.push((color.unwrap_or(Color::BLACK), stop.float(gradient::STOP_FRACTION).unwrap_or(0.0)));
            }
            let angle = archive
                .message(gradient::ANGLE)?
                .and_then(|e| e.float(gradient::ANGLE_DEGREES))
                .unwrap_or(0.0);
            let opacity = archive.float(gradient::OPACITY).unwrap_or(1.0);
            for stop in &mut stops {
                stop.0.a *= opacity;
            }
            return Ok(Fill::Gradient {
                angle: keynote_angle(angle),
                stops,
            });
        }
        if let Some(archive) = message.message(fill::IMAGE)? {
            let data = archive.message(image_fill::IMAGE_DATA)?.and_then(|e| e.varint(1));
            if let Some(data) = data {
                let size = archive.message(image_fill::FILL_SIZE)?.as_ref().map(pair_of).unwrap_or_default();
                return Ok(Fill::Image {
                    data,
                    technique: archive.varint(image_fill::TECHNIQUE).unwrap_or(image_fill::NATURAL_SIZE),
                    size,
                });
            }
        }
        Ok(Fill::None)
    }
}

////////////////////////////////////////////////////////////////////////////////
// Strokes and shadows
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dash {
    Solid,
    Dash,
    Dot,
    DashDot,
}

impl Dash {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
            "solid" => Ok(Dash::Solid),
            "dash" | "dashed" => Ok(Dash::Dash),
            "dot" | "dotted" => Ok(Dash::Dot),
            "dash_dot" => Ok(Dash::DashDot),
            other => bail!("unknown dash {other:?}, use solid, dash, dot or dash_dot"),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Pattern lengths in multiples of the stroke width
    fn lengths(&self) -> &'static [f32] {
        match self {
            Dash::Solid => &[],
            Dash::Dash => &[3.0, 2.0],
            Dash::Dot => &[0.01, 2.0],
            Dash::DashDot => &[3.0, 2.0, 0.01, 2.0],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub color: Color,
    pub width: f32,
    pub dash: Dash,
}

impl Stroke {
    ////////////////////////////////////////////////////////////////////////////////
    /// TSD.StrokeArchive; None gives Keynote's "no line"
    pub fn message(stroke: Option<&Stroke>) -> Message {
        let (color, width, kind, lengths): (Color, f32, u64, &[f32]) = match stroke {
            None => (Color::BLACK, 1.0, stroke::PATTERN_EMPTY, &[]),
            Some(e) if e.dash == Dash::Solid => (e.color, e.width, stroke::PATTERN_SOLID, &[]),
            Some(e) => (e.color, e.width, stroke::PATTERN_DASHED, e.dash.lengths()),
        };

        let mut pattern = Message::default();
        pattern.set_varint(stroke::PATTERN_TYPE, kind);
        pattern.set(stroke::PATTERN_PHASE, float(0.0));
        pattern.set_varint(stroke::PATTERN_COUNT, lengths.len() as u64);
        for index in 0..stroke::PATTERN_SLOTS {
            pattern.fields.push(super::proto::Field {
                number: stroke::PATTERN_LENGTHS,
                value: float(lengths.get(index).copied().unwrap_or(0.0)),
            });
        }

        let mut message = Message::default();
        message.set_message(stroke::COLOR, &color.message());
        message.set(stroke::WIDTH, float(width));
        message.set_varint(stroke::CAP, if stroke.is_some_and(|e| e.dash == Dash::Dot) { 1 } else { 0 });
        message.set_varint(stroke::JOIN, 0);
        message.set(stroke::MITER_LIMIT, float(4.0));
        message.set_message(stroke::PATTERN, &pattern);
        message
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// None when the stroke draws nothing
    pub fn from_message(message: &Message) -> Result<Option<Self>> {
        let pattern = message.message(stroke::PATTERN)?.unwrap_or_default();
        let kind = pattern.varint(stroke::PATTERN_TYPE).unwrap_or(stroke::PATTERN_SOLID);
        let width = message.float(stroke::WIDTH).unwrap_or(1.0);
        if kind == stroke::PATTERN_EMPTY || width <= 0.0 || message.fields.is_empty() {
            return Ok(None);
        }

        let dash = if kind == stroke::PATTERN_SOLID {
            Dash::Solid
        } else {
            let count = pattern.varint(stroke::PATTERN_COUNT).unwrap_or(0) as usize;
            let lengths: Vec<f32> = pattern
                .fields
                .iter()
                .filter(|e| e.number == stroke::PATTERN_LENGTHS)
                .filter_map(|e| match e.value {
                    Value::Fixed32(e) => Some(f32::from_bits(e)),
                    _ => None,
                })
                .take(count)
                .collect();
            match lengths.as_slice() {
                [a, ..] if *a < 0.5 => Dash::Dot,
                [_, _, _, _, ..] => Dash::DashDot,
                _ => Dash::Dash,
            }
        };
        let color = message.message(stroke::COLOR)?.as_ref().and_then(Color::from_message);
        Ok(Some(Self {
            color: color.unwrap_or(Color::BLACK),
            width,
            dash,
        }))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    pub color: Color,
    pub blur: f32,
    pub dx: f32,
    pub dy: f32,
}

impl Shadow {
    ////////////////////////////////////////////////////////////////////////////////
    /// TSD.ShadowArchive; None gives a disabled shadow
    pub fn message(shadow: Option<&Shadow>) -> Message {
        let mut message = Message::default();
        let Some(shadow) = shadow else {
            // what Keynote writes for "no shadow"
            message.set_message(shadow::COLOR, &Color::BLACK.message());
            message.set(shadow::ANGLE, float(315.0));
            message.set(shadow::OFFSET, float(5.0));
            message.set_varint(shadow::RADIUS, 1);
            message.set(shadow::OPACITY, float(1.0));
            message.set_bool(shadow::ENABLED, false);
            message.set_varint(shadow::TYPE, 0);
            return message;
        };

        // Keynote measures the angle counter-clockwise with y pointing up
        let angle = (-shadow.dy).atan2(shadow.dx).to_degrees().rem_euclid(360.0);
        message.set_message(shadow::COLOR, &shadow.color.with_alpha(1.0).message());
        message.set(shadow::ANGLE, float(angle));
        message.set(shadow::OFFSET, float(shadow.dx.hypot(shadow.dy)));
        message.set_varint(shadow::RADIUS, shadow.blur.round().max(0.0) as u64);
        message.set(shadow::OPACITY, float(shadow.color.a));
        message.set_bool(shadow::ENABLED, true);
        message.set_varint(shadow::TYPE, 0);
        message
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn from_message(message: &Message) -> Result<Option<Self>> {
        if !message.bool(shadow::ENABLED).unwrap_or(true) || message.fields.is_empty() {
            return Ok(None);
        }
        let color = message.message(shadow::COLOR)?.as_ref().and_then(Color::from_message);
        let angle = message.float(shadow::ANGLE).unwrap_or(315.0).to_radians();
        let offset = message.float(shadow::OFFSET).unwrap_or(5.0);
        let opacity = message.float(shadow::OPACITY).unwrap_or(1.0);
        Ok(Some(Self {
            color: color.unwrap_or(Color::BLACK).with_alpha(opacity),
            blur: message.varint(shadow::RADIUS).unwrap_or(1) as f32,
            dx: offset * angle.cos(),
            dy: -offset * angle.sin(),
        }))
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Arrow heads of lines
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnd {
    None,
    Arrow,
    FilledArrow,
    Circle,
    Square,
    Diamond,
    Bar,
}

impl LineEnd {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
            "none" => Ok(LineEnd::None),
            "arrow" | "open_arrow" => Ok(LineEnd::Arrow),
            "filled_arrow" | "triangle" => Ok(LineEnd::FilledArrow),
            "circle" | "dot" => Ok(LineEnd::Circle),
            "square" => Ok(LineEnd::Square),
            "diamond" => Ok(LineEnd::Diamond),
            "bar" | "line" => Ok(LineEnd::Bar),
            other => bail!("unknown line end {other:?}, use none, arrow, filled_arrow, circle, square, diamond or bar"),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Keynote's identifier and the head outline in a unit box pointing right, tip at (1, 0.5)
    fn shape(&self) -> Option<(&'static str, bool, Vec<(f64, f64)>)> {
        match self {
            LineEnd::None => None,
            LineEnd::Arrow => Some(("simple arrow", false, vec![(0.0, 0.0), (1.0, 0.5), (0.0, 1.0)])),
            LineEnd::FilledArrow => Some(("filled arrow", true, vec![(0.0, 0.0), (1.0, 0.5), (0.0, 1.0)])),
            LineEnd::Circle => Some(("filled circle", true, Vec::new())),
            LineEnd::Square => Some(("filled square", true, vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])),
            LineEnd::Diamond => Some((
                "filled diamond",
                true,
                vec![(0.0, 0.5), (0.5, 0.0), (1.0, 0.5), (0.5, 1.0)],
            )),
            LineEnd::Bar => Some(("line", false, vec![(1.0, 0.0), (1.0, 1.0)])),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// TSD.LineEndArchive, an empty one for no head
    pub fn message(&self) -> Message {
        let mut message = Message::default();
        let Some((identifier, filled, points)) = self.shape() else {
            return message;
        };
        let size = 10.0;
        let path = if points.is_empty() {
            kurbo::Circle::new((size / 2.0, size / 2.0), size / 2.0).to_path(0.01)
        } else {
            let mut path = BezPath::new();
            for (index, (x, y)) in points.iter().enumerate() {
                let point = Point::new(x * size, y * size);
                if index == 0 { path.move_to(point) } else { path.line_to(point) }
            }
            if filled {
                path.close_path();
            }
            path
        };
        message.set_message(line_end::PATH, &path_message(&path));
        message.set_varint(line_end::JOIN, 0);
        message.set_message(line_end::END_POINT, &pair_message(size as f32, size as f32 / 2.0));
        message.set_bool(line_end::FILLED, filled);
        message.set_bytes(line_end::IDENTIFIER, identifier.as_bytes().to_vec());
        message
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn from_message(message: &Message) -> Self {
        match message.string(line_end::IDENTIFIER).as_deref() {
            None | Some("") | Some("none") => LineEnd::None,
            Some(e) if e.contains("circle") => LineEnd::Circle,
            Some(e) if e.contains("square") => LineEnd::Square,
            Some(e) if e.contains("diamond") => LineEnd::Diamond,
            Some(e) if e.contains("filled") => LineEnd::FilledArrow,
            Some("line") => LineEnd::Bar,
            Some(_) => LineEnd::Arrow,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_colours() {
        let color = Color::parse("#F2762E").unwrap();
        assert_eq!(color.hex(), "#F2762E");
        assert_eq!(Color::parse("#00000080").unwrap().hex(), "#00000080");
        assert_eq!(Color::parse("#fff").unwrap(), Color::WHITE);
        assert!(Color::parse("red").is_err());
        assert_eq!(Color::from_message(&color.message()), Some(color));
    }

    #[test]
    fn round_trips_geometry_and_rotation() {
        let frame = Frame {
            x: 96.0,
            y: 290.0,
            w: 400.0,
            h: 200.0,
        };
        let (decoded, rotation) = Frame::from_geometry(&frame.geometry_message(30.0)).unwrap();
        assert_eq!(decoded, frame);
        assert!((rotation - 30.0).abs() < 1e-3);
        assert_eq!(keynote_angle(0.0), 0.0);
    }

    #[test]
    fn round_trips_paths_fills_and_strokes() {
        let path = outline_path(&Outline::RoundedRect { radius: 16.0 }, 400.0, 200.0).unwrap();
        let decoded = path_from_message(&path_message(&path)).unwrap();
        assert_eq!(decoded.elements().len(), path.elements().len());

        let source = bezier_path_source(&path, 400.0, 200.0);
        let scaled = path_source_outline(&source, 800.0, 400.0).unwrap().unwrap();
        assert!((scaled.bounding_box().width() - 800.0).abs() < 1e-3);

        let fill = Fill::Gradient {
            angle: 135.0,
            stops: vec![(Color::BLACK, 0.0), (Color::WHITE, 1.0)],
        };
        assert_eq!(Fill::from_message(&fill.message()).unwrap(), fill);
        assert_eq!(Fill::from_message(&Fill::None.message()).unwrap(), Fill::None);

        let stroke = Stroke {
            color: Color::WHITE,
            width: 2.0,
            dash: Dash::Dash,
        };
        assert_eq!(Stroke::from_message(&Stroke::message(Some(&stroke))).unwrap(), Some(stroke));
        assert_eq!(Stroke::from_message(&Stroke::message(None)).unwrap(), None);

        let shadow = Shadow {
            color: Color::parse("#00000080").unwrap(),
            blur: 12.0,
            dx: 0.0,
            dy: 4.0,
        };
        let decoded = Shadow::from_message(&Shadow::message(Some(&shadow))).unwrap().unwrap();
        assert!((decoded.dy - 4.0).abs() < 1e-3 && decoded.dx.abs() < 1e-3);
    }

    #[test]
    fn builds_outlines_from_svg_paths() {
        let bounds = svg_path_bounds("M10 20 L110 20 L60 120 Z").unwrap();
        assert_eq!((bounds.x, bounds.y, bounds.w, bounds.h), (10.0, 20.0, 100.0, 100.0));
        let path = outline_path(&Outline::Path("M10 20 L110 20 L60 120 Z".into()), 50.0, 50.0).unwrap();
        assert_eq!(path.bounding_box(), kurbo::Rect::new(0.0, 0.0, 50.0, 50.0));
        assert!(outline_path(&Outline::Path("nonsense".into()), 1.0, 1.0).is_err());
    }
}
