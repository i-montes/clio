//! The cost of turning the passphrase into the master key. It is stored in the vault file, so it
//! can be raised without breaking the files written before: a file with no parameters was made
//! with [`Kdf::LEGACY`] and is read with those.

use crate::VaultError;
use serde::{Deserialize, Serialize};

/// Argon2id parameters: memory in KiB, passes, lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kdf {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl Kdf {
    /// What every vault used before the parameters were stored: the OWASP minimum.
    pub const LEGACY: Self = Self {
        m_kib: 19 * 1024,
        t: 2,
        p: 1,
    };
    /// RFC 9106's second recommended choice, with one lane: 64 MiB and 3 passes.
    pub const CURRENT: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };

    /// A file is not trusted for its own parameters: too cheap would make a weaker vault than
    /// ever existed, too dear would make the service hang or run out of memory on unlock.
    pub(crate) fn check(self) -> Result<Self, VaultError> {
        let ok = (Self::LEGACY.m_kib..=256 * 1024).contains(&self.m_kib)
            && (Self::LEGACY.t..=10).contains(&self.t)
            && (1..=4).contains(&self.p);
        ok.then_some(self).ok_or(VaultError::Format)
    }

    pub fn is_current(self) -> bool {
        self == Self::CURRENT
    }
}

impl std::fmt::Display for Kdf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "argon2id m={} MiB t={} p={}",
            self.m_kib / 1024,
            self.t,
            self.p
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{Dest, Kind, PersonCap, Scope, SecretSpec, Vault, VaultError};
    use clio_core::SystemClock;
    use std::path::Path;
    use std::sync::Arc;

    const PASS: &str = "una frase larga de prueba";
    const VALUE: &str = "valor-secreto-del-baul-31415";

    fn spec(name: &str, value: &str) -> SecretSpec {
        SecretSpec {
            name: name.into(),
            value: value.into(),
            kind: Kind::ApiKey,
            destinations: vec![Dest::new("http", "example.com")],
            scope: Scope::Personal,
            rotate_every_ms: None,
            expires_at_ms: None,
        }
    }

    /// A vault file as the versions before the parameters were stored wrote it: made with the old
    /// cost and without a `kdf` field.
    fn legacy_file(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("vault.bin");
        let (v, cap) =
            Vault::init_with(Some(&path), PASS, Arc::new(SystemClock), Kdf::LEGACY).unwrap();
        v.put(&cap, spec("clave", VALUE)).unwrap();
        v.put(&cap, spec("otra", "otro-valor-secreto-27182"))
            .unwrap();
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("kdf");
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        path
    }

    fn stored_kdf(path: &Path) -> Option<Kdf> {
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        json.get("kdf")
            .map(|k| serde_json::from_value(k.clone()).unwrap())
    }

    fn open(path: &Path) -> Vault {
        Vault::open(path, Arc::new(SystemClock)).unwrap()
    }

    fn cap() -> PersonCap {
        PersonCap::for_person_socket()
    }

    fn knows(v: &Vault, text: &str) -> bool {
        v.scrubber().scrub(text) != text
    }

    #[test]
    fn a_new_vault_records_the_current_cost_and_opens_with_it() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("vault.bin");
        let (v, c) = Vault::init(Some(&path), PASS, Arc::new(SystemClock)).unwrap();
        v.put(&c, spec("clave", VALUE)).unwrap();
        assert_eq!(stored_kdf(&path), Some(Kdf::CURRENT));
        let again = open(&path);
        assert!(again.unlock("otra frase").is_err());
        again.unlock(PASS).unwrap();
        assert!(knows(&again, VALUE));
    }

    #[test]
    fn a_file_without_parameters_is_read_with_the_old_cost() {
        let t = tempfile::tempdir().unwrap();
        let path = legacy_file(t.path());
        assert_eq!(stored_kdf(&path), None);
        let v = open(&path);
        assert!(!v.kdf().is_current());
        v.unlock(PASS).unwrap();
        assert!(knows(&v, VALUE));
    }

    #[test]
    fn upgrading_re_seals_everything_keeps_a_copy_and_changes_nothing_else() {
        let t = tempfile::tempdir().unwrap();
        let path = legacy_file(t.path());
        let before = std::fs::read(&path).unwrap();
        let salt_before =
            serde_json::from_slice::<serde_json::Value>(&before).unwrap()["salt"].clone();
        let v = open(&path);
        v.unlock(PASS).unwrap();
        assert!(v.upgrade_kdf(&cap(), PASS).unwrap());
        assert!(v.kdf().is_current());
        assert_eq!(stored_kdf(&path), Some(Kdf::CURRENT));
        let after =
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap();
        assert_ne!(after["salt"], salt_before, "a new salt");
        // The copy of the old file is there, private, and is the old file.
        let copy = path.with_extension("pre-argon2");
        assert_eq!(std::fs::read(&copy).unwrap(), before);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Everything in the file opens with the key of the new cost, the digests included (a lazy
        // unlock would fall back to the values and hide a digest sealed with the old key).
        let data: crate::FileData = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let key = crate::crypto::derive(PASS, &data.salt, Kdf::CURRENT).unwrap();
        assert!(crate::crypto::open(&key, b"check", &data.check).is_some());
        assert_eq!(data.secrets.len(), 2);
        for s in &data.secrets {
            let name = &s.meta.name;
            assert!(
                crate::crypto::open(&key, name.as_bytes(), &s.blob).is_some(),
                "{name}"
            );
            assert!(
                crate::crypto::open(&key, &crate::prints_aad(name), &s.prints).is_some(),
                "{name}: digests"
            );
        }
        // Same passphrase, same secrets, after a reopen; the vault that did it keeps working.
        let again = open(&path);
        assert!(again.unlock("otra frase").is_err());
        again.unlock(PASS).unwrap();
        assert!(knows(&again, VALUE) && knows(&again, "otro-valor-secreto-27182"));
        v.put(&cap(), spec("nueva", "valor-nuevo-secreto-16180"))
            .unwrap();
        let third = open(&path);
        third.unlock(PASS).unwrap();
        assert!(knows(&third, "valor-nuevo-secreto-16180") && knows(&third, VALUE));
    }

    #[test]
    fn upgrading_a_locked_vault_keeps_it_locked() {
        let t = tempfile::tempdir().unwrap();
        let path = legacy_file(t.path());
        let v = open(&path);
        assert!(v.is_locked());
        assert!(v.upgrade_kdf(&cap(), PASS).unwrap());
        assert!(v.is_locked());
        v.unlock(PASS).unwrap();
        assert!(knows(&v, VALUE));
    }

    #[test]
    fn a_wrong_passphrase_upgrades_nothing() {
        let t = tempfile::tempdir().unwrap();
        let path = legacy_file(t.path());
        let before = std::fs::read(&path).unwrap();
        let v = open(&path);
        assert!(matches!(
            v.upgrade_kdf(&cap(), "otra frase"),
            Err(VaultError::BadPassphrase)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!path.with_extension("pre-argon2").exists());
        assert!(!v.kdf().is_current());
    }

    #[test]
    fn upgrading_twice_is_a_no_op() {
        let t = tempfile::tempdir().unwrap();
        let path = legacy_file(t.path());
        let v = open(&path);
        assert!(v.upgrade_kdf(&cap(), PASS).unwrap());
        let once = std::fs::read(&path).unwrap();
        assert!(!v.upgrade_kdf(&cap(), PASS).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), once);
    }

    #[test]
    fn a_secret_that_cannot_be_opened_stops_the_upgrade_and_loses_nothing() {
        let t = tempfile::tempdir().unwrap();
        let path = legacy_file(t.path());
        // Damage one sealed value in the file: flip a bit, so the byte always changes (setting it
        // to a fixed value left it as it was once in 256 runs).
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let byte = json["secrets"][0]["blob"][30].as_u64().unwrap();
        json["secrets"][0]["blob"][30] = serde_json::json!(byte ^ 1);
        let damaged = serde_json::to_vec(&json).unwrap();
        std::fs::write(&path, &damaged).unwrap();
        let v = open(&path);
        assert!(v.upgrade_kdf(&cap(), PASS).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            damaged,
            "the file is as it was"
        );
        assert!(!v.kdf().is_current());
    }

    #[test]
    fn a_file_may_not_set_its_own_cost_out_of_range() {
        let t = tempfile::tempdir().unwrap();
        let path = legacy_file(t.path());
        for bad in [
            Kdf {
                m_kib: 1024,
                t: 2,
                p: 1,
            },
            Kdf {
                m_kib: 19 * 1024,
                t: 1,
                p: 1,
            },
            Kdf {
                m_kib: 4 * 1024 * 1024,
                t: 3,
                p: 1,
            },
            Kdf {
                m_kib: 64 * 1024,
                t: 1000,
                p: 1,
            },
            Kdf {
                m_kib: 64 * 1024,
                t: 3,
                p: 0,
            },
            Kdf {
                m_kib: 64 * 1024,
                t: 3,
                p: 64,
            },
        ] {
            let mut json: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            json["kdf"] = serde_json::to_value(bad).unwrap();
            let p = t.path().join("tampered.bin");
            std::fs::write(&p, serde_json::to_vec(&json).unwrap()).unwrap();
            assert!(
                matches!(
                    Vault::open(&p, Arc::new(SystemClock)),
                    Err(VaultError::Format)
                ),
                "{bad:?}"
            );
        }
    }
}
