#define GL_GLEXT_PROTOTYPES 1
#include <EGL/egl.h>
#include <GL/glcorearb.h>
#include <stdio.h>
#include <stdlib.h>

/* Buffer textures over ranges at GL_TEXTURE_BUFFER_OFFSET_ALIGNMENT, not
 * the 256 bytes a WebGPU storage binding's offset needs: texelFetch and
 * textureSize through a samplerBuffer, and imageLoad and imageSize through an
 * imageBuffer, each in two draws at ranges with different offsets in one
 * frame, read back once at the end, so state one draw leaves cannot stand in
 * for the next one's. */

#define CHECK(condition, message) do { \
    if (!(condition)) { \
        fprintf(stderr, "texture buffer probe failed: %s (line %d, error=0x%x)\n", \
                message, __LINE__, glGetError()); \
        return __LINE__; \
    } \
} while (0)

static GLuint compile_shader(GLenum type, const char *source) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    GLint ok = GL_FALSE;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[4096] = {0};
        glGetShaderInfoLog(shader, sizeof(log), NULL, log);
        fprintf(stderr, "texture buffer shader failed: %s\n", log);
        exit(2);
    }
    return shader;
}

static GLuint link_program(const char *vertex_source, const char *fragment_source) {
    GLuint program = glCreateProgram();
    glAttachShader(program, compile_shader(GL_VERTEX_SHADER, vertex_source));
    glAttachShader(program, compile_shader(GL_FRAGMENT_SHADER, fragment_source));
    glLinkProgram(program);
    GLint linked = GL_FALSE;
    glGetProgramiv(program, GL_LINK_STATUS, &linked);
    if (!linked) {
        char log[4096] = {0};
        glGetProgramInfoLog(program, sizeof(log), NULL, log);
        fprintf(stderr, "texture buffer program failed: %s\n", log);
        exit(2);
    }
    return program;
}

static unsigned word(unsigned i) { return (i * 3u + 5u) & 0xffu; }

int main(void) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    EGLDisplay display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    CHECK(display != EGL_NO_DISPLAY, "eglGetDisplay");
    CHECK(eglInitialize(display, NULL, NULL), "eglInitialize");
    CHECK(eglBindAPI(EGL_OPENGL_API), "eglBindAPI");
    EGLConfig config = NULL;
    EGLint config_count = 0;
    CHECK(eglChooseConfig(display, NULL, &config, 1, &config_count), "eglChooseConfig");
    EGLContext context = eglCreateContext(display, config, EGL_NO_CONTEXT, NULL);
    CHECK(context != EGL_NO_CONTEXT, "eglCreateContext");
    const EGLint pbuffer_attributes[] = {EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE};
    EGLSurface surface = eglCreatePbufferSurface(display, config, pbuffer_attributes);
    CHECK(eglMakeCurrent(display, surface, surface, context), "eglMakeCurrent");

    GLint align = 0;
    glGetIntegerv(GL_TEXTURE_BUFFER_OFFSET_ALIGNMENT, &align);
    /* Desktop drivers report 16, and programs bind ranges at it. */
    CHECK(align > 0 && align <= 16, "GL_TEXTURE_BUFFER_OFFSET_ALIGNMENT at most 16");
    /* Two ranges off a 256-byte boundary, at 16 and 48 bytes, one 16 texels
     * long, one 8. */
    const GLintptr first = 16, second = 48;

    unsigned data[1024];
    for (unsigned i = 0; i < 1024; ++i) data[i] = word(i);
    GLuint buffer = 0;
    glGenBuffers(1, &buffer);
    glBindBuffer(GL_TEXTURE_BUFFER, buffer);
    glBufferData(GL_TEXTURE_BUFFER, sizeof(data), data, GL_STATIC_DRAW);

    GLuint views[2] = {0, 0};
    glGenTextures(2, views);
    glBindTexture(GL_TEXTURE_BUFFER, views[0]);
    glTexBufferRange(GL_TEXTURE_BUFFER, GL_R32UI, buffer, first, 16 * 4);
    glBindTexture(GL_TEXTURE_BUFFER, views[1]);
    glTexBufferRange(GL_TEXTURE_BUFFER, GL_R32UI, buffer, second, 8 * 4);
    CHECK(glGetError() == GL_NO_ERROR, "glTexBufferRange at the reported alignment");

    GLuint output_texture = 0, framebuffer = 0, vao = 0;
    glGenTextures(1, &output_texture);
    glBindTexture(GL_TEXTURE_2D, output_texture);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, 4, 1, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
    glGenFramebuffers(1, &framebuffer);
    glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, output_texture, 0);
    CHECK(glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE, "framebuffer completeness");
    glGenVertexArrays(1, &vao);
    glBindVertexArray(vao);

    const char *vertex_source =
        "#version 440 core\n"
        "const vec2 p[3] = vec2[3](vec2(-1,-1),vec2(3,-1),vec2(-1,3));\n"
        "void main(){gl_Position=vec4(p[gl_VertexID],0,1);}\n";
    GLuint sampled = link_program(vertex_source,
        "#version 440 core\n"
        "layout(binding=0) uniform usamplerBuffer range;\n"
        "layout(location=0) out vec4 color;\n"
        "void main(){color=vec4(texelFetch(range,0).r,texelFetch(range,2).r,"
        "uint(textureSize(range)),255u)/255.0;}\n");
    GLuint imaged = link_program(vertex_source,
        "#version 440 core\n"
        "layout(binding=0,r32ui) readonly uniform uimageBuffer range;\n"
        "layout(location=0) out vec4 color;\n"
        "void main(){color=vec4(imageLoad(range,1).r,imageLoad(range,3).r,"
        "uint(imageSize(range)),255u)/255.0;}\n");

    glClearColor(1, 0, 1, 1);
    glClear(GL_COLOR_BUFFER_BIT);
    /* One frame: the two ranges drawn one after the other, nothing read
     * between them. */
    glUseProgram(sampled);
    glActiveTexture(GL_TEXTURE0);
    for (int i = 0; i < 2; ++i) {
        glBindTexture(GL_TEXTURE_BUFFER, views[i]);
        glViewport(i, 0, 1, 1);
        glDrawArrays(GL_TRIANGLES, 0, 3);
    }
    glUseProgram(imaged);
    for (int i = 0; i < 2; ++i) {
        glBindImageTexture(0, views[i], 0, GL_FALSE, 0, GL_READ_ONLY, GL_R32UI);
        glViewport(2 + i, 0, 1, 1);
        glDrawArrays(GL_TRIANGLES, 0, 3);
    }
    CHECK(glGetError() == GL_NO_ERROR, "buffer texture draws");

    unsigned char pixels[16] = {0};
    glReadPixels(0, 0, 4, 1, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    const unsigned f = (unsigned)(first / 4), s = (unsigned)(second / 4);
    const unsigned want[16] = {
        word(f), word(f + 2), 16, 255,
        word(s), word(s + 2), 8, 255,
        word(f + 1), word(f + 3), 16, 255,
        word(s + 1), word(s + 3), 8, 255,
    };
    const char *what[4] = {"the first range's texelFetch and textureSize",
                           "the second range's, drawn after it in the same frame",
                           "imageLoad and imageSize at the first range",
                           "imageLoad and imageSize at the second, drawn after it"};
    int bad = 0;
    for (int p = 0; p < 4; ++p) {
        int ok = 1;
        for (int c = 0; c < 4; ++c) ok &= pixels[p * 4 + c] == want[p * 4 + c];
        if (!ok) {
            fprintf(stderr, "%s: rgba=(%u,%u,%u,%u), want (%u,%u,%u,%u)\n", what[p],
                    pixels[p * 4], pixels[p * 4 + 1], pixels[p * 4 + 2], pixels[p * 4 + 3],
                    want[p * 4], want[p * 4 + 1], want[p * 4 + 2], want[p * 4 + 3]);
            bad = 1;
        }
    }
    CHECK(!bad, "buffer texture ranges readback");
    printf("gl44 texture buffer ranges ok: two offsets in one frame, texelFetch, textureSize, imageLoad, imageSize\n");
    return 0;
}
