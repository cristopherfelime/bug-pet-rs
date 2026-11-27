use crate::config::{AppState, PetState, ReactPrefs, UiPrefs};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

pub const DEBOUNCE_MS: u64 = 400;

/// Sidecar key recording which BUG Clicker `state.json` revision the sidecar derives from.
const SYNC_KEY: &str = "_bugPetRsSync";

/// Cheap fingerprint (mtime + length) of the source `state.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMeta {
    pub mtime_ms: u64,
    pub len: u64,
}

/// Full fingerprint (mtime + length + SHA-256 hex) of the source `state.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSig {
    pub mtime_ms: u64,
    pub len: u64,
    pub sha256: String,
}

pub fn read_source_meta(path: &Path) -> Option<SourceMeta> {
    let md = fs::metadata(path).ok()?;
    let mtime_ms = md
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    Some(SourceMeta {
        mtime_ms,
        len: md.len(),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn compute_source_sig(path: &Path) -> Option<SourceSig> {
    let meta = read_source_meta(path)?;
    let bytes = fs::read(path).ok()?;
    Some(SourceSig {
        mtime_ms: meta.mtime_ms,
        len: meta.len,
        sha256: sha256_hex(&bytes),
    })
}

/// Reads the source file once (metadata first, then bytes) and returns the parsed JSON,
/// the typed `AppState` and a signature whose hash is computed from the exact bytes parsed.
/// Never touches the file system besides reading.
fn read_source_snapshot(path: &Path) -> Result<(Value, AppState, Option<SourceSig>), String> {
    let meta = read_source_meta(path);
    let bytes = fs::read(path).map_err(|e| format!("Failed reading {}: {e}", path.display()))?;
    let body: &[u8] = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes[..]);
    let val: Value = serde_json::from_slice(body)
        .map_err(|e| format!("Failed parsing {}: {e}", path.display()))?;
    let state: AppState = serde_json::from_value(val.clone())
        .map_err(|e| format!("Invalid state schema in {}: {e}", path.display()))?;
    let sig = meta.map(|m| SourceSig {
        mtime_ms: m.mtime_ms,
        len: m.len,
        sha256: sha256_hex(&bytes),
    });
    Ok((val, state, sig))
}

fn stamp_sync(doc: &mut Value, sig: Option<&SourceSig>) {
    let Some(sig) = sig else {
        return;
    };
    ensure_obj(doc).insert(
        SYNC_KEY.to_string(),
        json!({
            "sourceMtimeMs": sig.mtime_ms,
            "sourceLen": sig.len,
            "sourceSha256": sig.sha256,
        }),
    );
}

pub struct StatePersister {
    persist_enabled: bool,
    target_path: Option<PathBuf>,
    source_path: PathBuf,
    doc: Value,
    dirty: bool,
    dirty_since: Option<Instant>,
    bak_checked: bool,
    last_source_sig: Option<SourceSig>,
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = OsString::from(path.as_os_str());
    os.push(suffix);
    PathBuf::from(os)
}

fn default_target_path() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.trim().is_empty() {
            return Path::new(&appdata).join("bug-pet-rs").join("state.json");
        }
    }
    std::env::temp_dir().join("bug-pet-rs").join("state.json")
}

fn read_json_or_empty(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({}))
}

fn to_wide_null(path: &Path) -> Vec<u16> {
    OsStr::new(path.as_os_str())
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn ensure_obj(val: &mut Value) -> &mut Map<String, Value> {
    if !val.is_object() {
        *val = Value::Object(Map::new());
    }
    val.as_object_mut().expect("object just ensured")
}

fn pet_state_fields_map(pet: &PetState) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".to_string(), json!(pet.id));
    m.insert("charId".to_string(), json!(pet.char_id));
    m.insert("x".to_string(), json!(pet.x));
    m.insert("y".to_string(), json!(pet.y));
    m.insert("size".to_string(), json!(pet.size));
    m.insert("flip".to_string(), json!(pet.flip));
    m.insert("lock".to_string(), json!(pet.lock));
    m.insert("alwaysOnTop".to_string(), json!(pet.always_on_top));
    m.insert("clickThrough".to_string(), json!(pet.click_through));
    m.insert("mute".to_string(), json!(pet.mute));
    m.insert("music".to_string(), json!(pet.music));
    m.insert("typing".to_string(), json!(pet.typing));
    m.insert("keyBg".to_string(), json!(pet.key_bg));
    m.insert("boost".to_string(), json!(pet.boost));
    m.insert("boostAuto".to_string(), json!(pet.boost_auto));
    if let Some(ref a) = pet.anchor {
        m.insert(
            "anchor".to_string(),
            json!({
                "display": a.display,
                "fx": a.fx,
                "fy": a.fy,
            }),
        );
    }
    m.insert("lift".to_string(), json!(pet.lift));
    m
}

impl StatePersister {
    /// Backwards-compatible constructor (lenient: never fails, falls back to `{}`).
    #[allow(dead_code)]
    pub fn new(
        source_state_path: &Path,
        persist_enabled: bool,
        target_override: Option<PathBuf>,
    ) -> Self {
        let source_sig = compute_source_sig(source_state_path);
        let source_doc = || {
            let mut doc = if source_state_path.is_file() {
                read_json_or_empty(source_state_path)
            } else {
                json!({})
            };
            if source_state_path.is_file() {
                stamp_sync(&mut doc, source_sig.as_ref());
            }
            doc
        };

        if !persist_enabled {
            return Self {
                persist_enabled: false,
                target_path: None,
                source_path: source_state_path.to_path_buf(),
                doc: source_doc(),
                dirty: false,
                dirty_since: None,
                bak_checked: true,
                last_source_sig: source_sig,
            };
        }

        let target = target_override.unwrap_or_else(default_target_path);
        let doc = if target.is_file() {
            read_json_or_empty(&target)
        } else {
            source_doc()
        };

        let mut persister = Self {
            persist_enabled: true,
            target_path: Some(target),
            source_path: source_state_path.to_path_buf(),
            doc,
            dirty: false,
            dirty_since: None,
            bak_checked: false,
            last_source_sig: source_sig,
        };
        persister.ensure_one_time_backup();
        persister
    }

    /// Loads the effective state, preferring the sidecar only while it is still derived from
    /// the current BUG Clicker `state.json` (matching SHA-256). Returns
    /// `(effective_state, persister, loaded_from_sidecar)`. Never writes `source_state_path`.
    #[allow(dead_code)]
    pub fn load_synced_state(
        source_state_path: &Path,
        persist_enabled: bool,
        target_override: Option<PathBuf>,
    ) -> Result<(AppState, StatePersister, bool), String> {
        let (mut src_val, src_state, src_sig) = read_source_snapshot(source_state_path)?;

        let target = if persist_enabled {
            Some(target_override.unwrap_or_else(default_target_path))
        } else {
            None
        };

        if let (Some(target), Some(sig)) = (target.as_ref(), src_sig.as_ref()) {
            if target.is_file() {
                if let Ok(text) = fs::read_to_string(target) {
                    if let Ok(target_val) = serde_json::from_str::<Value>(&text) {
                        let hash_matches = target_val
                            .get(SYNC_KEY)
                            .and_then(|s| s.get("sourceSha256"))
                            .and_then(|v| v.as_str())
                            == Some(sig.sha256.as_str());
                        if hash_matches {
                            if let Ok(state) =
                                serde_json::from_value::<AppState>(target_val.clone())
                            {
                                let mut persister = Self {
                                    persist_enabled: true,
                                    target_path: Some(target.clone()),
                                    source_path: source_state_path.to_path_buf(),
                                    doc: target_val,
                                    dirty: false,
                                    dirty_since: None,
                                    bak_checked: false,
                                    last_source_sig: src_sig.clone(),
                                };
                                persister.ensure_one_time_backup();
                                return Ok((state, persister, true));
                            }
                        }
                    }
                }
            }
        }

        stamp_sync(&mut src_val, src_sig.as_ref());
        let stale_sidecar = target.as_ref().is_some_and(|t| t.is_file());
        let mut persister = Self {
            persist_enabled,
            target_path: target,
            source_path: source_state_path.to_path_buf(),
            doc: src_val,
            dirty: false,
            dirty_since: None,
            bak_checked: !persist_enabled,
            last_source_sig: src_sig,
        };
        persister.ensure_one_time_backup();
        if persist_enabled && stale_sidecar {
            persister.dirty = true;
            persister.dirty_since = Some(Instant::now());
        }
        Ok((src_state, persister, false))
    }

    /// Cheap change detection: metadata first, SHA-256 only when mtime/len differ.
    #[allow(dead_code)]
    pub fn source_changed(&self) -> bool {
        let Some(meta) = read_source_meta(&self.source_path) else {
            return false;
        };
        let Some(last) = self.last_source_sig.as_ref() else {
            return true;
        };
        if meta.mtime_ms == last.mtime_ms && meta.len == last.len {
            return false;
        }
        match compute_source_sig(&self.source_path) {
            Some(sig) => sig.sha256 != last.sha256,
            None => false,
        }
    }

    /// Re-reads the source file; on any read/parse failure `self` is left untouched.
    #[allow(dead_code)]
    pub fn reload_from_source(&mut self) -> Result<AppState, String> {
        let (mut val, state, sig) = read_source_snapshot(&self.source_path)?;
        stamp_sync(&mut val, sig.as_ref());
        self.doc = val;
        self.last_source_sig = sig;
        self.dirty = self.persist_enabled && self.target_path.as_ref().is_some_and(|p| p.is_file());
        self.dirty_since = if self.dirty { Some(Instant::now()) } else { None };
        Ok(state)
    }

    pub fn is_enabled(&self) -> bool {
        self.persist_enabled
    }

    #[allow(dead_code)]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    #[allow(dead_code)]
    pub fn target_path(&self) -> Option<&Path> {
        self.target_path.as_deref()
    }

    #[allow(dead_code)]
    pub fn snapshot(&self) -> &Value {
        &self.doc
    }

    fn ensure_one_time_backup(&mut self) {
        if !self.persist_enabled || self.bak_checked {
            return;
        }
        self.bak_checked = true;
        let Some(ref target) = self.target_path else {
            return;
        };
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let bak_path = append_suffix(target, ".bak");
        if bak_path.exists() {
            return;
        }
        if self.source_path.is_file() {
            let _ = fs::copy(&self.source_path, &bak_path);
        } else if target.is_file() {
            let _ = fs::copy(target, &bak_path);
        } else if let Ok(pretty) = serde_json::to_string_pretty(&self.doc) {
            let _ = fs::write(&bak_path, pretty);
        }
    }

    pub fn record_patch<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Value),
    {
        if !self.persist_enabled {
            return;
        }
        f(&mut self.doc);
        self.dirty = true;
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    pub fn patch_pet_pos(
        &mut self,
        pet_id: &str,
        x: f64,
        y: f64,
        anchor: Option<(u64, f64, f64)>,
    ) {
        self.record_patch(|doc| {
            let Some(pets) = doc.get_mut("pets").and_then(|v| v.as_array_mut()) else {
                return;
            };
            for pet in pets {
                if pet.get("id").and_then(|v| v.as_str()) == Some(pet_id) {
                    let obj = ensure_obj(pet);
                    obj.insert("x".to_string(), json!(x));
                    obj.insert("y".to_string(), json!(y));
                    if let Some((display, fx, fy)) = anchor {
                        obj.insert(
                            "anchor".to_string(),
                            json!({
                                "display": display,
                                "fx": fx,
                                "fy": fy,
                            }),
                        );
                    }
                    break;
                }
            }
        });
    }

    pub fn patch_pet_top(&mut self, pet_id: &str, always_on_top: bool) {
        self.record_patch(|doc| {
            let mut char_id: Option<String> = None;
            if let Some(pets) = doc.get_mut("pets").and_then(|v| v.as_array_mut()) {
                for pet in pets {
                    if pet.get("id").and_then(|v| v.as_str()) == Some(pet_id) {
                        let obj = ensure_obj(pet);
                        obj.insert("alwaysOnTop".to_string(), json!(always_on_top));
                        char_id = obj
                            .get("charId")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        break;
                    }
                }
            }
            if let Some(cid) = char_id {
                let root = ensure_obj(doc);
                let char_pet = ensure_obj(root.entry("charPet").or_insert_with(|| json!({})));
                let entry = ensure_obj(char_pet.entry(cid).or_insert_with(|| json!({})));
                entry.insert("alwaysOnTop".to_string(), json!(always_on_top));
            }
        });
    }

    pub fn patch_pet_key_bg(&mut self, pet_id: &str, key_bg: &str) {
        self.record_patch(|doc| {
            let mut char_id: Option<String> = None;
            if let Some(pets) = doc.get_mut("pets").and_then(|v| v.as_array_mut()) {
                for pet in pets {
                    if pet.get("id").and_then(|v| v.as_str()) == Some(pet_id) {
                        let obj = ensure_obj(pet);
                        obj.insert("keyBg".to_string(), json!(key_bg));
                        char_id = obj
                            .get("charId")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        break;
                    }
                }
            }
            if let Some(cid) = char_id {
                let root = ensure_obj(doc);
                let char_pet = ensure_obj(root.entry("charPet").or_insert_with(|| json!({})));
                let entry = ensure_obj(char_pet.entry(cid).or_insert_with(|| json!({})));
                entry.insert("keyBg".to_string(), json!(key_bg));
            }
        });
    }

    pub fn patch_ghost(&mut self, ghost: bool) {
        self.record_patch(|doc| {
            let root = ensure_obj(doc);
            let prefs = ensure_obj(root.entry("prefs").or_insert_with(|| json!({})));
            let ui = ensure_obj(prefs.entry("ui").or_insert_with(|| json!({})));
            ui.insert("ghost".to_string(), json!(ghost));
        });
    }

    pub fn patch_launch_at_login(&mut self, launch_at_login: bool) {
        self.record_patch(|doc| {
            let root = ensure_obj(doc);
            root.insert("launchAtLogin".to_string(), json!(launch_at_login));
        });
    }

    pub fn patch_pill_taken(&mut self, date_str: &str, slot_hhmm: &str) {
        self.record_patch(|doc| {
            let root = ensure_obj(doc);
            let remind = ensure_obj(root.entry("remind").or_insert_with(|| json!({})));
            let pill = ensure_obj(remind.entry("pill").or_insert_with(|| json!({})));
            let taken = ensure_obj(pill.entry("taken").or_insert_with(|| json!({})));

            let cur_date = taken.get("date").and_then(|v| v.as_str()).unwrap_or("");
            if cur_date != date_str {
                taken.insert("date".to_string(), json!(date_str));
                taken.insert("times".to_string(), json!([]));
            }
            let times_val = taken.entry("times").or_insert_with(|| json!([]));
            if !times_val.is_array() {
                *times_val = json!([]);
            }
            if let Some(arr) = times_val.as_array_mut() {
                if !arr.iter().any(|v| v.as_str() == Some(slot_hhmm)) && arr.len() < 8 {
                    arr.push(json!(slot_hhmm));
                }
            }
        });
    }

    pub fn patch_alarm_off(&mut self, alarm_id: &str) {
        self.record_patch(|doc| {
            let Some(alarms) = doc
                .get_mut("remind")
                .and_then(|v| v.get_mut("alarms"))
                .and_then(|v| v.as_array_mut())
            else {
                return;
            };
            for alarm in alarms {
                if alarm.get("id").and_then(|v| v.as_str()) == Some(alarm_id) {
                    let obj = ensure_obj(alarm);
                    obj.insert("on".to_string(), json!(false));
                    break;
                }
            }
        });
    }

    pub fn patch_spawn_or_remove_pet(&mut self, pet_id: &str, spawned_pet: Option<&PetState>) {
        self.record_patch(|doc| {
            let root = ensure_obj(doc);
            let pets_val = root.entry("pets").or_insert_with(|| json!([]));
            if !pets_val.is_array() {
                *pets_val = json!([]);
            }
            let pets = pets_val.as_array_mut().expect("array just ensured");

            match spawned_pet {
                Some(pet) => {
                    let fields = pet_state_fields_map(pet);
                    let mut updated = false;
                    for existing in pets.iter_mut() {
                        if existing.get("id").and_then(|v| v.as_str()) == Some(pet_id) {
                            let obj = ensure_obj(existing);
                            for (k, v) in &fields {
                                obj.insert(k.clone(), v.clone());
                            }
                            updated = true;
                            break;
                        }
                    }
                    if !updated {
                        pets.push(Value::Object(fields));
                    }
                    if let Some(num_str) = pet.id.strip_prefix("pet") {
                        if let Ok(num) = num_str.parse::<u64>() {
                            let cur_next = root
                                .get("nextId")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(1);
                            if num + 1 > cur_next {
                                root.insert("nextId".to_string(), json!(num + 1));
                            }
                        }
                    }
                }
                None => {
                    pets.retain(|p| p.get("id").and_then(|v| v.as_str()) != Some(pet_id));
                }
            }
        });
    }

    /// Generic per-pet field patch; optionally mirrors into `charPet[charId]`.
    #[allow(dead_code)]
    pub fn patch_pet_field(&mut self, pet_id: &str, field: &str, value: Value, mirror_char_pet: bool) {
        self.record_patch(|doc| {
            let mut char_id: Option<String> = None;
            if let Some(pets) = doc.get_mut("pets").and_then(|v| v.as_array_mut()) {
                for pet in pets {
                    if pet.get("id").and_then(|v| v.as_str()) == Some(pet_id) {
                        let obj = ensure_obj(pet);
                        obj.insert(field.to_string(), value.clone());
                        char_id = obj
                            .get("charId")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        break;
                    }
                }
            }
            if mirror_char_pet {
                if let Some(cid) = char_id.filter(|c| !c.is_empty()) {
                    let root = ensure_obj(doc);
                    let char_pet = ensure_obj(root.entry("charPet").or_insert_with(|| json!({})));
                    let entry = ensure_obj(char_pet.entry(cid).or_insert_with(|| json!({})));
                    entry.insert(field.to_string(), value);
                }
            }
        });
    }

    #[allow(dead_code)]
    pub fn patch_pet_size(&mut self, pet_id: &str, size: f64) {
        self.patch_pet_field(pet_id, "size", json!(size), true);
    }

    #[allow(dead_code)]
    pub fn patch_react_prefs(&mut self, prefs: &ReactPrefs) {
        self.record_patch(|doc| {
            let root = ensure_obj(doc);
            let p = ensure_obj(root.entry("prefs").or_insert_with(|| json!({})));
            let react = ensure_obj(p.entry("react").or_insert_with(|| json!({})));
            react.insert("sense".to_string(), json!(prefs.sense));
            react.insert("keyEvery".to_string(), json!(prefs.key_every));
            react.insert("beatOffset".to_string(), json!(prefs.beat_offset));
            react.insert("sound".to_string(), json!(prefs.sound));
            react.insert("gain".to_string(), json!(prefs.gain));
        });
    }

    #[allow(dead_code)]
    pub fn patch_ui_prefs(&mut self, prefs: &UiPrefs) {
        self.record_patch(|doc| {
            let root = ensure_obj(doc);
            let p = ensure_obj(root.entry("prefs").or_insert_with(|| json!({})));
            let ui = ensure_obj(p.entry("ui").or_insert_with(|| json!({})));
            ui.insert("ghost".to_string(), json!(prefs.ghost));
            ui.insert("restIdle".to_string(), json!(prefs.rest_idle));
            ui.insert("restMin".to_string(), json!(prefs.rest_min));
            ui.insert("bubbleFont".to_string(), json!(prefs.bubble_font));
            ui.insert("reduceMotion".to_string(), json!(prefs.reduce_motion));
            ui.insert("lang".to_string(), json!(prefs.lang));
            ui.insert("lite".to_string(), json!(prefs.lite));
        });
    }

    pub fn flush_if_due(&mut self) -> Result<bool, String> {
        if !self.persist_enabled || !self.dirty {
            return Ok(false);
        }
        let Some(since) = self.dirty_since else {
            return Ok(false);
        };
        if since.elapsed() < Duration::from_millis(DEBOUNCE_MS) {
            return Ok(false);
        }
        self.flush()?;
        Ok(true)
    }

    pub fn flush(&mut self) -> Result<(), String> {
        if !self.persist_enabled {
            return Ok(());
        }
        self.ensure_one_time_backup();

        let Some(target) = self.target_path.clone() else {
            return Ok(());
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed creating {}: {e}", parent.display()))?;
        }

        let tmp_path = append_suffix(&target, ".tmp");
        let json_str = serde_json::to_string_pretty(&self.doc)
            .map_err(|e| format!("Failed serializing state.json: {e}"))?;

        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)
                .map_err(|e| format!("Failed opening {}: {e}", tmp_path.display()))?;
            file.write_all(json_str.as_bytes())
                .map_err(|e| format!("Failed writing {}: {e}", tmp_path.display()))?;
            file.sync_all()
                .map_err(|e| format!("Failed syncing {}: {e}", tmp_path.display()))?;
        }

        let tmp_w = to_wide_null(&tmp_path);
        let dst_w = to_wide_null(&target);
        unsafe {
            MoveFileExW(
                PCWSTR(tmp_w.as_ptr()),
                PCWSTR(dst_w.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|e| format!("MoveFileExW({} -> {}) failed: {e}", tmp_path.display(), target.display()))?;

        self.dirty = false;
        self.dirty_since = None;
        Ok(())
    }
}

/// Free-function entry point for [`StatePersister::load_synced_state`].
#[allow(dead_code)]
pub fn load_synced_state(
    source_state_path: &Path,
    persist_enabled: bool,
    target_override: Option<PathBuf>,
) -> Result<(AppState, StatePersister, bool), String> {
    StatePersister::load_synced_state(source_state_path, persist_enabled, target_override)
}

impl Drop for StatePersister {
    fn drop(&mut self) {
        if self.persist_enabled && self.dirty {
            let _ = self.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQ: AtomicU64 = AtomicU64::new(1);

    fn temp_test_dir(tag: &str) -> PathBuf {
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bug-pet-rs-persist-{}-{}-{}",
            tag,
            std::process::id(),
            seq
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_only_mode_never_writes_or_creates_backup() {
        let dir = temp_test_dir("ro");
        let source = dir.join("source_state.json");
        let target = dir.join("out").join("state.json");
        let original = r#"{"pets":[{"id":"pet1","x":10,"y":20}],"launchAtLogin":false}"#;
        fs::write(&source, original).unwrap();

        let mut p = StatePersister::new(&source, false, Some(target.clone()));
        assert!(!p.is_enabled());
        p.patch_pet_pos("pet1", 999.0, 888.0, Some((1, 0.5, 0.5)));
        p.patch_ghost(true);
        p.patch_launch_at_login(true);
        assert!(!p.is_dirty());
        assert_eq!(p.flush_if_due().unwrap(), false);
        p.flush().unwrap();

        assert!(!target.exists());
        assert!(!append_suffix(&target, ".bak").exists());
        assert_eq!(fs::read_to_string(&source).unwrap(), original);
        drop(p);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_created_once_and_unknown_fields_preserved() {
        let dir = temp_test_dir("rw");
        let source = dir.join("source.json");
        let target = dir.join("appdata").join("state.json");
        let source_json = json!({
            "pets": [{
                "id": "pet1",
                "charId": "AlphaPet",
                "x": 100.0,
                "y": 200.0,
                "alwaysOnTop": true,
                "keyBg": "",
                "customPetField": "keep_me"
            }],
            "nextId": 2,
            "launchAtLogin": false,
            "prefs": {
                "react": { "sense": 9 },
                "ui": { "ghost": false, "theme": { "mode": "dark" } },
                "rotate": { "on": true, "pool": ["AlphaPet"] }
            },
            "remind": {
                "alarms": [{ "id": "a1", "on": true, "extraAlarmKey": 42 }],
                "pill": { "taken": { "date": "2026-11-26", "times": ["08:00"] } }
            },
            "charPet": {
                "AlphaPet": { "alwaysOnTop": true, "keyBg": "", "customCharMeta": 7 }
            },
            "customRootKey": { "nested": [1, 2, 3] }
        });
        fs::write(&source, serde_json::to_string_pretty(&source_json).unwrap()).unwrap();

        let mut p = StatePersister::new(&source, true, Some(target.clone()));
        let bak = append_suffix(&target, ".bak");
        assert!(bak.is_file());
        let bak_first = fs::read_to_string(&bak).unwrap();

        p.patch_pet_pos("pet1", 350.0, 450.0, Some((123, 0.25, 0.75)));
        p.patch_pet_top("pet1", false);
        p.patch_pet_key_bg("pet1", "#00ff00");
        p.patch_ghost(true);
        p.patch_launch_at_login(true);
        p.patch_pill_taken("2026-11-27", "09:00");
        p.patch_alarm_off("a1");
        assert!(p.is_dirty());
        // Immediately after patch, debounce has not elapsed
        assert_eq!(p.flush_if_due().unwrap(), false);

        p.flush().unwrap();
        assert!(!p.is_dirty());
        assert!(!append_suffix(&target, ".tmp").exists());

        let saved: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(saved["customRootKey"]["nested"][1], 2);
        assert_eq!(saved["prefs"]["ui"]["ghost"], true);
        assert_eq!(saved["prefs"]["ui"]["theme"]["mode"], "dark");
        assert_eq!(saved["prefs"]["rotate"]["on"], true);
        assert_eq!(saved["launchAtLogin"], true);
        assert_eq!(saved["pets"][0]["x"], 350.0);
        assert_eq!(saved["pets"][0]["alwaysOnTop"], false);
        assert_eq!(saved["pets"][0]["keyBg"], "#00ff00");
        assert_eq!(saved["pets"][0]["customPetField"], "keep_me");
        assert_eq!(saved["charPet"]["AlphaPet"]["alwaysOnTop"], false);
        assert_eq!(saved["charPet"]["AlphaPet"]["keyBg"], "#00ff00");
        assert_eq!(saved["charPet"]["AlphaPet"]["customCharMeta"], 7);
        assert_eq!(saved["remind"]["alarms"][0]["on"], false);
        assert_eq!(saved["remind"]["alarms"][0]["extraAlarmKey"], 42);
        assert_eq!(saved["remind"]["pill"]["taken"]["date"], "2026-11-27");
        assert_eq!(saved["remind"]["pill"]["taken"]["times"], json!(["09:00"]));

        // Mutate again & re-init: .bak must remain original snapshot
        p.patch_ghost(false);
        p.flush().unwrap();
        drop(p);
        let _p2 = StatePersister::new(&source, true, Some(target.clone()));
        assert_eq!(fs::read_to_string(&bak).unwrap(), bak_first);

        let _ = fs::remove_dir_all(&dir);
    }

    fn source_json(x: f64, note: &str) -> String {
        json!({
            "pets": [{
                "id": "pet1",
                "charId": "AlphaPet",
                "x": x,
                "y": 20.0,
                "size": 100.0,
                "customPetField": "keep_me"
            }],
            "nextId": 2,
            "customRootKey": { "n": 1 },
            "note": note
        })
        .to_string()
    }

    #[test]
    fn sidecar_preserved_when_source_unchanged() {
        let dir = temp_test_dir("sync-keep");
        let source = dir.join("source.json");
        let target = dir.join("appdata").join("state.json");
        let src_text = source_json(10.0, "v1");
        fs::write(&source, &src_text).unwrap();

        let (s0, mut p, from_sidecar) =
            load_synced_state(&source, true, Some(target.clone())).unwrap();
        assert!(!from_sidecar);
        assert_eq!(s0.pets[0].size, 100.0);
        let sig = compute_source_sig(&source).unwrap();
        assert_eq!(p.snapshot()["_bugPetRsSync"]["sourceSha256"], sig.sha256.as_str());
        assert_eq!(p.snapshot()["_bugPetRsSync"]["sourceLen"], sig.len);
        p.patch_pet_size("pet1", 222.0);
        p.flush().unwrap();
        drop(p);

        let (s1, p1, from_sidecar) =
            load_synced_state(&source, true, Some(target.clone())).unwrap();
        assert!(from_sidecar);
        assert_eq!(s1.pets[0].size, 222.0);
        assert!(!p1.is_dirty());
        assert!(!p1.source_changed());
        assert_eq!(p1.snapshot()["customRootKey"]["n"], 1);
        assert_eq!(fs::read_to_string(&source).unwrap(), src_text);
        drop(p1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_synced_load_never_writes() {
        let dir = temp_test_dir("sync-ro");
        let source = dir.join("source.json");
        let target = dir.join("appdata").join("state.json");
        let src_text = source_json(10.0, "ro");
        fs::write(&source, &src_text).unwrap();

        let (_s, mut p, from_sidecar) =
            load_synced_state(&source, false, Some(target.clone())).unwrap();
        assert!(!from_sidecar);
        p.patch_pet_size("pet1", 300.0);
        assert!(!p.is_dirty());
        p.flush().unwrap();
        assert!(!target.exists());
        assert_eq!(fs::read_to_string(&source).unwrap(), src_text);
        assert!(load_synced_state(&dir.join("missing.json"), false, None).is_err());
        drop(p);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_source_overrides_stale_sidecar_and_live_reload_works() {
        let dir = temp_test_dir("sync-live");
        let source = dir.join("source.json");
        let target = dir.join("appdata").join("state.json");
        fs::write(&source, source_json(10.0, "v1")).unwrap();

        let (_s, mut p, _) = load_synced_state(&source, true, Some(target.clone())).unwrap();
        p.patch_pet_size("pet1", 222.0);
        p.flush().unwrap();
        drop(p);

        // BUG Clicker rewrites its state.json: stale sidecar must lose.
        fs::write(&source, source_json(55.0, "version-two-longer")).unwrap();
        let (s2, mut p2, from_sidecar) =
            load_synced_state(&source, true, Some(target.clone())).unwrap();
        assert!(!from_sidecar);
        assert_eq!(s2.pets[0].x, 55.0);
        assert_eq!(s2.pets[0].size, 100.0);
        assert!(p2.is_dirty());
        p2.flush().unwrap();
        let saved: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(saved["pets"][0]["x"], 55.0);
        assert_eq!(
            saved["_bugPetRsSync"]["sourceSha256"],
            compute_source_sig(&source).unwrap().sha256.as_str()
        );

        // Live edits: identical rewrite is not a change.
        assert!(!p2.source_changed());
        fs::write(&source, source_json(55.0, "version-two-longer")).unwrap();
        assert!(!p2.source_changed());

        fs::write(&source, source_json(77.0, "v3-even-longer-note-text")).unwrap();
        assert!(p2.source_changed());
        let s3 = p2.reload_from_source().unwrap();
        assert_eq!(s3.pets[0].x, 77.0);
        assert_eq!(p2.snapshot()["pets"][0]["x"], 77.0);
        assert_eq!(p2.snapshot()["customRootKey"]["n"], 1);
        assert!(p2.is_dirty());
        assert!(!p2.source_changed());

        // Mid-write garbage: reload fails and leaves the document untouched.
        fs::write(&source, "{\"pets\":[").unwrap();
        assert!(p2.source_changed());
        assert!(p2.reload_from_source().is_err());
        assert_eq!(p2.snapshot()["pets"][0]["x"], 77.0);

        drop(p2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_tweak_patches_update_keys_and_preserve_unknown_fields() {
        let dir = temp_test_dir("tweak");
        let source = dir.join("source.json");
        let target = dir.join("state.json");
        let src = json!({
            "pets": [{ "id": "pet1", "charId": "AlphaPet", "customPetField": "keep_me" }],
            "charPet": { "AlphaPet": { "customCharMeta": 7 } },
            "prefs": {
                "react": { "sense": 1, "device": "dev0", "customReact": true },
                "ui": { "theme": { "mode": "dark" }, "menuStyle": "radial" }
            },
            "customRootKey": [1, 2]
        });
        fs::write(&source, src.to_string()).unwrap();
        let mut p = StatePersister::new(&source, true, Some(target.clone()));

        p.patch_pet_field("pet1", "lock", json!(true), false);
        assert_eq!(p.snapshot()["pets"][0]["lock"], true);
        assert!(p.snapshot()["charPet"]["AlphaPet"].get("lock").is_none());

        p.patch_pet_field("pet1", "mute", json!(true), true);
        assert_eq!(p.snapshot()["charPet"]["AlphaPet"]["mute"], true);

        p.patch_pet_size("pet1", 144.0);
        assert_eq!(p.snapshot()["pets"][0]["size"], 144.0);
        assert_eq!(p.snapshot()["charPet"]["AlphaPet"]["size"], 144.0);
        assert_eq!(p.snapshot()["charPet"]["AlphaPet"]["customCharMeta"], 7);
        assert_eq!(p.snapshot()["pets"][0]["customPetField"], "keep_me");

        let react = ReactPrefs {
            sense: 7.0,
            key_every: 3,
            beat_offset: 0.25,
            sound: "mic".to_string(),
            gain: 1.5,
            ..Default::default()
        };
        p.patch_react_prefs(&react);
        let r = &p.snapshot()["prefs"]["react"];
        assert_eq!(r["sense"], 7.0);
        assert_eq!(r["keyEvery"], 3);
        assert_eq!(r["beatOffset"], 0.25);
        assert_eq!(r["sound"], "mic");
        assert_eq!(r["gain"], 1.5);
        assert_eq!(r["device"], "dev0");
        assert_eq!(r["customReact"], true);

        let ui = UiPrefs {
            ghost: true,
            rest_idle: false,
            rest_min: 9.0,
            bubble_font: 18.0,
            reduce_motion: true,
            lang: "ja".to_string(),
            lite: true,
            ..Default::default()
        };
        p.patch_ui_prefs(&ui);
        let u = &p.snapshot()["prefs"]["ui"];
        assert_eq!(u["ghost"], true);
        assert_eq!(u["restIdle"], false);
        assert_eq!(u["restMin"], 9.0);
        assert_eq!(u["bubbleFont"], 18.0);
        assert_eq!(u["reduceMotion"], true);
        assert_eq!(u["lang"], "ja");
        assert_eq!(u["lite"], true);
        assert_eq!(u["theme"]["mode"], "dark");
        assert_eq!(u["menuStyle"], "radial");
        assert_eq!(p.snapshot()["customRootKey"][1], 2);

        drop(p);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawn_and_remove_pet_updates_array_and_next_id() {
        let dir = temp_test_dir("spawn");
        let source = dir.join("source.json");
        let target = dir.join("state.json");
        fs::write(&source, r#"{"pets":[],"nextId":2}"#).unwrap();

        let mut p = StatePersister::new(&source, true, Some(target.clone()));
        let new_pet = PetState {
            id: "pet2".to_string(),
            char_id: "BetaPet".to_string(),
            x: 120.0,
            y: 220.0,
            size: 180.0,
            always_on_top: true,
            ..Default::default()
        };
        p.patch_spawn_or_remove_pet("pet2", Some(&new_pet));
        p.flush().unwrap();

        let v1: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(v1["pets"].as_array().unwrap().len(), 1);
        assert_eq!(v1["pets"][0]["charId"], "BetaPet");
        assert_eq!(v1["nextId"], 3);

        p.patch_spawn_or_remove_pet("pet2", None);
        p.flush().unwrap();
        let v2: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(v2["pets"].as_array().unwrap().len(), 0);

        let _ = fs::remove_dir_all(&dir);
    }
}
