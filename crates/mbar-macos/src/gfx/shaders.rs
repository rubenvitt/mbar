//! Metal shading language source, compiled at runtime with
//! `-[MTLDevice newLibraryWithSource:options:error:]` (no `xcrun metal` at build time).
//!
//! Two pipelines:
//! * `quad_*`: instanced quads (triangle strip of 4 vertices per instance) for SDF rounded
//!   rects with border, A8 text masks (tinted), BGRA text runs and images (rounded clip,
//!   inner border, optional tint). Clipping is per instance: an axis-aligned clip rect
//!   (the quad geometry is intersected with it in the vertex stage) plus one optional
//!   rounded clip evaluated as an SDF.
//! * `path_*`: triangle lists for graph strokes (analytic AA across the line) and fills.
//!
//! All colours are premultiplied; blending is `src + dst * (1 - src.a)`.
//!
//! The struct layouts must match `renderer::QuadInstance`, `renderer::Uniforms`,
//! `renderer::ClipParams` and `util::PathVertex`.

pub const KIND_RECT: u32 = 0;
pub const KIND_MASK: u32 = 1;
pub const KIND_COLOR_TEXT: u32 = 2;
pub const KIND_IMAGE: u32 = 3;
/// Flag bit in `kind`: sample with the nearest-neighbour sampler.
pub const FLAG_NEAREST: u32 = 0x100;

pub const SOURCE: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Uniforms {
    float2 viewport;     // window size in points
    float scale;         // backing scale (pixels per point)
    float _pad;
};

struct QuadInstance {
    float4 rect;         // x, y, w, h (points, top-left origin)
    float4 color;        // fill (rect) / tint (mask, image)
    float4 border_color;
    float4 uv;           // u0, v0, u1, v1
    float4 clip;         // x0, y0, x1, y1 axis-aligned clip
    float4 rclip;        // rounded clip rect x, y, w, h
    float radius;
    float border_width;
    float rclip_radius;  // < 0: no rounded clip
    uint kind;
};

struct ClipParams {
    float4 clip;
    float4 rclip;
    float rclip_radius;
    float scale;
    float2 _pad;
};

struct PathVertex {
    float2 pos;
    float dist;
    float half_width;
    float4 color;
};

struct QuadOut {
    float4 position [[position]];
    float2 p;
    uint inst [[flat]];
};

struct PathOut {
    float4 position [[position]];
    float2 p;
    float dist;
    float half_width [[flat]];
    float4 color;
};

static inline float sd_round_rect(float2 p, float4 r, float radius) {
    float2 half_size = r.zw * 0.5;
    float2 c = r.xy + half_size;
    float2 q = abs(p - c) - (half_size - radius);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

static inline float clip_coverage(float2 p, float4 clip, float4 rclip, float rclip_radius, float s) {
    float2 dlo = p - clip.xy;
    float2 dhi = clip.zw - p;
    float dmin = min(min(dlo.x, dlo.y), min(dhi.x, dhi.y));
    float cov = saturate(dmin * s + 0.5);
    if (rclip_radius >= 0.0) {
        float d = sd_round_rect(p, rclip, rclip_radius);
        cov *= saturate(0.5 - d * s);
    }
    return cov;
}

static inline float4 to_ndc(float2 p, float2 viewport) {
    float2 n = p / viewport * 2.0 - 1.0;
    return float4(n.x, -n.y, 0.0, 1.0);
}

vertex QuadOut quad_vertex(uint vid [[vertex_id]],
                           uint iid [[instance_id]],
                           const device QuadInstance* instances [[buffer(0)]],
                           constant Uniforms& u [[buffer(1)]]) {
    QuadInstance q = instances[iid];
    uint kind = q.kind & 0xffu;
    float margin = (kind == 0u || kind == 3u) ? (1.0 / u.scale) : 0.0;
    float2 lo = q.rect.xy - margin;
    float2 hi = q.rect.xy + q.rect.zw + margin;
    lo = max(lo, q.clip.xy);
    hi = min(hi, q.clip.zw);
    if (q.rclip_radius >= 0.0) {
        lo = max(lo, q.rclip.xy);
        hi = min(hi, q.rclip.xy + q.rclip.zw);
    }
    hi = max(hi, lo);
    float2 corner = float2((vid & 1u) ? hi.x : lo.x, (vid & 2u) ? hi.y : lo.y);
    QuadOut o;
    o.position = to_ndc(corner, u.viewport);
    o.p = corner;
    o.inst = iid;
    return o;
}

fragment float4 quad_fragment(QuadOut in [[stage_in]],
                              const device QuadInstance* instances [[buffer(0)]],
                              constant Uniforms& u [[buffer(1)]],
                              texture2d<float> tex [[texture(0)]],
                              sampler nearest_sampler [[sampler(0)]],
                              sampler linear_sampler [[sampler(1)]]) {
    QuadInstance q = instances[in.inst];
    float2 p = in.p;
    float s = u.scale;
    uint kind = q.kind & 0xffu;
    bool nearest = (q.kind & 0x100u) != 0u;
    float cov = clip_coverage(p, q.clip, q.rclip, q.rclip_radius, s);
    float4 col;
    if (kind == 0u) {
        float lw = q.border_width;
        float4 inset = float4(q.rect.xy + lw * 0.5, q.rect.zw - lw);
        float d = sd_round_rect(p, inset, q.radius);
        col = q.color * saturate(0.5 - d * s);
        if (lw > 0.0) {
            float stroke = saturate((lw * 0.5 - abs(d)) * s + 0.5);
            col = q.border_color * stroke + col * (1.0 - q.border_color.a * stroke);
        }
    } else {
        float2 t = saturate((p - q.rect.xy) / max(q.rect.zw, float2(1e-6)));
        float2 uv = mix(q.uv.xy, q.uv.zw, t);
        float4 texel = nearest ? tex.sample(nearest_sampler, uv) : tex.sample(linear_sampler, uv);
        if (kind == 1u) {
            col = q.color * texel.r;
        } else if (kind == 2u) {
            col = texel;
        } else {
            texel = q.color * texel.a + texel * (1.0 - q.color.a * texel.a);
            float d = sd_round_rect(p, q.rect, q.radius);
            float bw = q.border_width;
            if (bw > 0.0) {
                float band = 1.0 - saturate(0.5 - (d + bw) * s);
                texel = q.border_color * band + texel * (1.0 - q.border_color.a * band);
            }
            col = texel * saturate(0.5 - d * s);
        }
    }
    return col * cov;
}

vertex PathOut path_vertex(uint vid [[vertex_id]],
                           const device PathVertex* vertices [[buffer(0)]],
                           constant Uniforms& u [[buffer(1)]]) {
    PathVertex v = vertices[vid];
    PathOut o;
    o.position = to_ndc(v.pos, u.viewport);
    o.p = v.pos;
    o.dist = v.dist;
    o.half_width = v.half_width;
    o.color = v.color;
    return o;
}

fragment float4 path_fragment(PathOut in [[stage_in]],
                              constant ClipParams& c [[buffer(0)]]) {
    float s = c.scale;
    float cov = saturate((in.half_width - abs(in.dist)) * s + 0.5);
    cov *= clip_coverage(in.p, c.clip, c.rclip, c.rclip_radius, s);
    return in.color * cov;
}
"#;
