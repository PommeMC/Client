#version 120

// The sky disc tinted red by its render stage; the rest as vanilla colours it.

uniform int renderStage;

varying vec4 glcolor;

void main() {
    vec4 color = glcolor;
    if (renderStage == MC_RENDER_STAGE_SKY) {
        color.rgb *= vec3(1.0, 0.6, 0.6);
    }
/* DRAWBUFFERS:0 */
    gl_FragData[0] = color;
}
