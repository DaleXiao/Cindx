//! Public-web URL policy for network-capable tools.
//!
//! Every URL a tool fetches must resolve to public-routable addresses only:
//! loopback, RFC1918 private space, link-local (including the cloud metadata
//! range), CGNAT, broadcast, multicast, reserved, and their IPv6 equivalents
//! are rejected fail-closed, as are literal-IP spellings of the same ranges
//! and IPv4-bearing IPv6 forms (mapped, compatible, 6to4, Teredo). Callers
//! pin the audited IPs with `curl --resolve` so the connection cannot drift
//! to a different (re-resolved) address between validation and use.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use super::ToolError;

/// An audited fetch target: the URL passed validation and `pinned_ips` is the
/// complete set of public addresses the host resolved to at audit time.
pub(crate) struct PublicHttpTarget {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) pinned_ips: Vec<IpAddr>,
}

/// Parse and audit one URL. Returns the audited target or a fail-closed error
/// describing the rejection class (scheme, credentials, host, or address).
pub(crate) fn validate_public_http_url(url: &str) -> Result<PublicHttpTarget, ToolError> {
    let trimmed = url.trim();
    let (scheme, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(ToolError::new("url must start with http:// or https://"));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err(ToolError::new("url must carry a host"));
    }
    if authority.contains('@') {
        return Err(ToolError::new("url must not embed credentials"));
    }
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        // Bracketed IPv6 literal with an optional :port after the closing ].
        let Some(end) = inner.find(']') else {
            return Err(ToolError::new("unterminated IPv6 literal in url"));
        };
        let host = inner[..end].to_string();
        let port_part = &inner[end + 1..];
        let port = if let Some(port_text) = port_part.strip_prefix(':') {
            Some(parse_port(port_text)?)
        } else if port_part.is_empty() {
            None
        } else {
            return Err(ToolError::new("malformed IPv6 authority in url"));
        };
        (host, port)
    } else if let Some(colon) = authority.rfind(':') {
        let host = &authority[..colon];
        let port_text = &authority[colon + 1..];
        if host.contains(':') {
            // Bare IPv6 literal without brackets and no usable port split.
            (authority.to_string(), None)
        } else {
            (host.to_string(), Some(parse_port(port_text)?))
        }
    } else {
        (authority.to_string(), None)
    };
    if host.is_empty() {
        return Err(ToolError::new("url must carry a host"));
    }
    let port = port.unwrap_or(if scheme == "https" { 443 } else { 80 });

    // Literal IPs are audited directly; names resolve first, then every
    // resolved address must be public (one private answer fails the fetch).
    let pinned_ips: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|error| ToolError::new(format!("could not resolve host: {error}")))?
            .map(|address| address.ip())
            .collect(),
    };
    if pinned_ips.is_empty() {
        return Err(ToolError::new("host resolved to no addresses"));
    }
    for ip in &pinned_ips {
        if !ip_is_public(*ip) {
            return Err(ToolError::new(
                "url resolves to a loopback, private, link-local, metadata, or otherwise non-public address",
            ));
        }
    }
    Ok(PublicHttpTarget {
        host,
        port,
        pinned_ips,
    })
}

fn parse_port(port_text: &str) -> Result<u16, ToolError> {
    port_text
        .parse::<u16>()
        .map_err(|_| ToolError::new("url port must be a number between 1 and 65535"))
}

/// True when an address is public-routable. Everything with a special or
/// local meaning fails closed, including IPv4-bearing IPv6 forms.
pub(crate) fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_is_public(v4),
        IpAddr::V6(v6) => ipv6_is_public(v6),
    }
}

fn ipv4_is_public(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    if ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
    {
        return false;
    }
    // 0.0.0.0/8 "this" network, 100.64/10 CGNAT, 192.0.0/24 reserved,
    // 198.18/15 benchmarking, 240/4 reserved future.
    !(a == 0
        || (a == 100 && (b & 0xC0) == 64)
        || (a == 192 && b == 0)
        || (a == 198 && (b == 18 || b == 19))
        || a >= 240)
}

fn ipv6_is_public(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    // fe80::/10 link-local (covers the metadata-style local ranges).
    if (segments[0] & 0xFFC0) == 0xFE80 {
        return false;
    }
    // fc00::/7 unique-local.
    if (segments[0] & 0xFE00) == 0xFC00 {
        return false;
    }
    // ::ffff:0:0/96 IPv4-mapped and ::/96 legacy IPv4-compatible: the
    // embedded IPv4 address carries the real routability.
    if segments[0] == 0 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0 {
        if segments[4] == 0 && segments[5] == 0xFFFF {
            return ipv4_is_public(Ipv4Addr::new(
                (segments[6] >> 8) as u8,
                segments[6] as u8,
                (segments[7] >> 8) as u8,
                segments[7] as u8,
            ));
        }
        if segments[4] == 0 && segments[5] == 0 {
            return ipv4_is_public(Ipv4Addr::new(
                (segments[6] >> 8) as u8,
                segments[6] as u8,
                (segments[7] >> 8) as u8,
                segments[7] as u8,
            ));
        }
        // A bare short-form address (::x) is not public-routable here.
        return false;
    }
    // 2002::/16 6to4 embeds the IPv4 address in segments 1-2.
    if segments[0] == 0x2002 {
        return ipv4_is_public(Ipv4Addr::new(
            (segments[1] >> 8) as u8,
            segments[1] as u8,
            (segments[2] >> 8) as u8,
            segments[2] as u8,
        ));
    }
    // 2001::/32 Teredo embeds the client IPv4 address (inverted) in the
    // final two segments.
    if segments[0] == 0x2001 && segments[1] == 0 {
        return ipv4_is_public(Ipv4Addr::new(
            !(segments[6] >> 8) as u8,
            !(segments[6] & 0xFF) as u8,
            !(segments[7] >> 8) as u8,
            !(segments[7] & 0xFF) as u8,
        ));
    }
    true
}

/// Resolve a redirect `Location` against the current hop URL. Only absolute
/// http(s) targets and root-relative paths are supported; anything else fails
/// closed so an exotic Location can never smuggle a fetch past the audit.
pub(crate) fn resolve_redirect_location(
    current_url: &str,
    location: &str,
) -> Result<String, ToolError> {
    let location = location.trim();
    if location.is_empty() {
        return Err(ToolError::new("redirect carried no location"));
    }
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(location.to_string());
    }
    if let Some(root_relative) = location.strip_prefix('/') {
        if root_relative.starts_with('/') {
            return Err(ToolError::new(
                "protocol-relative redirect locations are not allowed",
            ));
        }
        let (scheme, rest) = if let Some(rest) = current_url.strip_prefix("https://") {
            ("https", rest)
        } else if let Some(rest) = current_url.strip_prefix("http://") {
            ("http", rest)
        } else {
            return Err(ToolError::new("redirect from a non-http url"));
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() {
            return Err(ToolError::new("redirect origin has no host"));
        }
        return Ok(format!("{scheme}://{authority}/{root_relative}"));
    }
    Err(ToolError::new(
        "only absolute http(s) or root-relative redirect locations are allowed",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_loopback_and_metadata_addresses_are_rejected() {
        for url in [
            "http://127.0.0.1/admin",
            "http://127.0.0.1:8080/",
            "http://[::1]/",
            "http://[::1]:8080/",
            "http://localhost/",
            "http://0.0.0.0/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/",
            "http://172.16.0.1/",
            "http://192.168.1.1/",
            "http://100.64.0.1/",
            "http://255.255.255.255/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:169.254.169.254]/",
            "http://[2002:7f00:1::]/",
        ] {
            assert!(
                validate_public_http_url(url).is_err(),
                "{url} must fail the public-address audit"
            );
        }
    }

    #[test]
    fn public_literal_addresses_pass_the_audit_with_pinning() {
        let target =
            validate_public_http_url("https://93.184.216.34/page").expect("public literal");
        assert_eq!(target.port, 443);
        assert_eq!(target.pinned_ips.len(), 1);
        assert!(ip_is_public(target.pinned_ips[0]));

        let http_default_port =
            validate_public_http_url("http://93.184.216.34").expect("public literal http");
        assert_eq!(http_default_port.port, 80);
    }

    #[test]
    fn embedded_credentials_and_bad_shapes_fail_closed() {
        for url in [
            "http://user:pass@example.com/",
            "ftp://example.com/",
            "https://",
            "http://[::1",
            "not a url",
        ] {
            assert!(
                validate_public_http_url(url).is_err(),
                "{url} must fail closed"
            );
        }
    }

    #[test]
    fn ipv4_bearing_ipv6_forms_inherit_their_inner_routability() {
        assert!(ip_is_public(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
        assert!(!ip_is_public(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(!ip_is_public(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
        assert!(!ip_is_public(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
        assert!(!ip_is_public(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!ip_is_public(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(ip_is_public(IpAddr::V6(Ipv6Addr::new(
            0x2606, 0x2800, 0x220, 0x1, 0x248, 0x1893, 0x25c8, 0x1946
        ))));
    }

    #[test]
    fn redirect_locations_resolve_only_as_absolute_or_root_relative() {
        let current = "https://example.com/a/b?q=1";
        assert_eq!(
            resolve_redirect_location(current, "https://other.example/x").unwrap(),
            "https://other.example/x"
        );
        assert_eq!(
            resolve_redirect_location(current, "/next").unwrap(),
            "https://example.com/next"
        );
        assert!(resolve_redirect_location(current, "//evil.example/x").is_err());
        assert!(resolve_redirect_location(current, "relative/path").is_err());
        assert!(resolve_redirect_location(current, "file:///etc/passwd").is_err());
        assert!(resolve_redirect_location(current, "").is_err());
    }
}
