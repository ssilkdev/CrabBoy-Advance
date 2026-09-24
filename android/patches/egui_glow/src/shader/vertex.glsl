#if NEW_SHADER_INTERFACE
    #define I in
    #define O out
    #define V(x) x
#else
    #define I attribute
    #define O varying
    #define V(x) vec3(x)
#endif

#ifdef GL_ES
    // CrabBoy patch: full precision when the GPU has it. With mediump, phone
    // GPUs use 16-bit floats, which can't address the font atlas (up to
    // 16384 px wide) exactly: glyphs sample a sliver of their neighbour and
    // show a thin line beside letters. GLES 3 guarantees highp here.
    #if defined(GL_FRAGMENT_PRECISION_HIGH) || __VERSION__ >= 300
        precision highp float;
    #else
        precision mediump float;
    #endif
#endif

uniform vec2 u_screen_size;
I vec2 a_pos;
I vec4 a_srgba; // 0-255 sRGB
I vec2 a_tc;
O vec4 v_rgba_in_gamma;
O vec2 v_tc;

void main() {
    gl_Position = vec4(
                      2.0 * a_pos.x / u_screen_size.x - 1.0,
                      1.0 - 2.0 * a_pos.y / u_screen_size.y,
                      0.0,
                      1.0);
    v_rgba_in_gamma = a_srgba / 255.0;
    v_tc = a_tc;
}
