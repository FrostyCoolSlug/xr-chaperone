// This file exists because the Canvas drawer is a pretty involved piece of code, and is making
// reading ui.rs almost impossible :D

use crate::ui::Message;
use glam::Vec2;
use iced::widget::canvas::{Frame, Path, Stroke};
use iced::widget::{canvas, Action};
use iced::{mouse, Color, Point, Rectangle, Size};

/// Half-width of the fixed world view in metres. The fixed canvas always
/// represents a (VIEW_HALF*2) x (VIEW_HALF*2) metre area, regardless of
/// how many pixels the canvas occupies.
pub(crate) const VIEW_HALF: f32 = 5.0;
pub(crate) const VERTEX_RADIUS: f32 = 7.0;
pub(crate) const GRID_SIZE_METERS: f32 = 0.25;

pub(crate) const COLOUR_CONTROLLER: Color = Color {
    r: 1.0,
    g: 0.8,
    b: 0.0,
    a: 1.0,
};
pub(crate) const COLOUR_HEADSET: Color = Color {
    r: 0.7,
    g: 0.7,
    b: 0.7,
    a: 1.0,
};
pub(crate) const COLOUR_EDGE: Color = Color {
    r: 0.0,
    g: 0.6,
    b: 1.0,
    a: 1.0,
};
pub(crate) const COLOUR_FILL: Color = Color {
    r: 0.0,
    g: 0.5,
    b: 1.0,
    a: 0.12,
};

/// Behaviour for the coordinate systems of the canvas drawer
///
/// - `Fixed` - A completely static canvas (used during tracing)
/// - `Fitted`: A canvas that zooms to the canvas area (used on main page)
/// - `FittedEditable`: Similar to fitted, except with a larger margin (used during review)
#[derive(Clone)]
pub enum CanvasMode {
    Fixed { closed: bool },
    Fitted { closed: bool },
    FittedEditable,
}

/// The state of the canvas, tracks the current point being dragged, and the fitting points
/// for rendering in FittedEditable to prevent the canvas expanding while nodes are being
/// shifted.
#[derive(Default)]
pub struct BoundaryCanvasState {
    pub dragging: Option<usize>,
    pub last_fitted: Option<CanvasTransform>,
}

/// Defines the canvas configuration, positions and modes
pub struct BoundaryCanvas {
    pub points: Vec<Vec2>,
    pub headset_pos: Option<Vec2>,
    pub left_controller_pos: Option<Vec2>,
    pub right_controller_pos: Option<Vec2>,
    pub mode: CanvasMode,
}

impl canvas::Program<Message> for BoundaryCanvas {
    type State = BoundaryCanvasState;

    fn update(
        &self,
        state: &mut BoundaryCanvasState,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        if !matches!(self.mode, CanvasMode::FittedEditable) {
            return None;
        }

        let size = bounds.size();
        let pos = cursor.position_in(bounds)?;

        if state.dragging.is_none() {
            state.last_fitted = fit_transform(size, &self.points, edit_margin(&self.points), 48.0);
        }

        let to_canvas: Box<dyn Fn(Vec2) -> Point> =
            match (self.mode.clone(), state.last_fitted.as_ref()) {
                (CanvasMode::FittedEditable, Some(transform)) => {
                    Box::new(move |p| world_to_canvas(p, size, Some(transform)))
                }
                _ => Box::new(move |p| world_to_canvas(p, size, None)),
            };

        let fitted = if matches!(self.mode, CanvasMode::FittedEditable) {
            state.last_fitted.as_ref()
        } else {
            None
        };

        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // Check vertices first, then edge midpoints for insertion
                for (i, &v) in self.points.iter().enumerate() {
                    if pos.distance(to_canvas(v)) <= VERTEX_RADIUS + 2.0 {
                        state.dragging = Some(i);
                        return Some(Action::publish(Message::DragStart(i)));
                    }
                }
                let n = self.points.len();
                for i in 0..n {
                    let a = to_canvas(self.points[i]);
                    let b = to_canvas(self.points[(i + 1) % n]);
                    let mid = Point {
                        x: (a.x + b.x) / 2.0,
                        y: (a.y + b.y) / 2.0,
                    };
                    if pos.distance(mid) <= VERTEX_RADIUS + 2.0 {
                        let world = match fitted {
                            Some(transform) => canvas_to_world(mid, size, Some(transform)),
                            None => canvas_to_world(mid, size, None),
                        };
                        return Some(Action::publish(Message::InsertVertex(i, world)));
                    }
                }
                None
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                for (i, &v) in self.points.iter().enumerate() {
                    if pos.distance(to_canvas(v)) <= VERTEX_RADIUS + 2.0 {
                        return Some(Action::publish(Message::DeleteVertex(i)));
                    }
                }
                None
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.dragging.is_some() {
                    Some(Action::publish(Message::DragMove(
                        pos,
                        size,
                        fitted.copied(),
                    )))
                } else {
                    None
                }
            }
            // Release drag on button-up or if the cursor leaves the canvas mid-drag
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            | canvas::Event::Mouse(mouse::Event::CursorLeft) => {
                if state.dragging.take().is_some() {
                    Some(Action::publish(Message::DragEnd))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &BoundaryCanvasState,
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let size = bounds.size();
        let mut frame = Frame::new(renderer, size);

        match &self.mode {
            CanvasMode::Fixed { closed } => {
                // Use generic helpers with None transform for fixed mode
                let transform = None;
                draw_grid(&mut frame, size, transform);
                draw_polygon(&mut frame, &self.points, *closed, transform, size);
                draw_tracked_positions(&mut frame, self, transform, size);
            }
            CanvasMode::FittedEditable => {
                let transform = state.last_fitted.as_ref();
                draw_grid(&mut frame, size, transform);
                if let Some(canvas_transform) = transform {
                    draw_polygon(&mut frame, &self.points, true, Some(canvas_transform), size);
                    draw_tracked_positions(&mut frame, self, Some(canvas_transform), size);
                    draw_edge_midpoints(&mut frame, &self.points, canvas_transform);
                    draw_vertices(
                        &mut frame,
                        &self.points,
                        canvas_transform,
                        state,
                        cursor,
                        bounds,
                    );
                }
            }
            CanvasMode::Fitted { closed } => {
                let all_pts: Vec<Vec2> = self
                    .points
                    .iter()
                    .copied()
                    .chain(self.headset_pos)
                    .chain(self.left_controller_pos)
                    .chain(self.right_controller_pos)
                    .collect();

                if let Some(ref canvas_transform) = fit_transform(size, &all_pts, 0.0, 24.0) {
                    let transform = Some(canvas_transform);
                    draw_grid(&mut frame, size, transform);
                    draw_polygon(&mut frame, &self.points, *closed, transform, size);
                    draw_tracked_positions(&mut frame, self, transform, size);
                }
            }
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &BoundaryCanvasState,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if !matches!(self.mode, CanvasMode::FittedEditable) {
            return mouse::Interaction::default();
        }
        if state.dragging.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if let Some(pos) = cursor.position_in(bounds) {
            let size = bounds.size();
            for &v in &self.points {
                let cp = match &state.last_fitted {
                    Some(transform) if matches!(self.mode, CanvasMode::FittedEditable) => {
                        world_to_canvas(v, size, Some(transform))
                    }
                    _ => world_to_canvas(v, size, None),
                };
                if pos.distance(cp) <= VERTEX_RADIUS + 2.0 {
                    return mouse::Interaction::Grab;
                }
            }
        }
        mouse::Interaction::default()
    }
}

/// Encapsulates canvas transform parameters
#[derive(Debug, Copy, Clone)]
pub struct CanvasTransform {
    pub scale: f32,
    pub offset: Vec2,
}

/// Converts a world coordinate to a canvas position
pub fn world_to_canvas(p: Vec2, size: Size, transform: Option<&CanvasTransform>) -> Point {
    match transform {
        Some(CanvasTransform { scale, offset, .. }) => Point {
            x: size.width - (p.x * *scale + offset.x),
            y: -p.y * *scale + offset.y,
        },
        None => {
            let s = fixed_scale(size);
            Point {
                x: size.width / 2.0 - p.x * s,
                y: size.height / 2.0 - p.y * s,
            }
        }
    }
}

/// Converts a canvas position to a world coordinate
pub fn canvas_to_world(p: Point, size: Size, transform: Option<&CanvasTransform>) -> Vec2 {
    match transform {
        Some(CanvasTransform { scale, offset, .. }) => {
            Vec2::new((size.width - (p.x) - offset.x) / *scale, -(p.y - offset.y) / *scale)
        }
        None => {
            let s = fixed_scale(size);
            Vec2::new((size.width / 2.0 - p.x) / s, -(p.y - size.height / 2.0) / s)
        }
    }
}

/// Returns the correct scale based on the VIEW_HALF
fn fixed_scale(size: Size) -> f32 {
    size.width.min(size.height) / (VIEW_HALF * 2.0)
}

/// Returns a CanvasTransform that fits the points bounding box plus a world
/// space margin in meters.
fn fit_transform(
    size: Size,
    points: &[Vec2],
    margin_m: f32,
    margin_px: f32,
) -> Option<CanvasTransform> {
    if points.is_empty() {
        return None;
    }
    let min = points.iter().copied().reduce(Vec2::min)?;
    let max = points.iter().copied().reduce(Vec2::max)?;

    let min = min - Vec2::splat(margin_m);
    let max = max + Vec2::splat(margin_m);

    let width = (max.x - min.x).max(0.01);
    let height = (max.y - min.y).max(0.01);
    let avail_w = (size.width - 2.0 * margin_px).max(1.0);
    let avail_h = (size.height - 2.0 * margin_px).max(1.0);

    let scale = (avail_w / width).min(avail_h / height);
    let bcx = (min.x + max.x) / 2.0;
    let bcy = (min.y + max.y) / 2.0;
    let offset = Vec2::new(
        size.width / 2.0 - bcx * scale,
        size.height / 2.0 + bcy * scale,
    );
    Some(CanvasTransform { scale, offset })
}

/// Calculates a 15% additional margin (for use in edit mode) based on the points.
fn edit_margin(points: &[Vec2]) -> f32 {
    if points.is_empty() {
        return 1.0;
    }
    let min = points.iter().copied().reduce(Vec2::min).unwrap();
    let max = points.iter().copied().reduce(Vec2::max).unwrap();
    let largest_dim = (max.x - min.x).max(max.y - min.y);
    (largest_dim * 0.15).clamp(0.3, 2.0)
}

// Polygon area in meters squared
pub fn polygon_area(pts: &[Vec2]) -> f32 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let area: f32 = (0..n)
        .map(|i| {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            a.x * b.y - b.x * a.y
        })
        .sum();
    (area / 2.0).abs()
}

// Draws the background grid
fn draw_grid(frame: &mut Frame, size: Size, transform: Option<&CanvasTransform>) {
    frame.fill_rectangle(Point::ORIGIN, size, Color::from_rgb(0.08, 0.08, 0.10));

    let grid_col = Color::from_rgba(1.0, 1.0, 1.0, 0.07);
    let grid_size_m = GRID_SIZE_METERS;

    // Determine scale (pixels per meter)
    let scale = match transform {
        Some(CanvasTransform { scale, .. }) => *scale,
        None => fixed_scale(size),
    };
    let grid_spacing_px = grid_size_m * scale;
    if grid_spacing_px <= 0.0 {
        return;
    }

    // Draw vertical grid lines at exact multiples of grid_spacing_px
    let mut x = 0.0;
    while x <= size.width {
        frame.stroke(
            &Path::line(Point::new(x, 0.0), Point::new(x, size.height)),
            Stroke::default().with_color(grid_col).with_width(1.0),
        );
        x += grid_spacing_px;
    }

    // Draw horizontal grid lines at exact multiples of grid_spacing_px
    let mut y = 0.0;
    while y <= size.height {
        frame.stroke(
            &Path::line(Point::new(0.0, y), Point::new(size.width, y)),
            Stroke::default().with_color(grid_col).with_width(1.0),
        );
        y += grid_spacing_px;
    }
}

/// Shared polygon draw logic given pre-transformed canvas points.
fn draw_polygon_points(frame: &mut Frame, points: &[Point], closed: bool) {
    if closed && points.len() >= 3 {
        let poly = Path::new(|b| {
            b.move_to(points[0]);
            for &p in &points[1..] {
                b.line_to(p);
            }
            b.close();
        });
        frame.fill(&poly, COLOUR_FILL);
        frame.stroke(
            &poly,
            Stroke::default().with_color(COLOUR_EDGE).with_width(2.0),
        );
    } else {
        let chain = Path::new(|b| {
            b.move_to(points[0]);
            for &p in &points[1..] {
                b.line_to(p);
            }
        });
        frame.stroke(
            &chain,
            Stroke::default().with_color(COLOUR_EDGE).with_width(2.0),
        );
        for &p in points {
            frame.fill(&Path::circle(p, 3.5), COLOUR_EDGE);
        }
    }
}

/// Draws the polygon using the fixed world-area transform.
/// Unified polygon drawing helper for both fixed and transformed modes
fn draw_polygon(
    frame: &mut Frame,
    points: &[Vec2],
    closed: bool,
    transform: Option<&CanvasTransform>,
    size: Size,
) {
    if points.is_empty() {
        return;
    }
    let cpts: Vec<Point> = points
        .iter()
        .map(|&p| world_to_canvas(p, size, transform))
        .collect();
    draw_polygon_points(frame, &cpts, closed);
}

fn draw_edge_midpoints(frame: &mut Frame, points: &[Vec2], transform: &CanvasTransform) {
    let n = points.len();
    for i in 0..n {
        let a = world_to_canvas(
            points[i],
            Size {
                width: 0.0,
                height: 0.0,
            },
            Some(transform),
        );
        let b = world_to_canvas(
            points[(i + 1) % n],
            Size {
                width: 0.0,
                height: 0.0,
            },
            Some(transform),
        );
        let mid = Point {
            x: (a.x + b.x) / 2.0,
            y: (a.y + b.y) / 2.0,
        };
        frame.fill(
            &Path::circle(mid, 4.0),
            Color::from_rgba(0.0, 0.8, 0.4, 0.7),
        );
    }
}

fn draw_vertices(
    frame: &mut Frame,
    points: &[Vec2],
    transform: &CanvasTransform,
    state: &BoundaryCanvasState,
    cursor: mouse::Cursor,
    bounds: Rectangle,
) {
    for (i, &v) in points.iter().enumerate() {
        let cp = world_to_canvas(v, bounds.size(), Some(transform));
        let hovered = cursor
            .position_in(bounds)
            .map(|p| p.distance(cp) <= VERTEX_RADIUS + 2.0)
            .unwrap_or(false);
        let col = if Some(i) == state.dragging || hovered {
            Color::from_rgb(1.0, 1.0, 0.2)
        } else {
            COLOUR_EDGE
        };
        frame.fill(&Path::circle(cp, VERTEX_RADIUS), col);
    }
}

/// Unified tracked positions drawing helper for both fixed and transformed modes
fn draw_tracked_positions(
    frame: &mut Frame,
    canvas: &BoundaryCanvas,
    transform: Option<&CanvasTransform>,
    size: Size,
) {
    for pos in canvas
        .left_controller_pos
        .iter()
        .chain(canvas.right_controller_pos.iter())
    {
        frame.fill(
            &Path::circle(world_to_canvas(*pos, size, transform), 4.0),
            COLOUR_CONTROLLER,
        );
    }
    if let Some(h) = canvas.headset_pos {
        frame.fill(
            &Path::circle(world_to_canvas(h, size, transform), 4.0),
            COLOUR_HEADSET,
        );
    }
}
