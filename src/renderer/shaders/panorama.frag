#version 450

layout(set = 0, binding = 0) uniform PanoramaParams {
    float scroll;
    float aspect;
    float blur;
    float _pad;
};

layout(set = 1, binding = 0) uniform samplerCube cube_tex;

layout(location = 0) in vec2 v_uv;

layout(location = 0) out vec4 out_color;

vec3 get_dir(vec2 uv) {
    float fov = 1.1;
    vec2 ndc = (uv - 0.5) * 2.0;

    vec3 dir = vec3(ndc.x * aspect * fov, -ndc.y * fov, 1.0);

    float pitch = -0.17;
    float cp = cos(pitch);
    float sp = sin(pitch);
    dir = vec3(dir.x, dir.y * cp - dir.z * sp, dir.y * sp + dir.z * cp);

    float angle = -scroll * 6.28318530718;
    float c = cos(angle);
    float s = sin(angle);
    return vec3(dir.x * c - dir.z * s, dir.y, dir.x * s + dir.z * c);
}

void main() {
    vec3 dir = get_dir(v_uv);
    float lod = blur * 5.0;
    out_color = textureLod(cube_tex, dir, lod);
}
