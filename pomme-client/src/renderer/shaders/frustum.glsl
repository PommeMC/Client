layout(set = 0, binding = 1) uniform FrustumBuf {
    vec4 planes[6];
    uint chunk_count;
    uint region_count;
    int cam_x;
    int cam_y;
    int cam_z;
    float frac_x;
    float frac_y;
    float frac_z;
    int player_cx;
    int player_cz;
    uint limit_rd;
    uint draw_capacity;
    uint occlusion_enabled;
    uint _pad0;
    uint _pad1;
};

bool bounds_visible(ivec3 origin, vec3 aabb_min, vec3 aabb_max) {
    vec3 base = vec3(origin - ivec3(cam_x, cam_y, cam_z)) - vec3(frac_x, frac_y, frac_z);
    vec3 mn = base + aabb_min;
    vec3 mx = base + aabb_max;
    for (int i = 0; i < 6; ++i) {
        vec4 p = planes[i];
        float d = p.x * (p.x >= 0.0 ? mx.x : mn.x)
                + p.y * (p.y >= 0.0 ? mx.y : mn.y)
                + p.z * (p.z >= 0.0 ? mx.z : mn.z) + p.w;
        if (d < 0.0) return false;
    }
    return true;
}

bool section_in_distance(ivec3 origin) {
    if (limit_rd == 0u) return true;
    int dx = abs((origin.x >> 4) - player_cx);
    int dz = abs((origin.z >> 4) - player_cz);
    return uint(max(dx, dz)) <= limit_rd;
}

bool camera_inside(ivec3 origin, vec3 aabb_min, vec3 aabb_max) {
    vec3 eye = vec3(cam_x, cam_y, cam_z) + vec3(frac_x, frac_y, frac_z);
    vec3 mn = vec3(origin) + aabb_min - vec3(0.1);
    vec3 mx = vec3(origin) + aabb_max + vec3(0.1);
    return all(greaterThanEqual(eye, mn)) && all(lessThanEqual(eye, mx));
}
