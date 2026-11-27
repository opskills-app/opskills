//! Finding vault secrets by reference, for runs, notifications and email: a path in the
//! vault of the workspace that uses it, or `shared/<path>` for a shared secret. This is
//! the one place that decides whether a workspace may use a secret.

use crate::model::{DEFAULT_WORKSPACE, SecretRef, secret_binding, shared_secret_binding};
use crate::store::Store;
use crate::vault::{Sealed, Vault};

/// Whether runbooks of `workspace` can use `reference`: `Ok(Err(why))` when they can't.
pub fn check(store: &Store, workspace: &str, reference: &str) -> anyhow::Result<Result<(), String>> {
    Ok(match SecretRef::parse(reference) {
        Err(e) => Err(e),
        Ok(SecretRef::Workspace(path)) => {
            if store.secret(workspace, &path)?.is_some() {
                Ok(())
            } else {
                Err(format!("The vault has no secret named {path}. Add it in the vault first."))
            }
        }
        Ok(SecretRef::Shared(path)) => match store.shared_secret(&path)? {
            None => Err(format!("There's no shared secret named {path}.")),
            Some(s) if !s.meta.available_to(workspace) => Err(format!("The shared secret {path} isn't shared with this workspace. Ask a server admin to share it.")),
            Some(_) => Ok(()),
        },
    })
}

/// The value of `reference` for runbooks of `workspace`.
pub fn value(store: &Store, vault: &Vault, workspace: &str, reference: &str) -> Result<String, String> {
    match SecretRef::parse(reference)? {
        SecretRef::Workspace(path) => {
            let s = store.secret(workspace, &path).map_err(|e| e.to_string())?.ok_or_else(|| format!("the vault has no secret named {path}"))?;
            open(vault, &secret_binding(workspace, &path), s.nonce, s.ciphertext, reference)
        }
        SecretRef::Shared(path) => {
            let s = store.shared_secret(&path).map_err(|e| e.to_string())?.ok_or_else(|| format!("there's no shared secret named {path}"))?;
            if !s.meta.available_to(workspace) {
                return Err(format!("the shared secret {path} isn't shared with this workspace"));
            }
            open(vault, &shared_secret_binding(&path), s.nonce, s.ciphertext, reference)
        }
    }
}

/// For server-wide settings (the email password): any shared secret, or a secret of the
/// Default workspace, which held the email password before there were shared secrets.
pub fn server_check(store: &Store, reference: &str) -> anyhow::Result<Result<(), String>> {
    Ok(match SecretRef::parse(reference) {
        Err(e) => Err(e),
        Ok(SecretRef::Shared(path)) => {
            if store.shared_secret(&path)?.is_some() {
                Ok(())
            } else {
                Err(format!("There's no shared secret named {path}. Add it under Vault > Shared first."))
            }
        }
        Ok(SecretRef::Workspace(path)) => {
            if store.secret(DEFAULT_WORKSPACE, &path)?.is_some() {
                Ok(())
            } else {
                Err(format!("The Default workspace's vault has no secret named {path}. Use a shared secret instead."))
            }
        }
    })
}

/// The value of a server-wide reference; see [`server_check`].
pub fn server_value(store: &Store, vault: &Vault, reference: &str) -> Result<String, String> {
    match SecretRef::parse(reference)? {
        SecretRef::Shared(path) => {
            let s = store.shared_secret(&path).map_err(|e| e.to_string())?.ok_or_else(|| format!("there's no shared secret named {path}"))?;
            open(vault, &shared_secret_binding(&path), s.nonce, s.ciphertext, reference)
        }
        SecretRef::Workspace(_) => value(store, vault, DEFAULT_WORKSPACE, reference),
    }
}

fn open(vault: &Vault, binding: &str, nonce: String, ciphertext: String, reference: &str) -> Result<String, String> {
    let value = vault.open(binding, &Sealed { nonce, ciphertext }).map_err(|e| format!("opening {reference}: {e}"))?;
    Ok(String::from_utf8_lossy(&value).into_owned())
}
