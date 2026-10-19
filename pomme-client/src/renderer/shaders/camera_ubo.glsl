// The frame camera UBO (set 0, binding 0), shared by every world pass; a
// single declaration so the std140 layout can't drift between them. Lane
// meanings live on the Rust struct (camera::CameraUniform).
#extension GL_EXT_multiview : require

layout(set = 0, binding = 0) uniform CameraUniform {
    // One per eye in a multiview (VR) pass.
    mat4 view_projs[2];
    vec4 camera_pos;
    vec4 fog_color;
    ivec4 camera_block;
    vec4 fog_env;
};
#define view_proj view_projs[gl_ViewIndex]
