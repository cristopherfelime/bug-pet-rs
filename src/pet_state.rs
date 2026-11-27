use crate::assets::{downscale_if_larger, HitMask, LoadedCharacter};
use crate::bubble::{
    fill_memo, rasterize_bubble, rasterize_remind_badge, resolve_remind_pool, style_of_line,
    BubbleFrameState, BubbleRuntime, CharBox, DipRect, FontStack, ResolvedBubbleStyle,
};
use crate::config::{
    parse_hex_color, parse_hex_color_rgba, BugGeometry, CharacterConfig, PetState, UiPrefs,
};
use crate::motion::{HoldOutputs, HoldPhysics, ReactorState, ShiverState};
use crate::pet_fx::{PetAreaMask, PetFxState, PetParticleAtlas, RectDip, RenderParticle};
use crate::radial::RadialMenuState;
use crate::remind::{RemindFireEvent, RemindKind};
use crate::renderer::{GpuContext, GpuTexture, PetSurface, SpriteCB};
use crate::rig::{build_and_upload_rig, CharacterRigBundle, RigRuntime, RigZone};
use rand::Rng;
use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::path::Path;
use std::sync::Arc;

/// 2D affine matrix `[a, c, tx, b, d, ty]` mapping unit quad `[0..1, 0..1]`
/// (or points) into pet `800x800` coordinate space:
/// `x' = a * x + c * y + tx`, `y' = b * x + d * y + ty`.
#[derive(Clone, Copy, Debug)]
pub struct Mat2D {
    pub m: [f32; 6],
}

impl Mat2D {
    pub fn identity() -> Self {
        Self {
            m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        }
    }

    pub fn mul(self, rhs: Self) -> Self {
        let [a1, c1, tx1, b1, d1, ty1] = self.m;
        let [a2, c2, tx2, b2, d2, ty2] = rhs.m;
        Self {
            m: [
                a1 * a2 + c1 * b2,
                a1 * c2 + c1 * d2,
                a1 * tx2 + c1 * ty2 + tx1,
                b1 * a2 + d1 * b2,
                b1 * c2 + d1 * d2,
                b1 * tx2 + d1 * ty2 + ty1,
            ],
        }
    }

    pub fn translate(tx: f32, ty: f32) -> Self {
        Self {
            m: [1.0, 0.0, tx, 0.0, 1.0, ty],
        }
    }

    pub fn scale_about(sx: f32, sy: f32, px: f32, py: f32) -> Self {
        Self {
            m: [sx, 0.0, px * (1.0 - sx), 0.0, sy, py * (1.0 - sy)],
        }
    }

    pub fn rotate_about(rad: f32, px: f32, py: f32) -> Self {
        let (s, c) = rad.sin_cos();
        Self {
            m: [c, -s, px - c * px + s * py, s, c, py - s * px - c * py],
        }
    }

    pub fn unit_to_rect(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            m: [w, 0.0, x, 0.0, h, y],
        }
    }

    pub fn contain_bottom_center(tex_w: u32, tex_h: u32) -> Self {
        let aspect = if tex_h > 0 {
            tex_w as f32 / tex_h as f32
        } else {
            1.0
        };
        let (w, h) = if aspect >= 1.0 {
            (800.0, 800.0 / aspect)
        } else {
            (800.0 * aspect, 800.0)
        };
        let x = (800.0 - w) * 0.5;
        let y = 800.0 - h;
        Self::unit_to_rect(x, y, w, h)
    }
}

pub struct CharacterGpuTextures {
    pub config: CharacterConfig,
    pub hit_mask: HitMask,
    pub visible_bounds: [f64; 4],
    pub pet_area_mask: PetAreaMask,
    pub fonts: Arc<FontStack>,
    pub rig_bundle: Arc<CharacterRigBundle>,
    pub body: Option<GpuTexture>,
    pub body2: Option<GpuTexture>,
    pub heads: [Option<GpuTexture>; 5],
    pub neck_shadow: Option<GpuTexture>,
}

impl CharacterGpuTextures {
    pub fn upload(
        gpu: &GpuContext,
        data_dir: &Path,
        char_id: &str,
        protect: &crate::protect::ProtectKey,
        mut loaded: LoadedCharacter,
        max_w: u32,
    ) -> Result<Self, String> {
        let char_dir = data_dir.join("characters").join(char_id);
        let rig_bundle = Arc::new(build_and_upload_rig(
            gpu,
            &char_dir,
            &loaded.config,
            loaded.heads[0].as_ref(),
            protect,
            max_w,
        )?);
        let pet_area_mask = PetAreaMask::build(
            loaded.heads[0].as_ref(),
            loaded.body.as_ref(),
            loaded.config.pet_cut,
        );
        let fonts = Arc::new(FontStack::new(&char_dir, &loaded.config.font_file));

        let sprite_max = (max_w * 4 / 3).clamp(512, 1024);
        let upload_opt = |opt: Option<crate::assets::RgbaSprite>| -> Result<Option<GpuTexture>, String> {
            opt.map(|sp| {
                let scaled = downscale_if_larger(sp, sprite_max);
                GpuTexture::from_sprite(gpu, &scaled)
            })
            .transpose()
        };

        let body = upload_opt(loaded.body.take())?;
        let body2 = upload_opt(loaded.body2.take())?;
        let heads = [
            upload_opt(loaded.heads[0].take())?,
            upload_opt(loaded.heads[1].take())?,
            upload_opt(loaded.heads[2].take())?,
            upload_opt(loaded.heads[3].take())?,
            upload_opt(loaded.heads[4].take())?,
        ];
        let neck_shadow = upload_opt(loaded.neck_shadow.take())?;

        Ok(Self {
            config: loaded.config,
            hit_mask: loaded.hit_mask,
            visible_bounds: loaded.visible_bounds,
            pet_area_mask,
            fonts,
            rig_bundle,
            body,
            body2,
            heads,
            neck_shadow,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PendingSayText {
    pub text: String,
    pub min_ms: Option<f64>,
    pub style_pool_key: Option<String>,
    pub style_src: Option<String>,
}

pub struct ActiveRemind {
    pub event: RemindFireEvent,
    pub shown_at_s: f64,
    pub tex: Option<GpuTexture>,
    pub tex_dpr: f64,
    pub pad_l: f32,
    pub pad_t: f32,
    pub box_w_dip: f64,
    pub box_h_dip: f64,
    pub hot_rect_local_dip: (f64, f64, f64, f64),
    pub is_icon_only: bool,
    pub before_pool_key: Option<String>,
    pub before_raw_line: Option<String>,
    pub before_display_text: Option<String>,
}

pub struct PetRuntime {
    pub id: String,
    pub state: PetState,
    pub ui_prefs: UiPrefs,
    pub geom: BugGeometry,
    pub margin_frac: f64,
    pub win_w_dip: f64,
    pub win_h_dip: f64,
    pub gpu_chars: Arc<CharacterGpuTextures>,
    pub hit_mask: HitMask,
    pub rig: RigRuntime,
    pub hold: HoldPhysics,
    pub shiver: ShiverState,
    pub reactor: ReactorState,

    // Milestone 4 interactive features
    pub pet_fx: PetFxState,
    pub pet_atlas: PetParticleAtlas,
    pub active_particles: Vec<RenderParticle>,
    pub bubble: BubbleRuntime,
    pub radial: RadialMenuState,
    pub active_remind: Option<ActiveRemind>,
    pub remind_queue: VecDeque<RemindFireEvent>,
    pub next_remind_at_s: Option<f64>,
    pub nod_nod_start_s: Option<f64>,
    pub pending_say_pool: Option<&'static str>,
    pub pending_say_text: Option<PendingSayText>,

    bubble_tex: Option<GpuTexture>,
    bubble_dirty_key: u64,
    bubble_pad: (f32, f32),
    bubble_frame: Option<BubbleFrameState>,
    radial_tex: Option<GpuTexture>,

    // Interactive / animation state
    pub resting: bool,
    pub down: bool,
    pub press_start_s: f64,
    pub release_due_s: Option<f64>,
    pub press_blend: f32,
    pub dragging: bool,
    pub held: bool,
    pub blink_active: bool,
    pub blink_off_at_s: f64,
    pub double_blink_at_s: Option<f64>,
    pub next_blink_at_s: f64,

    last_drag_sample: Option<(f64, f64, f64)>,
    hold_out: HoldOutputs,
    nod_out: (f32, f32, bool),
}

impl PetRuntime {
    pub fn new(
        state: PetState,
        ui_prefs: UiPrefs,
        gpu_chars: Arc<CharacterGpuTextures>,
        now_s: f64,
    ) -> Self {
        let geom = BugGeometry::from_size(state.size);
        let margin_frac = gpu_chars.rig_bundle.max_margin.max(0.55);
        let win_w_dip = (geom.size * (1.0 + 2.0 * margin_frac)).round();
        let win_h_dip = (geom.size * (1.0 + 2.0 * margin_frac)).round();
        let hit_mask = gpu_chars.hit_mask.clone();
        let rig = RigRuntime::new(gpu_chars.rig_bundle.clone());
        let part_toggles = rig.part_toggles();
        let radial = RadialMenuState::new(
            state.always_on_top,
            state.key_bg.clone(),
            part_toggles,
            ui_prefs.lang.clone(),
            ui_prefs.lite || ui_prefs.reduce_motion,
        );

        let blink_ms = gpu_chars.config.blink_ms;
        let next_blink_at_s = if blink_ms >= 300.0 && gpu_chars.heads[2].is_some() {
            let mut rng = rand::thread_rng();
            now_s + (blink_ms / 1000.0) * (0.55 + 0.9 * rng.gen::<f64>())
        } else {
            f64::INFINITY
        };

        let mut bubble = BubbleRuntime::new();
        bubble.reschedule_idle(&gpu_chars.config, now_s);

        Self {
            id: state.id.clone(),
            state,
            ui_prefs,
            geom,
            margin_frac,
            win_w_dip,
            win_h_dip,
            gpu_chars,
            hit_mask,
            rig,
            hold: HoldPhysics::default(),
            shiver: ShiverState::default(),
            reactor: ReactorState::default(),
            pet_fx: PetFxState::new(),
            pet_atlas: PetParticleAtlas::new(),
            active_particles: Vec::new(),
            bubble,
            radial,
            active_remind: None,
            remind_queue: VecDeque::new(),
            next_remind_at_s: None,
            nod_nod_start_s: None,
            pending_say_pool: None,
            pending_say_text: None,
            bubble_tex: None,
            bubble_dirty_key: 0,
            bubble_pad: (0.0, 0.0),
            bubble_frame: None,
            radial_tex: None,
            resting: false,
            down: false,
            press_start_s: -10.0,
            release_due_s: None,
            press_blend: 0.0,
            dragging: false,
            held: false,
            blink_active: false,
            blink_off_at_s: 0.0,
            double_blink_at_s: None,
            next_blink_at_s,
            last_drag_sample: None,
            hold_out: HoldOutputs {
                swing_rad: 0.0,
                pat_extra_rad: 0.0,
                sx: 1.0,
                sy: 1.0,
                plop_y_800: 0.0,
                lift_a: 0.0,
            },
            nod_out: (0.0, 0.0, false),
        }
    }

    pub fn set_size(&mut self, new_size: f64) {
        let clamped = new_size.clamp(64.0, 512.0).round();
        self.state.size = clamped;
        self.geom = BugGeometry::from_size(clamped);
        self.win_w_dip = (self.geom.size * (1.0 + 2.0 * self.margin_frac)).round();
        self.win_h_dip = (self.geom.size * (1.0 + 2.0 * self.margin_frac)).round();
        self.bubble_tex = None;
        self.bubble_dirty_key = 0;
        self.radial_tex = None;
    }

    pub fn set_ui_prefs(&mut self, ui: UiPrefs) {
        self.radial.lite_or_reduced = ui.lite || ui.reduce_motion;
        self.radial.lang = ui.lang.clone();
        self.ui_prefs = ui;
        self.bubble_tex = None;
        self.bubble_dirty_key = 0;
        self.radial_tex = None;
    }

    pub fn menu_snapshot(&self) -> crate::tray::PetMenuSnapshot {
        crate::tray::PetMenuSnapshot {
            id: self.id.clone(),
            char_id: self.state.char_id.clone(),
            size: self.state.size,
            flip: self.state.flip,
            lock: self.state.lock,
            click_through: self.state.click_through,
            always_on_top: self.state.always_on_top,
            mute: self.state.mute,
            music: self.state.music,
            typing: self.state.typing,
            key_bg: self.state.key_bg.clone(),
            parts: self.rig.part_toggles(),
        }
    }

    pub fn click_sound_params(&self) -> Option<(&str, f64)> {
        let cfg = &self.gpu_chars.config;
        if cfg.sound && cfg.volume > 0.0 && !self.state.mute {
            let kind = if cfg.sound_type.is_empty() {
                "click"
            } else {
                cfg.sound_type.as_str()
            };
            Some((kind, cfg.volume))
        } else {
            None
        }
    }

    pub fn char_box_pet_dip(&self) -> RectDip {
        let size = self.geom.size;
        let ox = (self.win_w_dip - size) * 0.5;
        let oy = (self.win_h_dip - size) * 0.5;
        let [vx0, vy0, vx1, vy1] = self.gpu_chars.visible_bounds;
        let (x0, x1) = if self.state.flip {
            (1.0 - vx1, 1.0 - vx0)
        } else {
            (vx0, vx1)
        };
        RectDip {
            x: ox + x0 * size,
            y: oy + vy0 * size,
            w: (x1 - x0).max(0.1) * size,
            h: (vy1 - vy0).max(0.1) * size,
            cx: None,
        }
    }

    pub fn char_box_in_overlay(
        &self,
        pet_rect_px: (i32, i32, i32, i32),
        overlay_origin_dip: (f64, f64),
        dpr: f64,
    ) -> CharBox {
        let pet_left_dip = pet_rect_px.0 as f64 / dpr;
        let pet_top_dip = pet_rect_px.1 as f64 / dpr;
        let cb = self.char_box_pet_dip();
        CharBox {
            x: pet_left_dip + cb.x - overlay_origin_dip.0,
            y: pet_top_dip + cb.y - overlay_origin_dip.1,
            w: cb.w,
            h: cb.h,
            cx: Some(pet_left_dip + cb.x + cb.w * 0.5 - overlay_origin_dip.0),
        }
    }

    pub fn core_center_in_overlay(
        &self,
        pet_rect_px: (i32, i32, i32, i32),
        overlay_origin_dip: (f64, f64),
        dpr: f64,
    ) -> (f64, f64) {
        let cb = self.char_box_in_overlay(pet_rect_px, overlay_origin_dip, dpr);
        (cb.x + cb.w * 0.5, cb.y + 0.45 * cb.h)
    }

    pub fn press(&mut self, now_s: f64) {
        self.down = true;
        self.press_start_s = now_s;
        self.release_due_s = None;
        self.blink_active = false;
        self.double_blink_at_s = None;
        self.pet_fx.reset_immediate();
        self.shiver.down(&self.gpu_chars.config, now_s);
        self.rig.press("down");
        self.pending_say_pool = Some("lines");
    }

    pub fn release(&mut self, now_s: f64) -> bool {
        self.shiver.up();
        if !self.down && self.release_due_s.is_none() {
            return false;
        }
        let hold_s = (self.gpu_chars.config.hold / 1000.0).max(0.0);
        let remain = (hold_s - (now_s - self.press_start_s)).max(0.0);
        if remain <= 0.001 {
            self.down = false;
            self.release_due_s = None;
            self.rig.press("up");
            self.schedule_blink(now_s);
            true
        } else {
            self.release_due_s = Some(now_s + remain);
            false
        }
    }

    pub fn begin_drag(&mut self, screen_x_dip: f64, screen_y_dip: f64, now_s: f64) {
        let cfg = &self.gpu_chars.config;
        self.dragging = true;
        self.held = cfg.drag_on;
        self.down = false;
        self.release_due_s = None;
        self.pending_say_pool = None;
        self.pending_say_text = None;
        self.pet_fx.reset_immediate();
        self.bubble.hide(now_s);
        self.shiver.up();
        if cfg.drag_on {
            self.hold.on_drag_start(now_s);
            self.hold.on_drag_move(screen_x_dip, now_s);
            self.rig.set_lift(
                1.0,
                0.0,
                &cfg.lift_still,
                &cfg.lift_hide,
                &cfg.lift_sad,
                cfg.drop_wobble,
            );
        }
        self.last_drag_sample = Some((screen_x_dip, screen_y_dip, now_s));
    }

    pub fn drag_move(&mut self, screen_x_dip: f64, screen_y_dip: f64, now_s: f64) {
        if !self.dragging {
            return;
        }
        self.hold.on_drag_move(screen_x_dip, now_s);
        if let Some((lx, ly, lt)) = self.last_drag_sample {
            if now_s - lt > 0.004 {
                let dt = (now_s - lt).min(0.1);
                let flip_sign = if self.state.flip { -1.0 } else { 1.0 };
                let dx_norm = ((screen_x_dip - lx) / dt / self.geom.size.max(48.0)) * flip_sign;
                let dy_norm = (screen_y_dip - ly) / dt / self.geom.size.max(48.0);
                self.rig.drag(dx_norm, dy_norm);
                self.last_drag_sample = Some((screen_x_dip, screen_y_dip, now_s));
            }
        } else {
            self.last_drag_sample = Some((screen_x_dip, screen_y_dip, now_s));
        }
    }

    pub fn end_drag(&mut self, now_s: f64, was_moved: bool) -> bool {
        self.dragging = false;
        self.last_drag_sample = None;
        if was_moved {
            let cfg = &self.gpu_chars.config;
            self.held = false;
            self.down = false;
            self.release_due_s = None;
            self.hold.on_drag_end(cfg.drop_wobble, now_s);
            self.rig.set_lift(
                0.0,
                0.0,
                &cfg.lift_still,
                &cfg.lift_hide,
                &cfg.lift_sad,
                cfg.drop_wobble,
            );
            self.schedule_blink(now_s);
            false
        } else {
            self.held = false;
            self.release(now_s)
        }
    }

    pub fn on_pet_hover(&mut self, local_x_dip: f64, local_y_dip: f64, now_s: f64) {
        if self.down
            || self.dragging
            || self.held
            || self.resting
            || self.active_remind.is_some()
            || self.radial.is_visible()
        {
            self.pet_fx.reset_immediate();
            return;
        }
        let cfg = &self.gpu_chars.config;
        if !cfg.pet_on {
            return;
        }
        if cfg.pet_area == "head" {
            let size = self.geom.size.max(1.0);
            let ox = (self.win_w_dip - size) * 0.5;
            let oy = (self.win_h_dip - size) * 0.5;
            let mut nx = (local_x_dip - ox) / size;
            let ny = (local_y_dip - oy) / size;
            if self.state.flip {
                nx = 1.0 - nx;
            }
            if !self.gpu_chars.pet_area_mask.hit(nx, ny) {
                self.pet_fx.leave(now_s);
                return;
            }
        }
        let cb = self.char_box_pet_dip();
        let head_box = RectDip {
            x: cb.x,
            y: cb.y,
            w: cb.w,
            h: cb.h * 0.55,
            cx: None,
        };
        let want_pat_say = self
            .pet_fx
            .on_move(local_x_dip, local_y_dip, now_s, cfg, head_box, self.geom.size);
        if self.pet_fx.is_active(now_s) {
            self.rig.pet();
        }
        if want_pat_say {
            let cur_key = self.bubble.active_key();
            if cur_key.is_none() || cur_key == Some("patLines") {
                self.pending_say_pool = Some("patLines");
            }
        }
    }

    pub fn on_pet_leave(&mut self, now_s: f64) {
        self.pet_fx.leave(now_s);
    }

    pub fn push_remind(&mut self, ev: RemindFireEvent, now_s: f64) {
        if let Some(ref cur) = self.active_remind {
            if cur.event.kind == ev.kind && ev.kind != RemindKind::Pill {
                self.activate_remind(ev, now_s);
            } else {
                self.remind_queue.push_back(ev);
            }
        } else {
            self.activate_remind(ev, now_s);
        }
    }

    fn format_remind_memo(ev: &RemindFireEvent) -> String {
        if ev.kind == RemindKind::Todo && !ev.memo.is_empty() && ev.left_count > 1 {
            format!("{} +{}", ev.memo, ev.left_count - 1)
        } else {
            ev.memo.clone()
        }
    }

    fn activate_remind(&mut self, ev: RemindFireEvent, now_s: f64) {
        self.bubble.hide(now_s);
        self.pending_say_pool = None;
        self.pending_say_text = None;
        self.next_remind_at_s = None;
        let memo_text = Self::format_remind_memo(&ev);
        let (pool_key, items) = resolve_remind_pool(
            &self.gpu_chars.config,
            ev.kind.as_str(),
            &ev.group_id,
            "before",
        );
        let (before_pool_key, before_raw_line, before_display_text, is_icon_only) =
            if !items.is_empty() {
                let raw = items[rand::thread_rng().gen_range(0..items.len())].clone();
                let display = fill_memo(&raw, &memo_text);
                (Some(pool_key), Some(raw), Some(display), false)
            } else {
                (None, None, None, memo_text.is_empty())
            };
        self.active_remind = Some(ActiveRemind {
            event: ev,
            shown_at_s: now_s,
            tex: None,
            tex_dpr: 0.0,
            pad_l: 0.0,
            pad_t: 0.0,
            box_w_dip: 58.0,
            box_h_dip: 44.0,
            hot_rect_local_dip: (0.0, 0.0, 0.0, 0.0),
            is_icon_only,
            before_pool_key,
            before_raw_line,
            before_display_text,
        });
    }

    /// Dismisses the active reminder badge, returning `Some((kind, slot_hhmm))` if one was dismissed.
    pub fn dismiss_remind(&mut self, now_s: f64) -> Option<(RemindKind, String)> {
        let cur = self.active_remind.take()?;
        let hold_s = (self.gpu_chars.config.hold / 1000.0).max(0.06);
        self.down = true;
        self.press_start_s = now_s;
        self.release_due_s = Some(now_s + hold_s);

        let mut spoke_after = false;
        let cfg = &self.gpu_chars.config;
        if cur.event.kind != RemindKind::Todo && cfg.bubble {
            let (pool_key, items) =
                resolve_remind_pool(cfg, cur.event.kind.as_str(), &cur.event.group_id, "after");
            if !items.is_empty() {
                let raw = items[rand::thread_rng().gen_range(0..items.len())].clone();
                let text = fill_memo(&raw, &cur.event.memo);
                self.pending_say_text = Some(PendingSayText {
                    text,
                    min_ms: Some(cfg.bubble_ms.max(2200.0)),
                    style_pool_key: Some(pool_key),
                    style_src: Some(raw),
                });
                spoke_after = true;
            }
        }
        if !spoke_after {
            self.nod_nod_start_s = Some(now_s);
        }
        if !self.remind_queue.is_empty() {
            self.next_remind_at_s = Some(now_s + 1.6);
        }
        Some((cur.event.kind, cur.event.slot_hhmm))
    }

    pub fn on_beat(&mut self, strength: f64, interval_s: f64, now_s: f64) {
        let cfg = &self.gpu_chars.config;
        let busy = self.down || self.release_due_s.is_some() || self.dragging;
        self.reactor
            .trigger_beat(cfg, strength, interval_s, now_s, busy);
        if !busy {
            self.rig.beat(strength);
        }
    }

    pub fn on_key(&mut self, now_s: f64) {
        if !self.state.typing || self.resting {
            return;
        }
        let cfg = &self.gpu_chars.config;
        let busy = self.down || self.release_due_s.is_some() || self.dragging;
        self.reactor.trigger_key(cfg, now_s, busy);
        if !busy {
            self.rig.key();
        }
    }

    pub fn set_resting(&mut self, resting: bool) {
        if self.resting != resting {
            self.resting = resting;
            if resting {
                self.blink_active = false;
                self.double_blink_at_s = None;
                self.pet_fx.reset_immediate();
            }
        }
    }

    pub fn has_overlay_content(&self, now_s: f64) -> bool {
        self.bubble.is_visible(now_s)
            || self.active_remind.is_some()
            || self.radial.is_visible()
    }

    fn schedule_blink(&mut self, now_s: f64) {
        let blink_ms = self.gpu_chars.config.blink_ms;
        if blink_ms >= 300.0 && self.gpu_chars.heads[2].is_some() {
            let mut rng = rand::thread_rng();
            self.next_blink_at_s = now_s + (blink_ms / 1000.0) * (0.55 + 0.9 * rng.gen::<f64>());
        } else {
            self.next_blink_at_s = f64::INFINITY;
        }
    }

    pub fn step(&mut self, now_s: f64, dt_s: f32) -> bool {
        let mut released_up = false;
        if let Some(due) = self.release_due_s {
            if now_s >= due {
                self.down = false;
                self.release_due_s = None;
                self.rig.press("up");
                self.schedule_blink(now_s);
                released_up = true;
            }
        }

        if let Some(t_next) = self.next_remind_at_s {
            if self.active_remind.is_none() && now_s >= t_next {
                self.next_remind_at_s = None;
                if let Some(ev) = self.remind_queue.pop_front() {
                    self.activate_remind(ev, now_s);
                }
            }
        }

        if let Some(t0) = self.nod_nod_start_s {
            if now_s - t0 >= 0.9 {
                self.nod_nod_start_s = None;
            }
        }

        let cfg = &self.gpu_chars.config;
        let busy = self.down || self.release_due_s.is_some() || self.dragging;
        self.hold_out = self.hold.step(
            dt_s as f64,
            now_s,
            cfg.drag_swing,
            cfg.drop_squish,
            self.geom.size,
        );
        if self.hold.active {
            self.rig.set_lift(
                if self.held { 1.0 } else { 0.0 },
                self.hold_out.lift_a,
                &cfg.lift_still,
                &cfg.lift_hide,
                &cfg.lift_sad,
                cfg.drop_wobble,
            );
        }
        self.shiver.step(cfg, now_s);
        self.nod_out = self.reactor.step(now_s, busy);
        let hold_cfg = cfg.hold;
        if self.reactor.pending_blink {
            self.reactor.pending_blink = false;
            if !self.down && !self.held && !self.resting && self.gpu_chars.heads[2].is_some() {
                self.blink_active = true;
                self.blink_off_at_s = now_s + 0.110;
            }
        }

        // Idle speech bubble polling
        let idle_busy =
            self.down || self.resting || self.dragging || self.active_remind.is_some() || self.radial.is_visible();
        if self.bubble.poll_idle(cfg, now_s, idle_busy)
            && self.pending_say_pool.is_none()
            && self.pending_say_text.is_none()
        {
            self.pending_say_pool = Some("idleLines");
        }

        // Petting particles update
        let cb_pet = self.char_box_pet_dip();
        self.active_particles = self.pet_fx.step(now_s, cfg, cb_pet, self.geom.size);

        // Radial menu idle auto-close / animations
        if self.radial.is_visible() {
            self.radial.tick(now_s);
        }

        let is_pressing = self.down || self.nod_out.2;
        let press_dur_s = ((0.6 * hold_cfg).max(40.0) / 1000.0) as f32;
        let rate = dt_s / press_dur_s;
        if is_pressing {
            self.press_blend = (self.press_blend + rate).min(1.0);
        } else {
            self.press_blend = (self.press_blend - rate).max(0.0);
        }

        if is_pressing || self.held || self.resting {
            self.blink_active = false;
            self.double_blink_at_s = None;
        } else {
            if self.blink_active && now_s >= self.blink_off_at_s {
                self.blink_active = false;
                let mut rng = rand::thread_rng();
                if self.double_blink_at_s.is_none() && rng.gen::<f64>() < 0.25 {
                    self.double_blink_at_s = Some(now_s + 0.150);
                } else {
                    self.double_blink_at_s = None;
                    self.schedule_blink(now_s);
                }
            }
            if let Some(t2) = self.double_blink_at_s {
                if now_s >= t2 {
                    self.blink_active = true;
                    self.blink_off_at_s = now_s + 0.110;
                    self.double_blink_at_s = Some(f64::INFINITY);
                }
            } else if now_s >= self.next_blink_at_s {
                self.blink_active = true;
                self.blink_off_at_s = now_s + 0.110;
                self.next_blink_at_s = f64::INFINITY;
            }
        }

        let (_, _, _, m_head, _) = self.compute_matrices(now_s);
        self.rig.step(dt_s as f64, self.resting, m_head);
        released_up
    }

    fn compute_matrices(&self, now_s: f64) -> (Mat2D, Mat2D, Mat2D, Mat2D, Mat2D) {
        let cfg = &self.gpu_chars.config;
        let flip_sx = if self.state.flip { -1.0 } else { 1.0 };

        // `#stack`: plopY, drag swing (`50% 64%` = `(400, 512)`), flip, squash scale, body shiver
        let m_stack = Mat2D::translate(0.0, self.hold_out.plop_y_800)
            .mul(Mat2D::rotate_about(self.hold_out.swing_rad, 400.0, 512.0))
            .mul(Mat2D::scale_about(
                flip_sx * self.hold_out.sx,
                self.hold_out.sy,
                400.0,
                512.0,
            ))
            .mul(Mat2D::translate(
                self.shiver.body_tx_800,
                self.shiver.body_ty_800,
            ))
            .mul(Mat2D::rotate_about(self.shiver.body_rot_rad, 400.0, 400.0));

        let m_body = m_stack;

        // `#bob`: breathing offset
        let breath_dy = if cfg.breathe && cfg.breath_amp > 0.0 && !self.resting {
            let period_s = (cfg.breath_ms.max(600.0)) / 1000.0;
            let phase = ((now_s % period_s) / period_s) as f32 * TAU;
            let amp_800 = (cfg.breath_amp as f32) * 0.8;
            -0.5 * amp_800 * (1.0 - phase.cos())
        } else {
            0.0
        };
        let m_bob = m_body.mul(Mat2D::translate(0.0, breath_dy));

        // `#nod`: music/key beat animation + reminder dismiss double-nod
        let (nod_dy, nod_rot, _) = self.nod_out;
        let nod_nod_dy = if let Some(t0) = self.nod_nod_start_s {
            let u = ((now_s - t0) / 0.9).clamp(0.0, 1.0) as f32;
            let s = (u * 2.0 * std::f32::consts::PI).sin().abs();
            36.0 * s
        } else {
            0.0
        };
        let m_nod = m_bob
            .mul(Mat2D::translate(0.0, nod_dy + nod_nod_dy))
            .mul(Mat2D::rotate_about(nod_rot, 400.0, 544.0));

        // `#pat`: hold wobble + petting sway/tilt (`pat_transform_800` already accounts for `flip`)
        let pat_flip_extra = self.hold_out.pat_extra_rad * flip_sx;
        let (pat_tx, pat_rot, _) = self.pet_fx.pat_transform_800(now_s, self.state.flip);
        let m_pat = m_nod
            .mul(Mat2D::translate(pat_tx, 0.0))
            .mul(Mat2D::rotate_about(pat_flip_extra + pat_rot, 400.0, 544.0));

        // `#head`: press depth/squash + head shiver (`50% 68%` = `(400, 544)`)
        let ease_p = self.press_blend * self.press_blend * (3.0 - 2.0 * self.press_blend);
        let head_dy = ease_p * (cfg.depth as f32) * 4.0;
        let head_sx = 1.0 + ease_p * (cfg.squash as f32) / 200.0;
        let head_sy = 1.0 - ease_p * (cfg.squash as f32) / 100.0;
        let m_head_local = Mat2D::translate(
            self.shiver.head_tx_800,
            self.shiver.head_ty_800 + head_dy,
        )
        .mul(Mat2D::rotate_about(self.shiver.head_rot_rad, 400.0, 544.0))
        .mul(Mat2D::scale_about(head_sx, head_sy, 400.0, 544.0));

        let m_head = m_pat.mul(m_head_local);
        let m_np = m_nod.mul(m_head_local);

        (m_body, m_bob, m_nod, m_head, m_np)
    }

    pub fn render(&mut self, gpu: &GpuContext, surface: &PetSurface, now_s: f64) -> Result<(), String> {
        let clear = if !self.state.key_bg.is_empty() {
            if let Some([r, g, b, _]) = parse_hex_color_rgba(&self.state.key_bg) {
                [r, g, b, 1.0]
            } else {
                [0.0, 0.0, 0.0, 0.0]
            }
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };
        surface.begin_frame_with_clear(gpu, clear);

        let cfg = &self.gpu_chars.config;
        let size = self.geom.size as f32;
        let vp_scale = size / 800.0;
        let core_off_x = ((self.win_w_dip as f32) - size) * 0.5;
        let core_off_y = ((self.win_h_dip as f32) - size) * 0.5;
        let vp = [vp_scale, vp_scale, core_off_x, core_off_y];
        let win_and_pad = [self.win_w_dip as f32, self.win_h_dip as f32, 0.0, 0.0];

        let (m_body, _m_bob, _m_nod, m_head, m_np) = self.compute_matrices(now_s);
        let m_ghost = self.rig.ghost_matrix(m_head);
        let rest = self.resting;

        // Floor shadow
        if cfg.floor_on && cfg.floor_alpha > 0.0 && (!self.held || cfg.lift_shadow) {
            if let Some(ref sh_tex) = gpu.shadow_tex {
                let sh_w = if self.held { 368.0 } else { 420.0 };
                let sh_h = if self.held { 36.0 } else { 48.0 };
                let sh_x = (800.0 - sh_w) * 0.5;
                let lift_y_800 = if self.held {
                    (cfg.lift_shadow_y as f32) * 4.0
                } else {
                    0.0
                };
                let sh_y = 800.0 - sh_h * 0.55 + (cfg.floor_depth as f32) * 2.0 + lift_y_800;
                let sh_alpha = cfg.floor_alpha * if self.held { 0.55 } else { 0.85 };
                let col = parse_hex_color(&cfg.floor_color, Some(sh_alpha));
                let tint = [col[0] * col[3], col[1] * col[3], col[2] * col[3], col[3]];
                let flip_only = if self.state.flip {
                    Mat2D::scale_about(-1.0, 1.0, 400.0, 400.0)
                } else {
                    Mat2D::identity()
                };
                let m = flip_only.mul(Mat2D::unit_to_rect(sh_x, sh_y, sh_w, sh_h));
                surface.draw_sprite(
                    gpu,
                    sh_tex,
                    &SpriteCB {
                        m0: [m.m[0], m.m[1], m.m[2], 0.0],
                        m1: [m.m[3], m.m[4], m.m[5], 0.0],
                        vp,
                        win_and_pad,
                        tint,
                    },
                )?;
            }
        }

        // Exact BUG Clicker back-to-front stack order
        self.rig
            .draw_zone(RigZone::Back, m_body, vp, win_and_pad, rest, gpu, surface)?;
        self.rig
            .draw_zone(RigZone::Ghost, m_ghost, vp, win_and_pad, rest, gpu, surface)?;
        self.rig
            .draw_zone(RigZone::BackMid, m_body, vp, win_and_pad, rest, gpu, surface)?;
        self.rig
            .draw_zone(RigZone::NpBack, m_np, vp, win_and_pad, rest, gpu, surface)?;

        // Body / Body2
        let using_body2 = self.held && self.gpu_chars.body2.is_some();
        if using_body2 {
            if let Some(ref b2) = self.gpu_chars.body2 {
                let s = (cfg.drag_body_s / 100.0) as f32;
                let bw = 800.0 * s;
                let bh = bw * (b2.height as f32 / b2.width.max(1) as f32);
                let bx = (800.0 - bw) * 0.5;
                let by = (cfg.drag_body_y as f32) + 512.0 * (1.0 - s);
                let m = m_body.mul(Mat2D::unit_to_rect(bx, by, bw, bh));
                surface.draw_sprite(
                    gpu,
                    b2,
                    &SpriteCB {
                        m0: [m.m[0], m.m[1], m.m[2], 0.0],
                        m1: [m.m[3], m.m[4], m.m[5], 0.0],
                        vp,
                        win_and_pad,
                        tint: [1.0; 4],
                    },
                )?;
            }
        } else if let Some(ref body) = self.gpu_chars.body {
            let m = m_body.mul(Mat2D::contain_bottom_center(body.width, body.height));
            surface.draw_sprite(
                gpu,
                body,
                &SpriteCB {
                    m0: [m.m[0], m.m[1], m.m[2], 0.0],
                    m1: [m.m[3], m.m[4], m.m[5], 0.0],
                    vp,
                    win_and_pad,
                    tint: [1.0; 4],
                },
            )?;
            if let Some(ref nsh) = self.gpu_chars.neck_shadow {
                let nm = m_body.mul(Mat2D::unit_to_rect(0.0, 0.0, 800.0, 800.0));
                surface.draw_sprite(
                    gpu,
                    nsh,
                    &SpriteCB {
                        m0: [nm.m[0], nm.m[1], nm.m[2], 0.0],
                        m1: [nm.m[3], nm.m[4], nm.m[5], 0.0],
                        vp,
                        win_and_pad,
                        tint: [1.0; 4],
                    },
                )?;
            }
        }

        self.rig
            .draw_zone(RigZone::Mid, m_body, vp, win_and_pad, rest, gpu, surface)?;
        self.rig
            .draw_zone(RigZone::NpBehind, m_np, vp, win_and_pad, rest, gpu, surface)?;
        self.rig
            .draw_zone(RigZone::HeadBehind, m_head, vp, win_and_pad, rest, gpu, surface)?;

        // Head expression (press > held > petting > blink > idle)
        let is_pressing = self.down || self.nod_out.2 || self.press_blend > 0.2;
        let (_, _, petting_expr) = self.pet_fx.pat_transform_800(now_s, self.state.flip);
        let head_tex = if self.held {
            self.gpu_chars.heads[4]
                .as_ref()
                .or(self.gpu_chars.heads[3].as_ref())
                .or(self.gpu_chars.heads[0].as_ref())
        } else if is_pressing {
            self.gpu_chars.heads[1]
                .as_ref()
                .or(self.gpu_chars.heads[0].as_ref())
        } else if petting_expr {
            self.gpu_chars.heads[3]
                .as_ref()
                .or(self.gpu_chars.heads[2].as_ref())
                .or(self.gpu_chars.heads[0].as_ref())
        } else if self.blink_active {
            self.gpu_chars.heads[2]
                .as_ref()
                .or(self.gpu_chars.heads[0].as_ref())
        } else {
            self.gpu_chars.heads[0].as_ref()
        };

        if let Some(h) = head_tex {
            let m = m_head.mul(Mat2D::contain_bottom_center(h.width, h.height));
            surface.draw_sprite(
                gpu,
                h,
                &SpriteCB {
                    m0: [m.m[0], m.m[1], m.m[2], 0.0],
                    m1: [m.m[3], m.m[4], m.m[5], 0.0],
                    vp,
                    win_and_pad,
                    tint: [1.0; 4],
                },
            )?;
        }

        self.rig
            .draw_zone(RigZone::HeadFront, m_head, vp, win_and_pad, rest, gpu, surface)?;
        self.rig
            .draw_zone(RigZone::HeadTop, m_head, vp, win_and_pad, rest, gpu, surface)?;
        self.rig
            .draw_zone(RigZone::NpFront, m_np, vp, win_and_pad, rest, gpu, surface)?;
        self.rig
            .draw_zone(RigZone::Top, m_body, vp, win_and_pad, rest, gpu, surface)?;
        self.rig
            .draw_zone(RigZone::TopTop, m_body, vp, win_and_pad, rest, gpu, surface)?;

        // GPU petting particles in pet-window DIP space
        if !self.active_particles.is_empty() {
            let vp_dip = [1.0f32, 1.0, 0.0, 0.0];
            for p in &self.active_particles {
                if p.alpha <= 0.004 || p.size_dip <= 0.5 {
                    continue;
                }
                if let Ok(tex) = self.pet_atlas.get_or_upload(gpu, &p.tex_key, cfg) {
                    let half = p.size_dip * 0.5;
                    let m = Mat2D::translate(p.center_x_dip, p.center_y_dip)
                        .mul(Mat2D::rotate_about(p.rot_rad, 0.0, 0.0))
                        .mul(Mat2D::unit_to_rect(-half, -half, p.size_dip, p.size_dip));
                    let a = p.alpha.clamp(0.0, 1.0);
                    surface.draw_sprite(
                        gpu,
                        tex,
                        &SpriteCB {
                            m0: [m.m[0], m.m[1], m.m[2], 0.0],
                            m1: [m.m[3], m.m[4], m.m[5], 0.0],
                            vp: vp_dip,
                            win_and_pad,
                            tint: [a, a, a, a],
                        },
                    )?;
                }
            }
        }

        surface.present();
        Ok(())
    }

    /// Updates pending speech bubble / reminder / radial textures and renders into the overlay surface.
    #[allow(clippy::too_many_arguments)]
    pub fn update_and_render_overlay(
        &mut self,
        gpu: &GpuContext,
        overlay_surface: &PetSurface,
        pet_rect_px: (i32, i32, i32, i32),
        overlay_rect_px: (i32, i32, i32, i32),
        dpr: f64,
        now_s: f64,
    ) -> Result<(), String> {
        let ov_origin_dip = (
            overlay_rect_px.0 as f64 / dpr,
            overlay_rect_px.1 as f64 / dpr,
        );
        let ov_w_dip = overlay_rect_px.2 as f64 / dpr;
        let ov_h_dip = overlay_rect_px.3 as f64 / dpr;
        let allowed = DipRect {
            left: 0.0,
            top: 0.0,
            right: ov_w_dip,
            bottom: ov_h_dip,
        };
        let char_box_raw = self.char_box_in_overlay(pet_rect_px, ov_origin_dip, dpr);
        let cfg = &self.gpu_chars.config;
        let char_box = char_box_raw.with_bubble_offset(
            cfg.bubble_x,
            cfg.bubble_y,
            self.geom.size,
            self.state.flip,
        );

        if let Some(pool_key) = self.pending_say_pool.take() {
            if self.active_remind.is_none() {
                self.bubble.say(
                    pool_key,
                    cfg,
                    &self.ui_prefs,
                    &char_box,
                    &allowed,
                    &self.gpu_chars.fonts,
                    now_s,
                );
            }
        }
        if let Some(pst) = self.pending_say_text.take() {
            if self.active_remind.is_none() {
                self.bubble.say_text(
                    &pst.text,
                    pst.min_ms,
                    pst.style_pool_key.as_deref(),
                    pst.style_src.as_deref(),
                    cfg,
                    &self.ui_prefs,
                    &char_box,
                    &allowed,
                    &self.gpu_chars.fonts,
                    now_s,
                );
            }
        }

        self.bubble_frame = if self.active_remind.is_none() {
            self.bubble
                .step(now_s, &char_box, &allowed, &self.gpu_chars.fonts)
        } else {
            None
        };

        overlay_surface.begin_frame(gpu);
        let vp_dip = [1.0f32, 1.0, 0.0, 0.0];
        let win_and_pad = [ov_w_dip as f32, ov_h_dip as f32, 0.0, 0.0];

        // 1. Speech bubble
        if let Some(ref frame) = self.bubble_frame {
            if frame.visible {
                if self.bubble_tex.is_none() || self.bubble_dirty_key != frame.dirty_key {
                    let (sprite, pad_l, pad_t) = rasterize_bubble(
                        &frame.text,
                        &frame.placement,
                        &frame.style,
                        &self.gpu_chars.fonts,
                        dpr,
                        frame.phase_idx,
                        frame.appear_scale,
                        frame.appear_alpha,
                        frame.fx_elapsed_s,
                        frame.shake_tick,
                    );
                    self.bubble_tex = Some(GpuTexture::from_sprite(gpu, &sprite)?);
                    self.bubble_dirty_key = frame.dirty_key;
                    self.bubble_pad = (pad_l, pad_t);
                }
                if let Some(ref tex) = self.bubble_tex {
                    let x = frame.placement.left_dip as f32 - self.bubble_pad.0;
                    let y =
                        frame.placement.top_dip as f32 - self.bubble_pad.1 + frame.appear_dy_dip;
                    let w = tex.width as f32 / dpr as f32;
                    let h = tex.height as f32 / dpr as f32;
                    let m = Mat2D::unit_to_rect(x, y, w, h);
                    overlay_surface.draw_sprite(
                        gpu,
                        tex,
                        &SpriteCB {
                            m0: [m.m[0], m.m[1], m.m[2], 0.0],
                            m1: [m.m[3], m.m[4], m.m[5], 0.0],
                            vp: vp_dip,
                            win_and_pad,
                            tint: [1.0; 4],
                        },
                    )?;
                }
            }
        } else {
            self.bubble_tex = None;
            self.bubble_dirty_key = 0;
        }

        // 2. Active reminder badge
        if let Some(ref mut rm) = self.active_remind {
            if rm.tex.is_none() || (rm.tex_dpr - dpr).abs() > 0.01 {
                let kind_s = rm.event.kind.as_str();
                let is_en = self.ui_prefs.lang.eq_ignore_ascii_case("en");
                let title = if !rm.event.group_name.is_empty() {
                    rm.event.group_name.clone()
                } else {
                    match rm.event.kind {
                        RemindKind::Alarm => if is_en { "Alarm" } else { "알림" }.to_string(),
                        RemindKind::Pill => if is_en { "Medication" } else { "약 먹는 시간" }.to_string(),
                        RemindKind::Water => if is_en { "Hydration" } else { "물 먹는 시간" }.to_string(),
                        RemindKind::Todo => if is_en { "To-Do" } else { "할 일" }.to_string(),
                    }
                };
                let memo_text = Self::format_remind_memo(&rm.event);
                let header = if memo_text.is_empty() {
                    title
                } else {
                    format!("{title} · {memo_text}")
                };
                let footer_left = if rm.event.kind == RemindKind::Pill {
                    if is_en { "Click: Taken" } else { "눌러서 \"먹었어요\"" }
                } else if is_en {
                    "Click to dismiss"
                } else {
                    "눌러서 닫기"
                };
                let custom_style_owned: Option<ResolvedBubbleStyle>;
                let (has_bubble, body) = if let Some(ref display) = rm.before_display_text {
                    let style_id = match (&rm.before_pool_key, &rm.before_raw_line) {
                        (Some(pk), Some(raw)) => style_of_line(cfg, pk, raw),
                        _ => String::new(),
                    };
                    custom_style_owned = if style_id.is_empty() {
                        None
                    } else {
                        Some(ResolvedBubbleStyle::resolve(
                            cfg,
                            &style_id,
                            &self.ui_prefs.bubble_theme,
                        ))
                    };
                    (true, display.as_str())
                } else {
                    custom_style_owned = None;
                    let hb = !memo_text.is_empty();
                    let b = if memo_text.is_empty() {
                        header.as_str()
                    } else {
                        memo_text.as_str()
                    };
                    (hb, b)
                };
                let (sprite, pad_l, pad_t) = rasterize_remind_badge(
                    kind_s,
                    &header,
                    body,
                    footer_left,
                    &rm.event.slot_hhmm,
                    has_bubble,
                    custom_style_owned.as_ref(),
                    &self.gpu_chars.fonts,
                    dpr,
                );
                let box_w = (sprite.width as f64 / dpr) - 2.0 * pad_l as f64;
                let box_h = (sprite.height as f64 / dpr) - 2.0 * pad_t as f64;
                rm.tex = Some(GpuTexture::from_sprite(gpu, &sprite)?);
                rm.tex_dpr = dpr;
                rm.pad_l = pad_l;
                rm.pad_t = pad_t;
                rm.box_w_dip = box_w.max(44.0);
                rm.box_h_dip = box_h.max(36.0);
                rm.is_icon_only = !has_bubble;
            }
            let cx = char_box_raw.x + char_box_raw.w * 0.5;
            let bx = (cx - rm.box_w_dip * 0.5).clamp(4.0, (ov_w_dip - rm.box_w_dip - 4.0).max(4.0));
            let hover_dy = if rm.is_icon_only {
                let phase = ((now_s - rm.shown_at_s) / 2.4) as f32 * TAU;
                (phase.sin() * 7.0) as f64
            } else {
                0.0
            };
            let by = (char_box_raw.y - rm.box_h_dip - 18.0).max(4.0) + hover_dy;
            rm.hot_rect_local_dip = (
                bx - 6.0,
                by - 12.0,
                bx + rm.box_w_dip + 6.0,
                by + rm.box_h_dip + 14.0,
            );
            if let Some(ref tex) = rm.tex {
                let m = Mat2D::unit_to_rect(
                    bx as f32 - rm.pad_l,
                    by as f32 - rm.pad_t,
                    tex.width as f32 / dpr as f32,
                    tex.height as f32 / dpr as f32,
                );
                overlay_surface.draw_sprite(
                    gpu,
                    tex,
                    &SpriteCB {
                        m0: [m.m[0], m.m[1], m.m[2], 0.0],
                        m1: [m.m[3], m.m[4], m.m[5], 0.0],
                        vp: vp_dip,
                        win_and_pad,
                        tint: [1.0; 4],
                    },
                )?;
            }
        }

        // 3. Radial menu
        if self.radial.is_visible() {
            if self.radial_tex.is_none()
                || self
                    .radial
                    .needs_render(ov_w_dip, ov_h_dip, 0.0, 0.0, dpr, now_s)
            {
                let sp = self
                    .radial
                    .render_to_sprite(ov_w_dip, ov_h_dip, 0.0, 0.0, dpr, now_s);
                self.radial_tex = Some(GpuTexture::from_sprite(gpu, &sp)?);
            }
            if let Some(ref tex) = self.radial_tex {
                let m = Mat2D::unit_to_rect(0.0, 0.0, ov_w_dip as f32, ov_h_dip as f32);
                overlay_surface.draw_sprite(
                    gpu,
                    tex,
                    &SpriteCB {
                        m0: [m.m[0], m.m[1], m.m[2], 0.0],
                        m1: [m.m[3], m.m[4], m.m[5], 0.0],
                        vp: vp_dip,
                        win_and_pad,
                        tint: [1.0; 4],
                    },
                )?;
            }
        } else {
            self.radial_tex = None;
        }

        overlay_surface.present();
        Ok(())
    }

    pub fn hit_test_screen(
        &self,
        cursor_px: (i32, i32),
        win_rect_px: (i32, i32, i32, i32),
        dpr: f64,
    ) -> bool {
        let core_off_x_px = ((self.win_w_dip - self.geom.size) * 0.5) * dpr;
        let core_off_y_px = ((self.win_h_dip - self.geom.size) * 0.5) * dpr;
        let core_size_px = self.geom.size * dpr;
        if core_size_px <= 1.0 {
            return false;
        }
        let nx = (cursor_px.0 as f64 - (win_rect_px.0 as f64 + core_off_x_px)) / core_size_px;
        let ny = (cursor_px.1 as f64 - (win_rect_px.1 as f64 + core_off_y_px)) / core_size_px;
        let test_x = if self.state.flip { 1.0 - nx } else { nx };
        self.hit_mask.test_normalized(test_x, ny)
    }
}
