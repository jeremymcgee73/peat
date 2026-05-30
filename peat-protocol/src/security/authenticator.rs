//! Device authenticator for challenge-response authentication.

use super::device_id::DeviceId;
use super::error::SecurityError;
use super::keypair::DeviceKeypair;
use super::{CHALLENGE_NONCE_SIZE, DEFAULT_CHALLENGE_TIMEOUT_SECS};
use peat_schema::security::v1::{Challenge, SignedChallengeResponse};
use rand_core::{OsRng, RngCore};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Current auth-handshake protocol version this build speaks (ADR-065).
///
/// Embedded in `Challenge.protocol_version` and
/// `SignedChallengeResponse.protocol_version`. The responder negotiates
/// `min(challenge.protocol_version, CURRENT_PROTOCOL_VERSION)` and uses
/// that version's signed-message byte construction. The verifier reads
/// `response.protocol_version` to know which construction to reconstruct.
///
/// **Version 0** is the pre-rc.21+1 fallback (no `protocol_version`
/// field on the wire; prost defaults it to 0). v0 signed-message:
/// `nonce || challenger_id || response.timestamp.seconds`.
///
/// **Version 1** (this build) extends v0 by appending
/// `response.protocol_version` (u32 LE, 4 bytes) to the signed bytes,
/// so a MITM cannot strip or modify the version field without
/// invalidating the signature.
///
/// See ADR-065 for the full negotiation rule and rollout semantics.
pub const CURRENT_PROTOCOL_VERSION: u32 = 1;

/// Prefix used by the v1 `respond_to_challenge` / `verify_response` path
/// when the version-mismatch case surfaces via
/// `SecurityError::AuthenticationFailed(msg)` (ADR-065).
///
/// External consumers that need to deterministically detect "peer
/// claims a protocol version we don't speak" pending the typed
/// `SecurityError::IncompatibleProtocolVersion` follow-up variant in
/// peat-mesh's security error module can match on this prefix:
///
/// ```ignore
/// match auth_result {
///     Err(SecurityError::AuthenticationFailed(msg))
///         if msg.starts_with(INCOMPATIBLE_PROTOCOL_VERSION_PREFIX) =>
///     {
///         // peer too new / too old for us
///     }
///     _ => { /* … */ }
/// }
/// ```
///
/// Once the typed variant lands, the prefix becomes redundant and is
/// removed; callers should switch to `matches!(_, SecurityError::IncompatibleProtocolVersion { .. })`.
pub const INCOMPATIBLE_PROTOCOL_VERSION_PREFIX: &str = "incompatible protocol version:";

/// Device authenticator manages challenge-response authentication.
///
/// # Overview
///
/// The authenticator uses Ed25519 signatures for mutual authentication:
/// 1. Generate a challenge with random nonce and timestamp
/// 2. Peer signs the challenge and returns their public key
/// 3. Verify signature and cache the verified peer identity
///
/// # Example
///
/// ```ignore
/// use peat_protocol::security::{DeviceKeypair, DeviceAuthenticator};
///
/// let keypair = DeviceKeypair::generate();
/// let authenticator = DeviceAuthenticator::new(keypair);
///
/// // Generate challenge for peer
/// let challenge = authenticator.generate_challenge();
///
/// // Peer creates response
/// let response = peer_authenticator.respond_to_challenge(&challenge)?;
///
/// // Verify response
/// let peer_id = authenticator.verify_response(&response)?;
/// println!("Authenticated peer: {}", peer_id);
/// ```
pub struct DeviceAuthenticator {
    /// This device's keypair
    keypair: DeviceKeypair,

    /// Verified peers cache
    verified_peers: RwLock<HashMap<DeviceId, VerifiedPeer>>,

    /// Challenge timeout duration
    challenge_timeout: Duration,
}

/// A verified peer's identity
#[derive(Debug, Clone)]
pub struct VerifiedPeer {
    /// The peer's device ID
    pub device_id: DeviceId,

    /// The peer's public key bytes
    pub public_key: [u8; 32],

    /// When this peer was verified
    pub verified_at: SystemTime,
}

impl DeviceAuthenticator {
    /// Create a new authenticator with the given keypair.
    pub fn new(keypair: DeviceKeypair) -> Self {
        Self::with_timeout(keypair, Duration::from_secs(DEFAULT_CHALLENGE_TIMEOUT_SECS))
    }

    /// Create an authenticator with a custom challenge timeout.
    pub fn with_timeout(keypair: DeviceKeypair, challenge_timeout: Duration) -> Self {
        DeviceAuthenticator {
            keypair,
            verified_peers: RwLock::new(HashMap::new()),
            challenge_timeout,
        }
    }

    /// Get this device's ID.
    pub fn device_id(&self) -> DeviceId {
        self.keypair.device_id()
    }

    /// Get this device's public key bytes.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.keypair.public_key_bytes()
    }

    /// Generate a challenge for authenticating a peer.
    ///
    /// The challenge contains:
    /// - Random 32-byte nonce
    /// - Current timestamp
    /// - This device's ID
    /// - Expiration timestamp
    /// - Protocol version this challenger speaks (ADR-065:
    ///   [`CURRENT_PROTOCOL_VERSION`])
    /// - Capability advertisements (ADR-065: empty by default at v1;
    ///   consumers driving feature-flagged behaviour can extend via
    ///   the public `Challenge.capabilities` field after this call)
    pub fn generate_challenge(&self) -> Challenge {
        let mut nonce = [0u8; CHALLENGE_NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let expires = now + self.challenge_timeout;

        Challenge {
            nonce: nonce.to_vec(),
            timestamp: Some(peat_schema::common::v1::Timestamp {
                seconds: now.as_secs(),
                nanos: now.subsec_nanos(),
            }),
            challenger_id: self.device_id().to_hex(),
            expires_at: Some(peat_schema::common::v1::Timestamp {
                seconds: expires.as_secs(),
                nanos: expires.subsec_nanos(),
            }),
            // ADR-065 version negotiation: advertise our maximum
            // supported protocol version so the responder can
            // negotiate `min(ours, theirs)`.
            protocol_version: CURRENT_PROTOCOL_VERSION,
            // ADR-065 capability advertising: empty at v1 by default;
            // callers can mutate the returned Challenge to populate
            // capabilities for soft-policy use cases.
            capabilities: Vec::new(),
        }
    }

    /// Create a signed response to a challenge.
    ///
    /// Signs the challenge data with this device's private key.
    ///
    /// **Version negotiation (ADR-065).** The responder picks
    /// `negotiated = min(challenge.protocol_version, CURRENT_PROTOCOL_VERSION)`,
    /// signs the byte construction associated with that version,
    /// and embeds `negotiated` in `response.protocol_version` so the
    /// verifier knows which construction to reconstruct.
    ///
    /// **Signed-message constructions by version:**
    ///
    /// - **v0** (pre-rc.21+1 challenger; `challenge.protocol_version == 0`):
    ///
    ///   ```text
    ///   signed = challenge.nonce
    ///         || challenge.challenger_id
    ///         || response.timestamp.seconds  (u64 LE, 8 bytes)
    ///   ```
    ///
    /// - **v1** (rc.21+1+; `challenge.protocol_version >= 1`):
    ///
    ///   ```text
    ///   signed = challenge.nonce
    ///         || challenge.challenger_id
    ///         || response.timestamp.seconds  (u64 LE, 8 bytes)
    ///         || response.protocol_version   (u32 LE, 4 bytes)
    ///   ```
    ///
    /// The signed message and the response's `timestamp` field share
    /// a single captured `SystemTime::now()` value (the peat#952 fix);
    /// signer and verifier reconstruct an identical byte string
    /// regardless of how long the auth flow takes or whether it
    /// spans a wall-clock second boundary.
    ///
    /// Replay protection: the challenge's `nonce` is the freshness
    /// anchor (challenger generates a fresh nonce per challenge),
    /// and the internal `check_challenge_expiry` helper enforces the
    /// challenge's `expires_at` window before signing. (Plain prose
    /// rather than an intra-doc link — `check_challenge_expiry` is
    /// private, so a `[\`Self::check_challenge_expiry\`]` link would
    /// fail `rustdoc::private-intra-doc-links` under
    /// `cargo doc -- -D warnings`.)
    ///
    /// **Capabilities (ADR-065 v1 semantics):** the responder
    /// advertises its capability set in `response.capabilities`.
    /// At v1 the field is **not** covered by the signature — see
    /// ADR-065 for the rationale and the v2 path. Callers driving
    /// soft-policy feature flagging can mutate the returned
    /// `SignedChallengeResponse.capabilities` after this call;
    /// hard-policy "reject peers missing a capability" should wait
    /// for the v2 signed-capability extension.
    pub fn respond_to_challenge(
        &self,
        challenge: &Challenge,
    ) -> Result<SignedChallengeResponse, SecurityError> {
        // Check challenge hasn't expired
        self.check_challenge_expiry(challenge)?;

        // ADR-065 version negotiation: pick min(peer, ours). v0 = the
        // pre-rc.21+1 byte construction; v1 = the same plus
        // response.protocol_version covered by the signature.
        let negotiated_version = challenge.protocol_version.min(CURRENT_PROTOCOL_VERSION);

        // Capture the response timestamp ONCE; use it for both the
        // signed-message byte string and the response.timestamp
        // field. This guarantees signer and verifier reconstruct
        // the same bytes (peat#952).
        let response_ts_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Build the signed-message bytes for the negotiated version.
        // The single shared helper keeps signer and verifier byte-
        // identical across both v0 and v1.
        let message = build_signed_message(
            &challenge.nonce,
            &challenge.challenger_id,
            response_ts_seconds,
            negotiated_version,
        );

        // Sign the message
        let signature = self.keypair.sign(&message);

        Ok(SignedChallengeResponse {
            challenge_nonce: challenge.nonce.clone(),
            public_key: self.keypair.public_key_bytes().to_vec(),
            signature: signature.to_bytes().to_vec(),
            timestamp: Some(peat_schema::common::v1::Timestamp {
                seconds: response_ts_seconds,
                nanos: 0,
            }),
            device_type: 0,       // DEVICE_TYPE_UNSPECIFIED for MVP
            certificates: vec![], // Empty for MVP (no X.509 chain)
            // ADR-065: negotiated version covered by the signature
            // from v1 onward; advertised verbatim on the wire so the
            // verifier knows which construction to reconstruct.
            protocol_version: negotiated_version,
            // ADR-065 capabilities: empty at v1 by default.
            capabilities: Vec::new(),
        })
    }

    /// Verify a peer's challenge response.
    ///
    /// On success, caches the peer's identity and returns their DeviceId.
    ///
    /// **Version negotiation (ADR-065).** Reads
    /// `response.protocol_version` to know which signed-message byte
    /// construction the responder used. A value greater than
    /// [`CURRENT_PROTOCOL_VERSION`] surfaces as
    /// `SecurityError::AuthenticationFailed(msg)` with the
    /// [`INCOMPATIBLE_PROTOCOL_VERSION_PREFIX`] prefix — distinct from
    /// `InvalidSignature` so operators can distinguish "peer is too
    /// new for me" from "signature was tampered with."
    pub fn verify_response(
        &self,
        response: &SignedChallengeResponse,
    ) -> Result<DeviceId, SecurityError> {
        // ADR-065 version negotiation: detect "peer claims a version
        // we don't speak" before attempting signature verification.
        // This way the operator-visible error names the actual
        // problem ("incompatible protocol version") rather than the
        // downstream symptom ("Verification equation was not
        // satisfied"). Future-version response cannot have been
        // produced by a v1 responder negotiating down (it would have
        // capped at min(peer, our_max)), so a value > our max means
        // the response was constructed by a higher-version build —
        // a coordination problem, not a tampering problem.
        if response.protocol_version > CURRENT_PROTOCOL_VERSION {
            return Err(SecurityError::AuthenticationFailed(format!(
                "{INCOMPATIBLE_PROTOCOL_VERSION_PREFIX} peer claims {peer}, our maximum is {ours}",
                peer = response.protocol_version,
                ours = CURRENT_PROTOCOL_VERSION,
            )));
        }

        // Parse public key
        let public_key = DeviceKeypair::verifying_key_from_bytes(&response.public_key)?;

        // Derive device ID from public key
        let peer_device_id = DeviceId::from_public_key(&public_key);

        // Reconstruct the signed-message bytes for the negotiated
        // version. `build_signed_message` is the single source of
        // truth for both signer and verifier byte construction; v0
        // and v1 differ only in whether response.protocol_version is
        // appended.
        let response_ts_seconds = response
            .timestamp
            .as_ref()
            .map(|ts| ts.seconds)
            .unwrap_or(0);
        let message = build_signed_message(
            &response.challenge_nonce,
            &self.device_id().to_hex(),
            response_ts_seconds,
            response.protocol_version,
        );

        // Parse and verify signature
        let signature = DeviceKeypair::signature_from_bytes(&response.signature)?;
        DeviceKeypair::verify_with_key(&public_key, &message, &signature)?;

        // Cache the verified peer
        let verified_peer = VerifiedPeer {
            device_id: peer_device_id,
            public_key: public_key.to_bytes(),
            verified_at: SystemTime::now(),
        };

        self.verified_peers
            .write()
            .map_err(|e| SecurityError::Internal(format!("lock poisoned: {}", e)))?
            .insert(peer_device_id, verified_peer);

        Ok(peer_device_id)
    }

    /// Check if a peer is verified.
    pub fn is_verified(&self, device_id: &DeviceId) -> bool {
        self.verified_peers
            .read()
            .map(|cache| cache.contains_key(device_id))
            .unwrap_or(false)
    }

    /// Get a verified peer's info.
    pub fn get_verified_peer(&self, device_id: &DeviceId) -> Option<VerifiedPeer> {
        self.verified_peers
            .read()
            .ok()
            .and_then(|cache| cache.get(device_id).cloned())
    }

    /// Remove a peer from the verified cache.
    pub fn remove_peer(&self, device_id: &DeviceId) {
        if let Ok(mut cache) = self.verified_peers.write() {
            cache.remove(device_id);
        }
    }

    /// Clear all verified peers.
    pub fn clear_verified_peers(&self) {
        if let Ok(mut cache) = self.verified_peers.write() {
            cache.clear();
        }
    }

    /// Get number of verified peers.
    pub fn verified_peer_count(&self) -> usize {
        self.verified_peers
            .read()
            .map(|cache| cache.len())
            .unwrap_or(0)
    }

    /// Check if a challenge has expired.
    fn check_challenge_expiry(&self, challenge: &Challenge) -> Result<(), SecurityError> {
        if let Some(expires) = &challenge.expires_at {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();

            if now.as_secs() > expires.seconds {
                return Err(SecurityError::ChallengeExpired(expires.seconds));
            }
        }
        Ok(())
    }
}

/// Build the byte string covered by the Ed25519 signature in the
/// challenge-response handshake (ADR-065).
///
/// Shared between [`DeviceAuthenticator::respond_to_challenge`] (the
/// signer) and [`DeviceAuthenticator::verify_response`] (the verifier)
/// so the byte construction lives in exactly one place, byte-identical
/// across the two call sites. Any future change to the construction
/// (e.g. v2 extending coverage to capabilities) goes here and shows up
/// on both sides.
///
/// # Constructions by version
///
/// - **v0** (`protocol_version == 0`, pre-rc.21+1 peer):
///
///   ```text
///   nonce || challenger_id || response_ts_seconds  (u64 LE, 8 bytes)
///   ```
///
/// - **v1+** (`protocol_version >= 1`, this build's
///   [`CURRENT_PROTOCOL_VERSION`] = 1):
///
///   ```text
///   nonce || challenger_id || response_ts_seconds  (u64 LE, 8 bytes)
///                          || protocol_version     (u32 LE, 4 bytes)
///   ```
///
/// The v1 extension binds the version field into the signature so a
/// MITM cannot strip or modify it without invalidating the signature.
/// A v0 peer's `protocol_version == 0` falls through the `>= 1` arm
/// and gets the v0 construction, preserving wire-compat with
/// pre-version-token peers.
fn build_signed_message(
    nonce: &[u8],
    challenger_id: &str,
    response_ts_seconds: u64,
    protocol_version: u32,
) -> Vec<u8> {
    let mut message = Vec::with_capacity(
        nonce.len() + challenger_id.len() + 8 + if protocol_version >= 1 { 4 } else { 0 },
    );
    message.extend_from_slice(nonce);
    message.extend_from_slice(challenger_id.as_bytes());
    message.extend_from_slice(&response_ts_seconds.to_le_bytes());
    if protocol_version >= 1 {
        message.extend_from_slice(&protocol_version.to_le_bytes());
    }
    message
}

impl std::fmt::Debug for DeviceAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceAuthenticator")
            .field("device_id", &self.device_id())
            .field("verified_peer_count", &self.verified_peer_count())
            .field("challenge_timeout", &self.challenge_timeout)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_authenticator() -> DeviceAuthenticator {
        let keypair = DeviceKeypair::generate();
        DeviceAuthenticator::new(keypair)
    }

    #[test]
    fn test_generate_challenge() {
        let auth = create_test_authenticator();
        let challenge = auth.generate_challenge();

        assert_eq!(challenge.nonce.len(), CHALLENGE_NONCE_SIZE);
        assert!(!challenge.challenger_id.is_empty());
        assert!(challenge.timestamp.is_some());
        assert!(challenge.expires_at.is_some());
    }

    #[test]
    fn test_challenge_nonce_unique() {
        let auth = create_test_authenticator();
        let c1 = auth.generate_challenge();
        let c2 = auth.generate_challenge();

        assert_ne!(c1.nonce, c2.nonce);
    }

    #[test]
    fn test_respond_to_challenge() {
        let auth1 = create_test_authenticator();
        let auth2 = create_test_authenticator();

        let challenge = auth1.generate_challenge();
        let response = auth2.respond_to_challenge(&challenge).unwrap();

        assert_eq!(response.public_key.len(), 32);
        assert_eq!(response.signature.len(), 64);
        assert_eq!(response.challenge_nonce, challenge.nonce);
    }

    #[test]
    fn test_full_authentication_flow() {
        let auth1 = create_test_authenticator();
        let auth2 = create_test_authenticator();

        // Auth1 generates challenge for Auth2
        let challenge = auth1.generate_challenge();

        // Auth2 responds
        let response = auth2.respond_to_challenge(&challenge).unwrap();

        // Auth1 verifies
        let peer_id = auth1.verify_response(&response).unwrap();

        // Peer ID should match Auth2's device ID
        assert_eq!(peer_id, auth2.device_id());

        // Peer should now be in verified cache
        assert!(auth1.is_verified(&peer_id));
    }

    #[test]
    fn test_expired_challenge_rejected() {
        let auth = create_test_authenticator();

        // Create a challenge with expiration in the past
        let mut challenge = auth.generate_challenge();
        challenge.expires_at = Some(peat_schema::common::v1::Timestamp {
            seconds: 0, // Way in the past
            nanos: 0,
        });

        let result = auth.respond_to_challenge(&challenge);
        assert!(matches!(result, Err(SecurityError::ChallengeExpired(_))));
    }

    #[test]
    fn test_invalid_signature_rejected() {
        let auth1 = create_test_authenticator();
        let auth2 = create_test_authenticator();

        let challenge = auth1.generate_challenge();
        let mut response = auth2.respond_to_challenge(&challenge).unwrap();

        // Corrupt the signature
        response.signature[0] ^= 0xFF;

        let result = auth1.verify_response(&response);
        assert!(matches!(result, Err(SecurityError::InvalidSignature(_))));
    }

    #[test]
    fn test_remove_peer() {
        let auth1 = create_test_authenticator();
        let auth2 = create_test_authenticator();

        // Authenticate
        let challenge = auth1.generate_challenge();
        let response = auth2.respond_to_challenge(&challenge).unwrap();
        let peer_id = auth1.verify_response(&response).unwrap();

        assert!(auth1.is_verified(&peer_id));

        // Remove
        auth1.remove_peer(&peer_id);
        assert!(!auth1.is_verified(&peer_id));
    }

    #[test]
    fn test_clear_verified_peers() {
        let auth1 = create_test_authenticator();
        let auth2 = create_test_authenticator();
        let auth3 = create_test_authenticator();

        // Authenticate two peers
        let c1 = auth1.generate_challenge();
        let r1 = auth2.respond_to_challenge(&c1).unwrap();
        auth1.verify_response(&r1).unwrap();

        let c2 = auth1.generate_challenge();
        let r2 = auth3.respond_to_challenge(&c2).unwrap();
        auth1.verify_response(&r2).unwrap();

        assert_eq!(auth1.verified_peer_count(), 2);

        auth1.clear_verified_peers();
        assert_eq!(auth1.verified_peer_count(), 0);
    }

    /// peat#952 second-boundary regression: pre-fix, the signer's
    /// `respond_to_challenge` signed over `challenge.timestamp.seconds`
    /// (captured in `generate_challenge`) but embedded a freshly-
    /// captured `SystemTime::now().as_secs()` in `response.timestamp`
    /// (captured immediately *after* signing). The verifier's
    /// `verify_response` reconstructed the signed message from
    /// `response.timestamp.seconds`. When the two SystemTime captures
    /// fell within the same wall-clock second they happened to match
    /// — but any time the auth flow spanned a second boundary (a few
    /// microseconds either side of the tick), they differed by 1 and
    /// Ed25519 verification failed with "Verification equation was
    /// not satisfied."
    ///
    /// This pin reconstructs that exact scenario deterministically:
    /// hand-build a `Challenge` with `timestamp.seconds = T - 5`,
    /// `respond_to_challenge` runs at wall-clock time T (or later),
    /// so pre-fix the signer's message includes `T - 5` while the
    /// verifier's message includes `T` — mismatched, fails. Post-fix
    /// the signer uses the same `response.timestamp.seconds` value
    /// the verifier reads, so the message is consistent and the
    /// signature verifies.
    #[test]
    fn timestamp_mismatch_between_challenge_and_response_does_not_break_verification() {
        let auth1 = create_test_authenticator();
        let auth2 = create_test_authenticator();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();

        // Hand-construct a challenge with a timestamp from 5 seconds
        // ago. `generate_challenge` would normally use `now`; using
        // a divergent value here simulates the wall-clock-boundary
        // race that the production code path can hit at a 10^-6
        // probability per attempt under normal load.
        let challenge = Challenge {
            nonce: vec![7u8; CHALLENGE_NONCE_SIZE],
            timestamp: Some(peat_schema::common::v1::Timestamp {
                seconds: now.as_secs() - 5,
                nanos: 0,
            }),
            challenger_id: auth1.device_id().to_hex(),
            expires_at: Some(peat_schema::common::v1::Timestamp {
                seconds: now.as_secs() + 60,
                nanos: 0,
            }),
            // ADR-065: v1 protocol — both ends speak the current
            // version, so the signed-message byte construction
            // includes response.protocol_version.
            protocol_version: CURRENT_PROTOCOL_VERSION,
            capabilities: Vec::new(),
        };

        // Sign. Pre-fix this signs over (nonce, challenger_id,
        // T-5); post-fix it signs over (nonce, challenger_id,
        // response.timestamp.seconds), where response.timestamp is
        // captured once and used for both the signed message and
        // the response field.
        let response = auth2
            .respond_to_challenge(&challenge)
            .expect("respond_to_challenge succeeds (challenge not expired)");

        // Verify. The verifier reads `response.timestamp.seconds`
        // and reconstructs the signed message. Post-fix this is the
        // same value the signer used; pre-fix it's a different
        // value (now-time, not T-5).
        let peer_id = auth1
            .verify_response(&response)
            .expect("verify_response must succeed regardless of when the challenge was issued");

        assert_eq!(peer_id, auth2.device_id());
    }

    /// ADR-065 v1↔v1 roundtrip: both peers speak the current
    /// version, so the negotiated version is 1 and the signed-message
    /// construction includes `response.protocol_version` (u32 LE).
    /// `generate_challenge()` advertises `CURRENT_PROTOCOL_VERSION`
    /// automatically; the responder picks min(peer, ours) = 1.
    #[test]
    fn v1_v1_roundtrip_negotiates_to_current_version() {
        let auth1 = create_test_authenticator();
        let auth2 = create_test_authenticator();

        let challenge = auth1.generate_challenge();
        assert_eq!(
            challenge.protocol_version, CURRENT_PROTOCOL_VERSION,
            "generate_challenge must advertise our current version"
        );

        let response = auth2.respond_to_challenge(&challenge).unwrap();
        assert_eq!(
            response.protocol_version, CURRENT_PROTOCOL_VERSION,
            "v1 responder must negotiate to CURRENT_PROTOCOL_VERSION when peer also speaks it"
        );

        let peer_id = auth1.verify_response(&response).expect("verify v1");
        assert_eq!(peer_id, auth2.device_id());
    }

    /// ADR-065 v0 challenger ↔ v1 responder: the responder receives
    /// a Challenge with `protocol_version == 0` (a pre-rc.21+1 peer
    /// or a v1 peer that for some reason left the field as default),
    /// picks `min(0, CURRENT) = 0`, and falls through to the v0
    /// signed-message construction (no version field appended).
    /// `response.protocol_version` is set to 0 so a v0-aware
    /// verifier reconstructs the same v0 bytes.
    #[test]
    fn v0_challenger_negotiates_down_to_v0_construction() {
        let auth1 = create_test_authenticator();
        let auth2 = create_test_authenticator();

        // Hand-build a v0 Challenge (explicit protocol_version=0
        // to simulate a peer that didn't advertise).
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let challenge = Challenge {
            nonce: vec![3u8; CHALLENGE_NONCE_SIZE],
            timestamp: Some(peat_schema::common::v1::Timestamp {
                seconds: now_secs,
                nanos: 0,
            }),
            challenger_id: auth1.device_id().to_hex(),
            expires_at: Some(peat_schema::common::v1::Timestamp {
                seconds: now_secs + 60,
                nanos: 0,
            }),
            protocol_version: 0,
            capabilities: Vec::new(),
        };

        let response = auth2.respond_to_challenge(&challenge).unwrap();
        assert_eq!(
            response.protocol_version, 0,
            "v1 responder must negotiate down to 0 when peer advertises 0"
        );

        // Verifier sees response.protocol_version=0 and uses v0
        // construction. Verification succeeds.
        let peer_id = auth1
            .verify_response(&response)
            .expect("v0 roundtrip verifies cleanly");
        assert_eq!(peer_id, auth2.device_id());
    }

    /// ADR-065 future-version response: a verifier receives a
    /// response claiming `protocol_version > CURRENT_PROTOCOL_VERSION`
    /// (a peer running a newer build). The verifier cannot
    /// reconstruct the signed bytes for an unknown version and must
    /// surface the operator-visible error
    /// `SecurityError::AuthenticationFailed("incompatible protocol version: ...")`
    /// — distinct from `InvalidSignature` so operators can
    /// distinguish "peer is too new" from "sig was tampered with."
    #[test]
    fn v1_verifier_rejects_future_protocol_version_with_distinct_error() {
        let auth = create_test_authenticator();

        // Hand-build a response claiming version u32::MAX
        // (representing "the far future"). Public_key + signature
        // can be junk — we want to verify the version-check fires
        // BEFORE signature verification.
        let response = SignedChallengeResponse {
            challenge_nonce: vec![0u8; CHALLENGE_NONCE_SIZE],
            public_key: vec![0u8; 32],
            signature: vec![0u8; 64],
            timestamp: Some(peat_schema::common::v1::Timestamp {
                seconds: 0,
                nanos: 0,
            }),
            device_type: 0,
            certificates: vec![],
            protocol_version: u32::MAX,
            capabilities: Vec::new(),
        };

        let err = auth
            .verify_response(&response)
            .expect_err("version-mismatch must surface as Err");

        match err {
            SecurityError::AuthenticationFailed(msg) => {
                assert!(
                    msg.starts_with(INCOMPATIBLE_PROTOCOL_VERSION_PREFIX),
                    "operator-visible message must start with the documented \
                     prefix for callers to detect; got: {msg}"
                );
                assert!(
                    msg.contains(&u32::MAX.to_string())
                        && msg.contains(&CURRENT_PROTOCOL_VERSION.to_string()),
                    "diagnostic must name both the peer's claimed version \
                     and our maximum; got: {msg}"
                );
            }
            other => panic!(
                "version mismatch must surface as AuthenticationFailed \
                 (distinct from InvalidSignature), not: {other:?}"
            ),
        }
    }

    /// ADR-065 MITM downgrade resistance: at v1, `response.protocol_version`
    /// is included in the signed bytes. A network attacker that
    /// modifies the field on the wire — flipping a v1 response to
    /// claim v0 — cannot get the signature to verify because the
    /// verifier reconstructs bytes for the claimed v0 (omitting the
    /// version-bind suffix) while the signer's bytes include it.
    /// Bytes mismatch → Ed25519 fails → `InvalidSignature`. The
    /// attacker cannot strip or modify the version without breaking
    /// the signature.
    #[test]
    fn v1_signature_binds_protocol_version_against_mitm_downgrade() {
        let auth1 = create_test_authenticator();
        let auth2 = create_test_authenticator();

        let challenge = auth1.generate_challenge();
        let mut response = auth2.respond_to_challenge(&challenge).unwrap();
        assert_eq!(response.protocol_version, CURRENT_PROTOCOL_VERSION);

        // Simulate MITM: peer's response was constructed with v1
        // semantics (signed-bytes include version), but the attacker
        // flips the field to 0 on the wire. The verifier reconstructs
        // v0 bytes (omitting the version-bind suffix), which differ
        // from what the signer signed → InvalidSignature.
        response.protocol_version = 0;
        let err = auth1
            .verify_response(&response)
            .expect_err("MITM-downgraded version must fail signature verification");

        assert!(
            matches!(err, SecurityError::InvalidSignature(_)),
            "downgrade attempt must surface as InvalidSignature (the bytes \
             don't match), not as a clean negotiation result; got: {err:?}"
        );
    }
}
