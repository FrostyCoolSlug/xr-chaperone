use bytemuck::{Pod, Zeroable};
use glam::Vec2;

/// A single vertex in the chaperone mesh.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ChaperoneVertex {
    pub position: [f32; 3],
    pub wall_u: f32,
    pub wall_v: f32,
}

/// Output of mesh generation
pub struct ChaperoneMesh {
    pub vertices: Vec<ChaperoneVertex>,
    pub indices: Vec<u32>,
}

pub fn build_mesh(polygon: &[Vec2], wall_height: f32, grid_spacing: f32) -> ChaperoneMesh {
    // TODO: this might crash if we send an empty mesh, other code attempts to avoid this.
    if polygon.len() < 3 {
        return ChaperoneMesh {
            vertices: Vec::new(),
            indices: Vec::new(),
        };
    }

    let spacing = grid_spacing.max(0.01);

    // Snap wall height up to the nearest grid line so the top edge is always
    // a clean grid boundary with no partial cell sticking over the top.
    let snapped_height = (wall_height / spacing).ceil() * spacing;

    // Number of rows is exact after snapping.
    let rows = ((snapped_height / spacing).round() as u32).max(1);

    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // Walk the perimeter, accumulating arc-length as the U coordinate.
    // This makes the grid continuous across corners: where one wall ends
    // the next picks up at the same U offset, so grid lines align.
    let n = polygon.len();
    let mut perimeter_u = 0.0_f32;

    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        let length = a.distance(b);
        if length < 1e-4 {
            continue;
        }

        // Enough columns to have at least one vertex per grid cell.
        let cols = ((length / spacing).ceil() as u32).max(1);
        let base = vertices.len() as u32;

        for row in 0..=rows {
            let v = row as f32 * spacing;
            for col in 0..=cols {
                let t = col as f32 / cols as f32;
                let u = perimeter_u + t * length;
                let xz = Vec2::lerp(a, b, t);
                vertices.push(ChaperoneVertex {
                    position: [xz.x, v, xz.y],
                    wall_u: u,
                    wall_v: v,
                });
            }
        }

        let stride = cols + 1;
        for row in 0..rows {
            for col in 0..cols {
                let tl = base + row * stride + col;
                let tr = base + row * stride + col + 1;
                let bl = base + (row + 1) * stride + col;
                let br = base + (row + 1) * stride + col + 1;
                indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
            }
        }

        perimeter_u += length;
    }
    ChaperoneMesh { vertices, indices }
}
