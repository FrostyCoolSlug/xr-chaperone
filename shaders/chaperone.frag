#version 450

layout(location = 0) in float in_wall_u;
layout(location = 1) in float in_wall_v;

layout(location = 0) out vec4 out_colour;

// This should match the structure in renderer.rs
layout(push_constant) uniform PushConstants {
    mat4  view_proj;
    vec4  colour;
    float opacity;
    float line_width;
    float grid_spacing;
} pc;

// Returns 1.0 within line_width/2 of a grid line, 0.0 elsewhere.
float grid_line(float uv, float grid_spacing, float half_width) {
    float cell = uv / grid_spacing;   // now cycles 0..1 per cell
    float cell_fraction = fract(cell);
    float distance_to_line = min(cell_fraction, 1.0 - cell_fraction);
    return step(distance_to_line, half_width);
}

void main() {
    float half_width = (pc.line_width * 0.5) / pc.grid_spacing;
    float horiz   = grid_line(in_wall_v, pc.grid_spacing, half_width);
    float vert    = grid_line(in_wall_u, pc.grid_spacing, half_width);
    float on_line = max(horiz, vert);

    if (on_line < 0.01) discard;

    float alpha = on_line * pc.opacity * pc.colour.a;
    out_colour  = vec4(pc.colour.rgb, alpha);
}
