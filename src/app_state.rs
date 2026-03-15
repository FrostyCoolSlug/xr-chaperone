use crate::config::Config;
use glam::Vec2;
use openxr_sys::Posef;
use parking_lot::Mutex;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Time for a double click
const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(500);

/// Distance to auto-close the space (in cm)
const CLOSE_DISTANCE: f32 = 0.15;

/// The running state of the XR Thread
#[derive(Debug, Clone, PartialEq)]
pub enum XRState {
    Starting,
    Error(String),
    Running,
}

/// The overall lifecycle of the application.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// We don't have a poly to draw yet
    Unconfigured,

    /// User is actively Drawing the play boundaries
    Drawing,

    /// User is reviewing and adjusting the boundries
    Review,

    /// The XR thread is actively rendering the chaperone.
    Active,
}

/// All mutable state shared between threads.
#[derive(Debug)]
pub struct AppState {
    /// The state of the XR Thread
    pub xr_state: XRState,

    /// The configuration phase we're currently in
    pub phase: Phase,

    /// Points captured during the current (or last) trace.
    pub trace_points: Vec<Vec2>,

    /// The confirmed polygon used by the chaperone renderer.
    pub polygon: Vec<Vec2>,

    /// Set by the XR thread when the runtime requests exit.
    pub xr_exit_requested: bool,

    /// Set by the UI when the user clicks the window close button.
    pub ui_exit_requested: bool,

    /// Used to send config updates from the UI to the XR thread
    pub pending_config: Option<Config>,

    /// Time of the last trigger pull during tracing (to detect double-clicks)
    pub last_trigger_time: Option<Instant>,

    /// Real-time headset position
    pub headset_pos: Option<Posef>,

    /// Real-time left controller position
    pub left_controller_pos: Option<Posef>,

    /// Real-time right controller position
    pub right_controller_pos: Option<Posef>,

    /// Set to true when the monado thread is running
    pub monado_available: bool,

    /// The current stage offset
    pub stage_reference_offset: Option<Posef>,

    /// Set when the monado code finds an offset change, handled by xr_thread
    pub stage_reference_offset_change: Option<Posef>,

    /// When we recalibrate, the stage offset needs to be reset, this bool is true while that
    /// reset is occurring, the UI thread sets it to true, the xr_thread sets it to false
    pub stage_reset_await: bool,
}

impl AppState {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            xr_state: XRState::Starting,
            phase: Phase::Unconfigured,
            trace_points: Vec::new(),
            polygon: Vec::new(),
            xr_exit_requested: false,
            ui_exit_requested: false,
            pending_config: None,
            last_trigger_time: None,
            headset_pos: None,
            left_controller_pos: None,
            right_controller_pos: None,
            monado_available: false,
            stage_reference_offset: None,
            stage_reference_offset_change: None,
            stage_reset_await: false,
        }))
    }

    /// Called by the XR thread to append a tracing point.
    pub fn push_trace_point(&mut self, p: Vec2) {
        let now = Instant::now();

        let is_double_click = self
            .last_trigger_time
            .is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK_TIME);

        let is_close_to_start = self
            .trace_points
            .first()
            .is_some_and(|start| p.distance(*start) < CLOSE_DISTANCE);

        // If double click or close to start, finish tracing WITHOUT adding the new point
        if (is_double_click || is_close_to_start)
            && self.trace_points.len() >= 3
            && self.phase == Phase::Drawing
        {
            self.polygon = self.trace_points.clone();
            self.phase = Phase::Review;
            self.last_trigger_time = Some(now);
            return;
        }

        // Otherwise, add the new point as usual
        self.trace_points.push(p);
        self.last_trigger_time = Some(now);
    }

    /// Called by the UI to confirm the current trace_points as the active polygon.
    pub fn confirm_polygon(&mut self) {
        if self.trace_points.len() >= 3 {
            self.polygon = self.trace_points.clone();
            self.phase = Phase::Active;
        }
    }

    /// Discard the current trace and go back to the setup screen.
    pub fn reset_trace(&mut self) {
        self.trace_points.clear();
        self.phase = Phase::Unconfigured;
    }
}
