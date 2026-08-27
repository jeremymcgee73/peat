//! Validated selected-IP bindings for platform-owned network routing.

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[cfg(any(target_os = "android", test))]
use std::net::SocketAddrV6;

#[cfg(target_os = "android")]
use std::sync::Arc;

use peat_mesh::network::IpBindSpec;
use serde::Deserialize;

use crate::PeatError;

const MAX_IP_BINDINGS: usize = 32;
#[cfg(target_os = "android")]
const MAX_INTERFACE_NAME_BYTES: usize = 64;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlatformIpBinding {
    address: String,
    prefix_length: u8,
    #[serde(default)]
    network_handle: Option<u64>,
    #[serde(default)]
    interface_name: Option<String>,
    #[serde(default)]
    scope_id: Option<u32>,
    is_default_route: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketOwner {
    AndroidNetwork(u64),
    LocalInterface,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
enum SocketBindingKey {
    V4(Ipv4Addr),
    V6(Ipv6Addr, u32),
}

impl From<SocketAddr> for SocketBindingKey {
    fn from(value: SocketAddr) -> Self {
        match value {
            SocketAddr::V4(value) => Self::V4(*value.ip()),
            SocketAddr::V6(value) => Self::V6(*value.ip(), value.scope_id()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ParsedIpBinding {
    spec: IpBindSpec,
    owner: SocketOwner,
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

fn parse_bindings(json: &str) -> Result<Vec<ParsedIpBinding>, PeatError> {
    parse_bindings_with(json, validate_local_interface)
}

fn parse_bindings_with<F>(
    json: &str,
    mut validate_interface: F,
) -> Result<Vec<ParsedIpBinding>, PeatError>
where
    F: FnMut(&str, IpAddr, Option<u32>) -> Result<SocketAddr, PeatError>,
{
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
            let max_prefix = if ip.is_ipv4() { 32 } else { 128 };
            if binding.prefix_length > max_prefix {
                return Err(invalid("IP binding prefix length is invalid"));
            }

            let (addr, owner) = match binding.network_handle {
                Some(handle) => {
                    if handle == 0 {
                        return Err(invalid("Android network handle must be non-zero"));
                    }
                    if binding.interface_name.is_some() || binding.scope_id.is_some() {
                        return Err(invalid(
                            "handle-backed IP bindings must not declare interface fields",
                        ));
                    }
                    (SocketAddr::new(ip, 0), SocketOwner::AndroidNetwork(handle))
                }
                None => {
                    if binding.is_default_route {
                        return Err(invalid(
                            "local-interface IP bindings cannot be default routes",
                        ));
                    }
                    let interface_name = binding.interface_name.as_deref().ok_or_else(|| {
                        invalid("local-interface IP binding requires an interface name")
                    })?;
                    validate_interface(interface_name, ip, binding.scope_id)
                        .map(|addr| (addr, SocketOwner::LocalInterface))?
                }
            };
            if !addresses.insert(SocketBindingKey::from(addr)) {
                return Err(invalid("duplicate IP binding socket address"));
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
            Ok(ParsedIpBinding {
                spec: IpBindSpec {
                    addr,
                    prefix_len: binding.prefix_length,
                    is_default_route: binding.is_default_route,
                },
                owner,
            })
        })
        .collect()
}

#[cfg(target_os = "android")]
fn validate_local_interface(
    interface_name: &str,
    ip: IpAddr,
    requested_scope_id: Option<u32>,
) -> Result<SocketAddr, PeatError> {
    if interface_name.is_empty()
        || interface_name != interface_name.trim()
        || interface_name.len() > MAX_INTERFACE_NAME_BYTES
        || interface_name
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(invalid("local-interface name is invalid"));
    }

    let interface = netdev::get_interfaces()
        .into_iter()
        .find(|candidate| candidate.name == interface_name)
        .ok_or_else(|| invalid("local-interface IP binding names an unavailable interface"))?;
    if !interface.is_up() {
        return Err(invalid(
            "local-interface IP binding names an inactive interface",
        ));
    }

    match ip {
        IpAddr::V4(address) => {
            if requested_scope_id.is_some() {
                return Err(invalid(
                    "IPv4 local-interface binding must not declare a scope ID",
                ));
            }
            if !interface
                .ipv4
                .iter()
                .any(|network| network.addr() == address)
            {
                return Err(invalid(
                    "local-interface IPv4 address is not assigned to the declared interface",
                ));
            }
            Ok(SocketAddr::new(IpAddr::V4(address), 0))
        }
        IpAddr::V6(address) => {
            let position = interface
                .ipv6
                .iter()
                .position(|network| network.addr() == address)
                .ok_or_else(|| {
                    invalid(
                        "local-interface IPv6 address is not assigned to the declared interface",
                    )
                })?;
            let actual_scope_id = interface
                .ipv6_scope_ids
                .get(position)
                .copied()
                .ok_or_else(|| invalid("local-interface IPv6 scope is unavailable"))?;
            let scope_id =
                if address.is_unicast_link_local() {
                    let requested = requested_scope_id.filter(|scope| *scope != 0).ok_or_else(|| {
                    invalid("link-local IPv6 local-interface binding requires a nonzero scope ID")
                })?;
                    if requested != actual_scope_id || actual_scope_id != interface.index {
                        return Err(invalid(
                            "link-local IPv6 scope does not match the declared interface",
                        ));
                    }
                    requested
                } else {
                    if requested_scope_id.is_some_and(|scope| scope != 0) {
                        return Err(invalid(
                        "non-link-local IPv6 local-interface binding must not declare a scope ID",
                    ));
                    }
                    0
                };
            Ok(SocketAddr::V6(SocketAddrV6::new(address, 0, 0, scope_id)))
        }
    }
}

#[cfg(not(target_os = "android"))]
fn validate_local_interface(
    _interface_name: &str,
    _ip: IpAddr,
    _requested_scope_id: Option<u32>,
) -> Result<SocketAddr, PeatError> {
    Err(invalid(
        "Android local-interface bindings are unavailable on this platform",
    ))
}

fn socket_owner(
    owners: &HashMap<SocketBindingKey, SocketOwner>,
    addr: SocketAddr,
) -> io::Result<SocketOwner> {
    owners
        .get(&SocketBindingKey::from(addr))
        .copied()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("no selected Android socket owner declared local address {addr}"),
            )
        })
}

#[cfg(target_os = "android")]
fn install_hook(
    owners: HashMap<SocketBindingKey, SocketOwner>,
) -> Result<netwatch::UdpSocketBindHookGuard, PeatError> {
    use std::os::fd::AsRawFd;

    #[link(name = "android")]
    unsafe extern "C" {
        fn android_setsocknetwork(network: u64, socket: std::os::raw::c_int)
            -> std::os::raw::c_int;
    }

    netwatch::install_udp_socket_bind_hook(Arc::new(
        move |addr: SocketAddr, socket: &socket2::Socket| {
            match socket_owner(&owners, addr)? {
                SocketOwner::LocalInterface => Ok(()),
                SocketOwner::AndroidNetwork(handle) => {
                    // SAFETY: `socket` is live for the duration of the callback;
                    // Android's API neither takes ownership of the fd nor retains
                    // the pointer. The network handle came from
                    // `Network.getNetworkHandle()` and was validated as non-zero at
                    // the FFI boundary.
                    let result = unsafe { android_setsocknetwork(handle, socket.as_raw_fd()) };
                    if result == 0 {
                        Ok(())
                    } else {
                        Err(io::Error::from_raw_os_error(-result))
                    }
                }
            }
        },
    ))
    .map_err(|error| PeatError::ConnectionError {
        msg: format!("failed to install selected-network socket hook: {error}"),
    })
}

#[cfg(not(target_os = "android"))]
fn install_hook(
    _owners: HashMap<SocketBindingKey, SocketOwner>,
) -> Result<netwatch::UdpSocketBindHookGuard, PeatError> {
    Err(invalid(
        "selected Android network bindings are unavailable on this platform",
    ))
}

pub(crate) fn prepare(json: &str) -> Result<PreparedIpBindings, PeatError> {
    let parsed = parse_bindings(json)?;
    let owners = parsed
        .iter()
        .map(|binding| (SocketBindingKey::from(binding.spec.addr), binding.owner))
        .collect::<HashMap<_, _>>();
    let specs = parsed.into_iter().map(|binding| binding.spec).collect();
    let hook_guard = install_hook(owners)?;
    Ok(PreparedIpBindings { specs, hook_guard })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validated_test_interface(
        interface_name: &str,
        ip: IpAddr,
        scope_id: Option<u32>,
    ) -> Result<SocketAddr, PeatError> {
        if interface_name != "p2p-test0" {
            return Err(invalid("test interface is unavailable"));
        }
        match ip {
            IpAddr::V4(address) if scope_id.is_none() => {
                Ok(SocketAddr::new(IpAddr::V4(address), 0))
            }
            IpAddr::V6(address) if scope_id == Some(7) => {
                Ok(SocketAddr::V6(SocketAddrV6::new(address, 0, 0, 7)))
            }
            _ => Err(invalid("test interface scope is invalid")),
        }
    }

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
        assert!(!parsed[0].spec.is_default_route);
        assert!(parsed[1].spec.is_default_route);
        assert_eq!(parsed[0].owner, SocketOwner::AndroidNetwork(1001));
        assert_eq!(parsed[1].owner, SocketOwner::AndroidNetwork(2002));
    }

    #[test]
    fn parser_accepts_non_default_validated_local_interfaces_and_ipv6_scope() {
        let parsed = parse_bindings_with(
            r#"[
              {"address":"192.168.49.1","prefix_length":24,"interface_name":"p2p-test0","is_default_route":false},
              {"address":"fe80::1234","prefix_length":64,"interface_name":"p2p-test0","scope_id":7,"is_default_route":false}
            ]"#,
            validated_test_interface,
        )
        .unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].owner, SocketOwner::LocalInterface);
        assert_eq!(parsed[0].spec.addr, "192.168.49.1:0".parse().unwrap());
        assert_eq!(parsed[1].owner, SocketOwner::LocalInterface);
        assert_eq!(
            parsed[1].spec.addr,
            SocketAddr::V6(SocketAddrV6::new("fe80::1234".parse().unwrap(), 0, 0, 7))
        );
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

    #[test]
    fn parser_rejects_ambiguous_or_default_local_interface_shapes() {
        let local_default = r#"[
          {"address":"192.168.49.1","prefix_length":24,"interface_name":"p2p-test0","is_default_route":true}
        ]"#;
        assert!(
            parse_bindings_with(local_default, validated_test_interface).is_err(),
            "a handle-less binding must never become a default route"
        );

        let missing_interface = r#"[
          {"address":"192.168.49.1","prefix_length":24,"is_default_route":false}
        ]"#;
        assert!(parse_bindings_with(missing_interface, validated_test_interface).is_err());

        let mixed_owner = r#"[
          {"address":"192.168.49.1","prefix_length":24,"network_handle":9,"interface_name":"p2p-test0","is_default_route":false}
        ]"#;
        assert!(parse_bindings_with(mixed_owner, validated_test_interface).is_err());

        let ipv4_scope = r#"[
          {"address":"192.168.49.1","prefix_length":24,"interface_name":"p2p-test0","scope_id":7,"is_default_route":false}
        ]"#;
        assert!(parse_bindings_with(ipv4_scope, validated_test_interface).is_err());
    }

    #[test]
    fn parser_rejects_duplicate_scoped_socket_and_preserves_distinct_scopes() {
        let duplicate = r#"[
          {"address":"fe80::1234","prefix_length":64,"interface_name":"p2p-test0","scope_id":7,"is_default_route":false},
          {"address":"fe80::1234","prefix_length":64,"interface_name":"p2p-test0","scope_id":7,"is_default_route":false}
        ]"#;
        assert!(parse_bindings_with(duplicate, validated_test_interface).is_err());

        let distinct = r#"[
          {"address":"fe80::1234","prefix_length":64,"interface_name":"first","scope_id":7,"is_default_route":false},
          {"address":"fe80::1234","prefix_length":64,"interface_name":"second","scope_id":8,"is_default_route":false}
        ]"#;
        let parsed = parse_bindings_with(distinct, |name, ip, scope| {
            let expected = if name == "first" { 7 } else { 8 };
            if scope != Some(expected) {
                return Err(invalid("test scope mismatch"));
            }
            let IpAddr::V6(address) = ip else {
                return Err(invalid("test address family mismatch"));
            };
            Ok(SocketAddr::V6(SocketAddrV6::new(address, 0, 0, expected)))
        })
        .unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn socket_owner_matches_exact_address_and_scope_but_ignores_ephemeral_port() {
        let mut owners = HashMap::new();
        owners.insert(
            SocketBindingKey::V4("192.168.49.1".parse().unwrap()),
            SocketOwner::AndroidNetwork(42),
        );
        owners.insert(
            SocketBindingKey::V6("fe80::1234".parse().unwrap(), 7),
            SocketOwner::LocalInterface,
        );

        assert_eq!(
            socket_owner(&owners, "192.168.49.1:38000".parse().unwrap()).unwrap(),
            SocketOwner::AndroidNetwork(42)
        );
        assert_eq!(
            socket_owner(
                &owners,
                SocketAddr::V6(SocketAddrV6::new(
                    "fe80::1234".parse().unwrap(),
                    38000,
                    0,
                    7,
                )),
            )
            .unwrap(),
            SocketOwner::LocalInterface
        );
        assert!(
            socket_owner(
                &owners,
                SocketAddr::V6(SocketAddrV6::new(
                    "fe80::1234".parse().unwrap(),
                    38000,
                    0,
                    8,
                )),
            )
            .is_err(),
            "a different IPv6 scope must not inherit local-interface ownership"
        );
        assert!(socket_owner(&owners, "192.168.49.2:38000".parse().unwrap()).is_err());
    }
}
