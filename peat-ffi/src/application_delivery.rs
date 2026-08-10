//! Bounded UniFFI projection of peat-mesh durable application delivery.
//!
//! This module owns no transport, scheduler, persistence, or acknowledgement
//! state. Every mutation delegates to the manager embedded in the canonical
//! mesh backend. Status subscriptions are bounded, cursor-paginated durable
//! rescans: callers restart at `None` after any notification gap.
//!
//! The immutable owner API currently exposes received application documents by
//! known `(collection, document_id)` only. It does not expose a bounded received
//! collection iterator, so this facade deliberately does not claim collection
//! catch-up. Adding that capability requires an owner-layer API rather than a
//! second FFI index or access to owner persistence internals.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use peat_protocol::network::EndpointId;
use peat_protocol::storage::application_delivery::{
    DeliveryAudience, DeliveryOperation, DeliveryPriority, DeliveryRequest, DeliveryStatus,
    RegistryValidator,
};
use peat_schema::type_registry::{BuiltinRegistry, TypeId, TypeRegistry};

use crate::{PeatError, PeatNode};

const MAX_OPERATION_ID_BYTES: usize = 256;
const MAX_COLLECTION_BYTES: usize = 128;
const MAX_TYPE_ID_BYTES: usize = 256;
const MAX_DOCUMENT_ID_BYTES: usize = 256;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_TARGETS: usize = 256;
const MAX_PAGE_LIMIT: u32 = 100;
const MAX_CURSOR_BYTES: usize = 64;
const MAX_RECEIVED_PAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ApplicationDeliveryAudience {
    Direct,
    Group,
    Broadcast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ApplicationDeliveryPriority {
    Metadata,
    Normal,
    Bulk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ApplicationDeliveryStatus {
    Queued,
    Sent,
    Delivered,
    Failed,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ApplicationDeliverySubmitRequest {
    pub client_operation_id: String,
    pub audience: ApplicationDeliveryAudience,
    pub target_node_ids: Vec<String>,
    pub priority: ApplicationDeliveryPriority,
    pub collection: String,
    pub type_id: String,
    pub document_id: String,
    pub body: Vec<u8>,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ApplicationRecipientEvidence {
    pub recipient_node_id: String,
    pub status: ApplicationDeliveryStatus,
    pub updated_at_ms: u64,
    pub attempts: u32,
}

/// Body-free durable status projection.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ApplicationDeliveryOperation {
    pub client_operation_id: String,
    pub sender_node_id: String,
    pub audience: ApplicationDeliveryAudience,
    pub priority: ApplicationDeliveryPriority,
    pub collection: String,
    pub type_id: String,
    pub document_id: String,
    pub expires_at_ms: u64,
    pub created_at_ms: u64,
    pub recipients: Vec<ApplicationRecipientEvidence>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ApplicationDeliveryPage {
    pub operations: Vec<ApplicationDeliveryOperation>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReceivedApplicationDocument {
    pub collection: String,
    pub document_id: String,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReceivedApplicationDocumentPage {
    pub documents: Vec<ReceivedApplicationDocument>,
    pub next_cursor: Option<String>,
}

struct BuiltinDeliveryValidator {
    registry: BuiltinRegistry,
}

impl BuiltinDeliveryValidator {
    fn new() -> Self {
        Self {
            registry: BuiltinRegistry::with_peat_schema_types(),
        }
    }

    fn validate_collection_and_body(
        &self,
        collection: &str,
        type_id: &str,
        body: &[u8],
    ) -> Result<(), String> {
        let descriptor = self
            .registry
            .get(&TypeId::new(type_id))
            .ok_or_else(|| "type ID is not registered".to_string())?;
        if descriptor.canonical_collection.as_deref() != Some(collection) {
            return Err("collection does not match the registered type".to_string());
        }
        let value: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| "body is not valid JSON".to_string())?;
        (descriptor.validate_json)(&value).map_err(|error| error.to_string())
    }
}

impl RegistryValidator for BuiltinDeliveryValidator {
    fn validate(&self, type_id: &str, body: &[u8]) -> Result<(), String> {
        let descriptor = self
            .registry
            .get(&TypeId::new(type_id))
            .ok_or_else(|| "type ID is not registered".to_string())?;
        let value: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| "body is not valid JSON".to_string())?;
        (descriptor.validate_json)(&value).map_err(|error| error.to_string())
    }
}

pub(crate) fn install_builtin_validator(
    backend: &peat_mesh::sync::AutomergeBackend,
) -> Result<(), PeatError> {
    backend
        .install_application_delivery_validator(Arc::new(BuiltinDeliveryValidator::new()))
        .map_err(|error| PeatError::SyncError {
            msg: format!("failed to install application delivery validator: {error}"),
        })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn validate_token(label: &str, value: &str, maximum: usize) -> Result<(), PeatError> {
    if value.is_empty()
        || value.len() > maximum
        || value.chars().any(char::is_control)
        || value.contains(':')
    {
        return Err(PeatError::InvalidInput {
            msg: format!("{label} is invalid"),
        });
    }
    Ok(())
}

fn canonical_endpoint_id(ffi_node_id: &str) -> Result<String, PeatError> {
    let bytes = hex::decode(ffi_node_id).map_err(|_| PeatError::InvalidInput {
        msg: "target node ID must be a 32-byte hexadecimal endpoint ID".to_string(),
    })?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| PeatError::InvalidInput {
        msg: "target node ID must be a 32-byte hexadecimal endpoint ID".to_string(),
    })?;
    EndpointId::from_bytes(&bytes)
        .map(|endpoint| endpoint.to_string())
        .map_err(|_| PeatError::InvalidInput {
            msg: "target node ID is not a valid endpoint ID".to_string(),
        })
}

fn ffi_endpoint_id(canonical: &str) -> String {
    canonical
        .parse::<EndpointId>()
        .map(|endpoint| hex::encode(endpoint.as_bytes()))
        .unwrap_or_else(|_| canonical.to_string())
}

fn owner_error(error: anyhow::Error) -> PeatError {
    PeatError::StorageError {
        msg: error.to_string(),
    }
}

fn received_document(
    collection: String,
    document_id: String,
    body: Vec<u8>,
) -> Result<ReceivedApplicationDocument, PeatError> {
    validate_token("received collection", &collection, MAX_COLLECTION_BYTES)?;
    validate_token("received document ID", &document_id, MAX_DOCUMENT_ID_BYTES)?;
    if body.is_empty() || body.len() > MAX_BODY_BYTES {
        return Err(PeatError::StorageError {
            msg: "received application document body violates FFI bounds".to_string(),
        });
    }
    Ok(ReceivedApplicationDocument {
        collection,
        document_id,
        body,
    })
}

fn received_page(
    documents: Vec<peat_mesh::storage::ApplicationDocument>,
    next_cursor: Option<String>,
) -> Result<ReceivedApplicationDocumentPage, PeatError> {
    let mut total_bytes = 0usize;
    let documents = documents
        .into_iter()
        .map(|document| {
            total_bytes = total_bytes
                .checked_add(document.body.len())
                .ok_or_else(|| PeatError::StorageError {
                    msg: "received application document page size overflow".to_string(),
                })?;
            if total_bytes > MAX_RECEIVED_PAGE_BYTES {
                return Err(PeatError::StorageError {
                    msg: "received application document page violates FFI bounds".to_string(),
                });
            }
            received_document(document.collection, document.document_id, document.body)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReceivedApplicationDocumentPage {
        documents,
        next_cursor,
    })
}

fn audience_kind(audience: &DeliveryAudience) -> ApplicationDeliveryAudience {
    match audience {
        DeliveryAudience::Direct(_) => ApplicationDeliveryAudience::Direct,
        DeliveryAudience::Group(_) => ApplicationDeliveryAudience::Group,
        DeliveryAudience::Broadcast(_) => ApplicationDeliveryAudience::Broadcast,
    }
}

fn priority_from_owner(priority: DeliveryPriority) -> ApplicationDeliveryPriority {
    match priority {
        DeliveryPriority::Metadata => ApplicationDeliveryPriority::Metadata,
        DeliveryPriority::Normal => ApplicationDeliveryPriority::Normal,
        DeliveryPriority::Bulk => ApplicationDeliveryPriority::Bulk,
    }
}

fn status_from_owner(status: DeliveryStatus) -> ApplicationDeliveryStatus {
    match status {
        DeliveryStatus::Queued => ApplicationDeliveryStatus::Queued,
        DeliveryStatus::Sent => ApplicationDeliveryStatus::Sent,
        DeliveryStatus::Acknowledged => ApplicationDeliveryStatus::Delivered,
        DeliveryStatus::Failed => ApplicationDeliveryStatus::Failed,
        DeliveryStatus::Expired => ApplicationDeliveryStatus::Expired,
        DeliveryStatus::Cancelled => ApplicationDeliveryStatus::Cancelled,
    }
}

fn operation_from_owner(operation: DeliveryOperation) -> ApplicationDeliveryOperation {
    ApplicationDeliveryOperation {
        client_operation_id: operation.client_operation_id,
        sender_node_id: ffi_endpoint_id(&operation.sender_node_id),
        audience: audience_kind(&operation.audience),
        priority: priority_from_owner(operation.priority),
        collection: operation.collection,
        type_id: operation.type_id,
        document_id: operation.document_id,
        expires_at_ms: operation.expires_at_ms,
        created_at_ms: operation.created_at_ms,
        recipients: operation
            .recipients
            .into_iter()
            .map(|evidence| ApplicationRecipientEvidence {
                recipient_node_id: ffi_endpoint_id(&evidence.recipient_node_id),
                status: status_from_owner(evidence.status),
                updated_at_ms: evidence.updated_at_ms,
                attempts: evidence.attempts,
            })
            .collect(),
    }
}

fn parse_cursor(cursor: Option<String>) -> Result<usize, PeatError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    if cursor.len() > MAX_CURSOR_BYTES {
        return Err(PeatError::InvalidInput {
            msg: "delivery cursor exceeds bound".to_string(),
        });
    }
    let offset = cursor
        .strip_prefix("v1:")
        .ok_or_else(|| PeatError::InvalidInput {
            msg: "delivery cursor version is unsupported".to_string(),
        })?;
    offset
        .parse::<usize>()
        .map_err(|_| PeatError::InvalidInput {
            msg: "delivery cursor is malformed".to_string(),
        })
}

#[uniffi::export]
impl PeatNode {
    pub fn application_delivery_submit(
        &self,
        request: ApplicationDeliverySubmitRequest,
    ) -> Result<String, PeatError> {
        validate_token(
            "client operation ID",
            &request.client_operation_id,
            MAX_OPERATION_ID_BYTES,
        )?;
        validate_token("collection", &request.collection, MAX_COLLECTION_BYTES)?;
        validate_token("type ID", &request.type_id, MAX_TYPE_ID_BYTES)?;
        validate_token("document ID", &request.document_id, MAX_DOCUMENT_ID_BYTES)?;
        if request.body.is_empty() || request.body.len() > MAX_BODY_BYTES {
            return Err(PeatError::InvalidInput {
                msg: "body is empty or exceeds the 1 MiB bound".to_string(),
            });
        }
        if request.expires_at_ms <= now_ms() {
            return Err(PeatError::InvalidInput {
                msg: "expiry must be in the future".to_string(),
            });
        }
        if request.target_node_ids.is_empty() || request.target_node_ids.len() > MAX_TARGETS {
            return Err(PeatError::InvalidInput {
                msg: "target count is outside bounds".to_string(),
            });
        }
        if request.audience == ApplicationDeliveryAudience::Direct
            && request.target_node_ids.len() != 1
        {
            return Err(PeatError::InvalidInput {
                msg: "direct audience requires exactly one target".to_string(),
            });
        }
        BuiltinDeliveryValidator::new()
            .validate_collection_and_body(&request.collection, &request.type_id, &request.body)
            .map_err(|msg| PeatError::InvalidInput { msg })?;

        let targets = request
            .target_node_ids
            .iter()
            .map(|target| canonical_endpoint_id(target))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if targets.len() != request.target_node_ids.len() {
            return Err(PeatError::InvalidInput {
                msg: "target node IDs must be unique".to_string(),
            });
        }
        let audience = match request.audience {
            ApplicationDeliveryAudience::Direct => DeliveryAudience::Direct(targets),
            ApplicationDeliveryAudience::Group => DeliveryAudience::Group(targets),
            ApplicationDeliveryAudience::Broadcast => DeliveryAudience::Broadcast(targets),
        };
        let priority = match request.priority {
            ApplicationDeliveryPriority::Metadata => DeliveryPriority::Metadata,
            ApplicationDeliveryPriority::Normal => DeliveryPriority::Normal,
            ApplicationDeliveryPriority::Bulk => DeliveryPriority::Bulk,
        };
        let owner_request = DeliveryRequest {
            client_operation_id: request.client_operation_id,
            audience,
            priority,
            collection: request.collection,
            type_id: request.type_id,
            document_id: request.document_id,
            body: request.body,
            expires_at_ms: request.expires_at_ms,
        };
        self.sync_backend
            .application_delivery()
            .manager()
            .submit(owner_request, now_ms())
            .map_err(owner_error)
    }

    pub fn application_delivery_get(
        &self,
        operation_id: &str,
    ) -> Result<ApplicationDeliveryOperation, PeatError> {
        validate_token("operation ID", operation_id, MAX_OPERATION_ID_BYTES)?;
        self.sync_backend
            .application_delivery()
            .manager()
            .get(operation_id)
            .map(operation_from_owner)
            .map_err(owner_error)
    }

    pub fn application_delivery_list(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<ApplicationDeliveryPage, PeatError> {
        if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
            return Err(PeatError::InvalidInput {
                msg: format!("limit must be 1..={MAX_PAGE_LIMIT}"),
            });
        }
        let offset = parse_cursor(cursor)?;
        let operations = self
            .sync_backend
            .application_delivery()
            .manager()
            .list()
            .map_err(owner_error)?;
        if offset > operations.len() {
            return Err(PeatError::InvalidInput {
                msg: "delivery cursor is beyond the durable operation set".to_string(),
            });
        }
        let end = offset.saturating_add(limit as usize).min(operations.len());
        let next_cursor = (end < operations.len()).then(|| format!("v1:{end}"));
        Ok(ApplicationDeliveryPage {
            operations: operations[offset..end]
                .iter()
                .cloned()
                .map(operation_from_owner)
                .collect(),
            next_cursor,
        })
    }

    pub fn application_delivery_cancel(
        &self,
        operation_id: &str,
    ) -> Result<ApplicationDeliveryOperation, PeatError> {
        validate_token("operation ID", operation_id, MAX_OPERATION_ID_BYTES)?;
        let manager = self.sync_backend.application_delivery().manager();
        manager
            .cancel(operation_id, now_ms())
            .map_err(owner_error)?;
        manager
            .get(operation_id)
            .map(operation_from_owner)
            .map_err(owner_error)
    }

    pub fn application_delivery_retry(
        &self,
        operation_id: &str,
    ) -> Result<ApplicationDeliveryOperation, PeatError> {
        validate_token("operation ID", operation_id, MAX_OPERATION_ID_BYTES)?;
        let manager = self.sync_backend.application_delivery().manager();
        manager.retry(operation_id, now_ms()).map_err(owner_error)?;
        manager
            .get(operation_id)
            .map(operation_from_owner)
            .map_err(owner_error)
    }

    /// Return a bounded durable status page for polling subscribers.
    ///
    /// Callers begin each authoritative rescan with `cursor = None` and page to
    /// completion. This remains correct if transient owner notifications lag or
    /// are dropped because notifications are never the status source of truth.
    pub fn application_delivery_subscribe(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<ApplicationDeliveryPage, PeatError> {
        self.application_delivery_list(cursor, limit)
    }

    /// Retrieve one durably materialized inbound body when its stable key is known.
    pub fn application_delivery_get_received_document(
        &self,
        collection: &str,
        document_id: &str,
    ) -> Result<Option<ReceivedApplicationDocument>, PeatError> {
        validate_token("collection", collection, MAX_COLLECTION_BYTES)?;
        validate_token("document ID", document_id, MAX_DOCUMENT_ID_BYTES)?;
        self.sync_backend
            .application_delivery()
            .documents()
            .get(collection, document_id)
            .map_err(owner_error)?
            .map(|body| received_document(collection.to_string(), document_id.to_string(), body))
            .transpose()
    }

    /// Page through durably materialized inbound documents without relying on
    /// transient delivery notifications.
    pub fn application_delivery_list_received_documents(
        &self,
        collection: &str,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<ReceivedApplicationDocumentPage, PeatError> {
        validate_token("collection", collection, MAX_COLLECTION_BYTES)?;
        if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
            return Err(PeatError::InvalidInput {
                msg: format!("limit must be 1..={MAX_PAGE_LIMIT}"),
            });
        }
        self.sync_backend
            .application_delivery()
            .documents()
            .query(collection, cursor.as_deref(), limit)
            .map_err(owner_error)
            .and_then(|page| received_page(page.documents, page.next_cursor))
    }
}

#[cfg(test)]
mod received_output_bounds_tests {
    use super::*;

    fn document(id: &str, body_len: usize) -> peat_mesh::storage::ApplicationDocument {
        peat_mesh::storage::ApplicationDocument {
            collection: "collaboration-geochat".to_string(),
            document_id: id.to_string(),
            body: vec![b'x'; body_len],
        }
    }

    #[test]
    fn received_projection_rejects_oversized_records_and_pages() {
        assert!(received_document(
            "collaboration-geochat".to_string(),
            "oversized".to_string(),
            vec![b'x'; MAX_BODY_BYTES + 1],
        )
        .is_err());

        let page = (0..5)
            .map(|index| document(&format!("document-{index}"), MAX_BODY_BYTES))
            .collect();
        assert!(received_page(page, None).is_err());
    }
}
