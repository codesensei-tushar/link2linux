//! Small secrets in the desktop's keyring: the trait. The Linux
//! implementation is in [`crate::linux::secrets`].

use std::future::Future;

use zeroize::Zeroizing;

use crate::PlatformError;

/// The desktop's keyring, holding small secrets such as the desktop's
/// private key, each under a name.
///
/// A keyring encrypts what it holds on disk and keeps it locked until the
/// user logs in. It does not hide secrets from other programs the same user
/// runs outside a sandbox: on most desktops those can ask for them too.
pub trait SecretStore: Send + Sync {
    /// The secret stored under `name`, or `None` when there is none. May
    /// ask the user to unlock the keyring first.
    ///
    /// Fails with [`PlatformError::Unavailable`] when there is no keyring,
    /// it stays locked, or it does not answer in time.
    fn read(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<Zeroizing<Vec<u8>>>, PlatformError>> + Send;

    /// Stores `secret` under `name`, replacing what was there. `label` is
    /// what the keyring's own app shows for it.
    fn write(
        &self,
        name: &str,
        label: &str,
        secret: &[u8],
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
}
