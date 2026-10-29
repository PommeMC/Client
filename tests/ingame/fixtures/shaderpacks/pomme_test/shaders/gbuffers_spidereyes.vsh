#version 120

// Pomme's test pack: emissive eyes, lit by the lightmap coordinates they get.

varying vec2 texcoord;
varying vec2 lmcoord;
varying vec4 glcolor;
varying vec3 normal;

void main() {
    gl_Position = ftransform();
    texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;
    lmcoord = (gl_TextureMatrix[1] * gl_MultiTexCoord1).xy;
    glcolor = gl_Color;
    normal = normalize(gl_NormalMatrix * gl_Normal);
}
