use crate::assets::RgbaSprite;
use crate::config::{RigCommonConfig, RigPartConfig};
use image::imageops::{resize, FilterType};
use image::{ImageBuffer, Rgba};
use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RigVertex {
    pub rest_pos: [f32; 2],
    pub uv: [f32; 2],
    pub t_param: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct RigMeshMeta {
    pub pin: [f32; 2],
    pub len: f32,
    pub dir: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct PropMeta {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug)]
pub struct RigMeshSide {
    pub sprite: RgbaSprite,
    pub vertices: Vec<RigVertex>,
    pub indices: Vec<u32>,
    pub pin: [f32; 2],
    #[allow(dead_code)]
    pub tip: [f32; 2],
    pub len: f32,
    pub dir: f32,
}

impl RigMeshSide {
    pub fn meta(&self) -> RigMeshMeta {
        RigMeshMeta {
            pin: self.pin,
            len: self.len,
            dir: self.dir,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PropSide {
    pub sprite: RgbaSprite,
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
}

impl PropSide {
    pub fn meta(&self) -> PropMeta {
        PropMeta {
            cx: self.cx,
            cy: self.cy,
            w: self.w,
            h: self.h,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FixedAnchors {
    pin: Option<[f32; 2]>,
    tip: Option<[f32; 2]>,
}

/// Extracts `head1` silhouette boundary points in `800x800` space (`ct()` in `rig.js:414-425`).
pub fn extract_head_edge(head1: Option<&RgbaSprite>) -> Vec<[f32; 2]> {
    let Some(h1) = head1 else {
        return Vec::new();
    };
    if h1.width == 0 || h1.height == 0 {
        return Vec::new();
    }
    const E: u32 = 800;
    let mut canvas = vec![0u8; (E * E) as usize];
    let iw = h1.width as f32;
    let ih = h1.height as f32;
    let (dw, dh, dx, dy) = if iw >= ih {
        let dh = (E as f32 * ih / iw).round().max(1.0) as u32;
        (E, dh, 0u32, E.saturating_sub(dh))
    } else {
        let dw = (E as f32 * iw / ih).round().max(1.0) as u32;
        (dw, E, (E.saturating_sub(dw)) / 2, 0u32)
    };
    let resized = resize_premul_to(h1, dw, dh);
    for ry in 0..dh {
        let cy = dy + ry;
        if cy >= E {
            continue;
        }
        for rx in 0..dw {
            let cx = dx + rx;
            if cx >= E {
                continue;
            }
            let a = resized.pixels[((ry * dw + rx) * 4 + 3) as usize];
            canvas[(cy * E + cx) as usize] = a;
        }
    }

    let is_opaque = |x: i32, y: i32| -> bool {
        if x < 0 || y < 0 || x >= E as i32 || y >= E as i32 {
            return false;
        }
        canvas[(y as u32 * E + x as u32) as usize] > 128
    };

    let mut pts = Vec::new();
    for y in 0..E as i32 {
        for x in 0..E as i32 {
            if !is_opaque(x, y) {
                continue;
            }
            if !(is_opaque(x + 1, y)
                && is_opaque(x - 1, y)
                && is_opaque(x, y + 1)
                && is_opaque(x, y - 1))
            {
                pts.push([x as f32, y as f32]);
            }
        }
    }
    pts
}

pub fn build_prop_side(sprite: &RgbaSprite, max_w: u32) -> Option<PropSide> {
    let pw = sprite.width;
    let ph = sprite.height;
    if pw == 0 || ph == 0 {
        return None;
    }
    let max_dim = pw.max(ph) as f32;
    let limit = (max_w.max(16) as f32).min(1600.0);
    let scale = (limit / max_dim).min(1.0);
    let u = (max_dim * scale).round().max(8.0) as u32;
    let gw = (pw as f32 * scale).round().max(1.0) as u32;
    let gh = (ph as f32 * scale).round().max(1.0) as u32;

    let mut square = vec![0u8; (u * u * 4) as usize];
    let ox = ((u as i32 - gw as i32) / 2).max(0) as u32;
    let oy = ((u as i32 - gh as i32) / 2).max(0) as u32;
    let resized = resize_premul_to(sprite, gw, gh);
    blit_premul(&resized, &mut square, u, u, ox, oy, false);

    let mut min_x = u;
    let mut min_y = u;
    let mut max_x: i32 = -1;
    let mut max_y: i32 = -1;
    for y in 0..u {
        for x in 0..u {
            let a = square[((y * u + x) * 4 + 3) as usize];
            if a > 8 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x as i32);
                max_y = max_y.max(y as i32);
            }
        }
    }
    if max_x < 0 || max_y < 0 {
        return None;
    }

    let cw = (max_x as u32 - min_x) + 1;
    let ch = (max_y as u32 - min_y) + 1;
    let mut cropped = vec![0u8; (cw * ch * 4) as usize];
    for y in 0..ch {
        let src_off = (((min_y + y) * u + min_x) * 4) as usize;
        let dst_off = (y * cw * 4) as usize;
        cropped[dst_off..dst_off + (cw * 4) as usize]
            .copy_from_slice(&square[src_off..src_off + (cw * 4) as usize]);
    }
    let b = u as f32 / 800.0;
    Some(PropSide {
        sprite: RgbaSprite {
            width: cw,
            height: ch,
            pixels: cropped,
        },
        cx: (min_x as f32 + cw as f32 * 0.5) / b,
        cy: (min_y as f32 + ch as f32 * 0.5) / b,
        w: cw as f32 / b,
        h: ch as f32 / b,
    })
}

pub fn default_pins_for_part(part: &RigPartConfig) -> Vec<[f32; 2]> {
    let drop_flag = part.s.get("drop").and_then(|v| v.as_bool()).unwrap_or(false);
    match part.kind.as_str() {
        "ear" => {
            if drop_flag {
                vec![[196.0, 232.0], [604.0, 232.0]]
            } else {
                vec![[221.0, 198.0], [579.0, 198.0]]
            }
        }
        "wing" => vec![[350.0, 604.0], [450.0, 604.0]],
        _ => vec![[400.0, 650.0]],
    }
}

pub fn build_rig_mesh_side(
    part: &RigPartConfig,
    raw_sprite: &RgbaSprite,
    side_idx: usize,
    n_sides: usize,
    head_edge: &[[f32; 2]],
    rig_common: &RigCommonConfig,
    max_w: u32,
) -> Option<RigMeshSide> {
    let pw = raw_sprite.width;
    let ph = raw_sprite.height;
    if pw == 0 || ph == 0 {
        return None;
    }

    let split = part.s.get("split").and_then(|v| v.as_bool()).unwrap_or(false);
    let head_flag = part.s.get("head").and_then(|v| v.as_bool()).unwrap_or(false);
    let drop_flag = part.s.get("drop").and_then(|v| v.as_bool()).unwrap_or(false);
    let vtip_flag = part.s.get("vtip").and_then(|v| v.as_bool()).unwrap_or(false);

    let sub_kind = if part.kind == "tail" && head_flag {
        "ahoge"
    } else if part.kind == "ear" && drop_flag {
        "earDrop"
    } else if part.kind == "tail" && vtip_flag {
        "tailV"
    } else {
        part.kind.as_str()
    };

    let def_pins = default_pins_for_part(part);
    let base_pin = if sub_kind == "ahoge" {
        [430.0, 150.0]
    } else {
        def_pins.get(side_idx).copied().unwrap_or([400.0, 650.0])
    };
    let fixed = resolve_fixed_anchors(part, side_idx, n_sides);

    let max_dim = pw.max(ph) as f32;
    let limit = (max_w.max(16) as f32).min(1600.0);
    let scale = (limit / max_dim).min(1.0);
    let u = (max_dim * scale).round().max(8.0) as u32;
    let gw = (pw as f32 * scale).round().max(1.0) as u32;
    let gh = (ph as f32 * scale).round().max(1.0) as u32;

    let mut canvas = vec![0u8; (u * u * 4) as usize];
    let ox = ((u as i32 - gw as i32) / 2).max(0) as u32;
    let oy = ((u as i32 - gh as i32) / 2).max(0) as u32;
    let mirror = n_sides == 2 && side_idx == 0 && !split;
    let resized = resize_premul_to(raw_sprite, gw, gh);
    blit_premul(&resized, &mut canvas, u, u, ox, oy, mirror);

    if n_sides == 2 {
        let half = u / 2;
        if side_idx == 0 {
            for y in 0..u {
                let start = ((y * u + half) * 4) as usize;
                let end = ((y * u + u) * 4) as usize;
                canvas[start..end].fill(0);
            }
        } else {
            for y in 0..u {
                let start = (y * u * 4) as usize;
                let end = ((y * u + half) * 4) as usize;
                canvas[start..end].fill(0);
            }
        }
    }

    let s = u as f32 / 800.0;
    let edge_opt = if part.kind == "ear" { Some(head_edge) } else { None };

    let mut pin = if let Some(fp) = fixed.as_ref().and_then(|f| f.pin) {
        fp
    } else if let Some(edge) = edge_opt {
        snap_pin(&canvas, u, u, s, edge, base_pin).unwrap_or(base_pin)
    } else {
        base_pin
    };

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = -1i32;
    let mut max_y = -1i32;
    let mut tip = pin;
    let mut best_d2 = -1.0f32;

    for y in (0..u as i32).step_by(2) {
        for x in (0..u as i32).step_by(2) {
            let a = canvas[((y as u32 * u + x as u32) * 4 + 3) as usize];
            if a > 128 {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
                let px800 = x as f32 / s;
                let py800 = y as f32 / s;
                let d2 = (px800 - pin[0]).powi(2) + (py800 - pin[1]).powi(2);
                if d2 > best_d2 {
                    best_d2 = d2;
                    tip = [px800, py800];
                }
            }
        }
    }
    if max_x < 0 {
        return None;
    }

    if sub_kind == "ahoge" && fixed.as_ref().and_then(|f| f.pin).is_none() {
        let band = (0.06 * (max_y - min_y) as f32).round().max(4.0) as i32;
        let mut sx = 0.0f64;
        let mut cnt = 0usize;
        for y in (max_y - band).max(0)..=max_y {
            for x in 0..u as i32 {
                if canvas[((y as u32 * u + x as u32) * 4 + 3) as usize] > 128 {
                    sx += x as f64;
                    cnt += 1;
                }
            }
        }
        if cnt > 0 {
            pin = [
                (sx / cnt as f64 / s as f64).round() as f32,
                (max_y as f32 / s).round(),
            ];
            let mut bd2 = -1.0f32;
            for y in (0..u as i32).step_by(2) {
                for x in (0..u as i32).step_by(2) {
                    if canvas[((y as u32 * u + x as u32) * 4 + 3) as usize] <= 128 {
                        continue;
                    }
                    let px800 = x as f32 / s;
                    let py800 = y as f32 / s;
                    let d2 = (px800 - pin[0]).powi(2) + (py800 - pin[1]).powi(2);
                    if d2 > bd2 {
                        bd2 = d2;
                        tip = [px800, py800];
                    }
                }
            }
        }
    }

    if (sub_kind == "ear" || sub_kind == "earDrop") && fixed.as_ref().and_then(|f| f.tip).is_none()
    {
        let mut cx = 0.0f64;
        let mut cy = 0.0f64;
        let mut cnt = 0usize;
        for y in (0..u).step_by(3) {
            for x in (0..u).step_by(3) {
                if canvas[((y * u + x) * 4 + 3) as usize] > 128 {
                    cx += (x as f32 / s) as f64;
                    cy += (y as f32 / s) as f64;
                    cnt += 1;
                }
            }
        }
        if cnt > 0 {
            let cent_dir = ((cy / cnt as f64) as f32 - pin[1])
                .atan2((cx / cnt as f64) as f32 - pin[0]);
            let edge_cent = edge_opt.and_then(|ep| {
                if ep.len() >= 2 {
                    let mut ex = 0.0f64;
                    let mut ey = 0.0f64;
                    for p in ep {
                        ex += p[0] as f64;
                        ey += p[1] as f64;
                    }
                    Some([
                        (ex / ep.len() as f64) as f32,
                        (ey / ep.len() as f64) as f32,
                    ])
                } else {
                    None
                }
            });
            let ec_dist = edge_cent
                .map(|c| (pin[0] - c[0]).hypot(pin[1] - c[1]))
                .unwrap_or(0.0);
            let mut best_score = -1.0f32;
            for y in (0..u as i32).step_by(2) {
                for x in (0..u as i32).step_by(2) {
                    if canvas[((y as u32 * u + x as u32) * 4 + 3) as usize] <= 128 {
                        continue;
                    }
                    let px = x as f32 / s;
                    let py = y as f32 / s;
                    let dx = px - pin[0];
                    let dy = py - pin[1];
                    let mut score =
                        dx.hypot(dy) * (0.45 + 0.55 * (dy.atan2(dx) - cent_dir).cos());
                    if let Some(ec) = edge_cent {
                        if (px - ec[0]).hypot(py - ec[1]) < ec_dist {
                            score *= 0.25;
                        }
                    }
                    if score > best_score {
                        best_score = score;
                        tip = [px, py];
                    }
                }
            }
        }
    }

    if sub_kind == "tailV" && fixed.as_ref().and_then(|f| f.tip).is_none() {
        let up = tip[1] < pin[1];
        let col = (pin[0] * s).round() as i32;
        let mut best_dy = -1.0f32;
        let mut v_tip = None;
        for y in 0..u as i32 {
            for x in (col - 2)..=(col + 2) {
                if x < 0 || x >= u as i32 {
                    continue;
                }
                if canvas[((y as u32 * u + x as u32) * 4 + 3) as usize] <= 128 {
                    continue;
                }
                let dy = if up {
                    pin[1] - y as f32 / s
                } else {
                    y as f32 / s - pin[1]
                };
                if dy > best_dy {
                    best_dy = dy;
                    v_tip = Some([pin[0], y as f32 / s]);
                }
            }
        }
        if let Some(vt) = v_tip {
            if best_dy >= 0.6 * (tip[0] - pin[0]).hypot(tip[1] - pin[1]) {
                tip = vt;
            }
        }
    }

    if let Some(ft) = fixed.as_ref().and_then(|f| f.tip) {
        tip = ft;
    }

    let len = (tip[0] - pin[0]).hypot(tip[1] - pin[1]).max(40.0);
    let dir = (tip[1] - pin[1]).atan2(tip[0] - pin[0]);

    // Build indexed triangle mesh BEFORE optional texture blur (matching `rig.js:507,639`)
    let mesh_step_px = ((rig_common.mesh as f32) * s).round().max(4.0) as i32;
    let gx0 = (((min_x - mesh_step_px).max(0) / mesh_step_px) * mesh_step_px) as u32;
    let gy0 = (((min_y - mesh_step_px).max(0) / mesh_step_px) * mesh_step_px) as u32;
    let step_u = mesh_step_px as u32;
    let cells_x = ((u.min((max_x + mesh_step_px).max(0) as u32) - gx0) as f32 / step_u as f32)
        .ceil()
        .max(0.0) as u32;
    let cells_y = ((u.min((max_y + mesh_step_px).max(0) as u32) - gy0) as f32 / step_u as f32)
        .ceil()
        .max(0.0) as u32;

    let cell_has_alpha = |cx: u32, cy: u32| -> bool {
        let x0 = (gx0 + cx * step_u).saturating_sub(3);
        let y0 = (gy0 + cy * step_u).saturating_sub(3);
        let x1 = (gx0 + (cx + 1) * step_u + 3).min(u);
        let y1 = (gy0 + (cy + 1) * step_u + 3).min(u);
        for y in (y0..y1).step_by(2) {
            for x in (x0..x1).step_by(2) {
                if canvas[((y * u + x) * 4 + 3) as usize] > 4 {
                    return true;
                }
            }
        }
        false
    };

    let (dirc, dirs) = (dir.cos(), dir.sin());
    let mut vert_map: HashMap<(u32, u32), u32> = HashMap::new();
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let mut get_vert = |vx: u32, vy: u32| -> u32 {
        if let Some(&idx) = vert_map.get(&(vx, vy)) {
            return idx;
        }
        let px = gx0 as f32 + vx as f32 * step_u as f32;
        let py = gy0 as f32 + vy as f32 * step_u as f32;
        let rx = px / s;
        let ry = py / s;
        let t_param = ((rx - pin[0]) * dirc + (ry - pin[1]) * dirs) / len;
        let idx = vertices.len() as u32;
        vertices.push(RigVertex {
            rest_pos: [rx, ry],
            uv: [px / u as f32, py / u as f32],
            t_param,
        });
        vert_map.insert((vx, vy), idx);
        idx
    };

    for cy in 0..cells_y {
        for cx in 0..cells_x {
            if !cell_has_alpha(cx, cy) {
                continue;
            }
            let n = get_vert(cx, cy);
            let r = get_vert(cx + 1, cy);
            let i = get_vert(cx + 1, cy + 1);
            let a = get_vert(cx, cy + 1);
            indices.extend_from_slice(&[n, r, i, n, i, a]);
        }
    }

    if indices.is_empty() {
        return None;
    }

    // Apply optional one-time texture softness blur matching `rig.js:639` (`blur(tex * s / 2 px)`)
    let tex_soft = (rig_common.tex as f32) * s * 0.5;
    if tex_soft >= 0.25 {
        blur_premul_in_place(&mut canvas, u, u, tex_soft);
    }

    Some(RigMeshSide {
        sprite: RgbaSprite {
            width: u,
            height: u,
            pixels: canvas,
        },
        vertices,
        indices,
        pin,
        tip,
        len,
        dir,
    })
}

fn resolve_fixed_anchors(
    part: &RigPartConfig,
    side_idx: usize,
    n_sides: usize,
) -> Option<FixedAnchors> {
    let split = part.s.get("split").and_then(|v| v.as_bool()).unwrap_or(false);
    if split && n_sides == 2 && side_idx == 0 {
        let pin = parse_vec2(part.s.get("pinAtL"));
        let tip = parse_vec2(part.s.get("tipAtL"));
        return if pin.is_some() || tip.is_some() {
            Some(FixedAnchors { pin, tip })
        } else {
            None
        };
    }
    let pin = parse_vec2(part.s.get("pinAt"));
    let tip = parse_vec2(part.s.get("tipAt"));
    if pin.is_none() && tip.is_none() {
        return None;
    }
    let xform = |p: Option<[f32; 2]>| -> Option<[f32; 2]> {
        p.map(|v| {
            if n_sides == 2 && side_idx == 0 {
                [800.0 - v[0], v[1]]
            } else {
                v
            }
        })
    };
    Some(FixedAnchors {
        pin: xform(pin),
        tip: xform(tip),
    })
}

fn parse_vec2(val: Option<&serde_json::Value>) -> Option<[f32; 2]> {
    let arr = val?.as_array()?;
    if arr.len() != 2 {
        return None;
    }
    let x = arr[0].as_f64()? as f32;
    let y = arr[1].as_f64()? as f32;
    if x.is_finite() && y.is_finite() {
        Some([x, y])
    } else {
        None
    }
}

fn snap_pin(
    rgba: &[u8],
    w: u32,
    h: u32,
    s: f32,
    edge: &[[f32; 2]],
    default_pin: [f32; 2],
) -> Option<[f32; 2]> {
    if edge.is_empty() {
        return None;
    }
    let mut overlap = Vec::new();
    for &[ex, ey] in edge {
        let px = (ex * s).round() as i32;
        let py = (ey * s).round() as i32;
        if px >= 0 && px < w as i32 && py >= 0 && py < h as i32 {
            if rgba[((py as u32 * w + px as u32) * 4 + 3) as usize] > 128 {
                overlap.push([ex, ey]);
            }
        }
    }
    if overlap.len() < 8 {
        return None;
    }
    let mut cent = [0.0f64; 2];
    for p in &overlap {
        cent[0] += p[0] as f64;
        cent[1] += p[1] as f64;
    }
    cent[0] /= overlap.len() as f64;
    cent[1] /= overlap.len() as f64;

    let farthest = |target: [f64; 2]| -> [f32; 2] {
        let mut best = overlap[0];
        let mut best_d2 = -1.0f64;
        for &p in &overlap {
            let d2 = (p[0] as f64 - target[0]).powi(2) + (p[1] as f64 - target[1]).powi(2);
            if d2 > best_d2 {
                best_d2 = d2;
                best = p;
            }
        }
        best
    };

    let c = farthest(cent);
    let p = farthest([c[0] as f64, c[1] as f64]);
    let mid = [(c[0] + p[0]) * 0.5, (c[1] + p[1]) * 0.5];
    if (mid[0] - default_pin[0]).hypot(mid[1] - default_pin[1]) > 90.0
        || (p[0] - c[0]).hypot(p[1] - c[1]) < 20.0
    {
        return None;
    }
    Some([mid[0].round(), mid[1].round()])
}

fn resize_premul_to(sprite: &RgbaSprite, dst_w: u32, dst_h: u32) -> RgbaSprite {
    if sprite.width == dst_w && sprite.height == dst_h {
        return sprite.clone();
    }
    let src_buf =
        ImageBuffer::<Rgba<u8>, _>::from_raw(sprite.width, sprite.height, sprite.pixels.as_slice())
            .expect("valid RgbaSprite buffer");
    let out = resize(&src_buf, dst_w, dst_h, FilterType::Triangle);
    RgbaSprite {
        width: dst_w,
        height: dst_h,
        pixels: out.into_raw(),
    }
}

fn blit_premul(
    src: &RgbaSprite,
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    ox: u32,
    oy: u32,
    mirror_x: bool,
) {
    for sy in 0..src.height {
        let dy = oy + sy;
        if dy >= dst_h {
            continue;
        }
        for sx in 0..src.width {
            let rx = if mirror_x { src.width - 1 - sx } else { sx };
            let dx = ox + rx;
            if dx >= dst_w {
                continue;
            }
            let si = ((sy * src.width + sx) * 4) as usize;
            let di = ((dy * dst_w + dx) * 4) as usize;
            dst[di..di + 4].copy_from_slice(&src.pixels[si..si + 4]);
        }
    }
}

/// Fast tiny Gaussian-like separable 3-tap/5-tap blur on premultiplied RGBA8 pixels for `rigCommon.tex`.
fn blur_premul_in_place(pixels: &mut [u8], w: u32, h: u32, sigma: f32) {
    let rad = (sigma * 2.0).ceil().clamp(1.0, 4.0) as i32;
    let mut kernel = Vec::with_capacity((rad * 2 + 1) as usize);
    let mut sum = 0.0f32;
    let s2 = 2.0 * sigma * sigma;
    for d in -rad..=rad {
        let w = (-((d * d) as f32) / s2.max(0.05)).exp();
        kernel.push(w);
        sum += w;
    }
    for k in &mut kernel {
        *k /= sum;
    }

    let mut tmp = vec![0.0f32; pixels.len()];
    let wi = w as i32;
    let hi = h as i32;

    // Horizontal pass
    for y in 0..hi {
        for x in 0..wi {
            let mut acc = [0.0f32; 4];
            for (ki, &kw) in kernel.iter().enumerate() {
                let sx = (x + ki as i32 - rad).clamp(0, wi - 1) as u32;
                let si = ((y as u32 * w + sx) * 4) as usize;
                acc[0] += pixels[si] as f32 * kw;
                acc[1] += pixels[si + 1] as f32 * kw;
                acc[2] += pixels[si + 2] as f32 * kw;
                acc[3] += pixels[si + 3] as f32 * kw;
            }
            let di = ((y as u32 * w + x as u32) * 4) as usize;
            tmp[di..di + 4].copy_from_slice(&acc);
        }
    }

    // Vertical pass
    for y in 0..hi {
        for x in 0..wi {
            let mut acc = [0.0f32; 4];
            for (ki, &kw) in kernel.iter().enumerate() {
                let sy = (y + ki as i32 - rad).clamp(0, hi - 1) as u32;
                let si = ((sy * w + x as u32) * 4) as usize;
                acc[0] += tmp[si] * kw;
                acc[1] += tmp[si + 1] * kw;
                acc[2] += tmp[si + 2] * kw;
                acc[3] += tmp[si + 3] * kw;
            }
            let di = ((y as u32 * w + x as u32) * 4) as usize;
            let a = acc[3].round().clamp(0.0, 255.0) as u8;
            pixels[di + 3] = a;
            pixels[di] = acc[0].round().clamp(0.0, a as f32) as u8;
            pixels[di + 1] = acc[1].round().clamp(0.0, a as f32) as u8;
            pixels[di + 2] = acc[2].round().clamp(0.0, a as f32) as u8;
        }
    }
}

/// Pre-bakes CSS `.neck-sh` (`head1` blurred shadow clipped inside `body` alpha) strictly once at load time!
pub fn build_neck_shadow(
    body: Option<&RgbaSprite>,
    head1: Option<&RgbaSprite>,
    neck_depth: f64,
    neck_color: &str,
    neck_alpha: f64,
) -> Option<RgbaSprite> {
    let (Some(b), Some(h)) = (body, head1) else {
        return None;
    };
    if neck_depth <= 0.0 || neck_alpha <= 0.0 {
        return None;
    }
    const S: u32 = 400;
    let b_res = resize_contain_bottom(b, S);
    let h_res = resize_contain_bottom(h, S);

    // Shift head alpha mask down by neck_depth * (S / 200) and blur by 0.9 * depth * (S / 200) * 0.5
    let scale = S as f32 / 200.0;
    let dy = (neck_depth as f32 * scale).round() as i32;
    let blur_sig = (0.9 * neck_depth as f32 * scale * 0.5).max(0.5);

    let mut mask = vec![0u8; (S * S * 4) as usize];
    for y in 0..S as i32 {
        let sy = y - dy;
        if sy < 0 || sy >= S as i32 {
            continue;
        }
        for x in 0..S as i32 {
            let si = ((sy as u32 * S + x as u32) * 4 + 3) as usize;
            let a = h_res.pixels[si];
            let di = ((y as u32 * S + x as u32) * 4) as usize;
            mask[di] = a;
            mask[di + 1] = a;
            mask[di + 2] = a;
            mask[di + 3] = a;
        }
    }
    blur_premul_in_place(&mut mask, S, S, blur_sig);

    let col = crate::config::parse_hex_color(neck_color, Some(neck_alpha));
    for i in 0..(S * S) as usize {
        let idx = i * 4;
        let ba = b_res.pixels[idx + 3] as f32 / 255.0;
        let ha = mask[idx + 3] as f32 / 255.0;
        let out_a = (ba * ha * col[3]).clamp(0.0, 1.0);
        mask[idx] = ((col[0] * out_a) * 255.0).round() as u8;
        mask[idx + 1] = ((col[1] * out_a) * 255.0).round() as u8;
        mask[idx + 2] = ((col[2] * out_a) * 255.0).round() as u8;
        mask[idx + 3] = (out_a * 255.0).round() as u8;
    }
    Some(RgbaSprite {
        width: S,
        height: S,
        pixels: mask,
    })
}

fn resize_contain_bottom(sprite: &RgbaSprite, sq: u32) -> RgbaSprite {
    let mut canvas = vec![0u8; (sq * sq * 4) as usize];
    let aspect = if sprite.height > 0 {
        sprite.width as f32 / sprite.height as f32
    } else {
        1.0
    };
    let (dw, dh) = if aspect >= 1.0 {
        (sq, (sq as f32 / aspect).round().max(1.0) as u32)
    } else {
        ((sq as f32 * aspect).round().max(1.0) as u32, sq)
    };
    let ox = (sq.saturating_sub(dw)) / 2;
    let oy = sq.saturating_sub(dh);
    let res = resize_premul_to(sprite, dw, dh);
    blit_premul(&res, &mut canvas, sq, sq, ox, oy, false);
    RgbaSprite {
        width: sq,
        height: sq,
        pixels: canvas,
    }
}

#[allow(dead_code)]
fn _keep_frac_pi_2() -> f32 {
    FRAC_PI_2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::load_optional_sprite;
    use crate::config::{discover_data_dir, load_character_config};
    use crate::protect::ProtectKey;

    #[test]
    fn builds_live_meshes_for_installed_characters() {
        let Ok(dir) = discover_data_dir(None) else {
            return;
        };
        let pk = ProtectKey::init(&dir).unwrap();
        let Ok(entries) = std::fs::read_dir(dir.join("characters")) else {
            return;
        };
        for entry in entries.flatten() {
            let cdir = entry.path();
            if !cdir.is_dir() || !cdir.join("character.json").is_file() {
                continue;
            }
            let Ok(cfg) = load_character_config(&cdir) else {
                continue;
            };
            if !cfg.rig.iter().any(|p| p.on && !p.img.is_empty()) {
                continue;
            }
            let needs_key = cfg.head1.trim().to_ascii_lowercase().ends_with(".bcp")
                || cfg
                    .rig
                    .iter()
                    .any(|p| p.on && p.img.trim().to_ascii_lowercase().ends_with(".bcp"));
            if !pk.has_key() && needs_key {
                continue;
            }
            let ch = cdir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("character");
            let head1 = load_optional_sprite(&cdir, &cfg.head1, &pk);
            if !cfg.head1.trim().is_empty() && head1.is_none() {
                continue;
            }
            let head_edge = extract_head_edge(head1.as_ref());
            if head1.is_some() {
                assert!(!head_edge.is_empty(), "{ch}: expected non-empty head_edge");
            }

            let mut loaded_parts = 0usize;
            let mut built_parts = 0usize;
            for part in &cfg.rig {
                if !part.on || part.img.is_empty() {
                    continue;
                }
                let Some(sp) = load_optional_sprite(&cdir, &part.img, &pk) else {
                    continue;
                };
                loaded_parts += 1;
                if part.kind == "prop" {
                    let prop = build_prop_side(&sp, 576).expect("prop must build");
                    assert!(prop.w > 0.0 && prop.h > 0.0);
                    built_parts += 1;
                } else {
                    let n_sides = if part.kind == "tail" { 1 } else { 2 };
                    let mut got_side = false;
                    for si in 0..n_sides {
                        if let Some(m) = build_rig_mesh_side(
                            part,
                            &sp,
                            si,
                            n_sides,
                            &head_edge,
                            &cfg.rig_common,
                            576,
                        ) {
                            assert!(!m.vertices.is_empty(), "{ch}/{} side {si} empty verts", part.id);
                            assert!(!m.indices.is_empty(), "{ch}/{} side {si} empty idx", part.id);
                            assert!(m.len >= 40.0);
                            got_side = true;
                        }
                    }
                    assert!(got_side, "{ch}/{} produced no sides", part.id);
                    built_parts += 1;
                }
            }
            if loaded_parts > 0 {
                assert!(
                    built_parts > 0,
                    "{ch}: expected non-empty meshes for {loaded_parts} loaded rig parts"
                );
            }
        }
    }
}
