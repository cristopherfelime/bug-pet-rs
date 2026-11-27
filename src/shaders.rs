/// Textured 2D quad shader in pet-local `800x800` coordinate space -> NDC.
/// Expects premultiplied RGBA textures and `(ONE, INV_SRC_ALPHA)` blend state.
pub const SPRITE_HLSL: &str = r#"
cbuffer SpriteCB : register(b0) {
    float4 m0;
    float4 m1;
    float4 vp;
    float4 win_and_pad;
    float4 tint;
};

struct VSIn {
    float2 pos : POSITION;
    float2 uv  : TEXCOORD0;
};

struct VSOut {
    float4 pos : SV_POSITION;
    float2 uv  : TEXCOORD0;
};

VSOut VSMain(VSIn input) {
    VSOut o;
    float2 pet_xy;
    pet_xy.x = m0.x * input.pos.x + m0.y * input.pos.y + m0.z;
    pet_xy.y = m1.x * input.pos.x + m1.y * input.pos.y + m1.z;

    float2 win_dip = pet_xy * vp.xy + vp.zw;
    float2 ndc;
    ndc.x = (win_dip.x / win_and_pad.x) * 2.0 - 1.0;
    ndc.y = 1.0 - (win_dip.y / win_and_pad.y) * 2.0;

    o.pos = float4(ndc, 0.0, 1.0);
    o.uv = input.uv;
    return o;
}

Texture2D    tex0 : register(t0);
SamplerState samp0 : register(s0);

float4 PSMain(VSOut input) : SV_TARGET {
    return tex0.Sample(samp0, input.uv) * tint;
}
"#;

/// Full GPU vertex-skinning shader reproducing `ut()` from `rig.js:536-585`
/// with zero CPU vertex loops and zero WebGL->Canvas2D readbacks.
pub const RIG_SKIN_HLSL: &str = r#"
cbuffer RigCB : register(b0) {
    float4 parent_m0;     // (a, c, tx, 0) parent hierarchy matrix -> 800 space
    float4 parent_m1;     // (b, d, ty, 0)
    float4 vp;            // (size/800, size/800, core_off_x_dip, core_off_y_dip)
    float4 win_and_pad;   // (win_w_dip, win_h_dip, 0, 0)
    float4 mode_and_fade; // (is_ear, fade, n_bones, part_scale)
    float4 pick_pose;     // (pick.pin.x, pick.pin.y, pose.pin.x, pose.pin.y)
    float4 dir_flip_len;  // (cos(pick.dir), sin(pick.dir), flip_m, len_c)
    float4 ear_curve;     // (dirs[0], a, b, bend)
    float4 ear_root;      // (cos(dirs[0]-pick.dir), sin(dirs[0]-pick.dir), 0, 0)
    float4 p_d[13];       // (p_frac[k], d_width[k], 0, 0)
    float4 joints_rot[12];// (joint.x, joint.y, cos(rel), sin(rel))
    float4 rest_start[12];// (rest.x, rest.y, 0, 0)
    float4 ear_spine[49]; // (pts[r].x, pts[r].y, 0, 0)
};

struct VSIn {
    float2 rest_pos : POSITION;
    float2 uv       : TEXCOORD0;
    float  t_param  : TEXCOORD1;
};

struct VSOut {
    float4 pos     : SV_POSITION;
    float2 uv      : TEXCOORD0;
    float  t_param : TEXCOORD1;
};

float smooth_range(float a, float b, float x) {
    if (b - a < 1e-6) {
        return x >= a ? 1.0 : 0.0;
    }
    float t = saturate((x - a) / (b - a));
    return t * t * (3.0 - 2.0 * t);
}

float eval_y(int idx, float x, int n_bones) {
    if (idx >= n_bones) return 0.0;
    return smooth_range(p_d[idx].x - p_d[idx].y, p_d[idx].x + p_d[idx].y, x);
}

VSOut VSMain(VSIn input) {
    VSOut o;
    bool is_ear = mode_and_fade.x > 0.5;
    int n_bones = (int)(mode_and_fade.z + 0.5);
    float b_scale = mode_and_fade.w;
    float2 rel = input.rest_pos - pick_pose.xy;
    float2 skinned;

    if (is_ear) {
        float2 rig_m = float2(
            pick_pose.z + (rel.x * ear_root.x - rel.y * ear_root.y) * b_scale,
            pick_pose.w + (rel.x * ear_root.y + rel.y * ear_root.x) * b_scale
        );
        float blend_m = eval_y(0, input.t_param, n_bones);
        if (blend_m <= 1e-5) {
            skinned = rig_m;
        } else {
            float lat_n = (-rel.x * dir_flip_len.y + rel.y * dir_flip_len.x) * b_scale;
            float r = input.t_param;
            float2 sp_pos;
            float sp_ang;
            if (r >= 1.0) {
                sp_ang = ear_curve.x + ear_curve.y + (ear_curve.z - ear_curve.y) * ear_curve.w;
                float extra = dir_flip_len.w * (r - 1.0);
                sp_pos = ear_spine[48].xy + float2(cos(sp_ang), sin(sp_ang)) * extra;
            } else {
                float cr = max(0.0, r);
                float u = cr * 48.0;
                int s_idx = min(47, (int)floor(u));
                float f = u - (float)s_idx;
                sp_pos = lerp(ear_spine[s_idx].xy, ear_spine[s_idx + 1].xy, f);
                sp_ang = ear_curve.x + ear_curve.y + (ear_curve.z - ear_curve.y) * ear_curve.w * cr * cr;
            }
            float2 bent = float2(sp_pos.x - sin(sp_ang) * lat_n, sp_pos.y + cos(sp_ang) * lat_n);
            skinned = lerp(rig_m, bent, blend_m);
        }
    } else {
        if (abs(dir_flip_len.z - 1.0) > 1e-5) {
            float axial = rel.x * dir_flip_len.x + rel.y * dir_flip_len.y;
            float lat   = (-rel.x * dir_flip_len.y + rel.y * dir_flip_len.x) * dir_flip_len.z;
            rel = float2(
                axial * dir_flip_len.x - lat * dir_flip_len.y,
                axial * dir_flip_len.y + lat * dir_flip_len.x
            );
        }
        float2 p = pick_pose.zw + rel * b_scale;
        float d0 = eval_y(0, input.t_param, n_bones);
        skinned = p * (1.0 - d0);
        for (int k = 0; k < 12; k++) {
            if (k < n_bones) {
                float w = eval_y(k, input.t_param, n_bones) - eval_y(k + 1, input.t_param, n_bones);
                if (w > 1e-5) {
                    float2 delta = p - rest_start[k].xy;
                    float4 jr = joints_rot[k];
                    float2 rotated = float2(delta.x * jr.z - delta.y * jr.w,
                                            delta.x * jr.w + delta.y * jr.z);
                    skinned += w * (jr.xy + rotated);
                }
            }
        }
    }

    float2 pet_xy = float2(
        parent_m0.x * skinned.x + parent_m0.y * skinned.y + parent_m0.z,
        parent_m1.x * skinned.x + parent_m1.y * skinned.y + parent_m1.z
    );
    float2 win_dip = pet_xy * vp.xy + vp.zw;
    o.pos = float4((win_dip.x / win_and_pad.x) * 2.0 - 1.0,
                   1.0 - (win_dip.y / win_and_pad.y) * 2.0,
                   0.0, 1.0);
    o.uv = input.uv;
    o.t_param = input.t_param;
    return o;
}

Texture2D    tex0 : register(t0);
SamplerState samp0 : register(s0);

float4 PSMain(VSOut input) : SV_TARGET {
    float fd = mode_and_fade.y;
    float u = (fd <= 0.0) ? 1.0 : saturate(input.t_param / fd);
    u = u * u * u * (u * (u * 6.0 - 15.0) + 10.0);
    return tex0.Sample(samp0, input.uv) * u;
}
"#;
