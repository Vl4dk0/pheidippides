use phi_core::{Error, KeyStore, Profile, Store};
use std::fs::{File, OpenOptions};
use zeroize::Zeroizing;

const SERVICE: &str = "sk.pheidippides.desktop";

pub fn profile_id(profile: &Profile) -> String {
    blake3::hash(
        format!(
            "{}/{}/{}",
            profile.server, profile.space_id, profile.device_id
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string()
}

pub struct OsKeyStore {
    namespace: String,
    lock: File,
}
impl OsKeyStore {
    pub fn open(store: &Store, profile: &Profile) -> phi_core::Result<Self> {
        std::fs::create_dir_all(&store.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&store.root, std::fs::Permissions::from_mode(0o700))?;
        }
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        Ok(Self {
            namespace: profile_id(profile),
            lock: options.open(store.root.join("keychain.lock"))?,
        })
    }
    fn entry(&self, account: &str) -> phi_core::Result<keyring::Entry> {
        let account = blake3::hash(format!("{}/{}", self.namespace, account).as_bytes())
            .to_hex()
            .to_string();
        keyring::Entry::new(SERVICE, &account).map_err(keyring_error)
    }
}
fn keyring_error(error: keyring::Error) -> Error {
    Error::User(format!("OS keychain unavailable: {error}"))
}
impl KeyStore for OsKeyStore {
    fn get(&self, account: &str) -> phi_core::Result<Option<Zeroizing<Vec<u8>>>> {
        match self.entry(account)?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error(error)),
        }
    }
    fn put(&self, account: &str, secret: &[u8]) -> phi_core::Result<()> {
        if secret.is_empty() {
            return Err(Error::User("cannot store empty keychain secret".into()));
        }
        // Keyring has no portable compare-and-set. Serialize across app processes so an
        // accepted identity or epoch can never be overwritten by a racing client.
        fs2::FileExt::lock_exclusive(&self.lock)?;
        let result = (|| match self.get(account)? {
            Some(existing) if existing.as_slice() == secret => Ok(()),
            Some(_) => Err(Error::User(
                "conflicting keychain account; accepted keys cannot be replaced".into(),
            )),
            None => self
                .entry(account)?
                .set_secret(secret)
                .map_err(keyring_error),
        })();
        fs2::FileExt::unlock(&self.lock)?;
        result
    }
}
