pub mod mesh;
pub mod physics;

use crate::assets::{load_optional_sprite, RgbaSprite};
use crate::config::{CharacterConfig, RigCommonConfig, RigPartConfig};
use crate::pet_state::Mat2D;
use crate::protect::ProtectKey;
use crate::renderer::{GpuContext, GpuTexture, PetSurface, RigCB, SpriteCB};
use crate::rig::mesh::{
    build_prop_side, build_rig_mesh_side, extract_head_edge, PropMeta, RigMeshMeta,
};
use crate::rig::physics::{
    apply_beat, apply_drag, apply_key, apply_lift, apply_pet, apply_press, canvas_margin_frac,
    eval_prop_motion, solve_pose, step_part_physics, PartParams, PartPhys, PosedBoneRig, Z_DT,
};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RigZone {
    Back,
    Ghost,
    BackMid,
    NpBack,
    Mid,
    NpBehind,
    HeadBehind,
    HeadFront,
    HeadTop,
    NpFront,
    Top,
    TopTop,
}

pub enum RigGpuSide {
    Mesh {
        meta: RigMeshMeta,
        gpu_mesh: crate::renderer::RigGpuMesh,
    },
    Prop {
        meta: PropMeta,
        tex: GpuTexture,
    },
}

pub struct RigGpuPart {
    pub cfg: RigPartConfig,
    pub params: PartParams,
    pub zone: RigZone,
    #[allow(dead_code)]
    pub margin: f64,
    pub sides: Vec<RigGpuSide>,
}

pub struct CharacterRigBundle {
    pub rig_common: RigCommonConfig,
    pub parts: Vec<RigGpuPart>,
    pub max_margin: f64,
}

fn is_layer_key(s: &str) -> bool {
    s.starts_with("L:")
        && matches!(
            &s[2..],
            "back" | "side" | "ex1" | "ex2" | "ex3" | "ex4"
        )
}

/// Normalizes `partOrder` matching `L()` in `rig.js:79-102`.
pub fn normalize_part_order(raw_order: &[String], parts: &[&RigPartConfig]) -> Vec<String> {
    let part_ids: Vec<&str> = parts.iter().map(|p| p.id.as_str()).collect();
    let fixed = ["body", "head", "hair", "front"];
    let tops = ["top", "top2", "top3"];

    let mut order: Vec<String> = Vec::new();
    for item in raw_order {
        let s = item.as_str();
        let valid = fixed.contains(&s)
            || part_ids.contains(&s)
            || is_layer_key(s)
            || tops.contains(&s);
        if valid && !order.contains(item) {
            order.push(item.clone());
        }
    }

    if !order.iter().any(|x| x == "body") {
        order.insert(0, "body".to_string());
    }
    if !order.iter().any(|x| x == "head") {
        let b = order.iter().position(|x| x == "body").unwrap();
        order.insert(b + 1, "head".to_string());
    }
    let hi = order.iter().position(|x| x == "head").unwrap();
    let bi = order.iter().position(|x| x == "body").unwrap();
    if hi < bi {
        order.remove(hi);
        let bi2 = order.iter().position(|x| x == "body").unwrap();
        order.insert(bi2 + 1, "head".to_string());
    }

    if !order.iter().any(|x| is_layer_key(x)) {
        if !order.iter().any(|x| x == "hair") {
            let bi = order.iter().position(|x| x == "body").unwrap();
            order.insert(bi, "hair".to_string());
        }
        if !order.iter().any(|x| x == "front") {
            order.push("front".to_string());
        }
        let h_idx = order.iter().position(|x| x == "hair").unwrap();
        let b_idx = order.iter().position(|x| x == "body").unwrap();
        if h_idx > b_idx {
            order.remove(h_idx);
            let b_idx2 = order.iter().position(|x| x == "body").unwrap();
            order.insert(b_idx2, "hair".to_string());
        }
        let f_idx = order.iter().position(|x| x == "front").unwrap();
        let hd_idx = order.iter().position(|x| x == "head").unwrap();
        if f_idx < hd_idx {
            order.remove(f_idx);
            let hd_idx2 = order.iter().position(|x| x == "head").unwrap();
            order.insert(hd_idx2 + 1, "front".to_string());
        }
    }

    let hair_anchor = |cur: &[String]| -> usize {
        if let Some(idx) = cur.iter().position(|x| x == "hair") {
            return idx;
        }
        let bi = cur.iter().position(|x| x == "body").unwrap();
        if let Some(idx) = cur[..bi].iter().position(|x| is_layer_key(x)) {
            return idx;
        }
        bi
    };

    for t in parts {
        if order.iter().any(|x| x == &t.id) {
            continue;
        }
        let is_head_tail = t.kind == "tail"
            && t.s.get("head").and_then(|v| v.as_bool()).unwrap_or(false);
        let is_peek_prop = t.kind == "prop"
            && t.s.get("anim").and_then(|v| v.as_str()) == Some("peek");
        if is_head_tail {
            let hd = order.iter().position(|x| x == "head").unwrap();
            order.insert(hd + 1, t.id.clone());
        } else if is_peek_prop {
            order.insert(0, t.id.clone());
        } else {
            let last_same = order.iter().rposition(|oid| {
                parts.iter().any(|op| op.id == *oid && op.kind == t.kind)
            });
            if let Some(idx) = last_same {
                order.insert(idx + 1, t.id.clone());
            } else if t.kind == "ear" {
                let hd = order.iter().position(|x| x == "head").unwrap();
                order.insert(hd, t.id.clone());
            } else if t.kind == "tail" {
                let ha = hair_anchor(&order);
                let wing_pos = order[..ha].iter().position(|oid| {
                    parts.iter().any(|op| op.id == *oid && op.kind == "wing")
                });
                order.insert(wing_pos.unwrap_or(ha), t.id.clone());
            } else {
                let ha = hair_anchor(&order);
                order.insert(ha, t.id.clone());
            }
        }
    }

    order
}

/// Maps a part into its exact `RigZone` matching `Mt()` (`rig.js:688-696`).
pub fn resolve_zone(part: &RigPartConfig, params: &PartParams, order: &[String]) -> RigZone {
    let mut kind = part.kind.as_str();
    let idx = |name: &str| order.iter().position(|x| x == name);

    if let PartParams::Tail(t) = params {
        if t.head {
            kind = "ear";
        }
    }
    if let PartParams::Prop(p) = params {
        if p.follow == "head" {
            kind = "ear";
        } else if p.follow == "noPet" {
            let i = idx(&part.id).unwrap_or(0);
            let o = idx("head").unwrap_or(order.len());
            let a = idx("body").unwrap_or(0);
            return if i > o {
                RigZone::NpFront
            } else if i > a {
                RigZone::NpBehind
            } else {
                RigZone::NpBack
            };
        }
    }

    let i = idx(&part.id).unwrap_or(0);
    let a = idx("body").unwrap_or(0);
    let o = idx("head").unwrap_or(order.len());
    let s_idx = idx("hair").or_else(|| order[..a].iter().position(|x| is_layer_key(x)));
    let l_idx = idx("front")
        .or_else(|| order.iter().enumerate().position(|(j, x)| j > o && is_layer_key(x)));

    if kind == "ear" {
        if i > o {
            if l_idx.is_some_and(|l| i > l) {
                RigZone::HeadTop
            } else {
                RigZone::HeadFront
            }
        } else if i > a {
            RigZone::HeadBehind
        } else {
            RigZone::Ghost
        }
    } else if i > o {
        if l_idx.is_some_and(|l| i > l) {
            RigZone::TopTop
        } else {
            RigZone::Top
        }
    } else if i > a {
        RigZone::Mid
    } else if s_idx.is_some_and(|s| i > s) {
        RigZone::BackMid
    } else {
        RigZone::Back
    }
}

pub fn build_and_upload_rig(
    gpu: &GpuContext,
    char_dir: &Path,
    config: &CharacterConfig,
    head1: Option<&RgbaSprite>,
    protect: &ProtectKey,
    max_w: u32,
) -> Result<CharacterRigBundle, String> {
    let mut valid_parts: Vec<&RigPartConfig> = Vec::new();
    for p in config.rig.iter().take(12) {
        if matches!(p.kind.as_str(), "ear" | "tail" | "wing" | "prop") && !p.id.is_empty() {
            valid_parts.push(p);
        }
    }

    let order = normalize_part_order(&config.part_order, &valid_parts);
    let has_ear = valid_parts.iter().any(|p| p.kind == "ear" && p.on);
    let head_edge = if has_ear {
        extract_head_edge(head1)
    } else {
        Vec::new()
    };

    let mut gpu_parts = Vec::new();
    let mut max_margin = 0.35f64;

    for id in &order {
        let Some(&part_cfg) = valid_parts.iter().find(|p| p.id == *id) else {
            continue;
        };
        if !part_cfg.on || part_cfg.img.is_empty() {
            continue;
        }
        let Some(sprite) = load_optional_sprite(char_dir, &part_cfg.img, protect) else {
            continue;
        };

        let params = PartParams::from_config(part_cfg);
        let zone = resolve_zone(part_cfg, &params, &order);
        let mut sides = Vec::new();
        let mut first_mesh_meta: Option<RigMeshMeta> = None;

        if part_cfg.kind == "prop" {
            if let Some(prop) = build_prop_side(&sprite, max_w) {
                let tex = GpuTexture::from_sprite(gpu, &prop.sprite)?;
                let meta = prop.meta();
                sides.push(RigGpuSide::Prop { meta, tex });
            }
        } else {
            let n_sides = if part_cfg.kind == "tail" { 1 } else { 2 };
            for si in 0..n_sides {
                if let Some(side) = build_rig_mesh_side(
                    part_cfg,
                    &sprite,
                    si,
                    n_sides,
                    &head_edge,
                    &config.rig_common,
                    max_w,
                ) {
                    let gpu_mesh = gpu.create_rig_mesh(&side.vertices, &side.indices, &side.sprite)?;
                    let meta = side.meta();
                    if first_mesh_meta.is_none() {
                        first_mesh_meta = Some(meta);
                    }
                    sides.push(RigGpuSide::Mesh { meta, gpu_mesh });
                }
            }
        }

        if sides.is_empty() {
            continue;
        }

        let margin = canvas_margin_frac(&part_cfg.kind, &params, first_mesh_meta.as_ref());
        max_margin = max_margin.max(margin);
        gpu_parts.push(RigGpuPart {
            cfg: part_cfg.clone(),
            params,
            zone,
            margin,
            sides,
        });
    }

    Ok(CharacterRigBundle {
        rig_common: config.rig_common.clone(),
        parts: gpu_parts,
        max_margin: max_margin.clamp(0.35, 1.0),
    })
}

pub struct RigRuntime {
    pub bundle: std::sync::Arc<CharacterRigBundle>,
    pub phys: Vec<PartPhys>,
    pub enabled: Vec<bool>,
    acc_t: f64,
    sim_t: f64,
    ghost_mat: Option<Mat2D>,
}

impl RigRuntime {
    pub fn new(bundle: std::sync::Arc<CharacterRigBundle>) -> Self {
        let count = bundle.parts.len();
        Self {
            bundle,
            phys: vec![PartPhys::default(); count],
            enabled: vec![true; count],
            acc_t: 0.0,
            sim_t: 0.0,
            ghost_mat: None,
        }
    }

    pub fn part_toggles(&self) -> Vec<(String, String, bool)> {
        self.bundle
            .parts
            .iter()
            .zip(self.enabled.iter())
            .map(|(p, &on)| (p.cfg.id.clone(), p.cfg.kind.clone(), on))
            .collect()
    }

    pub fn toggle_part(&mut self, part_id: &str) -> bool {
        for (p, on) in self.bundle.parts.iter().zip(self.enabled.iter_mut()) {
            if p.cfg.id == part_id {
                *on = !*on;
                return *on;
            }
        }
        false
    }

    pub fn set_all_parts(&mut self, on: bool) {
        for flag in &mut self.enabled {
            *flag = on;
        }
    }

    pub fn step(&mut self, dt_s: f64, resting: bool, head_target_mat: Mat2D) {
        if !resting {
            let capped_dt = dt_s.clamp(0.0, 0.5);
            self.sim_t += capped_dt;
            if self.sim_t - self.acc_t > 0.5 {
                self.acc_t = self.sim_t - 0.5;
            }
            while self.acc_t < self.sim_t {
                self.acc_t += Z_DT;
                for (part, ph) in self.bundle.parts.iter().zip(self.phys.iter_mut()) {
                    if let Some(RigGpuSide::Mesh { meta, .. }) = part.sides.first() {
                        if part.cfg.kind == "tail" {
                            ph.pk_dir = Some(meta.dir as f64);
                        }
                    }
                    step_part_physics(&part.params, ph, self.acc_t);
                }
            }
        }

        let lag = self.bundle.rig_common.lag;
        if lag > 0.0 {
            let f = (1.0 - lag.min(100.0) / 105.0).max(0.04) as f32;
            self.ghost_mat = Some(match self.ghost_mat {
                Some(prev) => Mat2D {
                    m: [
                        prev.m[0] + (head_target_mat.m[0] - prev.m[0]) * f,
                        prev.m[1] + (head_target_mat.m[1] - prev.m[1]) * f,
                        prev.m[2] + (head_target_mat.m[2] - prev.m[2]) * f,
                        prev.m[3] + (head_target_mat.m[3] - prev.m[3]) * f,
                        prev.m[4] + (head_target_mat.m[4] - prev.m[4]) * f,
                        prev.m[5] + (head_target_mat.m[5] - prev.m[5]) * f,
                    ],
                },
                None => head_target_mat,
            });
        } else {
            self.ghost_mat = Some(head_target_mat);
        }
    }

    pub fn ghost_matrix(&self, fallback: Mat2D) -> Mat2D {
        self.ghost_mat.unwrap_or(fallback)
    }

    pub fn press(&mut self, when: &str) {
        let t = self.acc_t;
        for (part, ph) in self.bundle.parts.iter().zip(self.phys.iter_mut()) {
            if part.params.when() == when {
                apply_press(&part.params, ph, t);
            }
        }
    }

    pub fn drag(&mut self, dx_norm: f64, dy_norm: f64) {
        for (part, ph) in self.bundle.parts.iter().zip(self.phys.iter_mut()) {
            apply_drag(&part.params, ph, dx_norm, dy_norm);
        }
    }

    pub fn beat(&mut self, intensity: f64) {
        let t = self.acc_t;
        for (part, ph) in self.bundle.parts.iter().zip(self.phys.iter_mut()) {
            apply_beat(&part.params, ph, t, intensity);
        }
    }

    pub fn key(&mut self) {
        let t = self.acc_t;
        for (part, ph) in self.bundle.parts.iter().zip(self.phys.iter_mut()) {
            apply_key(&part.params, ph, t);
        }
    }

    pub fn pet(&mut self) {
        let t = self.acc_t;
        for (part, ph) in self.bundle.parts.iter().zip(self.phys.iter_mut()) {
            apply_pet(&part.params, ph, t);
        }
    }

    pub fn set_lift(
        &mut self,
        amt: f64,
        body_a: f64,
        still_ids: &[String],
        hide_ids: &[String],
        sad_ids: &[String],
        drop_wobble: f64,
    ) {
        for (part, ph) in self.bundle.parts.iter().zip(self.phys.iter_mut()) {
            apply_lift(
                &part.cfg.id,
                &part.cfg.kind,
                ph,
                amt,
                body_a,
                still_ids,
                hide_ids,
                sad_ids,
                drop_wobble,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_zone(
        &mut self,
        zone: RigZone,
        parent_mat: Mat2D,
        vp: [f32; 4],
        win_and_pad: [f32; 4],
        rest: bool,
        gpu: &GpuContext,
        surface: &PetSurface,
    ) -> Result<(), String> {
        let sim_t = self.acc_t;
        let common = &self.bundle.rig_common;

        for (idx, (part, ph)) in self.bundle.parts.iter().zip(self.phys.iter_mut()).enumerate() {
            if part.zone != zone || !self.enabled.get(idx).copied().unwrap_or(true) {
                continue;
            }
            for (side_idx, side) in part.sides.iter().enumerate() {
                match side {
                    RigGpuSide::Prop { meta, tex } => {
                        let PartParams::Prop(ref prop_p) = part.params else {
                            continue;
                        };
                        let tr = eval_prop_motion(prop_p, ph, meta, sim_t, rest);
                        let quad = if tr.bottom_anchored {
                            Mat2D::unit_to_rect(-tr.w * 0.5, -tr.h, tr.w, tr.h)
                        } else {
                            Mat2D::unit_to_rect(-tr.w * 0.5, -tr.h * 0.5, tr.w, tr.h)
                        };
                        let tf = parent_mat
                            .mul(Mat2D::translate(tr.x, tr.y))
                            .mul(Mat2D::rotate_about(tr.rot_rad, 0.0, 0.0))
                            .mul(Mat2D::scale_about(tr.sx, tr.sy, 0.0, 0.0))
                            .mul(quad);
                        surface.draw_sprite(
                            gpu,
                            tex,
                            &SpriteCB {
                                m0: [tf.m[0], tf.m[1], tf.m[2], 0.0],
                                m1: [tf.m[3], tf.m[4], tf.m[5], 0.0],
                                vp,
                                win_and_pad,
                                tint: [1.0, 1.0, 1.0, 1.0],
                            },
                        )?;
                    }
                    RigGpuSide::Mesh { meta, gpu_mesh } => {
                        let pose = solve_pose(&part.cfg, &part.params, ph, side_idx, meta, common, rest);
                        let cb = build_rig_cb(&part.params, meta, &pose, parent_mat, vp, win_and_pad);
                        surface.draw_rig_mesh(gpu, gpu_mesh, &cb)?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn build_rig_cb(
    params: &PartParams,
    pick: &RigMeshMeta,
    pose: &PosedBoneRig,
    parent_mat: Mat2D,
    vp: [f32; 4],
    win_and_pad: [f32; 4],
) -> RigCB {
    let mut p_d = [[0.0f32; 4]; 13];
    for k in 0..13 {
        p_d[k] = [
            pose.p_fracs[k],
            if k < 12 { pose.d_widths[k] } else { 0.0 },
            0.0,
            0.0,
        ];
    }
    let mut joints_rot = [[0.0f32; 4]; 12];
    let mut rest_start = [[0.0f32; 4]; 12];
    for k in 0..12 {
        joints_rot[k] = [
            pose.joints[k][0],
            pose.joints[k][1],
            pose.rot_cs[k][0],
            pose.rot_cs[k][1],
        ];
        rest_start[k] = [pose.rest_start[k][0], pose.rest_start[k][1], 0.0, 0.0];
    }
    let mut ear_spine = [[0.0f32; 4]; 49];
    if pose.is_ear {
        for r in 0..49 {
            ear_spine[r] = [pose.ear_spine[r][0], pose.ear_spine[r][1], 0.0, 0.0];
        }
    }

    let l_ang = pose.dirs0 - pick.dir;
    RigCB {
        parent_m0: [parent_mat.m[0], parent_mat.m[1], parent_mat.m[2], 0.0],
        parent_m1: [parent_mat.m[3], parent_mat.m[4], parent_mat.m[5], 0.0],
        vp,
        win_and_pad,
        mode_and_fade: [
            if pose.is_ear { 1.0 } else { 0.0 },
            params.fade_frac(),
            pose.n_bones as f32,
            params.scale_frac() as f32,
        ],
        pick_pose: [pick.pin[0], pick.pin[1], pose.pose_pin[0], pose.pose_pin[1]],
        dir_flip_len: [pick.dir.cos(), pick.dir.sin(), pose.flip_m, pose.len_c],
        ear_curve: [pose.dirs0, pose.a_ang, pose.b_ang, pose.bend],
        ear_root: [l_ang.cos(), l_ang.sin(), 0.0, 0.0],
        p_d,
        joints_rot,
        rest_start,
        ear_spine,
    }
}
