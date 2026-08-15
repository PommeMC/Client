const uint BATCH_CULL_SHIFT = 29u;
const uint BATCH_FACE_COUNT_MASK = 0x1fffffffu;
const uint BATCH_CULL_UNCULLABLE = 6u;
const float BATCH_CULL_EPSILON = 0.001;

uint batch_face_count(uint word) {
    return mesh_data[word + 1u] & BATCH_FACE_COUNT_MASK;
}

bool batch_backfacing(uint word) {
    uint direction = mesh_data[word + 1u] >> BATCH_CULL_SHIFT;
    if (direction >= BATCH_CULL_UNCULLABLE) return false;

    vec3 eye = vec3(cam_x, cam_y, cam_z) + vec3(frac_x, frac_y, frac_z);
    vec3 origin = vec3(
        int(mesh_data[word + 4u]),
        int(mesh_data[word + 5u]),
        int(mesh_data[word + 6u])
    );
    vec3 mn = origin + uintBitsToFloat(uvec3(
        mesh_data[word + 8u],
        mesh_data[word + 9u],
        mesh_data[word + 10u]
    ));
    vec3 mx = origin + uintBitsToFloat(uvec3(
        mesh_data[word + 12u],
        mesh_data[word + 13u],
        mesh_data[word + 14u]
    ));

    if (direction == 0u) return eye.y > mx.y + BATCH_CULL_EPSILON; // Down
    if (direction == 1u) return eye.y < mn.y - BATCH_CULL_EPSILON; // Up
    if (direction == 2u) return eye.z > mx.z + BATCH_CULL_EPSILON; // North
    if (direction == 3u) return eye.z < mn.z - BATCH_CULL_EPSILON; // South
    if (direction == 4u) return eye.x > mx.x + BATCH_CULL_EPSILON; // West
    return eye.x < mn.x - BATCH_CULL_EPSILON; // East
}
