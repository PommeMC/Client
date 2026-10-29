#version 120

// Pomme's test pack: vanilla's untextured sky elements in their colours.

varying vec4 glcolor;

void main() {
    gl_Position = ftransform();
    glcolor = gl_Color;
}
