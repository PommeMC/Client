#version 120

// The drawn sky half blended with the custom uniform's; geometry lit from
// above by its normal.

#define SKY_TINT // the sky from shaders.properties' custom uniform

uniform sampler2D colortex0;
uniform sampler2D colortex2;
uniform sampler2D depthtex0;
uniform vec3 pommeSky;
uniform vec3 upPosition;

/*
const int colortex0Format = RGBA16F;
*/

varying vec2 texcoord;

void main() {
    vec3 color = texture2D(colortex0, texcoord).rgb;
    float depth = texture2D(depthtex0, texcoord).r;
    if (depth >= 1.0) {
#ifdef SKY_TINT
        color = mix(color, pommeSky, 0.5);
#endif
    } else {
        vec3 normal = texture2D(colortex2, texcoord).rgb * 2.0 - 1.0;
        float up = dot(normal, normalize(upPosition));
        color *= 0.6 + 0.4 * max(up, 0.0);
    }
/* DRAWBUFFERS:0 */
    gl_FragData[0] = vec4(color, 1.0);
}
