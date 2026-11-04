//! Ed25519 authentication for staged plan replies.

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use rand_core::OsRng;
use std::sync::Arc;
use type_bridge_contract::query_remote::{
    RemoteReplySignature, RemoteReplySigner, RemoteReplySigningDigest, RemoteReplyVerifier,
    RemoteSigningPublicKey,
};

/// Process- or deployment-epoch Ed25519 key used for remote reply authentication.
#[derive(Clone)]
pub struct RemoteReplySigningKey(Arc<SigningKey>);

impl RemoteReplySigningKey {
    /// Generate one fresh signer for a standalone executor lifetime.
    #[must_use]
    pub fn generate() -> Self {
        Self(Arc::new(SigningKey::generate(&mut OsRng)))
    }

    /// Construct a signer from an explicitly managed secret seed.
    ///
    /// This supports a shared execution epoch whose replay store and signer are
    /// provisioned together. The secret is never exposed by `Debug` or errors.
    #[must_use]
    pub fn from_secret_bytes(secret: [u8; 32]) -> Self {
        Self(Arc::new(SigningKey::from_bytes(&secret)))
    }

    /// Return the exact public key carried by the capability advertisement.
    #[must_use]
    pub fn public_key(&self) -> RemoteSigningPublicKey {
        RemoteSigningPublicKey::from_bytes(self.0.verifying_key().to_bytes())
    }
}

impl std::fmt::Debug for RemoteReplySigningKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteReplySigningKey")
            .field("public_key", &self.public_key())
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl RemoteReplySigner for RemoteReplySigningKey {
    fn public_key(&self) -> RemoteSigningPublicKey {
        self.public_key()
    }

    fn sign(&self, digest: &RemoteReplySigningDigest) -> RemoteReplySignature {
        RemoteReplySignature::from_bytes(self.0.sign(digest.as_bytes()).to_bytes())
    }
}

#[derive(Clone, Copy, Debug, Default)]
/// Stateless Ed25519 verifier for authenticated remote reply digests.
pub struct Ed25519RemoteReplyVerifier;

impl RemoteReplyVerifier for Ed25519RemoteReplyVerifier {
    fn verify(
        &self,
        key: RemoteSigningPublicKey,
        digest: &RemoteReplySigningDigest,
        signature: &RemoteReplySignature,
    ) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(key.as_bytes()) else {
            return false;
        };
        key.verify_strict(
            digest.as_bytes(),
            &Signature::from_bytes(signature.as_bytes()),
        )
        .is_ok()
    }
}
