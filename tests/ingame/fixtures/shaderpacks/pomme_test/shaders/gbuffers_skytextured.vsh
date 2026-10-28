#version 120

// Pomme's test pack: the sun, moon and End sky, textured.

varying vec2 texcoord;
varying vec4 glcolor;

void main() {
    gl_Position = ftransform();
    texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;
    glcolor = gl_Color;
}
