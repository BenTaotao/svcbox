use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use std::fs;
use std::path::Path;

use crate::error::{AppError, Result};

const PREFIX: &str = "enc:v1:";

/// 本机凭据加密：密钥文件只留在数据目录，SQLite 中不出现明文密码。
pub struct SecretCipher {
    key: [u8; 32],
}

impl SecretCipher {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let raw = fs::read(path)?;
            if raw.len() != 32 {
                return Err(AppError::other("secret.key 已损坏（长度不是 32 字节）"));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&raw);
            return Ok(Self { key });
        }
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| AppError::other(format!("随机数生成失败: {e}")))?;
        fs::write(path, key)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
        Ok(Self { key })
    }

    pub fn encrypt(&self, plain: &str) -> Result<String> {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.key));
        let mut nonce_bytes = [0u8; 12];
        getrandom::fill(&mut nonce_bytes)
            .map_err(|e| AppError::other(format!("随机数生成失败: {e}")))?;
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plain.as_bytes())
            .map_err(|e| AppError::other(format!("加密失败: {e}")))?;
        let mut out = Vec::with_capacity(12 + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(format!(
            "{PREFIX}{}",
            base64::engine::general_purpose::STANDARD.encode(out)
        ))
    }

    pub fn decrypt(&self, value: &str) -> Result<String> {
        let Some(encoded) = value.strip_prefix(PREFIX) else {
            return Err(AppError::other("凭据格式不合法"));
        };
        let raw = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| AppError::other(format!("凭据解码失败: {e}")))?;
        if raw.len() < 13 {
            return Err(AppError::other("凭据内容已损坏"));
        }
        let (nonce_bytes, ciphertext) = raw.split_at(12);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.key));
        let plain = cipher
            .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
            .map_err(|_| AppError::other("凭据解密失败，secret.key 可能已被更换"))?;
        String::from_utf8(plain).map_err(|e| AppError::other(format!("凭据内容不是 UTF-8: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_roundtrip() {
        let dir = std::env::temp_dir().join(format!("svcbox-secret-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let key_path = dir.join("secret.key");
        let cipher = SecretCipher::load_or_create(&key_path).unwrap();
        let enc = cipher.encrypt("root123").unwrap();
        assert!(enc.starts_with(PREFIX));
        assert_ne!(enc, "root123");
        assert_eq!(cipher.decrypt(&enc).unwrap(), "root123");
        let _ = fs::remove_dir_all(dir);
    }
}
