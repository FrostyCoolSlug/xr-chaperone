use crate::app_state::AppState;
use crate::config;
use anyhow::Result;
use libmonado::{Monado, Pose, ReferenceSpaceType};
use mint::{Quaternion, Vector3};
use openxr_sys::{Posef, Quaternionf, Vector3f};
use parking_lot::Mutex;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{debug, info, warn};

impl From<config::Pose> for Pose {
    fn from(value: config::Pose) -> Self {
        Pose {
            position: Vector3 {
                x: value.position.x,
                y: value.position.y,
                z: value.position.z,
            },
            orientation: Quaternion {
                v: Vector3 {
                    x: value.orientation.vector.x,
                    y: value.orientation.vector.y,
                    z: value.orientation.vector.z,
                },
                s: value.orientation.scalar,
            },
        }
    }
}

impl From<Pose> for config::Pose {
    fn from(value: Pose) -> Self {
        config::Pose {
            position: config::Vector3 {
                x: value.position.x,
                y: value.position.y,
                z: value.position.z,
            },
            orientation: config::Quaternion {
                vector: config::Vector3 {
                    x: value.orientation.v.x,
                    y: value.orientation.v.y,
                    z: value.orientation.v.z,
                },
                scalar: value.orientation.s,
            },
        }
    }
}

// TODO: We can probably tidy this up
// Both the setters do the same thing with tiny differences, so we can probably handle this better

pub fn set_initial_offset(input: config::Pose) -> Result<()> {
    // For the initial pass, we don't do any bonus calculations, we just set it to whatever is defined.
    let monado = Monado::auto_connect().map_err(|e| anyhow::anyhow!("{}", e))?;
    let api_version = monado.get_api_version();
    debug!("Connected to Monado, Version: {}", api_version);

    // Convert the config pose into a Monado Pose
    let input = Pose::from(input.with_only_yaw());

    // We'll grab the first tracking origin, we'll work under the assumption there's only one
    // for now, this is probably bad, but we can handle it better later.
    let origin = monado
        .tracking_origins()?
        .into_iter()
        .next()
        .ok_or(anyhow::anyhow!(
            "No tracking origins found in Monado, cannot apply offset"
        ))?;
    info!("{:?}", origin.name);

    // Set the required offset
    origin.set_offset(input)?;

    // Return the merged offset
    Ok(())
}

pub fn set_adjusted_offset(input: config::Pose) -> Result<config::Pose> {
    // OK, lets take some steps here... Firstly, attempt to connect to monado
    let monado = Monado::auto_connect().map_err(|e| anyhow::anyhow!("{}", e))?;
    let api_version = monado.get_api_version();
    debug!("Connected to Monado, Version: {}", api_version);

    // Convert the config pose into a Monado Pose
    let input = Pose::from(input.with_only_yaw());

    // We'll grab the first tracking origin, we'll work under the assumption there's only one
    // for now, this is probably bad, but we can handle it better later.
    let origin = monado
        .tracking_origins()?
        .into_iter()
        .next()
        .ok_or(anyhow::anyhow!(
            "No tracking origins found in Monado, cannot apply offset"
        ))?;
    info!("{:?}", origin.name);

    let current = origin.get_offset()?;
    let merged = apply_offset(&input, &current);

    // Set the new offset
    origin.set_offset(merged)?;

    // Return the merged offset
    Ok(merged.into())
}

fn apply_offset(offset: &Pose, current: &Pose) -> Pose {
    let input = invert_pose(offset);
    let orientation = multiply_quaternions(&current.orientation, &input.orientation);

    // Don't rotate the position, just combine directly
    let position = Vector3 {
        x: current.position.x + input.position.x,
        y: current.position.y + input.position.y,
        z: current.position.z + input.position.z,
    };

    Pose {
        position,
        orientation,
    }
}

fn invert_pose(pose: &Pose) -> Pose {
    let q_inv = Quaternion {
        v: Vector3 {
            x: -pose.orientation.v.x,
            y: -pose.orientation.v.y,
            z: -pose.orientation.v.z,
        },
        s: pose.orientation.s,
    };

    // Rotate the negated position by the inverse quaternion
    let neg_pos = Vector3 {
        x: -pose.position.x,
        y: -pose.position.y,
        z: -pose.position.z,
    };
    let inv_pos = rotate_vector_by_quaternion(&q_inv, &neg_pos);

    Pose {
        position: inv_pos,
        orientation: q_inv,
    }
}

// This code is fucking stupid, but Quaternion's children are 'v' and 's', with Vector3's being
// x,y,z, so enjoy how fucking terrible this reads
// Also, Fuck maths.
fn rotate_vector_by_quaternion(q: &Quaternion<f32>, v: &Vector3<f32>) -> Vector3<f32> {
    // Joining the terrible naming chain :D
    let tx = 2.0 * (q.v.y * v.z - q.v.z * v.y);
    let ty = 2.0 * (q.v.z * v.x - q.v.x * v.z);
    let tz = 2.0 * (q.v.x * v.y - q.v.y * v.x);

    Vector3 {
        x: v.x + q.s * tx + (q.v.y * tz - q.v.z * ty),
        y: v.y + q.s * ty + (q.v.z * tx - q.v.x * tz),
        z: v.z + q.s * tz + (q.v.x * ty - q.v.y * tx),
    }
}

fn multiply_quaternions(a: &Quaternion<f32>, b: &Quaternion<f32>) -> Quaternion<f32> {
    Quaternion {
        v: Vector3 {
            x: a.s * b.v.x + a.v.x * b.s + a.v.y * b.v.z - a.v.z * b.v.y,
            y: a.s * b.v.y - a.v.x * b.v.z + a.v.y * b.s + a.v.z * b.v.x,
            z: a.s * b.v.z + a.v.x * b.v.y - a.v.y * b.v.x + a.v.z * b.s,
        },
        s: a.s * b.s - a.v.x * b.v.x - a.v.y * b.v.y - a.v.z * b.v.z,
    }
}

////////// Stage Offset Management

/// Simple thread which monitors for changes to stage offset
pub fn monitor_stage_reference_offset(state: Arc<Mutex<AppState>>) {
    let monado = match Monado::auto_connect() {
        Ok(m) => m,
        Err(e) => {
            warn!("Stage offset monitor: could not connect to Monado: {e}");
            return;
        }
    };

    // Should be connected and running, flag us as active
    info!("Stage offset monitor connected to Monado");
    state.lock().monado_available = true;

    // libmonado::Pose doesn't implement PartialEq, so we'll do it here.
    let poses_differ = |a: &Pose, b: &Pose| -> bool {
        a.position.x != b.position.x
            || a.position.y != b.position.y
            || a.position.z != b.position.z
            || a.orientation.v.x != b.orientation.v.x
            || a.orientation.v.y != b.orientation.v.y
            || a.orientation.v.z != b.orientation.v.z
            || a.orientation.s != b.orientation.s
    };

    let mut last_pose: Option<Pose> = None;
    loop {
        // If anything is calling for an exit, stop the thread.
        if state.lock().ui_exit_requested || state.lock().xr_exit_requested {
            break;
        }

        // Fetch the offset for the stage
        match monado.get_reference_space_offset(ReferenceSpaceType::Stage) {
            Ok(pose) => {
                // If the last pose is None, or it doesn't match the new pose, flag changed.
                let changed = last_pose
                    .as_ref()
                    .map(|prev| poses_differ(prev, &pose))
                    .unwrap_or(true); // first read always writes

                // If we've changed, set the new ref in the state.
                if changed {
                    debug!(
                        "Stage reference space offset changed: pos=({:.4}, {:.4}, {:.4})",
                        pose.position.x, pose.position.y, pose.position.z
                    );
                    state.lock().stage_reference_offset = Some(pose_to_posef(&pose));
                    last_pose = Some(pose);
                }
            }
            Err(e) => {
                warn!("Stage offset monitor: get_reference_space_offset failed: {e:?}");
            }
        }

        // We don't need to be too heavy on this, 100ms should be fine
        thread::sleep(Duration::from_millis(100));
    }

    // Loop has ended, this thread is about to die, flag us as unavailable
    state.lock().monado_available = false;
    info!("Stage offset monitor exiting.");
}

/// Converts a monado pose to an OpenXR posef
fn pose_to_posef(pose: &Pose) -> Posef {
    Posef {
        position: Vector3f {
            x: pose.position.x,
            y: pose.position.y,
            z: pose.position.z,
        },
        orientation: Quaternionf {
            x: pose.orientation.v.x,
            y: pose.orientation.v.y,
            z: pose.orientation.v.z,
            w: pose.orientation.s,
        },
    }
}
