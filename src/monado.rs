use crate::config;
use anyhow::Result;
use libmonado::{Monado, Pose};
use mint::{Quaternion, Vector3};
use tracing::{debug, info};

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

pub fn set_offset(input: config::Pose) -> Result<config::Pose> {
    // OK, lets take some steps here... Firstly, attempt to connect to monado
    let monado = Monado::auto_connect().map_err(|e| anyhow::anyhow!("{}", e))?;
    let api_version = monado.get_api_version();
    debug!("Connected to Monado, Version: {}", api_version);

    // Convert the config pose into a Monado Pose
    let input = Pose::from(input);

    // Invert the input, as we need to apply this to the existing offset
    let input = invert_pose(&input);

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

    // Set the required offset
    origin.set_offset(merged)?;

    // Return the merged offset
    Ok(merged.into())
}

fn apply_offset(offset: &Pose, input: &Pose) -> Pose {
    // Combined rotation: offset.orientation * input.orientation
    let orientation = multiply_quaternions(&offset.orientation, &input.orientation);

    // Combined position: offset.position + rotate(offset.orientation, input.position)
    let rotated = rotate_vector_by_quaternion(&offset.orientation, &input.position);
    let position = Vector3 {
        x: offset.position.x + rotated.x,
        y: offset.position.y + rotated.y,
        z: offset.position.z + rotated.z,
    };

    Pose {
        position,
        orientation,
    }
}

fn invert_pose(pose: &Pose) -> Pose {
    // Conjugate of quaternion (assumes normalized)
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
