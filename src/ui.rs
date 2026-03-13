use crate::app_state::{AppState, Phase};
use crate::config::Config;
use crate::ui_canvas::{
    BoundaryCanvas, CanvasMode, CanvasTransform, canvas_to_world, polygon_area,
};
use glam::Vec2;
use iced::widget::Space;
use iced::{
    Color, Element, Length, Point, Size, Subscription, Task, Theme,
    widget::{button, canvas, column, container, row, scrollable, slider, text},
    window,
};
use iced_color_wheel::{WheelProgram, color_to_hsv, hsv_to_color};
use openxr_sys::Posef;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

// Which screen we're supposed to be displaying
#[derive(Debug, Clone, PartialEq)]
enum Screen {
    Main,
    Settings,
}

// UI messaging for state update
#[derive(Debug, Clone)]
pub enum Message {
    Tick,

    // Navigation
    OpenSettings,
    CloseSettings,

    // Settings sliders
    SetFadeStart(f32),
    SetFadeEnd(f32),
    SetWallHeight(f32),
    SetGridSpacing(f32),
    SetLineWidth(f32),
    SubmitColour(Color),

    // Save
    SaveSettings,
    SaveDone(Result<(), String>),

    // Boundary workflow
    StartTracing,
    StopTracing,
    ConfirmPolygon,
    ReTrace,
    CancelEdit,
    EditBoundary,
    Reconfigure,

    // Vertex editing (review screen)
    DragStart(usize),
    DragMove(Point, Size, Option<CanvasTransform>),
    DragEnd,
    InsertVertex(usize, Vec2),
    DeleteVertex(usize),
}

// In-progress edits for the settings screen. Created when the screen opens, dropped on close.
#[derive(Debug, Clone)]
struct SettingsEdit {
    fade_start: f32,
    fade_end: f32,
    wall_height: f32,
    grid_spacing: f32,
    line_width: f32,
    colour: Color,
}

impl SettingsEdit {
    fn from_cfg(cfg: &Config) -> Self {
        Self {
            fade_start: cfg.fade_start,
            fade_end: cfg.fade_end,
            wall_height: cfg.wall_height,
            grid_spacing: cfg.grid_spacing,
            line_width: cfg.line_width,
            colour: Color::from_rgba(
                cfg.grid_colour[0],
                cfg.grid_colour[1],
                cfg.grid_colour[2],
                cfg.grid_colour[3],
            ),
        }
    }

    fn to_cfg(&self, base: &Config) -> Config {
        Config {
            fade_start: self.fade_start,
            fade_end: self.fade_end,
            wall_height: self.wall_height,
            grid_spacing: self.grid_spacing,
            line_width: self.line_width,
            grid_colour: [self.colour.r, self.colour.g, self.colour.b, self.colour.a],
            boundary: base.boundary.clone(),
        }
    }
}

// State of the UI (duh)
pub struct UiState {
    pub shared: Arc<Mutex<AppState>>,
    pub cfg_path: PathBuf,
    pub cfg: Config,

    // Current phase and display points, refreshed each Tick
    pub phase: Phase,
    pub points: Vec<Vec2>,

    // Real-time tracked positions, refreshed each Tick
    pub headset_pos: Option<Posef>,
    pub left_controller_pos: Option<Posef>,
    pub right_controller_pos: Option<Posef>,

    // UI-only state
    screen: Screen,
    drag_vertex: Option<usize>,
    prev_edit: Option<Config>,

    // Only populated while the settings screen is open
    settings: Option<SettingsEdit>,
}

pub fn run(shared: Arc<Mutex<AppState>>, cfg: Config, cfg_path: PathBuf) -> iced::Result {
    iced::application(
        move || {
            let shared = shared.clone();
            let cfg = cfg.clone();
            let cfg_path = cfg_path.clone();

            let phase = shared.lock().phase.clone();

            let state = UiState {
                phase,
                shared,
                cfg,
                cfg_path,
                points: Vec::new(),
                headset_pos: None,
                left_controller_pos: None,
                right_controller_pos: None,
                screen: Screen::Main,
                drag_vertex: None,
                prev_edit: None,
                settings: None,
            };
            (state, Task::none())
        },
        update,
        view,
    )
    .title("XR Chaperone")
    .theme(|_: &UiState| -> Theme { Theme::Dark })
    .window_size((680, 800))
    .subscription(subscription)
    .run()
}

// A subscription that'll tick every 33ms (roughly 30fps) which will update tracked positions
// and other XR state changes
fn subscription(_: &UiState) -> Subscription<Message> {
    iced::time::every(Duration::from_millis(33)).map(|_| Message::Tick)
}

fn update(state: &mut UiState, msg: Message) -> Task<Message> {
    match msg {
        Message::Tick => {
            let s = state.shared.lock();
            if s.xr_exit_requested {
                // XR is dead, exit gracefully.
                return iced::exit();
            }

            state.phase = s.phase.clone();
            state.headset_pos = s.headset_pos;
            state.left_controller_pos = s.left_controller_pos;
            state.right_controller_pos = s.right_controller_pos;
            state.points = match state.phase {
                Phase::Active => s.polygon.clone(),
                _ => s.trace_points.clone(),
            };
        }

        Message::OpenSettings => {
            state.settings = Some(SettingsEdit::from_cfg(&state.cfg));
            state.screen = Screen::Settings;
        }
        Message::CloseSettings => {
            state.settings = None;
            state.screen = Screen::Main;
        }

        // We correctly clamp these values, but we should probably adjust the slider directly
        Message::SetFadeStart(v) => {
            if let Some(s) = &mut state.settings {
                s.fade_start = v.clamp(s.fade_end, 2.0);
            }
        }
        Message::SetFadeEnd(v) => {
            if let Some(s) = &mut state.settings {
                s.fade_end = v.min(s.fade_start);
            }
        }
        Message::SetWallHeight(v) => {
            if let Some(s) = &mut state.settings {
                s.wall_height = v;
            }
        }
        Message::SetGridSpacing(v) => {
            if let Some(s) = &mut state.settings {
                s.grid_spacing = v;
            }
        }
        Message::SetLineWidth(v) => {
            if let Some(s) = &mut state.settings {
                s.line_width = v;
            }
        }
        Message::SubmitColour(c) => {
            if let Some(s) = &mut state.settings {
                s.colour = c;
            }
        }

        Message::SaveSettings => {
            if let Some(edit) = &state.settings {
                let new_cfg = edit.to_cfg(&state.cfg);
                state.cfg = new_cfg.clone();
                state.shared.lock().pending_config = Some(new_cfg.clone());
                state.settings = None;
                state.screen = Screen::Main;
                let path = state.cfg_path.clone();
                return Task::perform(
                    async move { save_config_full(&path, &new_cfg).await },
                    Message::SaveDone,
                );
            }
        }

        // Fun stuff below, the actual tracing / rendering loop
        Message::StartTracing => {
            state.prev_edit = if state.cfg.boundary.len() >= 3 {
                Some(state.cfg.clone())
            } else {
                None
            };
            let mut s = state.shared.lock();
            s.trace_points.clear();
            s.phase = Phase::Drawing;
        }

        // Stop capturing and move to the review screen if enough points exist.
        Message::StopTracing => {
            let mut s = state.shared.lock();
            if s.trace_points.len() >= 3 {
                s.polygon = s.trace_points.clone();
                s.phase = Phase::Review;
            }
        }

        // Discard captured points and return to tracing.
        Message::ReTrace => {
            let mut s = state.shared.lock();
            s.trace_points.clear();
            s.phase = Phase::Drawing;
            state.phase = Phase::Drawing;
        }

        // Confirm the reviewed boundary, save to disk, and go active.
        Message::ConfirmPolygon => {
            let points = {
                let mut s = state.shared.lock();
                s.confirm_polygon();
                s.polygon.clone()
            };

            // Somehow we got less than 3 points, this shouldn't happen? :D
            if points.len() < 3 {
                return Task::none();
            }
            state.points = points.clone();
            state.cfg.boundary = points
                .iter()
                .map(|p| crate::config::BoundaryPoint { x: p.x, z: p.y })
                .collect();
            state.prev_edit = None;
            let cfg = state.cfg.clone();
            let path = state.cfg_path.clone();
            return Task::perform(
                async move { save_config_full(&path, &cfg).await },
                Message::SaveDone,
            );
        }

        // Cancel an in-progress edit, restoring the previous boundary if one existed.
        Message::CancelEdit => {
            if let Some(ref previous) = state.prev_edit {
                let mut s = state.shared.lock();
                s.polygon = previous
                    .boundary
                    .iter()
                    .map(|p| Vec2::new(p.x, p.z))
                    .collect();
                s.phase = Phase::Active;

                state.cfg = previous.clone();
                state.phase = Phase::Active;
            } else {
                let mut s = state.shared.lock();
                s.phase = Phase::Active;
                state.phase = Phase::Active;
            }
        }

        // Enter boundary edit mode from the active screen.
        Message::EditBoundary => {
            state.prev_edit = if state.cfg.boundary.len() >= 3 {
                Some(state.cfg.clone())
            } else {
                None
            };
            let mut s = state.shared.lock();
            s.trace_points = s.polygon.clone();
            s.phase = Phase::Review;
        }

        Message::DragStart(idx) => {
            state.drag_vertex = Some(idx);
        }
        Message::DragMove(pt, size, fitted) => {
            if let Some(idx) = state.drag_vertex {
                let world = match fitted {
                    Some(transform) => canvas_to_world(pt, size, Some(&transform)),
                    None => canvas_to_world(pt, size, None),
                };
                let mut s = state.shared.lock();
                if idx < s.trace_points.len() {
                    s.trace_points[idx] = world;
                }
            }
        }
        Message::DragEnd => {
            state.drag_vertex = None;
        }
        Message::InsertVertex(after, pos) => {
            let mut s = state.shared.lock();
            let at = (after + 1).min(s.trace_points.len());
            s.trace_points.insert(at, pos);
        }
        Message::DeleteVertex(idx) => {
            let mut s = state.shared.lock();
            if s.trace_points.len() > 3 {
                s.trace_points.remove(idx);
            }
        }

        // Full reset, clears all trace data and returns to setup
        Message::Reconfigure => {
            state.prev_edit = None;
            state.shared.lock().reset_trace();
        }

        Message::SaveDone(Ok(())) => {
            info!("Config saved.");
        }
        Message::SaveDone(Err(e)) => {
            error!("Save failed: {e}");
        }
    }
    Task::none()
}

// Which function to call depending on state
fn view(state: &UiState) -> Element<'_, Message> {
    if state.screen == Screen::Settings {
        return view_settings(state);
    }
    match state.phase {
        Phase::Unconfigured => view_setup(state),
        Phase::Drawing => view_tracing(state),
        Phase::Review => view_review(state),
        Phase::Active => view_active(state),
    }
}

fn settings_btn<'a>() -> Element<'a, Message> {
    button("Settings")
        .on_press(Message::OpenSettings)
        .padding([6, 12])
        .into()
}

fn cancel_btn<'a>() -> Element<'a, Message> {
    button("Cancel")
        .on_press(Message::CancelEdit)
        .padding(10)
        .into()
}

fn boundary_canvas(state: &UiState, mode: CanvasMode) -> Element<'_, Message> {
    canvas(BoundaryCanvas {
        points: state.points.clone(),
        headset_pos: state.headset_pos,
        left_controller_pos: state.left_controller_pos,
        right_controller_pos: state.right_controller_pos,
        mode,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn slider_row<'a>(
    label: &'a str,
    unit: &'a str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    on_change: impl Fn(f32) -> Message + 'a,
    fmt: impl Fn(f32) -> String,
) -> Element<'a, Message> {
    row![
        text(label).width(Length::Fixed(170.0)).size(14),
        slider(range, value, on_change)
            .step(step)
            .width(Length::Fixed(240.0)),
        text(fmt(value)).width(Length::Fixed(60.0)).size(13),
        text(unit).size(13),
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}

fn view_settings(state: &UiState) -> Element<'_, Message> {
    let Some(edit) = &state.settings else {
        return view(state);
    };

    let (hue, saturation, _) = color_to_hsv(edit.colour);

    let content = column![
        text("Settings").size(24),
        Space::new().height(12),
        text("Proximity").size(16),
        slider_row(
            "Fade start",
            "",
            edit.fade_start,
            edit.fade_end..=2.0,
            0.05,
            Message::SetFadeStart,
            |v| format!("{v:.2}m")
        ),
        slider_row(
            "Fade end",
            "",
            edit.fade_end.min(edit.fade_start),
            0.05..=edit.fade_start,
            0.05,
            Message::SetFadeEnd,
            |v| format!("{v:.2}m")
        ),
        text("Fade start is when the grid first appears, Fade End is maximum intensity").size(12),
        Space::new().height(12),
        text("Appearance").size(16),
        slider_row(
            "Wall height",
            "",
            edit.wall_height,
            1.0..=4.0,
            0.1,
            Message::SetWallHeight,
            |v| format!("{v:.1}m")
        ),
        slider_row(
            "Grid spacing",
            "",
            edit.grid_spacing,
            0.1..=1.0,
            0.01,
            Message::SetGridSpacing,
            |v| format!("{v:.2}m")
        ),
        slider_row(
            "Line width",
            "",
            edit.line_width,
            0.001..=edit.grid_spacing,
            0.001,
            Message::SetLineWidth,
            |v| format!("{v:.3}m")
        ),
        Space::new().height(12),
        text("Grid Colour").size(16),
        container(
            canvas(WheelProgram::new(hue, saturation, 1., |h, s| {
                Message::SubmitColour(hsv_to_color(h, s, 1.0))
            }))
            .width(200)
            .height(200)
        )
        .align_x(iced::Center)
        .width(Length::Fill),
        Space::new().height(12),
        container(
            row![
                button("Save & Apply")
                    .on_press(Message::SaveSettings)
                    .padding([8, 20]),
                button("Cancel")
                    .on_press(Message::CloseSettings)
                    .padding([8, 20]),
            ]
            .spacing(10)
        )
        .align_x(iced::Center)
        .width(Length::Fill)
    ]
    .spacing(8)
    .padding(32)
    .max_width(560);

    container(scrollable(content))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn view_setup(state: &UiState) -> Element<'_, Message> {
    let mut action_row = row![
        button("Start Tracing")
            .on_press(Message::StartTracing)
            .padding(10)
    ];
    if state.cfg.boundary.len() >= 3 {
        action_row = action_row.push(cancel_btn());
    }

    container(
        column![
            text("XR Chaperone Setup").size(28),
            text("").size(8),
            text("1. Place your headset in the middle of your play area").size(16),
            text("2. Click Start Tracing below.").size(16),
            text("3. Walk around the play area, pulling the trigger on corners").size(16),
            text("4. Click Done when you have traced the full boundary").size(16),
            text("").size(12),
            action_row.spacing(16),
        ]
        .spacing(6)
        .padding(40)
        .align_x(iced::Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center(Length::Fill)
    .into()
}

fn view_tracing(state: &UiState) -> Element<'_, Message> {
    let n = state.points.len();

    let done_btn = if n >= 3 {
        button("Done – Review Boundary")
            .on_press(Message::StopTracing)
            .padding(10)
    } else {
        button("Done – Review Boundary").padding(10)
    };

    let mut action_row = row![done_btn];
    if state.prev_edit.is_some() {
        action_row = action_row.push(cancel_btn());
    }

    // Fixed scale: always shows VIEW_HALF*2 metres regardless of canvas size.
    // A dynamic zoom would be disorienting as positions move constantly.
    column![
        text("Tracing Boundary").size(22),
        boundary_canvas(state, CanvasMode::Fixed { closed: false }),
        text(format!(
            "Walk to each corner and pull the trigger  •  {n} points captured"
        ))
        .size(14),
        action_row.spacing(16),
    ]
    .spacing(12)
    .padding(20)
    .align_x(iced::Center)
    .into()
}

fn view_review(state: &UiState) -> Element<'_, Message> {
    let mut action_row = row![
        button("Re-trace").on_press(Message::ReTrace).padding(10),
        button("Confirm & Save")
            .on_press(Message::ConfirmPolygon)
            .padding(10),
    ];
    if state.prev_edit.is_some() {
        action_row = action_row.push(cancel_btn());
    }

    column![
        text("Review Boundary").size(22),
        boundary_canvas(state, CanvasMode::FittedEditable),
        text("Drag vertices  •  Click edge midpoint to add  •  Right-click to remove").size(12),
        action_row.spacing(16),
    ]
    .spacing(12)
    .padding(20)
    .align_x(iced::Center)
    .into()
}

fn view_active(state: &UiState) -> Element<'_, Message> {
    let area = polygon_area(&state.points);

    let header = row![
        text("Chaperone Active").size(22),
        Space::new().width(Length::Fill),
        settings_btn(),
    ]
    .align_y(iced::Center);

    let footer = column![
        text(format!(
            "{} vertices  •  {:.1} m²",
            state.points.len(),
            area
        ))
        .size(14),
        row![
            button("Edit Boundary")
                .on_press(Message::EditBoundary)
                .padding(10),
            button("Re-configure")
                .on_press(Message::Reconfigure)
                .padding(10),
        ]
        .spacing(16),
    ]
    .spacing(8)
    .align_x(iced::Center)
    .width(Length::Fill);

    container(
        column![
            header,
            // Fitted: auto-zooms to boundary content.
            boundary_canvas(state, CanvasMode::Fitted { closed: true }),
            footer,
        ]
        .spacing(12)
        .height(Length::Fill),
    )
    .padding(20)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

async fn save_config_full(path: &PathBuf, cfg: &Config) -> Result<(), String> {
    let toml = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(path, toml).map_err(|e| e.to_string())
}

// ----- Tiny error box
pub(crate) fn error(error: String) -> iced::Result {
    type Msg = ();

    fn error_view(state: &String) -> Element<'_, Msg> {
        container(
            column![
                text(format!("Error: {}", state)).size(12),
                row![
                    Space::new().width(Length::Fill),
                    button(text("Ok"))
                        .on_press(())
                        .padding([2, 12])
                        .width(Length::Fixed(100.))
                ]
            ]
            .spacing(20),
        )
        .width(Length::Fill)
        .padding(20)
        .into()
    }

    iced::application(
        move || (error.clone(), Task::none()),
        |_state: &mut String, _msg: Msg| iced::exit::<Msg>(),
        error_view,
    )
    .title("XR Chaperone - Error")
    .window(window::Settings {
        size: Size::new(500., 100.),
        resizable: false,
        ..Default::default()
    })
    .theme(|_: &String| Theme::Dark)
    .run()
}
