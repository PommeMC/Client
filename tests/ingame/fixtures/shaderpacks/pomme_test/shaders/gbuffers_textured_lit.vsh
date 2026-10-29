#version 120

// Pomme's test pack: albedo × vertex colour × lightmap, and view normals.
// Stone (block.properties id 1) is tinted red.

varying vec2 texcoord;
varying vec2 lmcoord;
varying vec4 glcolor;
varying vec3 normal;

attribute vec4 mc_Entity;

void main() {
    gl_Position = ftransform();
    texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;
    lmcoord = (gl_TextureMatrix[1] * gl_MultiTexCoord1).xy;
    glcolor = gl_Color;
    if (mc_Entity.x == 1.0) glcolor.gb *= 0.4;
    normal = normalize(gl_NormalMatrix * gl_Normal);
}
