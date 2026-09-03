//! The single owner of outbound HTTP policy (audit P1-04).
//!
//! Every egress path in the product — `web.fetch`, the `web.search` public
//! fallback and configured API, the MCP streamable-HTTP/SSE transport, the
//! model-provider API calls, and skill-package downloads — takes its proxy
//! posture, redirect bound, timeout, response byte caps, User-Agent, scheme
//! allowlist, and public-address audit decision from [`http_policy`] for its
//! [`HttpEgressProfile`], and every `curl` path derives its shared argument
//! fragment from [`curl_policy_args`]. Before this module those knobs were
//! hardcoded per call site and had drifted: redirect bounds of 5, 10, 0, and
//! unlimited for the same concept, three User-Agents, scheme allowlists missing
//! on three paths, and one path out of nine disabling proxies.
//!
//! The values below are the deliberate, reviewed posture per path. Changing one
//! changes that path's live network behavior, so change it here, in one place,
//! with a test.
//!
//! The second half of this module is the public-web address audit. Every URL a
//! public fetch targets must resolve to public-routable addresses only:
//! loopback, RFC1918 private space, link-local (including the cloud metadata
//! range), CGNAT, broadcast, multicast, reserved, and their IPv6 equivalents
//! are rejected fail-closed, as are literal-IP spellings of the same ranges
//! and IPv4-bearing IPv6 forms (mapped, compatible, 6to4, Teredo). Callers
//! pin the audited IPs with `curl --resolve` so the connection cannot drift
//! to a different (re-resolved) address between validation and use. The audit
//! lives here, not in the tools crate, so any crate that egresses can reach it:
//! `tools` depends on `model-provider`, so a policy owned by either one could
//! never be shared by both.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

/// The User-Agent every Cindx-initiated HTTP request presents.
pub const HTTP_USER_AGENT: &str = "Cindx/1";
/// One web response body (`web.fetch`, `web.search`).
pub const HTTP_WEB_RESPONSE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// One MCP streamable-HTTP/SSE response.
pub const HTTP_MCP_RESPONSE_MAX_BYTES: usize = 16 * 1024 * 1024;
/// One model-provider response.
pub const HTTP_MODEL_RESPONSE_MAX_BYTES: usize = 64 * 1024 * 1024;
/// One downloaded skill package.
pub const HTTP_SKILL_PACKAGE_MAX_BYTES: usize = 50 * 1024 * 1024;
/// Subprocess stderr is diagnostic only and stays small on every path.
pub const HTTP_STDERR_MAX_BYTES: usize = 256 * 1024;
/// Default web request timeout, and the ceiling a caller may clamp to.
pub const HTTP_WEB_TIMEOUT_SECONDS: usize = 25;
pub const HTTP_WEB_MAX_TIMEOUT_SECONDS: usize = 60;
/// Audited public-fetch redirect hops (re-audited one hop at a time).
pub const HTTP_PUBLIC_FETCH_MAX_REDIRECTS: usize = 5;
/// Provider API redirects (reqwest-enforced).
pub const HTTP_PROVIDER_MAX_REDIRECTS: usize = 10;
/// MCP transport default timeout.
pub const HTTP_MCP_DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Skill download timeout.
pub const HTTP_SKILL_TIMEOUT_SECONDS: usize = 30;

/// Which endpoint a request targets. The profile decides every knob, including
/// whether the public-address audit applies and whether an ambient proxy is
/// allowed to carry the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpEgressProfile {
    /// A public web URL chosen by the model or a tool argument (`web.fetch`).
    /// Proxies are disabled because an ambient proxy would connect on our behalf
    /// and bypass the per-hop `--resolve` public-IP pin that is the SSRF
    /// defense; redirects are walked one audited hop at a time by the caller, so
    /// curl never follows one itself.
    PublicFetch,
    /// The built-in public search fallback. The URL is a compile-time constant,
    /// not model-chosen, so no address audit applies, but redirects are bounded
    /// and the scheme allowlist is HTTPS-only. An ambient proxy is honored:
    /// unlike `web.fetch` there is no IP pin to bypass, and refusing the proxy
    /// would break search for users whose only egress is proxied.
    PublicSearchFallback,
    /// A user-configured search endpoint without a credential. Loopback and
    /// private endpoints are supported by that explicit configuration, so no
    /// address audit applies; redirects are bounded.
    ConfiguredSearchApi,
    /// A user-configured search endpoint carrying an API key. Never follows a
    /// redirect (`-L --max-redirs 0`) and is HTTPS-only, so the credential can
    /// never be replayed to another origin or sent in the clear.
    CredentialedSearchApi,
    /// A user-configured MCP streamable-HTTP/SSE endpoint. Redirects are not
    /// followed at all: a 3xx is reported to the caller rather than silently
    /// re-dialed elsewhere.
    McpHttp,
    /// A user-configured model provider base URL (reqwest, not curl). Loopback
    /// providers such as a local Ollama are explicitly supported, so no address
    /// audit applies.
    ProviderApi,
    /// A skill package download from a user-supplied HTTPS URL.
    SkillInstall,
}

/// Whether the request may be carried by an ambient environment proxy
/// (`http_proxy` / `https_proxy` / `ALL_PROXY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    /// Never inherit an ambient proxy (`curl --noproxy '*'`).
    Disabled,
    /// Honor the environment, which is what a user behind a corporate proxy
    /// needs to reach an endpoint they configured themselves.
    Environment,
}

/// Every knob one egress path runs under. Constructed only by [`http_policy`];
/// a call site that needs a different timeout copies the profile's policy and
/// overrides that one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpPolicy {
    pub profile: HttpEgressProfile,
    pub proxy: ProxyMode,
    /// Whether curl is told to follow redirects at all.
    pub follows_redirects: bool,
    /// The redirect bound. `0` means a redirect is an error, not a hop. For
    /// [`HttpEgressProfile::PublicFetch`] this is the caller's audited-hop bound.
    pub max_redirects: usize,
    pub timeout_ms: u64,
    pub response_max_bytes: usize,
    pub stderr_max_bytes: usize,
    pub user_agent: &'static str,
    /// The curl scheme allowlist, applied to the request and to any redirect.
    pub allowed_schemes: &'static str,
    /// Whether the target must pass [`validate_public_http_url`] before any
    /// request is sent.
    pub audits_public_addresses: bool,
}

/// The one place every egress path's policy is written down.
pub fn http_policy(profile: HttpEgressProfile) -> HttpPolicy {
    let web = HttpPolicy {
        profile,
        proxy: ProxyMode::Environment,
        follows_redirects: true,
        max_redirects: HTTP_PUBLIC_FETCH_MAX_REDIRECTS,
        timeout_ms: (HTTP_WEB_TIMEOUT_SECONDS * 1000) as u64,
        response_max_bytes: HTTP_WEB_RESPONSE_MAX_BYTES,
        stderr_max_bytes: HTTP_STDERR_MAX_BYTES,
        user_agent: HTTP_USER_AGENT,
        allowed_schemes: "=http,https",
        audits_public_addresses: false,
    };
    match profile {
        HttpEgressProfile::PublicFetch => HttpPolicy {
            proxy: ProxyMode::Disabled,
            follows_redirects: false,
            allowed_schemes: "=http,https",
            audits_public_addresses: true,
            ..web
        },
        HttpEgressProfile::PublicSearchFallback => HttpPolicy {
            allowed_schemes: "=https",
            ..web
        },
        HttpEgressProfile::ConfiguredSearchApi => web,
        HttpEgressProfile::CredentialedSearchApi => HttpPolicy {
            // `-L --max-redirs 0`: curl is told to handle redirects and allowed
            // none, so a redirect fails the request instead of moving the
            // credential to another origin.
            max_redirects: 0,
            allowed_schemes: "=https",
            ..web
        },
        HttpEgressProfile::McpHttp => HttpPolicy {
            follows_redirects: false,
            max_redirects: 0,
            timeout_ms: HTTP_MCP_DEFAULT_TIMEOUT_MS,
            response_max_bytes: HTTP_MCP_RESPONSE_MAX_BYTES,
            ..web
        },
        HttpEgressProfile::ProviderApi => HttpPolicy {
            follows_redirects: true,
            max_redirects: HTTP_PROVIDER_MAX_REDIRECTS,
            response_max_bytes: HTTP_MODEL_RESPONSE_MAX_BYTES,
            ..web
        },
        HttpEgressProfile::SkillInstall => HttpPolicy {
            timeout_ms: (HTTP_SKILL_TIMEOUT_SECONDS * 1000) as u64,
            response_max_bytes: HTTP_SKILL_PACKAGE_MAX_BYTES,
            allowed_schemes: "=https",
            ..web
        },
    }
}

/// The shared `curl` argument fragment for a policy: quiet and error-reporting
/// flags, the bounded timeout, the product User-Agent, the scheme allowlist, the
/// redirect posture, and the proxy posture. Callers append only their own
/// request-specific arguments (`--include`, `--resolve`, `--header`, `--data`,
/// `--fail`) and the URL last.
pub fn curl_policy_args(policy: &HttpPolicy) -> Vec<String> {
    let timeout = if policy.timeout_ms.is_multiple_of(1000) {
        (policy.timeout_ms / 1000).to_string()
    } else {
        format!("{:.3}", policy.timeout_ms as f64 / 1000.0)
    };
    let mut args = vec![
        "-q".to_string(),
        "--silent".to_string(),
        "--show-error".to_string(),
        "--max-time".to_string(),
        timeout,
        "--user-agent".to_string(),
        policy.user_agent.to_string(),
        "--proto".to_string(),
        policy.allowed_schemes.to_string(),
    ];
    if policy.follows_redirects {
        args.push("-L".to_string());
        args.push("--max-redirs".to_string());
        args.push(policy.max_redirects.to_string());
        args.push("--proto-redir".to_string());
        args.push(policy.allowed_schemes.to_string());
    }
    if policy.proxy == ProxyMode::Disabled {
        args.push("--noproxy".to_string());
        args.push("*".to_string());
    }
    args
}

/// A policy violation: the request was refused before any bytes went out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpPolicyError {
    pub message: String,
}

impl HttpPolicyError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HttpPolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HttpPolicyError {}

/// An audited fetch target: the URL passed validation and `pinned_ips` is the
/// complete set of public addresses the host resolved to at audit time.
pub struct PublicHttpTarget {
    pub host: String,
    pub port: u16,
    pub pinned_ips: Vec<IpAddr>,
}

/// Parse and audit one URL. Returns the audited target or a fail-closed error
/// describing the rejection class (scheme, credentials, host, or address).
pub fn validate_public_http_url(url: &str) -> Result<PublicHttpTarget, HttpPolicyError> {
    let trimmed = url.trim();
    let (scheme, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(HttpPolicyError::new(
            "url must start with http:// or https://",
        ));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err(HttpPolicyError::new("url must carry a host"));
    }
    if authority.contains('@') {
        return Err(HttpPolicyError::new("url must not embed credentials"));
    }
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        // Bracketed IPv6 literal with an optional :port after the closing ].
        let Some(end) = inner.find(']') else {
            return Err(HttpPolicyError::new("unterminated IPv6 literal in url"));
        };
        let host = inner[..end].to_string();
        let port_part = &inner[end + 1..];
        let port = if let Some(port_text) = port_part.strip_prefix(':') {
            Some(parse_port(port_text)?)
        } else if port_part.is_empty() {
            None
        } else {
            return Err(HttpPolicyError::new("malformed IPv6 authority in url"));
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
        return Err(HttpPolicyError::new("url must carry a host"));
    }
    let port = port.unwrap_or(if scheme == "https" { 443 } else { 80 });

    // Literal IPs are audited directly; names resolve first, then every
    // resolved address must be public (one private answer fails the fetch).
    let pinned_ips: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|error| HttpPolicyError::new(format!("could not resolve host: {error}")))?
            .map(|address| address.ip())
            .collect(),
    };
    if pinned_ips.is_empty() {
        return Err(HttpPolicyError::new("host resolved to no addresses"));
    }
    for ip in &pinned_ips {
        if !ip_is_public(*ip) {
            return Err(HttpPolicyError::new(
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

fn parse_port(port_text: &str) -> Result<u16, HttpPolicyError> {
    port_text
        .parse::<u16>()
        .map_err(|_| HttpPolicyError::new("url port must be a number between 1 and 65535"))
}

/// True when an address is public-routable. Everything with a special or
/// local meaning fails closed, including IPv4-bearing IPv6 forms.
pub fn ip_is_public(ip: IpAddr) -> bool {
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
    // Remaining IANA special-purpose IPv6 ranges are never public-routable to an
    // arbitrary host, so they fail closed too (audit P1-04 completeness).
    // 2001:db8::/32 documentation.
    if segments[0] == 0x2001 && segments[1] == 0x0DB8 {
        return false;
    }
    // 100::/64 discard-only address block (blackhole).
    if segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0 {
        return false;
    }
    // fec0::/10 deprecated site-local.
    if (segments[0] & 0xFFC0) == 0xFEC0 {
        return false;
    }
    true
}

/// Resolve a redirect `Location` against the current hop URL. Only absolute
/// http(s) targets and root-relative paths are supported; anything else fails
/// closed so an exotic Location can never smuggle a fetch past the audit.
pub fn resolve_redirect_location(
    current_url: &str,
    location: &str,
) -> Result<String, HttpPolicyError> {
    let location = location.trim();
    if location.is_empty() {
        return Err(HttpPolicyError::new("redirect carried no location"));
    }
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(location.to_string());
    }
    if let Some(root_relative) = location.strip_prefix('/') {
        if root_relative.starts_with('/') {
            return Err(HttpPolicyError::new(
                "protocol-relative redirect locations are not allowed",
            ));
        }
        let (scheme, rest) = if let Some(rest) = current_url.strip_prefix("https://") {
            ("https", rest)
        } else if let Some(rest) = current_url.strip_prefix("http://") {
            ("http", rest)
        } else {
            return Err(HttpPolicyError::new("redirect from a non-http url"));
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() {
            return Err(HttpPolicyError::new("redirect origin has no host"));
        }
        return Ok(format!("{scheme}://{authority}/{root_relative}"));
    }
    Err(HttpPolicyError::new(
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
    fn reserved_ipv6_ranges_are_never_public() {
        // 2001:db8::/32 documentation.
        assert!(!ip_is_public(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x0db8, 0, 0, 0, 0, 0, 1
        ))));
        // 100::/64 discard-only blackhole.
        assert!(!ip_is_public(IpAddr::V6(Ipv6Addr::new(
            0x0100, 0, 0, 0, 0, 0, 0, 1
        ))));
        // fec0::/10 deprecated site-local.
        assert!(!ip_is_public(IpAddr::V6(Ipv6Addr::new(
            0xfec0, 0, 0, 0, 0, 0, 0, 1
        ))));
        // A genuine global unicast address still passes.
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

#[cfg(test)]
mod policy_tests {
    use super::*;

    const CURL_PROFILES: [HttpEgressProfile; 6] = [
        HttpEgressProfile::PublicFetch,
        HttpEgressProfile::PublicSearchFallback,
        HttpEgressProfile::ConfiguredSearchApi,
        HttpEgressProfile::CredentialedSearchApi,
        HttpEgressProfile::McpHttp,
        HttpEgressProfile::SkillInstall,
    ];

    #[test]
    fn every_curl_path_shares_one_agent_and_a_bounded_redirect_posture() {
        for profile in CURL_PROFILES {
            let policy = http_policy(profile);
            let args = curl_policy_args(&policy);
            assert_eq!(args[0], "-q", "{profile:?} must stay quiet-first");
            assert!(args.contains(&"--silent".to_string()), "{profile:?}");
            assert!(args.contains(&"--show-error".to_string()), "{profile:?}");
            assert!(args.contains(&"--max-time".to_string()), "{profile:?}");
            assert!(
                args.contains(&HTTP_USER_AGENT.to_string()),
                "{profile:?} must present the product user agent"
            );
            assert!(
                args.iter().any(|arg| arg.starts_with('=')),
                "{profile:?} must carry a scheme allowlist"
            );
            // No path follows a redirect without stating a bound any more.
            if args.contains(&"-L".to_string()) {
                let bound = args
                    .iter()
                    .position(|arg| arg == "--max-redirs")
                    .unwrap_or_else(|| panic!("{profile:?} states its redirect bound"));
                let value: usize = args[bound + 1].parse().expect("numeric bound");
                assert!(
                    value <= HTTP_PUBLIC_FETCH_MAX_REDIRECTS,
                    "{profile:?} follows at most {value} redirects"
                );
                assert!(args.contains(&"--proto-redir".to_string()), "{profile:?}");
            }
        }
    }

    #[test]
    fn only_the_audited_public_fetch_disables_ambient_proxies() {
        let fetch = http_policy(HttpEgressProfile::PublicFetch);
        assert_eq!(fetch.proxy, ProxyMode::Disabled);
        assert!(fetch.audits_public_addresses);
        assert!(
            !fetch.follows_redirects,
            "curl must not follow: every hop is re-audited by the caller"
        );
        assert_eq!(fetch.max_redirects, HTTP_PUBLIC_FETCH_MAX_REDIRECTS);
        let args = curl_policy_args(&fetch);
        let noproxy = args
            .iter()
            .position(|arg| arg == "--noproxy")
            .expect("the audited fetch never inherits a proxy");
        assert_eq!(args[noproxy + 1], "*");
        assert!(!args.contains(&"-L".to_string()));

        for profile in CURL_PROFILES
            .iter()
            .copied()
            .chain([HttpEgressProfile::ProviderApi])
            .filter(|candidate| *candidate != HttpEgressProfile::PublicFetch)
        {
            let policy = http_policy(profile);
            assert_eq!(policy.proxy, ProxyMode::Environment, "{profile:?}");
            assert!(
                !curl_policy_args(&policy).contains(&"--noproxy".to_string()),
                "{profile:?} honors the user's proxy"
            );
        }
    }

    #[test]
    fn a_credentialed_endpoint_never_follows_a_redirect_and_stays_on_https() {
        let policy = http_policy(HttpEgressProfile::CredentialedSearchApi);
        let args = curl_policy_args(&policy);

        assert_eq!(policy.max_redirects, 0);
        assert!(args.contains(&"-L".to_string()));
        let bound = args
            .iter()
            .position(|arg| arg == "--max-redirs")
            .expect("bound present");
        assert_eq!(args[bound + 1], "0");
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == "=https").count(),
            2,
            "both the request and any redirect stay on HTTPS"
        );
    }

    #[test]
    fn the_mcp_transport_does_not_follow_redirects_and_keeps_millisecond_timeouts() {
        let policy = http_policy(HttpEgressProfile::McpHttp);
        assert!(!policy.follows_redirects);
        assert_eq!(policy.max_redirects, 0);
        assert_eq!(policy.timeout_ms, HTTP_MCP_DEFAULT_TIMEOUT_MS);
        assert_eq!(policy.response_max_bytes, HTTP_MCP_RESPONSE_MAX_BYTES);
        assert_eq!(policy.stderr_max_bytes, HTTP_STDERR_MAX_BYTES);
        let args = curl_policy_args(&policy);
        assert!(!args.contains(&"-L".to_string()));
        assert!(args.contains(&"30".to_string()));

        // A caller-supplied millisecond timeout keeps its precision.
        let precise = HttpPolicy {
            timeout_ms: 1_500,
            ..policy
        };
        assert!(curl_policy_args(&precise).contains(&"1.500".to_string()));
    }

    #[test]
    fn provider_and_download_policies_keep_their_own_caps() {
        let provider = http_policy(HttpEgressProfile::ProviderApi);
        assert_eq!(provider.max_redirects, HTTP_PROVIDER_MAX_REDIRECTS);
        assert_eq!(provider.response_max_bytes, HTTP_MODEL_RESPONSE_MAX_BYTES);
        assert!(
            !provider.audits_public_addresses,
            "a loopback provider such as a local Ollama is explicitly supported"
        );

        let skill = http_policy(HttpEgressProfile::SkillInstall);
        assert_eq!(skill.response_max_bytes, HTTP_SKILL_PACKAGE_MAX_BYTES);
        assert_eq!(skill.timeout_ms, (HTTP_SKILL_TIMEOUT_SECONDS * 1000) as u64);
        assert_eq!(skill.allowed_schemes, "=https");
        assert!(skill.follows_redirects);
        assert_eq!(skill.max_redirects, HTTP_PUBLIC_FETCH_MAX_REDIRECTS);

        let search = http_policy(HttpEgressProfile::PublicSearchFallback);
        assert_eq!(search.response_max_bytes, HTTP_WEB_RESPONSE_MAX_BYTES);
        assert_eq!(search.timeout_ms, (HTTP_WEB_TIMEOUT_SECONDS * 1000) as u64);
        assert!(!search.audits_public_addresses);
    }

    #[test]
    fn a_call_site_may_override_only_the_timeout() {
        let policy = http_policy(HttpEgressProfile::PublicFetch);
        let clamped = HttpPolicy {
            timeout_ms: (HTTP_WEB_MAX_TIMEOUT_SECONDS * 1000) as u64,
            ..policy
        };

        assert_eq!(clamped.proxy, policy.proxy);
        assert_eq!(clamped.max_redirects, policy.max_redirects);
        assert_eq!(
            clamped.audits_public_addresses,
            policy.audits_public_addresses
        );
        assert!(curl_policy_args(&clamped).contains(&"60".to_string()));
    }
}
