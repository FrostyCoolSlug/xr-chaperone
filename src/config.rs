use anyhow::{Context, Result};
use glam::Vec2;
use openxr_sys::Posef;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::warn;

/// A point on the boundry (we don't need Y, as it represents up
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryPoint {
    pub x: f32,
    pub z: f32,
}

impl BoundaryPoint {
    pub fn to_vec2(&self) -> Vec2 {
        Vec2::new(self.x, self.z)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pose {
    pub position: Vector3<f32>,
    pub orientation: Quaternion<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quaternion<T> {
    /// Vector part of a quaternion.
    pub vector: Vector3<T>,
    /// Scalar part of a quaternion.
    pub scalar: T,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Vector3<T> {
    pub(crate) x: T,
    pub(crate) y: T,
    pub(crate) z: T,
}

impl From<Posef> for Pose {
    fn from(value: Posef) -> Self {
        Pose {
            position: Vector3 {
                x: value.position.x,
                y: value.position.y,
                z: value.position.z,
            },
            orientation: Quaternion {
                vector: Vector3 {
                    x: value.orientation.x,
                    y: value.orientation.y,
                    z: value.orientation.z,
                },
                scalar: value.orientation.w,
            },
        }
    }
}

/// Top-level config file structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Distance (metres) at which the grid starts becoming visible.
    #[serde(default = "default_fade_start")]
    pub fade_start: f32,

    /// Distance (metres) at which the grid is fully opaque.
    #[serde(default = "default_fade_end")]
    pub fade_end: f32,

    /// Height of the rendered grid walls (metres).
    #[serde(default = "default_wall_height")]
    pub wall_height: f32,

    /// Size of each grid square in metres.
    #[serde(default = "default_grid_spacing")]
    pub grid_spacing: f32,

    /// Thickness of grid lines in metres.
    #[serde(default = "default_line_width")]
    pub line_width: f32,

    /// RGBA colour of the grid at full opacity.
    #[serde(default = "default_grid_colour")]
    pub grid_colour: [f32; 4],

    /// The play area boundary points, must have at least 3 to render
    #[serde(default)]
    pub boundary: Vec<BoundaryPoint>,

    #[serde(default)]
    pub headset_offset: Option<Pose>,
}

fn default_fade_start() -> f32 {
    0.75
}
fn default_fade_end() -> f32 {
    0.0
}
fn default_wall_height() -> f32 {
    2.5
}
fn default_grid_spacing() -> f32 {
    0.4
}
fn default_line_width() -> f32 {
    0.01
}
fn default_grid_colour() -> [f32; 4] {
    [0.0, 0.6, 1.0, 1.0]
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("reading {:?}", path.as_ref()))?;
        let mut cfg: Self = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Returns the polygon as a list of Vec2 (X/Z)
    pub fn polygon(&self) -> Vec<Vec2> {
        if self.boundary.len() >= 3 {
            self.boundary.iter().map(|p| p.to_vec2()).collect()
        } else {
            // We need a basic mesh handled by the XR to prevent crashing, so pass in some
            // default values, this will never actually be rendered.
            vec![
                Vec2::new(-1.5, -1.5),
                Vec2::new(1.5, -1.5),
                Vec2::new(1.5, 1.5),
                Vec2::new(-1.5, 1.5),
            ]
        }
    }

    fn validate(&mut self) -> Result<()> {
        if !self.boundary.len() > 0 && self.boundary.len() < 3 {
            warn!("Boundary must have at least 3 points, resetting.");
            self.boundary.clear();
        }
        if self.fade_end > self.fade_start {
            warn!("Fade End greater than Fade Start, fixing..");
            self.fade_end = self.fade_start;
        }
        Ok(())
    }
}
impl Default for Config {
    fn default() -> Self {
        Self {
            boundary: Vec::new(),
            fade_start: default_fade_start(),
            fade_end: default_fade_end(),
            wall_height: default_wall_height(),
            grid_spacing: default_grid_spacing(),
            line_width: default_line_width(),
            grid_colour: default_grid_colour(),
            headset_offset: None,
        }
    }
}
