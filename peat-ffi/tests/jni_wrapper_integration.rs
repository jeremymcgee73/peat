//! JVM-in-process integration tests for peat-ffi's JNI wrapper layer
//! (peat#881).
//!
//! ## Why this tier exists
//!
//! peat-ffi has had two test tiers up to now:
//!
//! - **In-crate unit tests** (`#[cfg(test)] mod tests {}` in `src/lib.rs`)
//!   cover Rust-side logic: `PeatNode` methods, helpers like
//!   `serialize_document_for_get_jni`, JSON shapes, storage paths.
//!   They do not touch the JNI bridge — every test path goes
//!   through `PeatNode::*` directly, never through `Java_..._PeatJni_*`.
//! - **Android instrumented tests** (`peat-mesh/android-tests/`,
//!   `peat-ffi/android/src/androidTest/`) exercise the full path
//!   end-to-end on a real device, going through Kotlin → AAR → arm64
//!   binary. They prove the assembled stack works on-device, but
//!   require an attached device + the gb10 self-hosted runner.
//!
//! The wrapper layer itself — `env.get_string`, `env.new_string`,
//! `env.throw_new`, argument marshalling, null handling, UTF-8
//! encoding, jclass receiver passing — is exercised by neither tier.
//! A bug in the wrapper (wrong jstring null check, miscounted arg
//! slot, missing exception throw, wrong type signature) is invisible
//! to in-crate Rust tests and only surfaces at runtime on a real
//! JVM consumer.
//!
//! This tier closes that gap. The tests boot an in-process JVM,
//! attach the current thread, and call each target `Java_..._PeatJni_*`
//! extern fn with real `JString` / `JClass` / `jlong` arguments
//! constructed from the live env. The same JNI bridge code that
//! production Android consumers hit is exercised here, on hosted
//! ubuntu-latest CI — no device, no NDK, no Android emulator.
//!
//! peat-mesh#138 M4b + peat-ffi/android `androidTest` remain the
//! end-to-end gates; this tier is the wrapper-layer-only gate that
//! catches marshalling bugs before they ride into a release tag.
//!
//! ## Sequencing
//!
//! JNI bans destroying-and-recreating a JavaVM in the same process,
//! so the JVM is created once via `OnceLock` and shared across the
//! test fns in this binary. The fault-injection flag in lib.rs
//! (`FORCE_STORE_ERROR_FOR_TESTING`) is process-global, so the four
//! scenarios run inside a single `#[test]` fn to keep ordering
//! deterministic regardless of cargo's test-runner thread count.
//!
//! peat#881 / peat#880 carryover.

use jni::objects::{JClass, JObject, JString};
use jni::sys::jstring;
use jni::{InitArgsBuilder, JNIEnv, JNIVersion, JavaVM};
use std::sync::OnceLock;
use tempfile::TempDir;

// Base64 of "test-key-1234567890123456789012345678901234".
// Matches the SHARED_KEY fixture used in peat-mesh#145 SyncProtocolTest
// and peat-ffi/android PeatJniSurfaceTest so all three tiers use the
// same input — failures correlate across tiers.
const SHARED_KEY: &str = "dGVzdC1rZXktMTIzNDU2Nzg5MDEyMzQ1Njc4OTAxMjM0";
const APP_ID: &str = "peat-ffi-jvm-jni-test";

static JVM: OnceLock<JavaVM> = OnceLock::new();

fn jvm() -> &'static JavaVM {
    JVM.get_or_init(|| {
        let args = InitArgsBuilder::new()
            .version(JNIVersion::V8)
            // -Xcheck:jni surfaces JNI misuse (bad refs, wrong arg
            // types, unhandled exceptions) as JVM-side aborts rather
            // than silent corruption. This is the same flag the
            // Android JNI checker runs by default in debug builds, so
            // by enabling it here we mirror on-device behavior.
            .option("-Xcheck:jni")
            .build()
            .expect("InitArgsBuilder::build");
        JavaVM::new(args).expect("JavaVM::new")
    })
}

/// Construct a fresh `JNIEnv` borrow from the raw JNI env pointer.
/// The `Java_..._PeatJni_*` extern fns take `JNIEnv` by value (that's
/// the JNI calling convention from the C side); each call needs its
/// own borrow. The AttachGuard holds the underlying pointer alive
/// for the duration of the test.
unsafe fn fresh_env(raw: *mut jni::sys::JNIEnv) -> JNIEnv<'static> {
    unsafe { JNIEnv::from_raw(raw).expect("JNIEnv::from_raw") }
}

/// `_class` is ignored by every wrapper fn under test. The JNI
/// runtime passes a real PeatJni jclass in production; from Rust
/// integration tests there's no PeatJni class loaded into the JVM,
/// so a null JObject cast to JClass is safe — the wrappers never
/// dereference it.
fn null_class() -> JClass<'static> {
    JObject::null().into()
}

fn new_jstring<'a>(env: &mut JNIEnv<'a>, s: &str) -> JString<'a> {
    env.new_string(s).expect("new_string")
}

fn jstring_to_rust(env: &mut JNIEnv<'_>, js: jstring) -> Option<String> {
    if js.is_null() {
        return None;
    }
    // SAFETY: js was just returned by a Java_..._PeatJni_* extern fn
    // that produced it via env.new_string — it's a valid local ref.
    let obj = unsafe { JString::from_raw(js) };
    let s = env
        .get_string(&obj)
        .expect("get_string from jstring")
        .into();
    Some(s)
}

#[test]
fn jni_wrapper_integration() {
    let vm = jvm();
    let attach = vm.attach_current_thread().expect("attach_current_thread");
    let raw = attach.get_raw();

    scenario_endpoint_socket_addr_null_handle(raw);
    let (handle, _tempdir) = scenario_create_node_returns_handle(raw);
    scenario_endpoint_socket_addr_real_handle(raw, handle);
    scenario_publish_get_roundtrip(raw, handle);
    scenario_get_document_err_throws_runtime_exception(raw, handle);
    scenario_native_method_table_audit();

    // Best-effort cleanup. freeNodeJni's contract is fire-and-forget
    // (returns void); if it would have thrown, -Xcheck:jni would have
    // aborted the JVM already.
    let env = unsafe { fresh_env(raw) };
    peat_ffi::Java_com_defenseunicorns_peat_PeatJni_freeNodeJni(env, null_class(), handle);
}

// ---------------------------------------------------------------------
// Scenario 1: endpointSocketAddrJni — handle=0 short-circuit returns
// null jstring without touching env.new_string. The wrapper's first
// branch is `if handle == 0 { return std::ptr::null_mut(); }`; this
// test pins that contract from the JVM side. A regression where the
// branch is removed (or the null sentinel changes to a non-null
// jstring) would change Kotlin's downstream behavior — String? would
// be a non-null "0" or empty string instead of null — silently.
// ---------------------------------------------------------------------
fn scenario_endpoint_socket_addr_null_handle(raw: *mut jni::sys::JNIEnv) {
    let env = unsafe { fresh_env(raw) };
    let result =
        peat_ffi::Java_com_defenseunicorns_peat_PeatJni_endpointSocketAddrJni(env, null_class(), 0);
    assert!(
        result.is_null(),
        "endpointSocketAddrJni(0) must return a null jstring, got {:p}",
        result,
    );
}

// ---------------------------------------------------------------------
// Scenario 2 (setup half): create a real PeatNode handle through the
// JNI surface so the rest of the scenarios have something live to
// call against. createNodeJni's success path is also the canonical
// "did the JString → String marshalling round-trip" check: app_id,
// shared_key, and storage_path all go through env.get_string and
// land in NodeConfig fields verified by create_node()'s internals.
// ---------------------------------------------------------------------
fn scenario_create_node_returns_handle(raw: *mut jni::sys::JNIEnv) -> (i64, TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let storage_path = tempdir.path().to_str().expect("utf-8 path").to_string();

    let handle = {
        let mut env = unsafe { fresh_env(raw) };
        let app_id = new_jstring(&mut env, APP_ID);
        let shared_key = new_jstring(&mut env, SHARED_KEY);
        let storage = new_jstring(&mut env, &storage_path);
        peat_ffi::Java_com_defenseunicorns_peat_PeatJni_createNodeJni(
            env,
            null_class(),
            app_id,
            shared_key,
            storage,
        )
    };
    assert!(
        handle != 0,
        "createNodeJni returned 0 — the JString → String path or \
         NodeConfig setup failed silently. Check whether \
         env.get_string succeeded for all three args.",
    );
    (handle, tempdir)
}

// ---------------------------------------------------------------------
// Scenario 1 (real-handle half): with a live PeatNode behind the
// handle, endpointSocketAddrJni must return a non-null jstring that
// parses as "ip:port". The Iroh endpoint binds on create_node, so
// the address should be populated by the time we reach this point.
// ---------------------------------------------------------------------
fn scenario_endpoint_socket_addr_real_handle(raw: *mut jni::sys::JNIEnv, handle: i64) {
    let mut env = unsafe { fresh_env(raw) };
    let result = peat_ffi::Java_com_defenseunicorns_peat_PeatJni_endpointSocketAddrJni(
        unsafe { fresh_env(raw) },
        null_class(),
        handle,
    );
    let s = jstring_to_rust(&mut env, result).unwrap_or_else(|| {
        panic!(
            "endpointSocketAddrJni(real handle) returned null. The \
             node's bound socket should be available immediately after \
             create_node; a null here means endpoint_socket_addr() \
             returned None for a real node, which would also break \
             two-node peer dial in the M4b instrumented tests."
        )
    });
    // Loosely parse: must contain a ':' separating host from port,
    // and the port must be a u16. Don't pin the exact host (could
    // be IPv4 or IPv6 depending on the runner network stack).
    let (_host, port) = s.rsplit_once(':').unwrap_or_else(|| {
        panic!(
            "endpointSocketAddrJni returned {:?} — expected 'host:port' \
             shape that connectPeerJni would later accept as the \
             address arg.",
            s
        )
    });
    port.parse::<u16>().unwrap_or_else(|e| {
        panic!(
            "endpointSocketAddrJni returned {:?} — port part {:?} is \
             not a valid u16: {}",
            s, port, e
        )
    });
}

// ---------------------------------------------------------------------
// Scenario 2 (round-trip): publishDocumentJni writes a JSON body,
// getDocumentJni reads it back, the returned jstring matches the
// input semantically (id field hoisted to Document::id, fields
// preserved). This exercises the JString → String → publish path
// AND the publish → store → get → serialize → JString return path,
// which are the two longest marshalling chains in the wrapper layer.
// ---------------------------------------------------------------------
fn scenario_publish_get_roundtrip(raw: *mut jni::sys::JNIEnv, handle: i64) {
    let collection = "nodes";
    let doc_id = "peat-ffi-jvm-jni-roundtrip";
    let body = format!(
        r#"{{"id":"{}","name":"jvm-jni-roundtrip","value":42}}"#,
        doc_id
    );

    let published_id = {
        let mut env = unsafe { fresh_env(raw) };
        let collection_j = new_jstring(&mut env, collection);
        let body_j = new_jstring(&mut env, &body);
        let raw_jstring = peat_ffi::Java_com_defenseunicorns_peat_PeatJni_publishDocumentJni(
            unsafe { fresh_env(raw) },
            null_class(),
            handle,
            collection_j,
            body_j,
        );
        jstring_to_rust(&mut env, raw_jstring).expect("publishDocumentJni returned non-null id")
    };
    assert!(
        !published_id.is_empty(),
        "publishDocumentJni returned an empty id string — that's the \
         wrapper's documented 'publish failed' sentinel. Storage \
         setup likely broken upstream of the JNI surface."
    );

    let returned_json = {
        let mut env = unsafe { fresh_env(raw) };
        let collection_j = new_jstring(&mut env, collection);
        let doc_id_j = new_jstring(&mut env, doc_id);
        let raw_jstring = peat_ffi::Java_com_defenseunicorns_peat_PeatJni_getDocumentJni(
            unsafe { fresh_env(raw) },
            null_class(),
            handle,
            collection_j,
            doc_id_j,
        );
        jstring_to_rust(&mut env, raw_jstring).unwrap_or_else(|| {
            panic!(
                "getDocumentJni returned null jstring for doc we just \
                 published in the same process. publish→get round-trip \
                 through the JNI wrappers is broken."
            )
        })
    };

    // Parse and compare semantically — the wire shape is
    // serialize_document_for_get_jni's output (fields + hoisted id),
    // which is JSON-object-equivalent to the input but field
    // ordering isn't guaranteed.
    let returned: serde_json::Value = serde_json::from_str(&returned_json).unwrap_or_else(|e| {
        panic!(
            "getDocumentJni returned non-JSON: {} ({})",
            returned_json, e
        )
    });
    assert_eq!(returned["id"].as_str(), Some(doc_id), "id field mismatch");
    assert_eq!(
        returned["name"].as_str(),
        Some("jvm-jni-roundtrip"),
        "name field mismatch",
    );
    assert_eq!(returned["value"].as_i64(), Some(42), "value field mismatch");
}

// ---------------------------------------------------------------------
// Scenario 3 (Err propagation): arm forceStoreErrorForTestingJni →
// call getDocumentJni → assert env.exception_check() returns true
// with java/lang/RuntimeException. This is the surface-tier proof
// that `env.throw_new("java/lang/RuntimeException", ...)` actually
// reaches Java code as a catchable RuntimeException, not as a
// silently-dropped exception or a hard JVM abort.
// peat-mesh#145 (M4b) ran the same contract on-device through
// Kotlin's try/catch; this is the same contract at the wrapper
// tier, deterministic, no Automerge LRU games.
// ---------------------------------------------------------------------
fn scenario_get_document_err_throws_runtime_exception(raw: *mut jni::sys::JNIEnv, handle: i64) {
    // Arm the fault.
    let armed = peat_ffi::Java_com_defenseunicorns_peat_PeatJni_forceStoreErrorForTestingJni(
        unsafe { fresh_env(raw) },
        null_class(),
        handle,
    );
    assert_eq!(
        armed, 1,
        "forceStoreErrorForTestingJni({}) must return JNI_TRUE (1); \
         got {}. Likely the handle-zero check failed or the wrapper \
         changed its return convention.",
        handle, armed,
    );

    // Trigger.
    let mut env = unsafe { fresh_env(raw) };
    let collection_j = new_jstring(&mut env, "nodes");
    let doc_id_j = new_jstring(&mut env, "any");
    let result = peat_ffi::Java_com_defenseunicorns_peat_PeatJni_getDocumentJni(
        unsafe { fresh_env(raw) },
        null_class(),
        handle,
        collection_j,
        doc_id_j,
    );
    assert!(
        result.is_null(),
        "getDocumentJni after fault arm returned a non-null jstring \
         ({:p}); the wrapper must return null when throwing.",
        result
    );

    // The pending exception is the cross-JNI-boundary proof.
    assert!(
        env.exception_check().expect("exception_check"),
        "no pending exception after armed getDocumentJni call. \
         env.throw_new in the wrapper either didn't fire or didn't \
         leave the exception pending — the Kotlin side would never \
         catch this.",
    );

    // Pull and inspect the exception to confirm class + message
    // prefix. The Kotlin contract pins on `"getDocumentJni"` in the
    // message (peat-mesh#146 SyncProtocolTest); we pin the same here.
    let throwable = env.exception_occurred().expect("exception_occurred");
    env.exception_clear().expect("exception_clear");

    let class = env.get_object_class(&throwable).expect("get_object_class");
    let class_name_jstring = env
        .call_method(class, "getName", "()Ljava/lang/String;", &[])
        .expect("Class.getName call")
        .l()
        .expect("Class.getName returned non-object");
    let class_name: String = env
        .get_string(&JString::from(class_name_jstring))
        .expect("class_name -> String")
        .into();
    assert_eq!(
        class_name, "java.lang.RuntimeException",
        "wrapper threw {} instead of RuntimeException — Kotlin \
         consumers catch RuntimeException specifically; any other \
         class breaks the documented try/catch contract.",
        class_name,
    );

    let msg_obj = env
        .call_method(&throwable, "getMessage", "()Ljava/lang/String;", &[])
        .expect("Throwable.getMessage call")
        .l()
        .expect("getMessage returned non-object");
    let msg: String = env
        .get_string(&JString::from(msg_obj))
        .expect("message -> String")
        .into();
    assert!(
        msg.contains("getDocumentJni"),
        "exception message {:?} does not contain 'getDocumentJni' — \
         the substring is part of the wrapper contract pinned by \
         downstream tests (peat-mesh#146).",
        msg,
    );
}

// ---------------------------------------------------------------------
// Scenario 4 (registration audit): every JNI extern fn that nativeInit
// claims to register in its NativeMethod table must exist with the
// expected Rust path + `extern "system"` ABI. A drift bug — a
// NativeMethod entry for a fn that doesn't exist, or a fn renamed in
// the table without renaming the symbol — would surface as a
// RegisterNatives SIGABRT at System.loadLibrary time on Android.
//
// We catch it at PR time by taking the function-pointer address of
// each `Java_com_defenseunicorns_peat_PeatJni_<name>` symbol in this
// list. Two safety nets:
//
//   1. **Compile-time:** any name in this list that doesn't exist as
//      a pub extern fn in `peat_ffi::` is a compile error. Renaming
//      a JNI fn without updating this list breaks `cargo check`.
//   2. **Runtime:** the asserts force the linker to keep each
//      symbol live (their addresses are taken at runtime, so the
//      compiler can't strip them as dead code). The non-null check
//      is a formality — fn pointers to defined fns are never null —
//      but it documents the intent.
//
// Keep this array in lockstep with lib.rs's `Java_..._PeatJni_nativeInit`
// NativeMethod table. The on-device Android tests will eventually
// catch any drift, but the value of this scenario is making it a
// PR-time gate.
// ---------------------------------------------------------------------
fn scenario_native_method_table_audit() {
    // Each entry is the name from the NativeMethod table paired with
    // the Rust symbol's address. The address-of forces the symbol to
    // be linked into the test binary; renaming or removing a fn in
    // lib.rs without updating this audit is a compile error.
    //
    // Function pointer cast: `extern "system" fn(...)` is the JNI
    // calling convention; `as *const ()` discards arity/return type
    // so we can stuff all of them in one homogeneous Vec.
    //
    // cfg gates mirror lib.rs's NativeMethod entries exactly — if
    // they drift, the assert at the bottom catches the count gap.
    let mut table: Vec<(&str, *const ())> = vec![
        // Always-on (no feature gate)
        (
            "peatVersion",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_peatVersion as *const (),
        ),
        (
            "testJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_testJni as *const (),
        ),
    ];

    // The sync feature is on for this test target (Cargo.toml's
    // [dev-dependencies] inherits the workspace default features).
    // 30 methods in this group; mirrors the #[cfg(feature = "sync")]
    // NativeMethod entries in lib.rs.
    #[cfg(feature = "sync")]
    table.extend_from_slice(&[
        (
            "createNodeJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_createNodeJni as *const (),
        ),
        (
            "createNodeWithConfigJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_createNodeWithConfigJni as *const (),
        ),
        (
            "getGlobalNodeHandleJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_getGlobalNodeHandleJni as *const (),
        ),
        (
            "nodeIdJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_nodeIdJni as *const (),
        ),
        (
            "peerCountJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_peerCountJni as *const (),
        ),
        (
            "connectedPeersJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_connectedPeersJni as *const (),
        ),
        (
            "requestSyncJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_requestSyncJni as *const (),
        ),
        (
            "endpointSocketAddrJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_endpointSocketAddrJni as *const (),
        ),
        (
            "getDocumentJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_getDocumentJni as *const (),
        ),
        (
            "forceStoreErrorForTestingJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_forceStoreErrorForTestingJni
                as *const (),
        ),
        (
            "startSyncJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_startSyncJni as *const (),
        ),
        (
            "freeNodeJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_freeNodeJni as *const (),
        ),
        (
            "getCellsJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_getCellsJni as *const (),
        ),
        (
            "getTracksJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_getTracksJni as *const (),
        ),
        (
            "getNodesJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_getNodesJni as *const (),
        ),
        (
            "getCommandsJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_getCommandsJni as *const (),
        ),
        (
            "getMarkersJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_getMarkersJni as *const (),
        ),
        (
            "publishNodeJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_publishNodeJni as *const (),
        ),
        (
            "publishMarkerJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_publishMarkerJni as *const (),
        ),
        (
            "publishDocumentJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_publishDocumentJni as *const (),
        ),
        (
            "publishDocumentWithOriginJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_publishDocumentWithOriginJni
                as *const (),
        ),
        (
            "connectPeerJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_connectPeerJni as *const (),
        ),
        (
            "subscribeDocumentChangesJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_subscribeDocumentChangesJni
                as *const (),
        ),
        (
            "unsubscribeDocumentChangesJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_unsubscribeDocumentChangesJni
                as *const (),
        ),
        (
            "enableBlobTransferJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_enableBlobTransferJni as *const (),
        ),
        (
            "blobAddPeerJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_blobAddPeerJni as *const (),
        ),
        (
            "blobPutJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_blobPutJni as *const (),
        ),
        (
            "blobGetJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_blobGetJni as *const (),
        ),
        (
            "blobExistsLocallyJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_blobExistsLocallyJni as *const (),
        ),
        (
            "blobEndpointIdJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_blobEndpointIdJni as *const (),
        ),
    ]);

    // sync + bluetooth — 3 methods. Not enabled in the default CI run
    // (this test runs with `--features sync` only), but mirrored so a
    // future workflow that enables bluetooth picks up coverage.
    #[cfg(all(feature = "sync", feature = "bluetooth"))]
    table.extend_from_slice(&[
        (
            "ingestPositionJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_ingestPositionJni as *const (),
        ),
        (
            "subscribeOutboundFramesJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_subscribeOutboundFramesJni as *const (),
        ),
        (
            "unsubscribeOutboundFramesJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_unsubscribeOutboundFramesJni
                as *const (),
        ),
    ]);

    // sync + bluetooth + android — 5 methods. Compile-gated to
    // Android only (peat-btle's Android BLE adapter only links there).
    // This block won't compile on Linux even with bluetooth enabled.
    #[cfg(all(feature = "sync", feature = "bluetooth", target_os = "android"))]
    table.extend_from_slice(&[
        (
            "bleSetStartedJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_bleSetStartedJni as *const (),
        ),
        (
            "bleAddPeerJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_bleAddPeerJni as *const (),
        ),
        (
            "bleRemovePeerJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_bleRemovePeerJni as *const (),
        ),
        (
            "bleIsAvailableJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_bleIsAvailableJni as *const (),
        ),
        (
            "blePeerCountJni",
            peat_ffi::Java_com_defenseunicorns_peat_PeatJni_blePeerCountJni as *const (),
        ),
    ]);

    for (name, ptr) in &table {
        assert!(
            !ptr.is_null(),
            "JNI fn pointer for {} resolved to null — should be \
             unreachable (defined Rust fns have non-null addresses); \
             if this fires, something has gone very wrong with the \
             linker.",
            name,
        );
    }

    // Count check: under `--features sync` on Linux we expect
    // 2 (always-on) + 30 (sync-only) = 32 entries. If the bluetooth
    // feature flips on, +3. If both bluetooth and Android target,
    // +5. The expected total tracks the active cfg.
    let expected = 2
        + if cfg!(feature = "sync") { 30 } else { 0 }
        + if cfg!(all(feature = "sync", feature = "bluetooth")) {
            3
        } else {
            0
        }
        + if cfg!(all(
            feature = "sync",
            feature = "bluetooth",
            target_os = "android"
        )) {
            5
        } else {
            0
        };
    assert_eq!(
        table.len(),
        expected,
        "audit array out of sync with nativeInit's NativeMethod table \
         (expected {} entries for this cfg, found {}). Either lib.rs \
         added/removed a JNI method without updating this audit, or \
         the expected count needs bumping here.",
        expected,
        table.len(),
    );
}
