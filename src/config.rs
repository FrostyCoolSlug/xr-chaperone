use anyhow::{Context, Result};
use glam::Vec2;
use openxr_sys::Posef;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::warn;

/// A point on the boundary (we don't need Y, as it represents up
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

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Vector3<T> {
    pub(crate) x: T,
    pub(crate) y: T,
    pub(crate) z: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quaternion<T> {
    /// Vector part of a quaternion.
    pub vector: Vector3<T>,
    /// Scalar part of a quaternion.
    pub scalar: T,
}

impl Quaternion<f32> {
    /// Returns this Quaternion as just a Yaw value
    pub fn yaw_only(&self) -> Quaternion<f32> {
        Quaternion::from_yaw(self.to_yaw())
    }

    /// Extracts the yaw angle (rotation around Y axis) in radians.
    pub fn to_yaw(&self) -> f32 {
        let w = self.scalar;
        let x = self.vector.x;
        let y = self.vector.y;
        let z = self.vector.z;

        f32::atan2(2.0 * (w * y + x * z), 1.0 - 2.0 * (y * y + x * x))
    }

    /// Creates a yaw-only orientation from an angle in radians.
    pub fn from_yaw(yaw: f32) -> Self {
        let half = yaw / 2.0;
        Quaternion {
            scalar: half.cos(),
            vector: Vector3 {
                x: 0.0,
                y: half.sin(),
                z: 0.0,
            },
        }
    }

    /// Applies a yaw rotation on top of an existing orientation.
    pub fn with_yaw(self, yaw: f32) -> Self {
        let yaw_quat = Quaternion::from_yaw(yaw);
        yaw_quat.mul(&self)
    }

    /// Quaternion multiplication (Hamilton product).
    pub fn mul(&self, rhs: &Quaternion<f32>) -> Quaternion<f32> {
        let (w1, x1, y1, z1) = (self.scalar, self.vector.x, self.vector.y, self.vector.z);
        let (w2, x2, y2, z2) = (rhs.scalar, rhs.vector.x, rhs.vector.y, rhs.vector.z);

        Quaternion {
            scalar: w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
            vector: Vector3 {
                x: w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
                y: w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
                z: w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
            },
        }
    }
}

impl Default for Quaternion<f32> {
    fn default() -> Self {
        Self {
            vector: Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            scalar: 1.0,
        }
    }
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

impl Pose {
    /// Provides the pose removing all orientation data except yaw
    pub fn with_only_yaw(self) -> Self {
        Pose {
            position: self.position,
            orientation: self.orientation.yaw_only(),
        }
    }

    pub fn with_default_orientation(self) -> Self {
        Pose {
            position: self.position,
            orientation: Default::default(),
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
