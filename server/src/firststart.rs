//! First start: create the server identity, encryption key and admin password.
//! Each install gets its own values; nothing is a shared default.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;

pub struct State {
    pub server_id: String,
    /// Set only when this start created a new admin password.
    pub initial_password: Option<String>,
    /// The 256-bit vault key, hex.
    pub master_key: String,
}

const PASSWORD_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn prepare(cfg: &Config) -> Result<State> {
    let dir = &cfg.data.dir;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // Best effort: tighten permissions on a directory we created or own.
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o750));

    let server_id = read_or_create(&dir.join("server-id"), || uuid::Uuid::now_v7().to_string())?;
    let master_key = read_or_create(&dir.join("master.key"), || hex::encode(random_bytes::<32>()))?;

    let admin_hash = dir.join("admin.hash");
    let mut initial_password = None;
    if !admin_hash.exists() {
        let password = random_password(24);
        let salt = random_bytes::<16>();
        let mut out = [0u8; 32];
        argon2::Argon2::default()
            .hash_password_into(password.as_bytes(), &salt, &mut out)
            .map_err(|e| anyhow::anyhow!("hashing the admin password: {e}"))?;
        let record = format!("argon2id$v=19$m=19456,t=2,p=1${}${}\n", hex::encode(salt), hex::encode(out));
        write_new(&admin_hash, record.as_bytes())?;
        announce_admin_password(&password);
        initial_password = Some(password);
    }

    tracing::info!(server_id = %server_id, data_dir = %dir.display(), "data directory ready");
    Ok(State { server_id, initial_password, master_key })
}

/// Read a value file, or create it with mode 0600 if it does not exist.
fn read_or_create(path: &Path, make: impl FnOnce() -> String) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(v) => Ok(v.trim().to_string()),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let value = make();
            write_new(path, format!("{value}\n").as_bytes())?;
            Ok(value)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("reading /dev/urandom");
    buf
}

fn random_password(len: usize) -> String {
    // Rejection sampling keeps every character equally likely.
    let limit = 256 - (256 % PASSWORD_ALPHABET.len());
    let mut out = String::with_capacity(len);
    while out.len() < len {
        for b in random_bytes::<64>() {
            if (b as usize) < limit && out.len() < len {
                out.push(PASSWORD_ALPHABET[b as usize % PASSWORD_ALPHABET.len()] as char);
            }
        }
    }
    out
}

fn announce_admin_password(password: &str) {
    eprintln!();
    eprintln!("==============================================================");
    eprintln!(" OpSkills first start");
    eprintln!(" Sign in at http://<host>:<port> as:");
    eprintln!(" Admin user:     admin");
    eprintln!(" Admin password: {password}");
    eprintln!(" This password is shown only once. Store it somewhere safe.");
    eprintln!("==============================================================");
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_use_the_alphabet_and_length() {
        let p = random_password(24);
        assert_eq!(p.len(), 24);
        assert!(p.bytes().all(|b| PASSWORD_ALPHABET.contains(&b)));
        assert_ne!(p, random_password(24));
    }

    #[test]
    fn first_start_is_idempotent_and_private() {
        let dir = std::env::temp_dir().join(format!("opskills-test-{}", uuid::Uuid::now_v7()));
        let mut cfg = Config::default();
        cfg.data.dir = dir.clone();
        let a = prepare(&cfg).unwrap();
        let b = prepare(&cfg).unwrap();
        assert_eq!(a.server_id, b.server_id, "server id must persist");
        for f in ["server-id", "master.key", "admin.hash"] {
            let mode = fs::metadata(dir.join(f)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{f} must be private");
        }
        assert_eq!(fs::read_to_string(dir.join("master.key")).unwrap().trim().len(), 64);
        fs::remove_dir_all(dir).unwrap();
    }
}
