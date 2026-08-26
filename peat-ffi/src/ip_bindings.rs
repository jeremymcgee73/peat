//! Validated selected-IP bindings for platform-owned network routing.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};

#[cfg(target_os = "android")]
use std::{io, sync::Arc};

use peat_mesh::network::IpBindSpec;
use serde::Deserialize;

use crate::PeatError;

const MAX_IP_BINDINGS: usize = 32;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlatformIpBinding {
    address: String,
    prefix_length: u8,
    network_handle: u64,
    is_default_route: bool,
}

pub(crate) struct PreparedIpBindings {
    pub specs: Vec<IpBindSpec>,
    pub hook_guard: netwatch::UdpSocketBindHookGuard,
}

fn invalid(message: impl Into<String>) -> PeatError {
    PeatError::InvalidInput {
        msg: message.into(),
    }
}

fn parse_bindings(json: &str) -> Result<Vec<(IpBindSpec, u64)>, PeatError> {
    let parsed: Vec<PlatformIpBinding> =
        serde_json::from_str(json).map_err(|_| invalid("IP bindings JSON is malformed"))?;
    if parsed.is_empty() || parsed.len() > MAX_IP_BINDINGS {
        return Err(invalid(format!(
            "IP binding count must be between 1 and {MAX_IP_BINDINGS}"
        )));
    }

    let mut addresses = HashSet::new();
    let mut default_v4 = false;
    let mut default_v6 = false;
    parsed
        .into_iter()
        .map(|binding| {
            let ip: IpAddr = binding
                .address
                .parse()
                .map_err(|_| invalid("IP binding address must be a numeric IP address"))?;
            if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
                return Err(invalid(
                    "IP binding address must be concrete, non-loopback, and unicast",
                ));
            }
            if binding.network_handle == 0 {
                return Err(invalid("Android network handle must be non-zero"));
            }
            let max_prefix = if ip.is_ipv4() { 32 } else { 128 };
            if binding.prefix_length > max_prefix {
                return Err(invalid("IP binding prefix length is invalid"));
            }
            if !addresses.insert(ip) {
                return Err(invalid("duplicate IP binding address"));
            }
            if binding.is_default_route {
                let already_set = if ip.is_ipv4() {
                    std::mem::replace(&mut default_v4, true)
                } else {
                    std::mem::replace(&mut default_v6, true)
                };
                if already_set {
                    return Err(invalid(
                        "only one default IP binding is allowed per address family",
                    ));
                }
            }
            Ok((
                IpBindSpec {
                    addr: SocketAddr::new(ip, 0),
                    prefix_len: binding.prefix_length,
                    is_default_route: binding.is_default_route,
                },
                binding.network_handle,
            ))
        })
        .collect()
}

#[cfg(target_os = "android")]
fn install_hook(
    handles: HashMap<IpAddr, u64>,
) -> Result<netwatch::UdpSocketBindHookGuard, PeatError> {
    use std::os::fd::AsRawFd;

    #[link(name = "android")]
    unsafe extern "C" {
        fn android_setsocknetwork(network: u64, socket: std::os::raw::c_int)
            -> std::os::raw::c_int;
    }

    netwatch::install_udp_socket_bind_hook(Arc::new(
        move |addr: SocketAddr, socket: &socket2::Socket| {
            let handle = handles.get(&addr.ip()).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "no selected Android network owns local address {}",
                        addr.ip()
                    ),
                )
            })?;
            // SAFETY: `socket` is live for the duration of the callback; Android's
            // API neither takes ownership of the fd nor retains the pointer. The
            // network handle came from `Network.getNetworkHandle()` and was
            // validated as non-zero at the FFI boundary.
            let result = unsafe { android_setsocknetwork(*handle, socket.as_raw_fd()) };
            if result == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(-result))
            }
        },
    ))
    .map_err(|error| PeatError::ConnectionError {
        msg: format!("failed to install selected-network socket hook: {error}"),
    })
}

#[cfg(not(target_os = "android"))]
fn install_hook(
    _handles: HashMap<IpAddr, u64>,
) -> Result<netwatch::UdpSocketBindHookGuard, PeatError> {
    Err(invalid(
        "selected Android network bindings are unavailable on this platform",
    ))
}

pub(crate) fn prepare(json: &str) -> Result<PreparedIpBindings, PeatError> {
    let parsed = parse_bindings(json)?;
    let handles = parsed
        .iter()
        .map(|(spec, handle)| (spec.addr.ip(), *handle))
        .collect::<HashMap<_, _>>();
    let specs = parsed.into_iter().map(|(spec, _)| spec).collect();
    let hook_guard = install_hook(handles)?;
    Ok(PreparedIpBindings { specs, hook_guard })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_concurrent_local_and_default_paths() {
        let parsed = parse_bindings(
            r#"[
              {"address":"192.168.10.4","prefix_length":24,"network_handle":1001,"is_default_route":false},
              {"address":"10.20.30.4","prefix_length":24,"network_handle":2002,"is_default_route":true}
            ]"#,
        )
        .unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(!parsed[0].0.is_default_route);
        assert!(parsed[1].0.is_default_route);
    }

    #[test]
    fn parser_rejects_duplicate_defaults_and_unknown_fields() {
        let duplicate = r#"[
          {"address":"192.0.2.1","prefix_length":24,"network_handle":1,"is_default_route":true},
          {"address":"198.51.100.1","prefix_length":24,"network_handle":2,"is_default_route":true}
        ]"#;
        assert!(parse_bindings(duplicate).is_err());

        let unknown = r#"[
          {"address":"192.0.2.1","prefix_length":24,"network_handle":1,"is_default_route":false,"vendor":"x"}
        ]"#;
        assert!(parse_bindings(unknown).is_err());
    }

    #[test]
    fn parser_rejects_fallback_shaped_and_unsafe_addresses() {
        assert!(parse_bindings("[]").is_err());
        assert!(
            parse_bindings(
                r#"[{"address":"0.0.0.0","prefix_length":0,"network_handle":1,"is_default_route":true}]"#
            )
            .is_err()
        );
        assert!(
            parse_bindings(
                r#"[{"address":"192.0.2.1","prefix_length":24,"network_handle":0,"is_default_route":false}]"#
            )
            .is_err()
        );
    }
}
