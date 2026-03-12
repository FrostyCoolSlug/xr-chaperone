#version 450

// Per-vertex inputs
layout(location = 0) in vec3 in_position;
layout(location = 1) in float in_wall_u;
layout(location = 2) in float in_wall_v;

// Outputs to fragment shader
layout(location = 0) out float out_wall_u;
layout(location = 1) out float out_wall_v;

// Push constant: combined view-projection matrix for one eye
layout(push_constant) uniform PushConstants {
    mat4 view_proj;
} pc;

void main() {
    gl_Position  = pc.view_proj * vec4(in_position, 1.0);
    out_wall_u   = in_wall_u;
    out_wall_v   = in_wall_v;
}
