// FFIBuffer scaffolding for peat_ffi.dart Dart FFI bindings.
//
// The Dart bindings use an "ffibuffer" transport: each call passes all
// arguments through a flat array of 8-byte union elements and receives
// results through a second flat array of the same type. This module
// exports the entry points the Dart side looks up by name.
//
// Buffer layouts (matching peat_ffi.dart):
//   Void return:          [0]=status(i8)  [1..3]=error_buf(u64,u64,ptr)
//   u32 return:           [0]=value(u32)  [1]=status(i8)  [2..4]=error_buf
//   i8/bool return:       [0]=value(i8)   [1]=status(i8)  [2..4]=error_buf
//   u64 handle return:    [0]=handle(u64) [1]=status(i8)  [2..4]=error_buf
//   RustBuffer return:    [0..2]=buf(u64,u64,ptr) [3]=status(i8) [4..6]=error_buf
//
// Arg buffer elements per type:
//   u64 handle / u64 callback:  1 element (.u64)
//   f64 primitive:              1 element (.f64)
//   u32 primitive:              1 element (.u32)
//   RustBuffer (any complex):   3 elements (.u64 cap, .u64 len, .ptr data)

#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::c_void;

// 8-byte union matching Dart's _UniFfiFfiBufferElement
#[repr(C)]
pub union Elem {
    pub u8: u8,
    pub i8: i8,
    pub u16: u16,
    pub i16: i16,
    pub u32: u32,
    pub i32: i32,
    pub u64: u64,
    pub i64: i64,
    pub f32: f32,
    pub f64: f64,
    pub ptr: *mut c_void,
}

// Mirrors uniffi_core::RustBuffer (must match C layout)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RustBuf {
    pub capacity: u64,
    pub len: u64,
    pub data: *mut u8,
}

impl RustBuf {
    pub fn empty() -> Self {
        RustBuf {
            capacity: 0,
            len: 0,
            data: std::ptr::null_mut(),
        }
    }
}

// Mirrors uniffi_core::RustCallStatus
#[repr(C)]
pub struct CallStatus {
    pub code: i8,
    pub error_buf: RustBuf,
}

impl CallStatus {
    pub fn new() -> Self {
        CallStatus {
            code: 0,
            error_buf: RustBuf::empty(),
        }
    }
}

// Mirrors uniffi_core::ForeignBytes
#[repr(C)]
pub struct ForeignBytes {
    pub len: i32,
    pub data: *const u8,
}

// --- Helpers ----------------------------------------------------------------

unsafe fn read_buf(e: *const Elem, i: usize) -> RustBuf {
    RustBuf {
        capacity: (*e.add(i)).u64,
        len: (*e.add(i + 1)).u64,
        data: (*e.add(i + 2)).ptr as *mut u8,
    }
}

unsafe fn write_buf(e: *mut Elem, i: usize, b: RustBuf) {
    (*e.add(i)).u64 = b.capacity;
    (*e.add(i + 1)).u64 = b.len;
    (*e.add(i + 2)).ptr = b.data as *mut c_void;
}

unsafe fn write_err(e: *mut Elem, i: usize, s: &CallStatus) {
    (*e.add(i)).i8 = s.code;
    write_buf(e, i + 1, s.error_buf);
}

// void return:      ret[0]=status, ret[1..3]=error
unsafe fn ret_void(e: *mut Elem, s: &CallStatus) {
    write_err(e, 0, s);
}

// u64 handle/primitive return: ret[0]=value, ret[1]=status, ret[2..4]=error
unsafe fn ret_u64(e: *mut Elem, v: u64, s: &CallStatus) {
    (*e.add(0)).u64 = v;
    write_err(e, 1, s);
}

// u32 return: ret[0]=value(u32), ret[1]=status, ret[2..4]=error
unsafe fn ret_u32(e: *mut Elem, v: u32, s: &CallStatus) {
    (*e.add(0)).u32 = v;
    write_err(e, 1, s);
}

// i8/bool return: ret[0]=value(i8), ret[1]=status, ret[2..4]=error
unsafe fn ret_i8(e: *mut Elem, v: i8, s: &CallStatus) {
    (*e.add(0)).i8 = v;
    write_err(e, 1, s);
}

// RustBuffer return: ret[0..2]=buf, ret[3]=status, ret[4..6]=error
unsafe fn ret_rbuf(e: *mut Elem, b: RustBuf, s: &CallStatus) {
    write_buf(e, 0, b);
    write_err(e, 3, s);
}

// --- Standard UniFFI function declarations ----------------------------------

extern "C" {
    fn ffi_peat_ffi_rustbuffer_from_bytes(b: ForeignBytes, s: *mut CallStatus) -> RustBuf;
    fn ffi_peat_ffi_rustbuffer_free(b: RustBuf, s: *mut CallStatus);

    // top-level functions
    fn uniffi_peat_ffi_fn_func_create_node(config: RustBuf, s: *mut CallStatus) -> u64;
    fn uniffi_peat_ffi_fn_func_create_position(
        lat: f64,
        lon: f64,
        hae: RustBuf,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_func_create_velocity(
        bearing: f64,
        speed: f64,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_func_encode_track_to_cot(track: RustBuf, s: *mut CallStatus) -> RustBuf;
    fn uniffi_peat_ffi_fn_func_peat_version(s: *mut CallStatus) -> RustBuf;

    // PeatNode methods
    fn uniffi_peat_ffi_fn_method_peatnode_all_peer_transport_states(
        h: u64,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_connect_peer(h: u64, addr: RustBuf, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_connected_peers(h: u64, s: *mut CallStatus) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_delete_document(
        h: u64,
        col: RustBuf,
        id: RustBuf,
        s: *mut CallStatus,
    );
    fn uniffi_peat_ffi_fn_method_peatnode_disconnect_peer(
        h: u64,
        node_id: RustBuf,
        s: *mut CallStatus,
    );
    fn uniffi_peat_ffi_fn_method_peatnode_endpoint_addr(h: u64, s: *mut CallStatus) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_endpoint_socket_addr(
        h: u64,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_get_cell(
        h: u64,
        col: RustBuf,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_get_cells(h: u64, s: *mut CallStatus) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_get_commands(h: u64, s: *mut CallStatus) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_get_document(
        h: u64,
        col: RustBuf,
        id: RustBuf,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_get_markers(h: u64, s: *mut CallStatus) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_get_nodes(h: u64, s: *mut CallStatus) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_get_track(
        h: u64,
        id: RustBuf,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_get_tracks(h: u64, s: *mut CallStatus) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_ingest_inbound_frame(
        h: u64,
        col: RustBuf,
        data: RustBuf,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_list_documents(
        h: u64,
        col: RustBuf,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_node_id(h: u64, s: *mut CallStatus) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_peer_count(h: u64, s: *mut CallStatus) -> u32;
    fn uniffi_peat_ffi_fn_method_peatnode_peer_transport_state(
        h: u64,
        id: RustBuf,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_poll_outbound_frames(
        h: u64,
        s: *mut CallStatus,
    ) -> RustBuf;
    fn uniffi_peat_ffi_fn_method_peatnode_put_cell(h: u64, cell: RustBuf, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_put_command(h: u64, cmd: RustBuf, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_put_document(
        h: u64,
        col: RustBuf,
        id: RustBuf,
        data: RustBuf,
        s: *mut CallStatus,
    );
    fn uniffi_peat_ffi_fn_method_peatnode_put_marker(h: u64, m: RustBuf, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_put_node(h: u64, node: RustBuf, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_put_track(h: u64, t: RustBuf, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_request_sync(h: u64, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_start_outbound_frames(h: u64, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_start_sync(h: u64, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_stop_outbound_frames(h: u64, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_stop_sync(h: u64, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_peatnode_subscribe(h: u64, cb: u64, s: *mut CallStatus) -> u64;
    fn uniffi_peat_ffi_fn_method_peatnode_subscribe_poll(h: u64, s: *mut CallStatus) -> u64;
    fn uniffi_peat_ffi_fn_method_peatnode_sync_document(
        h: u64,
        col: RustBuf,
        id: RustBuf,
        s: *mut CallStatus,
    );
    fn uniffi_peat_ffi_fn_method_peatnode_sync_stats(h: u64, s: *mut CallStatus) -> RustBuf;

    // SubscriptionHandle methods
    fn uniffi_peat_ffi_fn_method_subscriptionhandle_cancel(h: u64, s: *mut CallStatus);
    fn uniffi_peat_ffi_fn_method_subscriptionhandle_is_active(h: u64, s: *mut CallStatus) -> i8;
    fn uniffi_peat_ffi_fn_method_subscriptionhandle_poll_changes(
        h: u64,
        s: *mut CallStatus,
    ) -> RustBuf;
}

// --- rustbuffer aliases the Dart bindings expect ----------------------------

#[no_mangle]
pub unsafe extern "C" fn ffi_uniffi_peat_ffi_rustbuffer_from_bytes(
    b: ForeignBytes,
    s: *mut CallStatus,
) -> RustBuf {
    ffi_peat_ffi_rustbuffer_from_bytes(b, s)
}

#[no_mangle]
pub unsafe extern "C" fn ffi_uniffi_peat_ffi_rustbuffer_free(b: RustBuf, s: *mut CallStatus) {
    ffi_peat_ffi_rustbuffer_free(b, s)
}

// --- FFIBuffer wrappers -----------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_func_create_node(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_func_create_node(read_buf(a, 0), &mut s);
    ret_u64(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_func_create_position(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let lat = (*a.add(0)).f64;
    let lon = (*a.add(1)).f64;
    let hae = read_buf(a, 2);
    let v = uniffi_peat_ffi_fn_func_create_position(lat, lon, hae, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_func_create_velocity(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let bearing = (*a.add(0)).f64;
    let speed = (*a.add(1)).f64;
    let v = uniffi_peat_ffi_fn_func_create_velocity(bearing, speed, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_func_encode_track_to_cot(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_func_encode_track_to_cot(read_buf(a, 0), &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_func_peat_version(
    _a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_func_peat_version(&mut s);
    ret_rbuf(r, v, &s);
}

// PeatNode methods -----------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_all_peer_transport_states(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_all_peer_transport_states((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_connect_peer(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_connect_peer((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_connected_peers(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_connected_peers((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_delete_document(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_delete_document(
        (*a.add(0)).u64,
        read_buf(a, 1),
        read_buf(a, 4),
        &mut s,
    );
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_disconnect_peer(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_disconnect_peer((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_endpoint_addr(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_endpoint_addr((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_endpoint_socket_addr(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_endpoint_socket_addr((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_get_cell(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_get_cell((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_get_cells(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_get_cells((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_get_commands(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_get_commands((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_get_document(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_get_document(
        (*a.add(0)).u64,
        read_buf(a, 1),
        read_buf(a, 4),
        &mut s,
    );
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_get_markers(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_get_markers((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_get_nodes(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_get_nodes((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_get_track(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_get_track((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_get_tracks(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_get_tracks((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_ingest_inbound_frame(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_ingest_inbound_frame(
        (*a.add(0)).u64,
        read_buf(a, 1),
        read_buf(a, 4),
        &mut s,
    );
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_list_documents(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v =
        uniffi_peat_ffi_fn_method_peatnode_list_documents((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_node_id(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_node_id((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_peer_count(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_peer_count((*a.add(0)).u64, &mut s);
    ret_u32(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_peer_transport_state(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_peer_transport_state(
        (*a.add(0)).u64,
        read_buf(a, 1),
        &mut s,
    );
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_poll_outbound_frames(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_poll_outbound_frames((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_put_cell(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_put_cell((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_put_command(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_put_command((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_put_document(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_put_document(
        (*a.add(0)).u64,
        read_buf(a, 1),
        read_buf(a, 4),
        read_buf(a, 7),
        &mut s,
    );
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_put_marker(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_put_marker((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_put_node(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_put_node((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_put_track(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_put_track((*a.add(0)).u64, read_buf(a, 1), &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_request_sync(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_request_sync((*a.add(0)).u64, &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_start_outbound_frames(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_start_outbound_frames((*a.add(0)).u64, &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_start_sync(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_start_sync((*a.add(0)).u64, &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_stop_outbound_frames(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_stop_outbound_frames((*a.add(0)).u64, &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_stop_sync(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_stop_sync((*a.add(0)).u64, &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_subscribe(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_subscribe((*a.add(0)).u64, (*a.add(1)).u64, &mut s);
    ret_u64(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_subscribe_poll(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_subscribe_poll((*a.add(0)).u64, &mut s);
    ret_u64(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_sync_document(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_peatnode_sync_document(
        (*a.add(0)).u64,
        read_buf(a, 1),
        read_buf(a, 4),
        &mut s,
    );
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_peatnode_sync_stats(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_peatnode_sync_stats((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}

// SubscriptionHandle methods -------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_subscriptionhandle_cancel(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    uniffi_peat_ffi_fn_method_subscriptionhandle_cancel((*a.add(0)).u64, &mut s);
    ret_void(r, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_subscriptionhandle_is_active(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_subscriptionhandle_is_active((*a.add(0)).u64, &mut s);
    ret_i8(r, v, &s);
}

#[no_mangle]
pub unsafe extern "C" fn uniffi_ffibuffer_peat_ffi_fn_method_subscriptionhandle_poll_changes(
    a: *const Elem,
    r: *mut Elem,
) {
    let mut s = CallStatus::new();
    let v = uniffi_peat_ffi_fn_method_subscriptionhandle_poll_changes((*a.add(0)).u64, &mut s);
    ret_rbuf(r, v, &s);
}
