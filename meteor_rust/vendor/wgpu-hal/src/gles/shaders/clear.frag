uniform vec4 color;

uniform vec4 color_workaround;
out vec4 frag;
void main() {
  frag = color + color_workaround;
}
