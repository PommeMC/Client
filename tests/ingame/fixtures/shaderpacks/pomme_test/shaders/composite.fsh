#version 120

// The drawn sky half blended with the custom uniform's; geometry lit from
// above by its normal and darkened where the shadow map is nearer the sun.

#define SKY_TINT // the sky from shaders.properties' custom uniform

uniform sampler2D colortex0;
uniform sampler2D colortex2;
uniform sampler2D depthtex0;
uniform sampler2DShadow shadowtex0;
uniform vec3 pommeSky;
uniform vec3 upPosition;
uniform mat4 gbufferProjectionInverse;
uniform mat4 gbufferModelViewInverse;
uniform mat4 shadowModelView;
uniform mat4 shadowProjection;

/*
const int colortex0Format = RGBA16F;
*/
const float sunPathRotation = 30.0;
const bool shadowHardwareFiltering = true;

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

        vec4 view = gbufferProjectionInverse * vec4(vec3(texcoord, depth) * 2.0 - 1.0, 1.0);
        vec4 player = gbufferModelViewInverse * vec4(view.xyz / view.w, 1.0);
        vec4 shadow = shadowProjection * shadowModelView * player;
        vec3 coord = shadow.xyz * 0.5 + 0.5;
        coord.z -= 0.002;
        color *= 0.4 + 0.6 * shadow2D(shadowtex0, coord).r;
    }
/* DRAWBUFFERS:0 */
    gl_FragData[0] = vec4(color, 1.0);
}
