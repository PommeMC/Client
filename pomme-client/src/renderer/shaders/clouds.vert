#version 450

layout(set = 0, binding = 0) uniform CameraUniform {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 fog_color;
};

// tint: per-frame cloud colour (rgb) and alpha (a).
// offset.xyz: camera-relative translation of the cloud grid (the sub-cell scroll
// plus the cloud-layer height), so the cached mesh only needs rebuilding when the
// camera crosses a whole cloud cell. offset.w: fog-fade end distance (blocks).
layout(push_constant) uniform Push {
    vec4 tint;
    vec4 offset;
} pc;

// Per-instance cloud face. in_cell = relative cell offset (rcx, rcz);
// in_dir_flags.x = face direction (0=down, 1=up, 2=N, 3=S, 4=W, 5=E),
// in_dir_flags.y bit0 = use the top (brightest) shade.
layout(location = 0) in ivec2 in_cell;
layout(location = 1) in uvec2 in_dir_flags;

layout(location = 0) out vec4 v_color;
layout(location = 1) out float v_dist;

// One cloud cell is 12 blocks wide and 4 blocks thick (vanilla CellSize).
const vec3 CELL = vec3(12.0, 4.0, 12.0);

// Unit-cube corners per face, ordered down, up, north, south, west, east —
// matching vanilla `rendertype_clouds.vsh` vertices[].
const vec3 CORNERS[24] = vec3[](
    // Bottom (down)
    vec3(1, 0, 0), vec3(1, 0, 1), vec3(0, 0, 1), vec3(0, 0, 0),
    // Top (up)
    vec3(0, 1, 0), vec3(0, 1, 1), vec3(1, 1, 1), vec3(1, 1, 0),
    // North
    vec3(0, 0, 0), vec3(0, 1, 0), vec3(1, 1, 0), vec3(1, 0, 0),
    // South
    vec3(1, 0, 1), vec3(1, 1, 1), vec3(0, 1, 1), vec3(0, 0, 1),
    // West
    vec3(0, 0, 1), vec3(0, 1, 1), vec3(0, 1, 0), vec3(0, 0, 0),
    // East
    vec3(1, 0, 0), vec3(1, 1, 0), vec3(1, 1, 1), vec3(1, 0, 1)
);

// Per-face shade: down 0.7, up 1.0, N/S 0.8, E/W 0.9.
const float SHADES[6] = float[](0.7, 1.0, 0.8, 0.8, 0.9, 0.9);

// Two triangles (0,1,2)(0,2,3) over the quad's four corners.
const int QUAD[6] = int[](0, 1, 2, 0, 2, 3);

void main() {
    int dir = int(in_dir_flags.x);
    bool use_top = (in_dir_flags.y & 1u) != 0u;

    vec3 corner = CORNERS[dir * 4 + QUAD[gl_VertexIndex]];
    // `corner`/`in_cell` are in the integer cell grid; adding the offset
    // (computed against the eye in f64 CPU-side) yields the eye-relative
    // position, the same space weather.vert ends up in.
    vec3 pos = corner * CELL
        + vec3(float(in_cell.x), 0.0, float(in_cell.y)) * CELL
        + pc.offset.xyz;
    gl_Position = view_proj * vec4(pos, 1.0);

    float shade = use_top ? SHADES[1] : SHADES[dir];
    v_color = vec4(vec3(shade), 1.0);
    // Spherical distance from the camera (pos is camera-relative), for the fade.
    v_dist = length(pos);
}
