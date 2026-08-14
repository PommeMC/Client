layout(set = 0, binding = 1) uniform FrustumBuf {
    vec4 planes[6];
    // Camera that drew the Hi-Z pyramid this dispatch samples (last frame's);
    // occlusion tests must project with it, not the live camera.
    mat4 prev_view_proj;
    uint chunk_count;
    // Camera block position plus the eye's small offset from it; the origins
    // are rebased in integer math so nothing here forms a large float.
    int cam_x;
    int cam_y;
    int cam_z;
    float frac_x;
    float frac_y;
    float frac_z;
    // The pyramid camera's anchor split, same convention as cam_*/frac_*.
    int prev_cam_x;
    int prev_cam_y;
    int prev_cam_z;
    float prev_frac_x;
    float prev_frac_y;
    float prev_frac_z;
    // 0 = fail open, skip the occlusion test (no pyramid yet, world change,
    // or F3+O off).
    uint occlusion_valid;
    // Player column + render distance for the column cull (0 = off).
    int player_cx;
    int player_cz;
    uint limit_rd;
    // Allocated indirect-command capacity shared by all emit passes.
    uint draw_capacity;
    uint _pad1;
    uint _pad2;
};
