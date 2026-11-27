use crate::config::{load_character_config, CharacterConfig};
use crate::protect::ProtectKey;
use image::imageops::FilterType;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct RgbaSprite {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8 pixel buffer
    pub pixels: Vec<u8>,
}

/// 52-column alpha hit-test bitmap matching `pet.js:459-486` and `hittest.js`.
#[derive(Debug, Clone)]
pub struct HitMask {
    pub gw: usize,
    pub gh: usize,
    pub bits: Vec<u8>,
}

impl HitMask {
    /// Coordinates `(nx, ny)` normalized across the `[0, 1] x [0, 1]` pet core box.
    pub fn test_normalized(&self, nx: f64, ny: f64) -> bool {
        if nx < 0.0 || nx >= 1.0 || ny < 0.0 || ny >= 1.0 || self.gw == 0 || self.gh == 0 {
            return false;
        }
        let gx = ((nx * self.gw as f64).floor() as usize).min(self.gw - 1);
        let gy = ((ny * self.gh as f64).floor() as usize).min(self.gh - 1);
        self.bits[gy * self.gw + gx] == 1
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&b| b == 0)
    }
}

pub struct LoadedCharacter {
    #[allow(dead_code)]
    pub char_id: String,
    pub config: CharacterConfig,
    pub body: Option<RgbaSprite>,
    pub body2: Option<RgbaSprite>,
    pub heads: [Option<RgbaSprite>; 5],
    pub neck_shadow: Option<RgbaSprite>,
    pub hit_mask: HitMask,
    pub visible_bounds: [f64; 4],
}

impl LoadedCharacter {
    pub fn load(
        data_dir: &Path,
        char_id: &str,
        protect: &ProtectKey,
    ) -> Result<Self, String> {
        let char_dir = data_dir.join("characters").join(char_id);
        let config = load_character_config(&char_dir)?;

        let body = load_optional_sprite(&char_dir, &config.body, protect);
        let body2 = load_optional_sprite(&char_dir, &config.body2, protect);
        let heads = [
            load_optional_sprite(&char_dir, &config.head1, protect),
            load_optional_sprite(&char_dir, &config.head2, protect),
            load_optional_sprite(&char_dir, &config.head3, protect),
            load_optional_sprite(&char_dir, &config.head4, protect),
            load_optional_sprite(&char_dir, &config.head5, protect),
        ];

        // Collect active rig sprites briefly to build a complete click-through mask
        let mut owned_rig_masks: Vec<RgbaSprite> = Vec::new();
        for part in &config.rig {
            if part.on && !part.img.is_empty() {
                if let Some(sp) = load_optional_sprite(&char_dir, &part.img, protect) {
                    owned_rig_masks.push(sp);
                }
            }
        }

        let hit_mask = build_hit_mask(&body, &heads[0], &owned_rig_masks);
        let visible_bounds = compute_visible_bounds(&body, &heads[0], &owned_rig_masks);
        let neck_shadow = if config.neck_on {
            crate::rig::mesh::build_neck_shadow(
                body.as_ref(),
                heads[0].as_ref(),
                config.neck_depth,
                &config.neck_color,
                config.neck_alpha,
            )
        } else {
            None
        };

        Ok(Self {
            char_id: char_id.to_string(),
            config,
            body,
            body2,
            heads,
            neck_shadow,
            hit_mask,
            visible_bounds,
        })
    }
}

/// Computes normalized `[x0, y0, x1, y1]` visible bounds across character sprites (`visibleBounds` in `pet.js:424-449`).
pub fn compute_visible_bounds(
    body: &Option<RgbaSprite>,
    head1: &Option<RgbaSprite>,
    rig_parts: &[RgbaSprite],
) -> [f64; 4] {
    const G: usize = 96;
    let mut x0 = G;
    let mut y0 = G;
    let mut x1: isize = -1;
    let mut y1: isize = -1;

    let mut scan = |sp: &RgbaSprite| {
        if sp.width == 0 || sp.height == 0 {
            return;
        }
        let aspect = sp.width as f64 / sp.height as f64;
        let (bw, bh) = if aspect >= 1.0 {
            (1.0, 1.0 / aspect)
        } else {
            (aspect, 1.0)
        };
        let ox = (1.0 - bw) * 0.5;
        let oy = 1.0 - bh;
        for gy in 0..G {
            let ny = (gy as f64 + 0.5) / G as f64;
            if ny < oy || ny >= oy + bh {
                continue;
            }
            let py = (((ny - oy) / bh) * sp.height as f64).floor() as u32;
            let py = py.min(sp.height - 1);
            for gx in 0..G {
                let nx = (gx as f64 + 0.5) / G as f64;
                if nx < ox || nx >= ox + bw {
                    continue;
                }
                let px = (((nx - ox) / bw) * sp.width as f64).floor() as u32;
                let px = px.min(sp.width - 1);
                let idx = ((py * sp.width + px) * 4 + 3) as usize;
                if sp.pixels.get(idx).copied().unwrap_or(0) > 12 {
                    x0 = x0.min(gx);
                    y0 = y0.min(gy);
                    x1 = x1.max(gx as isize);
                    y1 = y1.max(gy as isize);
                }
            }
        }
    };

    if let Some(b) = body {
        scan(b);
    }
    if let Some(h) = head1 {
        scan(h);
    }
    for r in rig_parts {
        scan(r);
    }

    if x1 < 0 || y1 < 0 {
        [0.0, 0.0, 1.0, 1.0]
    } else {
        [
            x0 as f64 / G as f64,
            y0 as f64 / G as f64,
            (x1 as f64 + 1.0) / G as f64,
            (y1 as f64 + 1.0) / G as f64,
        ]
    }
}

pub fn load_optional_sprite(
    char_dir: &Path,
    filename: &str,
    protect: &ProtectKey,
) -> Option<RgbaSprite> {
    let trimmed = filename.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = char_dir.join(trimmed);
    if !path.is_file() {
        return None;
    }
    let raw = fs::read(&path).ok()?;
    let img_bytes = if trimmed.to_ascii_lowercase().ends_with(".bcp") {
        protect.open_bcp(&raw).ok()?
    } else {
        raw
    };
    let dyn_img = image::load_from_memory(&img_bytes).ok()?;
    let rgba = dyn_img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut pixels = rgba.into_raw();

    // Premultiply RGB by alpha cleanly for DirectComposition & D3D11 ONE / INV_SRC_ALPHA blending
    for px in pixels.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a < 255 {
            px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
            px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
            px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
        }
    }

    Some(RgbaSprite {
        width: w,
        height: h,
        pixels,
    })
}

pub fn downscale_if_larger(sprite: RgbaSprite, max_dim: u32) -> RgbaSprite {
    let cur = sprite.width.max(sprite.height);
    if cur <= max_dim || max_dim < 16 {
        return sprite;
    }
    let scale = max_dim as f32 / cur as f32;
    let nw = (sprite.width as f32 * scale).round().max(1.0) as u32;
    let nh = (sprite.height as f32 * scale).round().max(1.0) as u32;
    if let Some(buf) =
        image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(sprite.width, sprite.height, sprite.pixels)
    {
        let resized = image::imageops::resize(&buf, nw, nh, FilterType::Triangle);
        RgbaSprite {
            width: nw,
            height: nh,
            pixels: resized.into_raw(),
        }
    } else {
        unreachable!()
    }
}

/// Builds a 52x52 dilated hit mask from base sprites (`body`, `head1`, and active `rig` parts).
fn build_hit_mask(
    body: &Option<RgbaSprite>,
    head1: &Option<RgbaSprite>,
    rig_parts: &[RgbaSprite],
) -> HitMask {
    const GW: usize = 52;
    const GH: usize = 52;
    let mut raw = vec![0u8; GW * GH];

    let sample_sprite = |sp: &RgbaSprite, out: &mut [u8]| {
        if sp.width == 0 || sp.height == 0 {
            return;
        }
        // `object-fit: contain; object-position: 50% 100%` mapping into [0,1]x[0,1] core box
        let aspect = sp.width as f64 / sp.height as f64;
        let (box_w, box_h) = if aspect >= 1.0 {
            (1.0, 1.0 / aspect)
        } else {
            (aspect, 1.0)
        };
        let off_x = (1.0 - box_w) * 0.5;
        let off_y = 1.0 - box_h;

        // Downsample quickly using box/point alpha check within each cell
        for gy in 0..GH {
            let ny0 = (gy as f64) / (GH as f64);
            let ny1 = ((gy + 1) as f64) / (GH as f64);
            if ny1 <= off_y || ny0 >= off_y + box_h {
                continue;
            }
            for gx in 0..GW {
                if out[gy * GW + gx] == 1 {
                    continue;
                }
                let nx0 = (gx as f64) / (GW as f64);
                let nx1 = ((gx + 1) as f64) / (GW as f64);
                if nx1 <= off_x || nx0 >= off_x + box_w {
                    continue;
                }
                let ux = (((nx0 + nx1) * 0.5 - off_x) / box_w).clamp(0.0, 0.9999);
                let uy = (((ny0 + ny1) * 0.5 - off_y) / box_h).clamp(0.0, 0.9999);

                let px0 = ((nx0 - off_x) / box_w * sp.width as f64).floor().max(0.0) as u32;
                let py0 = ((ny0 - off_y) / box_h * sp.height as f64).floor().max(0.0) as u32;
                let px1 = ((nx1 - off_x) / box_w * sp.width as f64)
                    .ceil()
                    .clamp(px0 as f64 + 1.0, sp.width as f64) as u32;
                let py1 = ((ny1 - off_y) / box_h * sp.height as f64)
                    .ceil()
                    .clamp(py0 as f64 + 1.0, sp.height as f64) as u32;

                let step_x = ((px1 - px0) / 4).max(1);
                let step_y = ((py1 - py0) / 4).max(1);
                let mut hit = false;
                let mut sy = py0;
                while sy < py1 && !hit {
                    let mut sx = px0;
                    while sx < px1 {
                        let idx = ((sy * sp.width + sx) * 4 + 3) as usize;
                        if sp.pixels[idx] > 12 {
                            hit = true;
                            break;
                        }
                        sx += step_x;
                    }
                    sy += step_y;
                }
                if !hit {
                    let cx = (ux * sp.width as f64) as u32;
                    let cy = (uy * sp.height as f64) as u32;
                    let idx = ((cy * sp.width + cx) * 4 + 3) as usize;
                    if sp.pixels.get(idx).copied().unwrap_or(0) > 12 {
                        hit = true;
                    }
                }
                if hit {
                    out[gy * GW + gx] = 1;
                }
            }
        }
    };

    if let Some(b) = body {
        sample_sprite(b, &mut raw);
    }
    if let Some(h) = head1 {
        sample_sprite(h, &mut raw);
    }
    for r in rig_parts {
        sample_sprite(r, &mut raw);
    }

    // 3x3 dilation by 1 cell matching `pet.js:476-484`
    let mut dilated = vec![0u8; GW * GH];
    for y in 0..GH {
        for x in 0..GW {
            if raw[y * GW + x] == 0 {
                continue;
            }
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && nx < GW as i32 && ny >= 0 && ny < GH as i32 {
                        dilated[ny as usize * GW + nx as usize] = 1;
                    }
                }
            }
        }
    }

    HitMask {
        gw: GW,
        gh: GH,
        bits: dilated,
    }
}
