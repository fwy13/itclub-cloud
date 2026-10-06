use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Nonce};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};

pub fn random_token()->String { let mut b=[0u8;32];rand::thread_rng().fill_bytes(&mut b);URL_SAFE_NO_PAD.encode(b) }
pub fn sha(s:impl AsRef<[u8]>)->String { hex::encode(Sha256::digest(s.as_ref())) }
pub async fn password_hash(password:String)->anyhow::Result<String> {
    if password.len()<10 || password.len()>1024 { anyhow::bail!("Mật khẩu cần từ 10 đến 1024 ký tự"); }
    tokio::task::spawn_blocking(move|| {
        let salt=SaltString::generate(&mut rand::rngs::OsRng);
        Argon2::default().hash_password(password.as_bytes(),&salt).map(|h|h.to_string()).map_err(|e|anyhow::anyhow!(e.to_string()))
    }).await?
}
pub async fn verify(password:String, hash:String)->bool {
    tokio::task::spawn_blocking(move|| PasswordHash::new(&hash).map(|h|Argon2::default().verify_password(password.as_bytes(),&h).is_ok()).unwrap_or(false)).await.unwrap_or(false)
}
pub fn seal(key:&[u8;32],plain:&str)->anyhow::Result<String> {
    let cipher=Aes256Gcm::new_from_slice(key).map_err(|_|anyhow::anyhow!("Invalid key"))?;
    let mut nonce=[0;12];rand::thread_rng().fill_bytes(&mut nonce);
    let mut out=nonce.to_vec();
    out.extend(cipher.encrypt(Nonce::from_slice(&nonce),plain.as_bytes()).map_err(|_|anyhow::anyhow!("Encryption failed"))?);
    Ok(STANDARD.encode(out))
}
pub fn open(key:&[u8;32],ciphertext:&str)->anyhow::Result<String> {
    let b=STANDARD.decode(ciphertext)?;if b.len()<28 {anyhow::bail!("Invalid encrypted value");}
    let cipher=Aes256Gcm::new_from_slice(key).map_err(|_|anyhow::anyhow!("Invalid key"))?;
    Ok(String::from_utf8(cipher.decrypt(Nonce::from_slice(&b[..12]),&b[12..]).map_err(|_|anyhow::anyhow!("Không giải mã được dữ liệu. Kiểm tra master.key"))?)?)
}
