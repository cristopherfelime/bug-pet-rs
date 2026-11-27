use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use std::fs;
use std::path::Path;
use windows::core::PWSTR;
use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};

const REST_MAGIC: &[u8; 4] = b"BCP1";
const LOCAL_MAGIC: &[u8; 5] = b"BCPL1";

#[derive(Clone)]
pub struct ProtectKey {
    rest_key: Option<[u8; 32]>,
    pub mode: &'static str,
}

impl ProtectKey {
    /// Initializes the local `.bcp` decryption key strictly in memory.
    /// Never prints, logs, or writes key material or decrypted image bytes to disk.
    #[allow(dead_code)]
    pub fn init(data_dir: &Path) -> Result<Self, String> {
        Self::init_with_key(data_dir, None)
    }

    /// Initializes `.bcp` decryption support in memory from an optional transport key.
    /// Falls back to `"plain-only"` mode when no valid transport key is configured.
    pub fn init_with_key(data_dir: &Path, cli_key: Option<&str>) -> Result<Self, String> {
        let Some(transport) = load_transport_key(data_dir, cli_key)? else {
            return Ok(Self {
                rest_key: None,
                mode: "plain-only",
            });
        };

        let hostname = get_hostname();
        let username = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "User".to_string());

        let local_path = data_dir.join("keys").join("protect-local.bin");
        if local_path.is_file() {
            let raw = fs::read(&local_path)
                .map_err(|e| format!("Failed reading {}: {e}", local_path.display()))?;
            if raw.len() > LOCAL_MAGIC.len() + 16 + 28 && &raw[..5] == LOCAL_MAGIC {
                let salt = &raw[5..21];
                let enc = &raw[21..];
                let mut local_secret = Vec::with_capacity(64);
                local_secret.extend_from_slice(&transport);
                local_secret.extend_from_slice(format!("\nhost:{hostname}\n:{username}").as_bytes());
                let local_key = hkdf_sha256(&local_secret, salt, b"rest-linux-machine-v1")?;
                if let Ok(plain) = aes_gcm_open(&local_key, enc) {
                    if plain.len() == 32 {
                        let mut rest_key = [0u8; 32];
                        rest_key.copy_from_slice(&plain);
                        return Ok(Self {
                            rest_key: Some(rest_key),
                            mode: "dpapi-local",
                        });
                    }
                }
            }
        }

        let fallback_salt = format!("{username}@{hostname}");
        let rest_key = hkdf_sha256(&transport, fallback_salt.as_bytes(), b"rest-fallback")?;
        Ok(Self {
            rest_key: Some(rest_key),
            mode: "fallback",
        })
    }

    /// Returns `true` when a `.bcp` decryption key is loaded in memory.
    #[allow(dead_code)]
    pub fn has_key(&self) -> bool {
        self.rest_key.is_some()
    }

    pub fn open_bcp(&self, data: &[u8]) -> Result<Vec<u8>, String> {
        let Some(ref rest_key) = self.rest_key else {
            return Err("Protected .bcp asset requires a local transport key (set BUG_PET_BCP_KEY, pass --bcp-key, or place key in %APPDATA%\\bug-pet-rs\\bcp-key.txt)".to_string());
        };
        if data.len() <= REST_MAGIC.len() + 28 || &data[..4] != REST_MAGIC {
            return Err("Invalid BCP1 header".to_string());
        }
        aes_gcm_open(rest_key, &data[4..])
    }
}

fn extract_key_line(raw: &str) -> Option<&str> {
    for line in raw.trim_start_matches('\u{feff}').lines() {
        let candidate = line.split('#').next().unwrap_or("").trim();
        if !candidate.is_empty() {
            return Some(candidate);
        }
    }
    None
}

fn read_key_file(path: &Path) -> Option<[u8; 32]> {
    if !path.is_file() {
        return None;
    }
    let raw = fs::read_to_string(path).ok()?;
    let line = extract_key_line(&raw)?;
    decode_hex_32(line).ok()
}

fn load_transport_key(data_dir: &Path, cli_key: Option<&str>) -> Result<Option<[u8; 32]>, String> {
    if let Some(raw) = cli_key {
        let key = extract_key_line(raw)
            .and_then(|s| decode_hex_32(s).ok())
            .ok_or_else(|| "Invalid --bcp-key: expected 64 hexadecimal characters".to_string())?;
        return Ok(Some(key));
    }

    if let Ok(val) = std::env::var("BUG_PET_BCP_KEY") {
        if let Some(key) = extract_key_line(&val).and_then(|s| decode_hex_32(s).ok()) {
            return Ok(Some(key));
        }
    }

    if let Ok(appdata) = std::env::var("APPDATA") {
        let path = Path::new(&appdata).join("bug-pet-rs").join("bcp-key.txt");
        if let Some(key) = read_key_file(&path) {
            return Ok(Some(key));
        }
    }

    if let Some(key) = read_key_file(&data_dir.join("keys").join("bcp-transport.hex")) {
        return Ok(Some(key));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if let Some(key) = read_key_file(&exe_dir.join("bcp-key.txt")) {
                return Ok(Some(key));
            }
        }
    }

    Ok(None)
}

fn get_hostname() -> String {
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    let ok = unsafe { GetComputerNameExW(ComputerNameDnsHostname, PWSTR(buf.as_mut_ptr()), &mut len) };
    if ok.is_ok() && len > 0 {
        return String::from_utf16_lossy(&buf[..len as usize]);
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
}

fn decode_hex_32(hex: &str) -> Result<[u8; 32], String> {
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid hex length".to_string());
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let s = std::str::from_utf8(chunk).map_err(|_| "Bad UTF-8 hex".to_string())?;
        out[i] = u8::from_str_radix(s, 16).map_err(|_| "Bad hex digit".to_string())?;
    }
    Ok(out)
}

fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8]) -> Result<[u8; 32], String> {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm)
        .map_err(|_| "HKDF-SHA256 expand failed".to_string())?;
    Ok(okm)
}

/// Decrypts Node's `seal()` wire layout: `[12B nonce][16B auth tag][ciphertext]`.
fn aes_gcm_open(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>, String> {
    if blob.len() < 28 {
        return Err("Encrypted blob too short".to_string());
    }
    let nonce = Nonce::from_slice(&blob[..12]);
    let tag = &blob[12..28];
    let ciphertext = &blob[28..];

    let mut ct_with_tag = Vec::with_capacity(ciphertext.len() + 16);
    ct_with_tag.extend_from_slice(ciphertext);
    ct_with_tag.extend_from_slice(tag);

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "Invalid AES-256-GCM key".to_string())?;
    cipher
        .decrypt(nonce, ct_with_tag.as_slice())
        .map_err(|_| "AES-GCM decryption/authentication failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::AeadInPlace;

    #[test]
    fn roundtrip_bcpl1_and_bcp1_matches_protect_js_layout() {
        let key = [0x42u8; 32];
        let nonce_bytes = [0x11u8; 12];
        let plaintext = b"\x89PNG\r\n\x1a\nhello-bug-clicker";

        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let mut buf = plaintext.to_vec();
        let tag = cipher
            .encrypt_in_place_detached(Nonce::from_slice(&nonce_bytes), b"", &mut buf)
            .unwrap();

        let mut sealed = Vec::new();
        sealed.extend_from_slice(REST_MAGIC);
        sealed.extend_from_slice(&nonce_bytes);
        sealed.extend_from_slice(tag.as_slice());
        sealed.extend_from_slice(&buf);

        let pk = ProtectKey {
            rest_key: Some(key),
            mode: "test",
        };
        assert!(pk.has_key());
        let dec = pk.open_bcp(&sealed).unwrap();
        assert_eq!(dec, plaintext);
    }

    #[test]
    fn plain_only_mode_errors_cleanly_on_bcp() {
        let pk = ProtectKey {
            rest_key: None,
            mode: "plain-only",
        };
        assert!(!pk.has_key());
        assert_eq!(pk.mode, "plain-only");

        let err = pk
            .open_bcp(b"BCP10123456789012345678901234567890123456789")
            .unwrap_err();
        assert!(err.contains("local transport key"));
        assert!(err.contains("BUG_PET_BCP_KEY"));

        assert!(ProtectKey::init_with_key(Path::new("."), Some("not-hex")).is_err());

        let sample =
            "\n  # comment\n  0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  # inline\n";
        let extracted = extract_key_line(sample).expect("expected key line");
        assert_eq!(
            extracted,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert!(decode_hex_32(extracted).is_ok());
    }

    #[test]
    fn live_bcp_decodes_in_memory_without_disk_writes() {
        let Ok(dir) = crate::config::discover_data_dir(None) else {
            return;
        };
        let pk = ProtectKey::init(&dir).expect("ProtectKey::init must succeed");
        if !pk.has_key() {
            return;
        }
        let Ok(entries) = fs::read_dir(dir.join("characters")) else {
            return;
        };
        for entry in entries.flatten() {
            let cdir = entry.path();
            if !cdir.is_dir() {
                continue;
            }
            let p = cdir.join("body.bcp");
            if p.is_file() {
                let enc = fs::read(&p).unwrap();
                let dec = pk.open_bcp(&enc).expect("body.bcp must decrypt in memory");
                let img = image::load_from_memory(&dec).expect("decrypted image must decode");
                assert!(img.width() > 0 && img.height() > 0);
            }
        }
    }
}
