# ADR-006: Security, Authentication, and Authorization for Peat Protocol

**Status**: Proposed (amended 2026-07-21 for the gateway/node authentication boundary)
**Date**: 2025-11-04
**Authors**: Codex, Kit Plummer
**Related**: ADR-005 (Data Sync Abstraction Layer), ADR-004 (Human-Machine Cell Composition), ADR-048 (Membership Certificates), ADR-055 (Peat Gateway), [ADR-060 §5 Cryptographic primitives (FIPS posture)](060-encryption-tiers-rest-and-transit.md#5-cryptographic-primitives-fips-posture)

> **Amendment 2026-05-18 (via PR #870):** ChaCha20-Poly1305 references in this ADR are superseded by **AES-256-GCM** per ADR-060 §5 driver #6 (FIPS-approved primitives only). The original code samples and acceptance criteria below have been updated inline; the original record is preserved in git history. This amendment also resolves the latent contradiction this ADR has carried since its initial draft — the "Compliance Considerations" section already named FIPS 140-2/3 as a target, but the rest of the ADR specified the non-FIPS-approved ChaCha20-Poly1305. ADR-060 §5 is the authoritative primitive list for the peat ecosystem.

## Context

Peat Protocol coordinates autonomous platforms in tactical military environments where security failures can result in:
- **Loss of life** (compromised UAVs, corrupted mission data)
- **Mission failure** (adversary disruption of coordination)
- **Tactical disadvantage** (enemy intelligence gathering)
- **Friendly fire** (spoofed identity or commands)

Current implementation has **no authentication or authorization**. All nodes trust all peers, any node can join any squad, and all data is accessible to all participants. This is acceptable for proof-of-concept but **completely unacceptable** for tactical deployment.

### Threat Model

**Adversaries**:
1. **External attackers** - Enemy attempting to disrupt operations
2. **Compromised nodes** - Captured platforms running adversary code
3. **Insider threats** - Rogue operators or compromised credentials
4. **Network eavesdroppers** - Passive monitoring of communications

**Attack Vectors**:
1. **Identity spoofing** - Pretend to be a legitimate node to join squads
2. **Man-in-the-middle** - Intercept and modify messages between peers
3. **Replay attacks** - Retransmit captured messages to cause confusion
4. **Privilege escalation** - Node attempts to exceed its authorized role
5. **Data exfiltration** - Compromised node leaks tactical information
6. **Denial of service** - Flood network with invalid requests

### Security Requirements

Peat Protocol must provide:

1. **Device Authentication** - Cryptographically verify device identity
2. **User Authentication** - Verify human operator credentials (for C2 apps)
3. **Application Authentication** - Verify software integrity and authorization
4. **Role-Based Authorization** - Enforce permissions based on role (Leader, Member, Observer)
5. **Hierarchical Authorization** - Enforce access control across organizational levels
6. **Data Confidentiality** - Encrypt all communications and storage
7. **Data Integrity** - Detect tampering with messages and documents
8. **Replay Protection** - Prevent reuse of captured messages
9. **Audit Trail** - Log all security-relevant events for forensics
10. **Graceful Degradation** - Continue operating if some security services fail

### Integration Points

Security must integrate with:
1. **Data Sync Layer** (ADR-005) - Authentication before sync, encrypted transport
2. **Cell Formation** (ADR-001) - Only authorized nodes join squads
3. **Human-in-the-Loop** (ADR-004) - Human operator authentication and approval
4. **Capability Advertisement** - Sign capability claims to prevent spoofing
5. **Hierarchical Aggregation** - Enforce data access by organizational level

## Decision

We will implement a **multi-layer security architecture** with:

### Layer 1: Device Identity and Authentication

Every device has a cryptographic identity verified before joining the mesh.

```rust
/// Device identity backed by PKI
pub struct DeviceIdentity {
    /// Unique device identifier (UUID)
    pub device_id: DeviceId,

    /// Public key for this device
    pub public_key: PublicKey,

    /// Certificate chain proving device authenticity
    pub certificates: Vec<X509Certificate>,

    /// Device type (UAV, ground vehicle, C2 station, etc.)
    pub device_type: DeviceType,

    /// Organizational unit (battalion, company, platoon)
    pub organization: OrganizationUnit,
}

/// Device authentication manager
pub struct DeviceAuthenticator {
    /// This device's identity
    own_identity: DeviceIdentity,

    /// Private key for signing
    private_key: PrivateKey,

    /// Trust store (root CAs, intermediate CAs)
    trust_store: TrustStore,

    /// Known peer identities (cached after first verification)
    peer_cache: Arc<RwLock<HashMap<DeviceId, DeviceIdentity>>>,
}

impl DeviceAuthenticator {
    /// Verify peer's identity during connection establishment
    pub async fn authenticate_peer(
        &self,
        peer_id: &DeviceId,
        challenge_response: &SignedChallenge,
    ) -> Result<DeviceIdentity> {
        // 1. Verify signature on challenge response
        let peer_pubkey = challenge_response.public_key;
        if !challenge_response.verify(&peer_pubkey)? {
            return Err(SecurityError::InvalidSignature);
        }

        // 2. Verify certificate chain
        let certs = &challenge_response.certificates;
        self.trust_store.verify_chain(certs)?;

        // 3. Check certificate validity (not expired, not revoked)
        for cert in certs {
            if cert.is_expired() {
                return Err(SecurityError::ExpiredCertificate);
            }
            if self.is_revoked(&cert)? {
                return Err(SecurityError::RevokedCertificate);
            }
        }

        // 4. Extract device identity from certificate
        let identity = DeviceIdentity::from_certificate(&certs[0])?;

        // 5. Cache for future use
        self.peer_cache.write().await.insert(*peer_id, identity.clone());

        Ok(identity)
    }

    /// Sign a message with this device's private key
    pub fn sign(&self, message: &[u8]) -> Result<Signature> {
        self.private_key.sign(message)
    }

    /// Generate a challenge for peer to prove identity
    pub fn generate_challenge(&self) -> Challenge {
        Challenge {
            nonce: random_bytes(32),
            timestamp: SystemTime::now(),
            challenger_id: self.own_identity.device_id,
        }
    }
}
```

### Layer 2: User Authentication (for Human Operators)

Human operators (C2 tablet, mission planning tools) authenticate separately from devices.

```rust
/// User identity for human operators
pub struct UserIdentity {
    /// Username (e.g., call sign)
    pub username: String,

    /// Full name and rank
    pub display_name: String,
    pub rank: MilitaryRank,

    /// Clearance level
    pub clearance: SecurityClearance,

    /// Organizational unit
    pub unit: OrganizationUnit,

    /// Roles (mission commander, operator, observer)
    pub roles: HashSet<UserRole>,
}

/// User authentication methods
pub enum AuthMethod {
    /// Password + TOTP (tactical environments)
    PasswordMFA { password_hash: PasswordHash, totp_secret: TotpSecret },

    /// CAC/PIV card (DoD standard)
    SmartCard { card_id: String, pin_hash: PasswordHash },

    /// Biometric (fingerprint, facial recognition)
    Biometric { biometric_template: Vec<u8> },

    /// Certificate-based (PKI)
    Certificate { certificate: X509Certificate },
}

/// User authentication manager
pub struct UserAuthenticator {
    /// User database (may be local or remote)
    user_store: Box<dyn UserStore>,

    /// Session manager (tracks logged-in users)
    sessions: Arc<RwLock<HashMap<SessionId, UserSession>>>,
}

impl UserAuthenticator {
    /// Authenticate user and create session
    pub async fn authenticate(
        &self,
        username: &str,
        credential: &Credential,
    ) -> Result<UserSession> {
        // 1. Lookup user
        let user = self.user_store
            .get_user(username)
            .await
            .ok_or(SecurityError::UserNotFound)?;

        // 2. Verify credential
        match (&user.auth_method, credential) {
            (AuthMethod::PasswordMFA { password_hash, totp_secret },
             Credential::PasswordMFA { password, totp_code }) => {
                // Verify password
                if !password_hash.verify(password)? {
                    return Err(SecurityError::InvalidCredential);
                }
                // Verify TOTP code
                if !totp_secret.verify(totp_code, SystemTime::now())? {
                    return Err(SecurityError::InvalidMFA);
                }
            }
            (AuthMethod::SmartCard { pin_hash, .. }, Credential::SmartCard { pin, .. }) => {
                if !pin_hash.verify(pin)? {
                    return Err(SecurityError::InvalidCredential);
                }
            }
            _ => return Err(SecurityError::UnsupportedAuthMethod),
        }

        // 3. Create session
        let session = UserSession {
            session_id: SessionId::new(),
            user_identity: user.identity,
            device_id: self.get_current_device_id(),
            created_at: SystemTime::now(),
            expires_at: SystemTime::now() + Duration::from_hours(8),
        };

        // 4. Store session
        self.sessions.write().await.insert(session.session_id, session.clone());

        Ok(session)
    }

    /// Verify session is still valid
    pub async fn verify_session(&self, session_id: &SessionId) -> Result<UserSession> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .ok_or(SecurityError::InvalidSession)?;

        if session.expires_at < SystemTime::now() {
            return Err(SecurityError::SessionExpired);
        }

        Ok(session.clone())
    }
}
```

### Layer 3: Application Authentication

Verify that running software is authorized and unmodified.

```rust
/// Application identity (software being executed)
pub struct ApplicationIdentity {
    /// Application name and version
    pub app_name: String,
    pub version: semver::Version,

    /// Code signing certificate
    pub code_signature: CodeSignature,

    /// Hash of executable for integrity check
    pub executable_hash: Hash,

    /// Permissions this app is allowed to request
    pub declared_permissions: HashSet<Permission>,
}

/// Application authenticator using code signing
pub struct ApplicationAuthenticator {
    /// Trust store for code signing certificates
    code_signing_trust: TrustStore,

    /// Runtime integrity checker
    integrity_monitor: IntegrityMonitor,
}

impl ApplicationAuthenticator {
    /// Verify application integrity at startup
    pub fn verify_application(&self) -> Result<ApplicationIdentity> {
        // 1. Locate executable
        let exe_path = std::env::current_exe()?;

        // 2. Read executable and compute hash
        let exe_bytes = std::fs::read(&exe_path)?;
        let computed_hash = Hash::sha256(&exe_bytes);

        // 3. Extract embedded signature
        let signature = CodeSignature::extract_from_binary(&exe_bytes)?;

        // 4. Verify signature
        self.code_signing_trust.verify_code_signature(&signature)?;

        // 5. Check signature matches executable
        if signature.signed_hash != computed_hash {
            return Err(SecurityError::TamperedExecutable);
        }

        // 6. Extract identity from signature
        let identity = ApplicationIdentity::from_signature(&signature)?;

        // 7. Start runtime integrity monitoring
        self.integrity_monitor.start_monitoring()?;

        Ok(identity)
    }

    /// Periodic integrity check (detect runtime tampering)
    pub async fn check_runtime_integrity(&self) -> Result<()> {
        // Check for code injection, memory tampering, etc.
        self.integrity_monitor.check()?;
        Ok(())
    }
}
```

### Layer 4: Role-Based Authorization (RBAC)

Control what each authenticated entity can do.

```rust
/// Roles in Peat Protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Squad/cell leader - can command cell, set objectives
    Leader,

    /// Squad/cell member - participates in missions
    Member,

    /// Observer - can view but not command
    Observer,

    /// Mission commander - can direct multiple cells
    Commander,

    /// Administrator - can configure system
    Admin,
}

/// Permissions that can be checked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    // Cell operations
    JoinCell,
    LeaveCell,
    CreateCell,
    DisbandCell,
    SetCellLeader,
    SetCellObjective,

    // Capability operations
    AdvertiseCapability,
    RequestCapability,

    // Data access
    ReadCellState,
    WriteCellState,
    ReadNodeState,
    WriteNodeState,
    ReadTelemetry,

    // Hierarchical operations
    FormPlatoon,
    AggregateToCompany,

    // Human-in-the-loop
    ApproveFormation,
    VetoCommand,

    // Administration
    ConfigureNetwork,
    ManageKeys,
    ViewAuditLog,
}

/// Role-based authorization controller
pub struct AuthorizationController {
    /// Policy defining role → permissions mapping
    policy: AuthorizationPolicy,

    /// Audit logger for authorization decisions
    audit_log: AuditLogger,
}

impl AuthorizationController {
    /// Check if entity has permission
    pub fn check_permission(
        &self,
        entity: &AuthenticatedEntity,
        permission: Permission,
        context: &AuthorizationContext,
    ) -> Result<()> {
        // 1. Get roles for entity
        let roles = self.get_roles(entity, context)?;

        // 2. Check if any role grants permission
        let granted = roles.iter().any(|role| {
            self.policy.role_has_permission(*role, permission)
        });

        if !granted {
            // Log denial
            self.audit_log.log_denial(entity, permission, context);
            return Err(SecurityError::PermissionDenied {
                permission,
                entity_id: entity.id(),
            });
        }

        // 3. Log grant
        self.audit_log.log_grant(entity, permission, context);

        Ok(())
    }

    /// Get roles for entity in given context
    fn get_roles(
        &self,
        entity: &AuthenticatedEntity,
        context: &AuthorizationContext,
    ) -> Result<HashSet<Role>> {
        let mut roles = HashSet::new();

        match entity {
            AuthenticatedEntity::Device(device) => {
                // Devices get roles based on cell membership
                if let Some(cell_id) = context.cell_id {
                    let cell = context.get_cell(cell_id)?;

                    if cell.leader_id == Some(device.device_id.to_string()) {
                        roles.insert(Role::Leader);
                    } else if cell.members.contains(&device.device_id.to_string()) {
                        roles.insert(Role::Member);
                    } else {
                        roles.insert(Role::Observer);
                    }
                }
            }
            AuthenticatedEntity::User(user) => {
                // Users have explicit roles
                roles = user.identity.roles.clone();
            }
        }

        Ok(roles)
    }
}

/// Authorization context provides situational information
pub struct AuthorizationContext {
    /// Cell being accessed (if applicable)
    pub cell_id: Option<CellId>,

    /// Organizational level
    pub hierarchy_level: Option<HierarchyLevel>,

    /// Time of access
    pub timestamp: SystemTime,

    /// Access to data stores for context lookups
    pub cell_store: Arc<dyn CellStoreReader>,
}

/// Default authorization policy for Peat Protocol
impl AuthorizationPolicy {
    pub fn default_policy() -> Self {
        let mut policy = AuthorizationPolicy::new();

        // Leader permissions
        policy.grant_role(Role::Leader, Permission::SetCellObjective);
        policy.grant_role(Role::Leader, Permission::SetCellLeader);
        policy.grant_role(Role::Leader, Permission::RequestCapability);
        policy.grant_role(Role::Leader, Permission::ReadCellState);
        policy.grant_role(Role::Leader, Permission::WriteCellState);

        // Member permissions
        policy.grant_role(Role::Member, Permission::JoinCell);
        policy.grant_role(Role::Member, Permission::LeaveCell);
        policy.grant_role(Role::Member, Permission::AdvertiseCapability);
        policy.grant_role(Role::Member, Permission::ReadCellState);
        policy.grant_role(Role::Member, Permission::WriteNodeState);

        // Observer permissions (read-only)
        policy.grant_role(Role::Observer, Permission::ReadCellState);
        policy.grant_role(Role::Observer, Permission::ReadNodeState);
        policy.grant_role(Role::Observer, Permission::ReadTelemetry);

        // Commander permissions (hierarchical)
        policy.grant_role(Role::Commander, Permission::FormPlatoon);
        policy.grant_role(Role::Commander, Permission::ApproveFormation);
        policy.grant_role(Role::Commander, Permission::VetoCommand);

        // Admin permissions (system-wide)
        policy.grant_role(Role::Admin, Permission::ConfigureNetwork);
        policy.grant_role(Role::Admin, Permission::ManageKeys);
        policy.grant_role(Role::Admin, Permission::ViewAuditLog);

        policy
    }
}
```

### Layer 5: Data Encryption

Encrypt all data in transit and at rest.

```rust
/// Encryption manager for Peat Protocol
pub struct EncryptionManager {
    /// Device's encryption keypair
    keypair: EncryptionKeypair,

    /// Symmetric keys for peer-to-peer encryption
    peer_keys: Arc<RwLock<HashMap<PeerId, SymmetricKey>>>,

    /// Cell-level group keys for broadcast encryption
    cell_keys: Arc<RwLock<HashMap<CellId, GroupKey>>>,
}

impl EncryptionManager {
    /// Establish encrypted channel with peer
    pub async fn establish_secure_channel(
        &self,
        peer_id: &PeerId,
        peer_pubkey: &PublicKey,
    ) -> Result<SecureChannel> {
        // 1. Perform Diffie-Hellman key exchange
        let shared_secret = self.keypair.dh_exchange(peer_pubkey)?;

        // 2. Derive symmetric key using HKDF
        let symmetric_key = hkdf_derive(&shared_secret, b"peat-protocol-v1")?;

        // 3. Store key for this peer
        self.peer_keys.write().await.insert(*peer_id, symmetric_key.clone());

        // 4. Return secure channel
        Ok(SecureChannel {
            peer_id: *peer_id,
            symmetric_key,
            cipher: Aes256Gcm::new(&symmetric_key),  // amended 2026-05-18 for FIPS posture (ADR-060 §5)
        })
    }

    /// Encrypt document for storage
    pub fn encrypt_document(&self, document: &Document) -> Result<EncryptedDocument> {
        // Use device's own key for at-rest encryption
        let plaintext = serde_json::to_vec(document)?;
        let nonce = random_nonce();
        let ciphertext = self.keypair.encrypt(&plaintext, &nonce)?;

        Ok(EncryptedDocument {
            ciphertext,
            nonce,
            encrypted_by: self.keypair.public_key(),
        })
    }

    /// Encrypt message for cell broadcast
    pub async fn encrypt_for_cell(
        &self,
        cell_id: &CellId,
        message: &[u8],
    ) -> Result<EncryptedMessage> {
        // Get or create group key for cell
        let cell_keys = self.cell_keys.read().await;
        let group_key = cell_keys
            .get(cell_id)
            .ok_or(SecurityError::NoGroupKey)?;

        // Encrypt with group key
        let nonce = random_nonce();
        let ciphertext = group_key.encrypt(message, &nonce)?;

        Ok(EncryptedMessage {
            cell_id: *cell_id,
            ciphertext,
            nonce,
        })
    }

    /// Rotate cell group key (e.g., when member leaves)
    pub async fn rotate_cell_key(&self, cell_id: &CellId) -> Result<()> {
        // Generate new group key
        let new_key = GroupKey::generate();

        // Store new key
        self.cell_keys.write().await.insert(*cell_id, new_key.clone());

        // Distribute to all current cell members (encrypted per-peer)
        // This requires peer_keys to be established first

        Ok(())
    }
}
```

### Layer 6: Integration with Data Sync Abstraction

Security must integrate with the abstraction layer from ADR-005.

**Critical Requirement**: Following the Ports & Adapters pattern from ADR-005/ADR-011, the security layer **must be backend-agnostic**. The Peat Protocol API should work identically regardless of whether Ditto or AutomergeIroh is the underlying backend.

#### Backend Implementation Notes

| Backend | Security Integration |
|---------|---------------------|
| **Ditto** | Ditto has built-in APP ID/license authentication. Our PKI layer adds device identity verification on top of Ditto's mesh authentication. |
| **AutomergeIroh** | Iroh uses QUIC/TLS 1.3 for transport encryption. We integrate PKI certificate validation with Iroh's connection establishment. |

The `SecurityManager` trait abstracts these differences, allowing protocol code to remain backend-agnostic:

```rust
/// Extend DataSyncBackend trait with security
pub trait SecureDataSyncBackend: DataSyncBackend {
    /// Get security manager
    fn security(&self) -> &dyn SecurityManager;
}

/// Security manager trait
pub trait SecurityManager: Send + Sync {
    /// Authenticate a peer before allowing sync
    async fn authenticate_peer(&self, peer_id: &PeerId) -> Result<DeviceIdentity>;

    /// Authorize an operation
    fn authorize(
        &self,
        entity: &AuthenticatedEntity,
        permission: Permission,
        context: &AuthorizationContext,
    ) -> Result<()>;

    /// Encrypt data before sending
    fn encrypt(&self, data: &[u8], recipient: &PeerId) -> Result<Vec<u8>>;

    /// Decrypt data after receiving
    fn decrypt(&self, data: &[u8], sender: &PeerId) -> Result<Vec<u8>>;

    /// Get audit logger
    fn audit_log(&self) -> &dyn AuditLogger;
}

/// Secure wrapper for CellStore
impl<B: SecureDataSyncBackend> CellStore<B> {
    /// Store cell with authorization check
    pub async fn store_cell_secure(
        &self,
        cell: &CellState,
        entity: &AuthenticatedEntity,
    ) -> Result<String> {
        // 1. Check authorization
        let context = AuthorizationContext {
            cell_id: Some(CellId::from_str(&cell.config.id)?),
            hierarchy_level: Some(HierarchyLevel::Squad),
            timestamp: SystemTime::now(),
            cell_store: self.as_reader(),
        };

        self.backend.security().authorize(
            entity,
            Permission::WriteCellState,
            &context,
        )?;

        // 2. Store cell (encryption handled by backend)
        let doc_id = self.store_cell(cell).await?;

        // 3. Audit log
        self.backend.security().audit_log().log_operation(
            entity,
            "store_cell",
            &cell.config.id,
            true,
        );

        Ok(doc_id)
    }

    /// Set cell leader with authorization check
    pub async fn set_leader_secure(
        &self,
        cell_id: &str,
        leader_id: String,
        entity: &AuthenticatedEntity,
    ) -> Result<()> {
        // 1. Check authorization
        let context = AuthorizationContext {
            cell_id: Some(CellId::from_str(cell_id)?),
            hierarchy_level: Some(HierarchyLevel::Squad),
            timestamp: SystemTime::now(),
            cell_store: self.as_reader(),
        };

        self.backend.security().authorize(
            entity,
            Permission::SetCellLeader,
            &context,
        )?;

        // 2. Execute operation
        self.set_leader(cell_id, leader_id.clone()).await?;

        // 3. Audit log
        self.backend.security().audit_log().log_operation(
            entity,
            "set_leader",
            &format!("{} -> {}", cell_id, leader_id),
            true,
        );

        Ok(())
    }
}
```

### Layer 7: Audit Logging

Track all security-relevant events for forensics.

```rust
/// Audit logger for security events
pub trait AuditLogger: Send + Sync {
    /// Log authentication event
    fn log_authentication(
        &self,
        entity: &AuthenticatedEntity,
        success: bool,
        reason: Option<&str>,
    );

    /// Log authorization grant
    fn log_grant(
        &self,
        entity: &AuthenticatedEntity,
        permission: Permission,
        context: &AuthorizationContext,
    );

    /// Log authorization denial
    fn log_denial(
        &self,
        entity: &AuthenticatedEntity,
        permission: Permission,
        context: &AuthorizationContext,
    );

    /// Log operation execution
    fn log_operation(
        &self,
        entity: &AuthenticatedEntity,
        operation: &str,
        target: &str,
        success: bool,
    );

    /// Log security violation
    fn log_violation(
        &self,
        entity: &AuthenticatedEntity,
        violation_type: SecurityViolation,
        details: &str,
    );
}

/// Audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// Timestamp
    pub timestamp: SystemTime,

    /// Entity performing action
    pub entity: String,

    /// Event type
    pub event_type: AuditEventType,

    /// Success or failure
    pub success: bool,

    /// Details
    pub details: String,

    /// Context (cell ID, hierarchy level, etc.)
    pub context: HashMap<String, String>,
}

/// Audit event types
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum AuditEventType {
    Authentication,
    Authorization,
    DataAccess,
    DataModification,
    KeyExchange,
    CellFormation,
    LeaderElection,
    SecurityViolation,
}

/// File-based audit logger
pub struct FileAuditLogger {
    log_file: Arc<Mutex<File>>,
}

impl AuditLogger for FileAuditLogger {
    fn log_operation(
        &self,
        entity: &AuthenticatedEntity,
        operation: &str,
        target: &str,
        success: bool,
    ) {
        let entry = AuditLogEntry {
            timestamp: SystemTime::now(),
            entity: entity.id().to_string(),
            event_type: AuditEventType::DataModification,
            success,
            details: format!("{} on {}", operation, target),
            context: HashMap::new(),
        };

        // Write to log file (append-only)
        let mut file = self.log_file.lock().unwrap();
        writeln!(file, "{}", serde_json::to_string(&entry).unwrap()).ok();
        file.flush().ok();
    }

    // ... other methods
}
```

## Integration with Peat Protocol Phases

### Phase 1: Discovery → Requires Device Authentication

```rust
// Before discovery, authenticate device
let device_auth = DeviceAuthenticator::new(config)?;
let device_identity = device_auth.verify_application()?;

// Discovery protocol includes signed beacon
let beacon = Beacon {
    device_id: device_identity.device_id,
    capabilities: my_capabilities,
    signature: device_auth.sign(&beacon_payload)?,
};

// Receiving node verifies beacon signature
if !peer_device_auth.verify_beacon(&beacon)? {
    warn!("Ignoring beacon from untrusted device");
    return;
}
```

### Phase 2: Cell Formation → Requires Authorization

```rust
// Human commander approves squad formation (ADR-004)
let user_auth = UserAuthenticator::new();
let user_session = user_auth.authenticate("commander_callsign", &credential).await?;

// Check authorization
let context = AuthorizationContext {
    cell_id: Some(proposed_cell_id),
    hierarchy_level: Some(HierarchyLevel::Squad),
    timestamp: SystemTime::now(),
    cell_store: cell_store.as_reader(),
};

authz.check_permission(
    &AuthenticatedEntity::User(user_session),
    Permission::ApproveFormation,
    &context,
)?;

// Form cell with encrypted group key
cell_store.store_cell_secure(&cell_state, &AuthenticatedEntity::User(user_session)).await?;
```

### Phase 3: Hierarchical Operations → Hierarchical Authorization

```rust
// Only commanders can aggregate cells into platoons
authz.check_permission(
    entity,
    Permission::FormPlatoon,
    &context,
)?;

// Platoon-level data only accessible to platoon members and above
let user = entity.as_user()?;
if !user.has_clearance_for_level(HierarchyLevel::Platoon) {
    return Err(SecurityError::InsufficientClearance);
}
```

## Deployment Scenarios

### Scenario 1: Tactical Edge (Fully Offline)

**Challenge**: No connection to PKI infrastructure or authentication servers

**Solution**:
- Pre-provision devices with certificates before deployment
- Use offline Certificate Revocation Lists (CRLs) synchronized during planning
- Local user database on mission commander's tablet
- Audit logs stored locally, uploaded post-mission

### Scenario 2: Contested Environment (Intermittent Connectivity)

**Challenge**: Network disruption, potential adversary interference

**Solution**:
- Use short-lived session tokens (8-hour expiry)
- Certificate stapling to reduce PKI dependencies
- Local authorization decisions with eventual consistency
- Cryptographic replay protection with time windows

### Scenario 3: Garrison/Training (Full Connectivity)

**Challenge**: Integration with existing DoD infrastructure

**Solution**:
- OCSP for real-time certificate validation
- CAC/PIV integration for user authentication
- Centralized audit log aggregation
- Integration with DoD PKI hierarchy

## Implementation Roadmap

### Phase 1: Foundation (Weeks 1-4)

- [ ] Define security traits and types
- [ ] Implement device identity and PKI verification
- [ ] Basic encryption (AES-256-GCM — see ADR-060 §5 FIPS posture)
- [ ] File-based audit logging
- [ ] **Milestone**: Two devices can authenticate and establish encrypted channel

### Phase 2: Authorization (Weeks 5-8)

- [ ] Implement RBAC policy engine
- [ ] Authorization checks in CellStore/NodeStore
- [ ] Context-aware permission checking
- [ ] **Milestone**: Only authorized nodes can join cells and set leaders

### Phase 3: User Authentication (Weeks 9-12)

- [ ] Password + TOTP authentication
- [ ] Session management
- [ ] CAC/PIV integration (if available)
- [ ] **Milestone**: Human commanders can approve cell formations

### Phase 4: Advanced Features (Weeks 13-16)

- [ ] Group key management for cells
- [ ] Key rotation protocols
- [ ] Certificate revocation checking
- [ ] **Milestone**: Complete security for offline tactical deployment

## Security Best Practices

1. **Defense in Depth**: Multiple layers (device, user, app, network, data)
2. **Principle of Least Privilege**: Minimal permissions by default
3. **Zero Trust**: Verify every request, don't trust network position
4. **Fail Securely**: Deny access when in doubt
5. **Audit Everything**: Log all security-relevant events
6. **Graceful Degradation**: Continue operating if some security services fail

## Compliance Considerations

Peat Protocol security must align with:

- **NIST SP 800-53** - Security and Privacy Controls for Information Systems
- **DoD 8500 Series** - Cybersecurity for DoD Information Systems
- **FIPS 140-2/3** - Cryptographic Module Validation (for tactical systems)
- **Common Criteria EAL** - Evaluation Assurance Level for security evaluation

## Multi-Hop Synchronization Trust Model

### Decision (2025-11-24)

**Phase 1: Trust All Mesh Members**

For MVP, all peers in the mesh are trusted to read any document they encounter during synchronization. This matches Ditto's apparent model and enables CRDT-based sync to function correctly.

### Rationale

1. **CRDT Sync Requirement**: Intermediate nodes performing aggregation or relay must read documents to merge CRDT states correctly
2. **Hierarchical Aggregation**: Squad leaders aggregating data to platoon level need document visibility
3. **Ditto Parity**: This matches Ditto's apparent trust model (all nodes share APP ID)
4. **Complexity Deferral**: End-to-end encryption with untrusted relays requires solving CRDT merge on encrypted data

### Multi-Hop Topology

```
Squad A ←→ Squad B (relay) ←→ Squad C
           ↓
    B can read A↔C data
```

In this topology:
- Squad B's leader acts as relay for A↔C communication
- B's leader can read, modify, and store documents passing through
- This is acceptable because B is part of the trusted mesh

### Security Implications

| Aspect | Phase 1 (MVP) | Phase 2 (Future) |
|--------|---------------|------------------|
| Relay visibility | Full access | Selective E2E encryption |
| Trust boundary | Mesh membership | Per-document policies |
| Compromised node | Sees all synced data | Sees only authorized data |
| Key management | Mesh-level PKI | Per-cell group keys |

### Phase 2 Considerations (Future)

For scenarios requiring untrusted relays:
- **Selective Document Encryption**: Encrypt sensitive fields before sync
- **Group Keys**: Cell-level encryption keys for authorized readers
- **Encrypted Sync Blobs**: Full document encryption with metadata visible
- **Key Rotation**: Re-key when cell membership changes

### Related Investigation

See `docs/research/MULTI_HOP_SYNC_INVESTIGATION.md` for full analysis of Ditto flood-fill and Iroh gossip protocols.

## Operator Credential Bundle File Format

### Decision (2026-05-29)

Mesh-joining processes (`peat-cli`, future `peat-gateway` operator interfaces, embedded `peat-lite` nodes — any Peat process that needs to *join* a formation rather than *configure* one) read operational credentials from a YAML file with the following canonical shape:

```yaml
# Required: formation identifier this credential targets.
app_id: <string>

# Required: base64-encoded 32-byte formation key. Same value the rest of
# the mesh uses (passed to peat-node via --shared-key).
shared_key: <base64-string>

# Optional: initial peers in `<endpoint_id>@<host>:<port>` form. Honored by
# the joining process to bootstrap reachability when ambient discovery isn't
# sufficient. `<endpoint_id>` is an Iroh `NodeId` (Ed25519 public key) in
# canonical base32-nopad lowercase — the same string `peat-node`'s GetStatus
# RPC emits in its `endpoint_addr` field.
peers:
  - <endpoint_id>@<host>:<port>
```

Readers MUST reject unknown fields strictly (e.g. `#[serde(deny_unknown_fields)]` in Rust impls) so format evolution is explicit.

### Field definitions

- **`app_id`**: opaque UTF-8 string. Identifies the formation. Same value passed to `peat-node` via `--app-id`.
- **`shared_key`**: base64 (standard, padded) encoding of the 32-byte formation key. Same value passed to `peat-node` via `--shared-key`. **This is the FormationKey** per [ADR-060](060-encryption-tiers-rest-and-transit.md) §Decision #1 — see *File-system custody* below.
- **`peers`** (optional): list of strings, each of the form `<endpoint_id>@<host>:<port>` where:
  - `<endpoint_id>` is an Iroh [`NodeId`](https://docs.rs/iroh/latest/iroh/struct.NodeId.html) (the device's Ed25519 public key) serialised in **canonical base32-nopad lowercase**. This is the encoding `iroh::NodeId`'s `Display` impl produces and what `peat-node` advertises via its `GetStatus` RPC's `endpoint_addr` field.
  - `<host>` is a DNS name or IP literal. Readers MAY resolve via DNS; both A and AAAA records are honored.
  - `<port>` is the peer's Iroh UDP port.

### File-system custody (MUST)

The credential file contains the FormationKey, which [ADR-060](060-encryption-tiers-rest-and-transit.md) §Risks identifies as **load-bearing for T3 protection**. A local user with read access to the bundle file silently recovers the formation key. Implementations MUST:

1. **Create the file with mode `0600` (or OS equivalent — owner-only read/write)** when writing or installing it. On Unix this is `chmod 0600`; on Windows the equivalent is an ACL granting access only to the owner.
2. **At load time, check the file's permissions and emit a security warning to stderr** when the bundle is world- or group-readable on Unix.
3. **On the production path, readers MUST refuse to load a bundle that is world- or group-readable.** Either bit grants the FormationKey to a principal outside the file's owner; [ADR-060](060-encryption-tiers-rest-and-transit.md) §Decision #1 treats both as equivalent T3-protection violations, and Bullet 2's warning surface covers the same union for consistency. Implementations MAY downgrade this refusal to a stderr warning only when an explicit dev/CI escape hatch is in effect — a dedicated environment variable (e.g. `PEAT_ALLOW_INSECURE_CREDS=1`) or build-time feature flag whose name signals the relaxation. The default production code path MUST refuse; the escape hatch MUST be intentional and opt-in.

This MUST closes the operational-surfacing gap the *"FormationKey custody is load-bearing for substrate-cipher T3 protection"* row of [ADR-060](060-encryption-tiers-rest-and-transit.md) §Risks names.

### Rationale

The on-disk credential format was not previously specified. The fields above existed in `peat-node`'s `SidecarConfig` and command-line surface, and `peat-cli`'s `crates/peat-cli/src/creds.rs` shipped a placeholder serde struct to unblock its initial development (per peat-node ADR-001). This amendment promotes that placeholder to the canonical bundle shape so multiple Peat clients converge on one format rather than diverging as more consumers are added.

Scope is deliberately **operational mesh-joining only**:

- **`app_id` + `shared_key`** are the same primitives Layer 3 (Application Authentication) and the formation-key transport authentication already use. The bundle records them on disk in a single document so a joining process can be configured with one file.
- **`peers`** is a reachability hint, not an identity claim. The joining process still authenticates each peer with the formation key.

Out of scope for this amendment (each tracked separately):

- **Device PKI material** (Layer 1 `DeviceIdentity`: X.509 chains, certificates, private keys). A future amendment can extend the bundle once Layer 1's handshake is enforced in `peat-mesh`; until then Device PKI is distributed through whatever certificate-infrastructure channel a deployment already uses.
- **Application-level encryption keys.** The cipher-layering question — whether the at-rest cipher operates at the application JSON layer (`peat-node`'s `StoreCipher`) or the byte storage layer (`peat-mesh`'s `Cipher` trait) — is unresolved; `encryption_key` is not part of this amendment's schema. Readers should reject the field (or any other field they don't yet honor) per the deny-unknown-fields stance.
- **Authorization claims.** Role / per-collection-scope authorization is the subject of peat#941's separate design exercise; the bundle does not carry role claims today.

### Schema Versioning

Future changes to the canonical shape (new fields, deprecations) land as additional entries in this ADR's Decision Log. The deny-unknown-fields stance forces every reader to surface the schema mismatch at load time rather than silently dropping unrecognised fields. If the format ever needs a hard break, a new amendment introduces a `bundle_version` envelope field at that time — until then the flat shape above is sufficient.

### Resolution Order (non-normative reference)

The `peat-cli` reference implementation resolves the bundle path as:

1. `--creds <PATH>` CLI argument
2. `PEAT_CREDS` environment variable
3. Platform default (`$XDG_CONFIG_HOME/peat/credentials.yaml` on Unix; OS equivalent on Windows / macOS)

Failure to resolve a bundle is a fatal error — the reference implementation does not silently fall back to anonymous join. Consumers MAY differ on the resolution mechanism; what's normative is the bundle's content.

### Consequences

- **Positive.** Multiple Peat clients share one version-1 operational credential file format. Migration is config-only only among clients that implement the same FormationKey-only admission semantics.
- **Negative.** The schema is intentionally minimal; richer use cases (one bundle carrying credentials for multiple formations, embedded device PKI) need follow-up amendments.
- **Risks.** Operators who hand-edit credential files surface schema errors at load time. The reject-unknown-fields stance flags typos and stale fields rather than silently ignoring them.

### Current Enforcement Boundary (2026-07-21 Amendment)

The version-1 bundle proves possession of a formation-wide secret. It does not identify an individual member and carries no tier, permission, expiry, or revocation claim. Possession of the `shared_key` is therefore sufficient for the current FormationKey challenge and must not be described as equivalent to certificate-authorized membership.

`peat-mesh` defines certificate and certificate-bundle primitives, and its sync handler can perform an additional certificate check when a bundle is explicitly configured. That optional capability does not make certificate validation universal: the operational node bootstrap and the version-1 credential bundle do not yet establish a normative certificate exchange and enforcement path in both connection directions.

Consequently, a certificate issued by an enterprise gateway can be cryptographically valid without its tier, permissions, expiry, or revocation state governing admission in every deployed node. Operator interfaces MUST NOT represent those claims as an enforced mesh authorization boundary until the node runtime wires the corresponding contract.

A follow-up security decision MUST define:

1. whether FormationKey remains a first admission gate or becomes bootstrap/key-wrapping material;
2. how a peer proves possession of the private key bound to its membership certificate;
3. bidirectional validation of mesh ID, issuer, expiry, revocation, tier, and permissions before sync and blob transfer;
4. disconnected revocation and expiration behavior;
5. credential rotation and migration for existing FormationKey-only deployments; and
6. the versioned credential-bundle shape once certificate material becomes normative.

ADR-055's managed-runtime cutover is blocked on this decision. Until it lands, the flat `app_id` + `shared_key` bundle remains the documented operational admission format and certificate authorization remains an additional, explicitly configured layer rather than a universal invariant.

### Related

- peat-node ADR-001 §Credentials
- `peat-node/crates/peat-cli/src/creds.rs` (reference implementation, serde-derived)
- peat-mesh #135 (operational-credential interop discussion, if applicable)

## Open Questions

1. **How to handle certificate distribution in disconnected environments?**
   - Pre-provisioning before deployment?
   - Secure transfer via physical media?

2. **What's the certificate revocation strategy without network?**
   - Offline CRLs updated during mission planning?
   - Time-limited certificates with short validity?

3. **How to handle compromised devices in the field?**
   - Manual removal from trust store?
   - Automatic detection and isolation?

4. **Should we support different security levels for different data?**
   - Unclassified, Secret, Top Secret handling?
   - Multi-level security (MLS) architecture?

5. **How to handle human-in-the-loop approval latency?**
   - Timeout policies?
   - Automated fallback for time-critical scenarios?

## References

- [NIST SP 800-53](https://csrc.nist.gov/publications/detail/sp/800-53/rev-5/final) - Security Controls
- [DoD Zero Trust Reference Architecture](https://dodcio.defense.gov/Portals/0/Documents/Library/(U)ZT_RA_v2.0(U)_Sep22.pdf)
- [FIPS 140-3](https://csrc.nist.gov/publications/detail/fips/140/3/final) - Cryptographic Standards
- [RFC 5280](https://datatracker.ietf.org/doc/html/rfc5280) - X.509 PKI Certificate Profile
- [RFC 6749](https://datatracker.ietf.org/doc/html/rfc6749) - OAuth 2.0 Authorization Framework (adapted for military use)

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2025-11-04 | Proposed multi-layer security architecture | Comprehensive defense for tactical systems |
| 2025-11-24 | Multi-hop trust: Trust all mesh members (Phase 1) | CRDT sync requires relay nodes to read documents; E2E encryption deferred to Phase 2 |
| 2026-05-29 | Operator credential bundle format ([peat#940](https://github.com/defenseunicorns/peat/issues/940)) | Promotes peat-cli's placeholder YAML shape to the canonical operational mesh-joining bundle so multiple Peat clients converge on one file format |
| TBD | Full ADR approval | After team and security review |

---

**Next Steps**:
1. Security review by DoD cybersecurity experts
2. Threat modeling workshop with red team
3. Prototype device authentication with test PKI
4. Integrate with ADR-005 abstraction layer
5. Define cryptographic cipher suites and key sizes
