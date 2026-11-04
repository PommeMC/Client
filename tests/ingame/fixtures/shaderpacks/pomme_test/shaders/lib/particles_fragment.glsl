// Particles tinted `TINT` in the particle stage, so a shot shows which program
// drew them. No discard: Iris's alpha test cuts the sprites out.

uniform sampler2D texture;
uniform sampler2D lightmap;
uniform int renderStage;

varying vec2 texcoord;
varying vec2 lmcoord;
varying vec4 glcolor;

void main() {
    vec4 color = texture2D(texture, texcoord) * glcolor;
    color.rgb *= texture2D(lightmap, lmcoord).rgb;
    if (renderStage == MC_RENDER_STAGE_PARTICLES) color.rgb *= TINT;
/* DRAWBUFFERS:0 */
    gl_FragData[0] = color;
}
