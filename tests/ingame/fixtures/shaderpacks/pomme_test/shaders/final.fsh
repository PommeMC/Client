#version 120

uniform sampler2D colortex0;

varying vec2 texcoord;

void main() {
    gl_FragColor = texture2D(colortex0, texcoord);
}
