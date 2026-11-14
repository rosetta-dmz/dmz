use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tracing::{error, info, warn};

/// Bounded proxy request header limit (16 KiB) to prevent memory exhaustion (CWE-400).
const MAX_PROXY_HEADER_BYTES: usize = 16 * 1024;
/// Maximum time allowed to read request headers from a connected agent.
const PROXY_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum time allowed for upstream DNS lookup + TCP connect.
const PROXY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum session lifetime for an open tunneled or forwarded proxy connection.
const PROXY_SESSION_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyError {
    AccessDenied { host: String },
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessDenied { host } => write!(
                f,
                "Security Exception: Access to host '{}' is prohibited by the DMZ Network Proxy policy.",
                host
            ),
        }
    }
}

impl std::error::Error for ProxyError {}

#[derive(Debug, Clone)]
pub struct DomainWhitelist {
    allowed_domains: HashSet<String>,
    allowed_wildcards: Vec<String>,
    allow_all: bool,
}

impl DomainWhitelist {
    /// Create network rule engine from list of domain strings
    pub fn new(rules: Vec<String>) -> Self {
        let mut allowed_domains = HashSet::new();
        let mut allowed_wildcards = Vec::new();
        let mut allow_all = false;

        for rule in rules {
            let trimmed = rule.trim().to_lowercase();
            // Remove trailing dot if present
            let normalized = trimmed.strip_suffix('.').unwrap_or(&trimmed);

            if normalized == "*" {
                allow_all = true;
            } else if let Some(wildcard) = normalized.strip_prefix("*.") {
                allowed_wildcards.push(wildcard.to_string());
            } else if let Some(wildcard) = normalized.strip_prefix('.') {
                allowed_wildcards.push(wildcard.to_string());
            } else {
                allowed_domains.insert(normalized.to_string());
            }
        }

        Self {
            allowed_domains,
            allowed_wildcards,
            allow_all,
        }
    }

    /// Evaluate whether an outbound host connection is permitted for the AI agent
    pub fn is_allowed(&self, host: &str) -> bool {
        if self.allow_all {
            return true;
        }

        let clean_host = Self::normalize_host(host);

        // Direct exact match (e.g. "api.openai.com")
        if self.allowed_domains.contains(&clean_host) {
            return true;
        }

        // Wildcard suffix match (e.g. "*.github.com" matches "raw.github.com")
        for wildcard in &self.allowed_wildcards {
            if clean_host.ends_with(wildcard) && clean_host.len() > wildcard.len() {
                let prefix_len = clean_host.len() - wildcard.len();
                // Safe slice indexing using .get() to prevent out-of-bounds panics
                if clean_host.as_bytes().get(prefix_len - 1) == Some(&b'.') {
                    return true;
                }
            }
        }

        false
    }

    /// Normalizes host strings, stripping schemes, user credentials, ports, and paths
    fn normalize_host(raw_host: &str) -> String {
        let trimmed = raw_host.trim();

        // Strip URI scheme if present
        let stripped_scheme = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .unwrap_or(trimmed);

        // Strip path and query components if present
        let authority = stripped_scheme.split('/').next().unwrap_or(stripped_scheme);

        // Strip userinfo (user:pass@) if present
        let host_part = authority.split('@').next_back().unwrap_or(authority);

        let clean = if let Some(stripped) = host_part.strip_prefix('[') {
            // IPv6 with brackets: e.g. "[::1]:8080" or "[::1]"
            stripped
                .split(']')
                .next()
                .unwrap_or(stripped)
                .trim()
                .to_lowercase()
        } else if host_part.matches(':').count() == 1 {
            // Standard IPv4 or hostname with port: e.g. "api.openai.com:443" or "127.0.0.1:80"
            host_part
                .split(':')
                .next()
                .unwrap_or(host_part)
                .trim()
                .to_lowercase()
        } else {
            // Bare hostname, bare IPv4, or bare IPv6 without brackets
            host_part.trim().to_lowercase()
        };

        // Strip trailing root dot (FQDN root e.g. "crates.io.")
        clean.strip_suffix('.').unwrap_or(&clean).to_string()
    }

    /// Intercept network calls and returns a typed error when host is blocked
    pub fn authorize_request(&self, target_host: &str) -> Result<(), ProxyError> {
        if self.is_allowed(target_host) {
            info!(host = %target_host, "Outbound network request ALLOWED by agent proxy");
            Ok(())
        } else {
            error!(host = %target_host, "Outbound network request BLOCKED by agent proxy");
            Err(ProxyError::AccessDenied {
                host: target_host.to_string(),
            })
        }
    }
}

/// Active loopback HTTP/CONNECT proxy enforcing [`DomainWhitelist`] on an OS-assigned ephemeral port.
#[derive(Debug)]
pub struct LoopbackAllowlistProxy {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl LoopbackAllowlistProxy {
    /// Binds a loopback TCP listener on `127.0.0.1:0` and spawns the async proxy accept loop.
    pub async fn start(policy: DomainWhitelist) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        info!(addr = %local_addr, "Shutting down loopback agent allowlist proxy");
                        break;
                    }
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((stream, _peer)) => {
                                let policy_clone = policy.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = handle_proxy_client(stream, policy_clone).await {
                                        warn!(error = %e, "Agent proxy connection terminated");
                                    }
                                });
                            }
                            Err(e) => {
                                warn!(error = %e, "Agent proxy accept error");
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            local_addr,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn proxy_url(&self) -> String {
        format!("http://{}", self.local_addr)
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for LoopbackAllowlistProxy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Writes an HTTP error response, half-closes the write side (`FIN`), and performs a bounded
/// lingering drain so BSD/macOS sockets do not emit `RST` before the peer reads the response.
async fn send_error_and_close(client: &mut TcpStream, response: &[u8]) {
    const MAX_LINGER_DRAIN_BYTES: usize = 64 * 1024;
    let _ = client.write_all(response).await;
    let _ = client.flush().await;
    let _ = client.shutdown().await;

    let mut drain_buf = [0u8; 2048];
    let mut drained = 0usize;
    let _ = tokio::time::timeout(std::time::Duration::from_millis(50), async {
        while drained < MAX_LINGER_DRAIN_BYTES {
            match client.read(&mut drain_buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => drained = drained.saturating_add(n),
            }
        }
    })
    .await;
}

fn is_disallowed_resolved_ip(
    ip: IpAddr,
    allow_explicit_literal: bool,
    allow_loopback: bool,
) -> bool {
    if ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    if allow_explicit_literal {
        return false;
    }
    if ip.is_loopback() {
        return !allow_loopback;
    }
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_link_local() || v4.is_broadcast() || v4.is_documentation()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_disallowed_resolved_ip(IpAddr::V4(v4), false, allow_loopback);
            }
            let seg0 = v6.segments().first().copied().unwrap_or(0);
            let is_ula = (seg0 & 0xfe00) == 0xfc00;
            let is_link_local = (seg0 & 0xffc0) == 0xfe80;
            is_ula || is_link_local
        }
    }
}

fn format_host_port(authority: &str, default_port: u16) -> String {
    let trimmed = authority.trim();
    if trimmed.starts_with('[') {
        if trimmed.contains("]:") {
            trimmed.to_string()
        } else {
            format!("{}:{}", trimmed, default_port)
        }
    } else if trimmed.matches(':').count() == 1 {
        trimmed.to_string()
    } else if trimmed.matches(':').count() > 1 {
        format!("[{}]:{}", trimmed, default_port)
    } else {
        format!("{}:{}", trimmed, default_port)
    }
}

async fn authorize_and_connect(
    client: &mut TcpStream,
    authority: &str,
    default_port: u16,
    policy: &DomainWhitelist,
) -> Option<TcpStream> {
    if policy.authorize_request(authority).is_err() {
        send_error_and_close(
            client,
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nForbidden",
        )
        .await;
        return None;
    }

    let clean_host = DomainWhitelist::normalize_host(authority);
    let allow_explicit_literal =
        clean_host.parse::<IpAddr>().is_ok() && policy.is_allowed(&clean_host);
    let allow_loopback = clean_host == "localhost" && policy.allowed_domains.contains("localhost");
    let connect_target = format_host_port(authority, default_port);

    let resolved = match tokio::time::timeout(
        PROXY_CONNECT_TIMEOUT,
        tokio::net::lookup_host(&connect_target),
    )
    .await
    {
        Ok(Ok(addrs)) => addrs.collect::<Vec<_>>(),
        Ok(Err(_)) => {
            send_error_and_close(
                client,
                b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n",
            )
            .await;
            return None;
        }
        Err(_) => {
            send_error_and_close(
                client,
                b"HTTP/1.1 504 Gateway Timeout\r\nConnection: close\r\n\r\n",
            )
            .await;
            return None;
        }
    };

    let permitted: Vec<SocketAddr> = resolved
        .into_iter()
        .filter(|sa| !is_disallowed_resolved_ip(sa.ip(), allow_explicit_literal, allow_loopback))
        .collect();

    if permitted.is_empty() {
        warn!(
            target = %authority,
            "Blocked upstream connection resolving exclusively to restricted/private IP addresses (SSRF guard)"
        );
        send_error_and_close(
            client,
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nForbidden",
        )
        .await;
        return None;
    }

    match tokio::time::timeout(
        PROXY_CONNECT_TIMEOUT,
        TcpStream::connect(permitted.as_slice()),
    )
    .await
    {
        Ok(Ok(stream)) => Some(stream),
        Ok(Err(_)) => {
            send_error_and_close(
                client,
                b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n",
            )
            .await;
            None
        }
        Err(_) => {
            send_error_and_close(
                client,
                b"HTTP/1.1 504 Gateway Timeout\r\nConnection: close\r\n\r\n",
            )
            .await;
            None
        }
    }
}

async fn handle_proxy_client(client: TcpStream, policy: DomainWhitelist) -> std::io::Result<()> {
    handle_proxy_client_with_header_timeout(client, policy, PROXY_HEADER_READ_TIMEOUT).await
}

async fn handle_proxy_client_with_header_timeout(
    mut client: TcpStream,
    policy: DomainWhitelist,
    header_timeout: Duration,
) -> std::io::Result<()> {
    let mut header_buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    let read_result = tokio::time::timeout(header_timeout, async {
        while header_buf.len() < MAX_PROXY_HEADER_BYTES {
            let n = client.read(&mut chunk).await?;
            if n == 0 {
                return Ok(None);
            }
            let slice = chunk.get(..n).unwrap_or(&chunk);
            header_buf.extend_from_slice(slice);
            if let Some(pos) = find_header_end(&header_buf) {
                return Ok(Some(pos));
            }
        }
        Ok(None)
    })
    .await;

    let end_idx = match read_result {
        Ok(Ok(Some(pos))) => pos,
        Ok(Ok(None)) => {
            if header_buf.is_empty() {
                return Ok(());
            }
            let response = if header_buf.len() >= MAX_PROXY_HEADER_BYTES {
                b"HTTP/1.1 431 Request Header Fields Too Large\r\nConnection: close\r\n\r\n"
                    .as_slice()
            } else {
                b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n".as_slice()
            };
            send_error_and_close(&mut client, response).await;
            return Ok(());
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            send_error_and_close(
                &mut client,
                b"HTTP/1.1 408 Request Timeout\r\nConnection: close\r\n\r\n",
            )
            .await;
            return Ok(());
        }
    };

    let head_bytes = header_buf.get(..end_idx).unwrap_or(&header_buf);
    let head_str = String::from_utf8_lossy(head_bytes);
    let mut lines = head_str.split("\r\n");
    let Some(request_line) = lines.next() else {
        send_error_and_close(
            &mut client,
            b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n",
        )
        .await;
        return Ok(());
    };

    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        send_error_and_close(
            &mut client,
            b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n",
        )
        .await;
        return Ok(());
    };

    if method.eq_ignore_ascii_case("CONNECT") {
        let Some(mut upstream) = authorize_and_connect(&mut client, target, 443, &policy).await
        else {
            return Ok(());
        };
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        if let Some(trailing) = header_buf.get(end_idx..) {
            if !trailing.is_empty() {
                upstream.write_all(trailing).await?;
            }
        }
        let _ = tokio::time::timeout(
            PROXY_SESSION_TIMEOUT,
            tokio::io::copy_bidirectional(&mut client, &mut upstream),
        )
        .await;
        return Ok(());
    }

    let Some((authority, origin_path)) = extract_http_target(target, lines) else {
        send_error_and_close(
            &mut client,
            b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n",
        )
        .await;
        return Ok(());
    };

    let Some(mut upstream) = authorize_and_connect(&mut client, &authority, 80, &policy).await
    else {
        return Ok(());
    };

    let rewritten_first_line = format!("{} {} {}\r\n", method, origin_path, version);
    upstream.write_all(rewritten_first_line.as_bytes()).await?;
    if let Some(first_crlf) = head_str.find("\r\n") {
        let rest_start = first_crlf.saturating_add(2);
        if let Some(rest_headers) = head_bytes.get(rest_start..) {
            upstream.write_all(rest_headers).await?;
        }
    }
    if let Some(trailing) = header_buf.get(end_idx..) {
        if !trailing.is_empty() {
            upstream.write_all(trailing).await?;
        }
    }
    let _ = tokio::time::timeout(
        PROXY_SESSION_TIMEOUT,
        tokio::io::copy_bidirectional(&mut client, &mut upstream),
    )
    .await;
    Ok(())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos.saturating_add(4))
}

fn extract_http_target<'a>(
    target: &'a str,
    headers: impl Iterator<Item = &'a str>,
) -> Option<(String, String)> {
    let mut host_header: Option<String> = None;
    for line in headers {
        if let Some((name, val)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("host") {
                let h = val.trim();
                if !h.is_empty() {
                    host_header = Some(h.to_string());
                }
                break;
            }
        }
    }

    if let Some(without_scheme) = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
    {
        let (auth, path) = match without_scheme.find('/') {
            Some(idx) => (
                without_scheme.get(..idx).unwrap_or(without_scheme),
                without_scheme.get(idx..).unwrap_or("/"),
            ),
            None => (without_scheme, "/"),
        };
        if auth.is_empty() {
            return None;
        }
        if let Some(ref hdr) = host_header {
            if DomainWhitelist::normalize_host(auth) != DomainWhitelist::normalize_host(hdr) {
                return None;
            }
        }
        return Some((auth.to_string(), path.to_string()));
    }

    host_header.map(|h| (h, target.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_whitelist_matching() {
        let whitelist = DomainWhitelist::new(vec![
            "allowed-domain.local".to_string(),
            "*.allowed-wildcard.local".to_string(),
            "exact-match.local".to_string(),
        ]);

        assert!(whitelist.is_allowed("allowed-domain.local"));
        assert!(whitelist.is_allowed("allowed-domain.local:443"));
        assert!(whitelist.is_allowed("sub.allowed-wildcard.local"));
        assert!(whitelist.is_allowed("deep.sub.allowed-wildcard.local"));

        assert!(!whitelist.is_allowed("blocked-domain.local"));
        assert!(!whitelist.is_allowed("notallowed-wildcard.local"));
        assert!(!whitelist.is_allowed("allowed-wildcard.local"));
    }

    #[test]
    fn test_authorize_request_typed_error() {
        let whitelist = DomainWhitelist::new(vec!["allowed-domain.local".to_string()]);

        assert!(whitelist.authorize_request("allowed-domain.local").is_ok());

        let err = whitelist
            .authorize_request("blocked-domain.local")
            .unwrap_err();
        assert_eq!(
            err,
            ProxyError::AccessDenied {
                host: "blocked-domain.local".to_string()
            }
        );
        assert!(err.to_string().contains("Security Exception"));
    }

    #[test]
    fn test_url_scheme_and_path_normalization() {
        let whitelist = DomainWhitelist::new(vec![
            "allowed-domain.local".to_string(),
            "*.allowed-wildcard.local".to_string(),
        ]);

        assert!(whitelist.is_allowed("https://allowed-domain.local/api/v1/resource"));
        assert!(whitelist.is_allowed("http://api.allowed-wildcard.local/test"));
        assert!(whitelist.is_allowed("allowed-domain.local:443"));
        assert!(!whitelist.is_allowed("blocked-domain.local"));
    }

    #[tokio::test]
    async fn test_loopback_proxy_enforces_whitelist_and_rejects_oversized_headers() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            if let Ok((mut sock, _)) = upstream.accept().await {
                let mut buf = [0u8; 512];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
            }
        });

        let policy = DomainWhitelist::new(vec!["127.0.0.1".to_string()]);
        let proxy = LoopbackAllowlistProxy::start(policy).await.unwrap();
        let proxy_addr = proxy.local_addr();

        // 1. Disallowed host -> 403 Forbidden
        let mut denied_stream = TcpStream::connect(proxy_addr).await.unwrap();
        denied_stream
            .write_all(
                b"CONNECT evil.example.com:443 HTTP/1.1\r\nHost: evil.example.com:443\r\n\r\n",
            )
            .await
            .unwrap();
        let mut resp = String::new();
        denied_stream.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 403 Forbidden"));

        // 2. Allowed host -> forwarded to upstream
        let mut allowed_stream = TcpStream::connect(proxy_addr).await.unwrap();
        let req = format!(
            "GET http://{}/ping HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            upstream_addr, upstream_addr
        );
        allowed_stream.write_all(req.as_bytes()).await.unwrap();
        let mut ok_resp = String::new();
        allowed_stream.read_to_string(&mut ok_resp).await.unwrap();
        assert!(ok_resp.contains("200 OK"));
        assert!(ok_resp.ends_with("ok"));

        // 3. Oversized headers -> 431
        let mut big_stream = TcpStream::connect(proxy_addr).await.unwrap();
        let big_req = format!(
            "GET http://127.0.0.1/ HTTP/1.1\r\nX-Pad: {}\r\n\r\n",
            "a".repeat(20_000)
        );
        big_stream.write_all(big_req.as_bytes()).await.unwrap();
        let mut big_resp = String::new();
        big_stream.read_to_string(&mut big_resp).await.unwrap();
        assert!(big_resp.starts_with("HTTP/1.1 431"));

        // 4. Host vs URI discrepancy -> 400 Bad Request
        let mut mismatch_stream = TcpStream::connect(proxy_addr).await.unwrap();
        let mismatch_req = format!(
            "GET http://{}/ping HTTP/1.1\r\nHost: evil.example.com\r\n\r\n",
            upstream_addr
        );
        mismatch_stream
            .write_all(mismatch_req.as_bytes())
            .await
            .unwrap();
        let mut mismatch_resp = String::new();
        mismatch_stream
            .read_to_string(&mut mismatch_resp)
            .await
            .unwrap();
        assert!(mismatch_resp.starts_with("HTTP/1.1 400"));

        drop(proxy);
        let _ = upstream_task.await;
    }

    #[tokio::test]
    async fn test_loopback_proxy_connect_tunnel_ssrf_and_header_timeout() {
        // 1. Live CONNECT tunnel forwarding bidirectional bytes
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let tunnel_server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = upstream.accept().await {
                let mut buf = [0u8; 4];
                if sock.read_exact(&mut buf).await.is_ok() {
                    assert_eq!(&buf, b"ping");
                    let _ = sock.write_all(b"pong").await;
                }
            }
        });

        let policy = DomainWhitelist::new(vec!["127.0.0.1".to_string()]);
        let proxy = LoopbackAllowlistProxy::start(policy.clone()).await.unwrap();
        let proxy_addr = proxy.local_addr();

        let mut tunnel_stream = TcpStream::connect(proxy_addr).await.unwrap();
        let connect_req = format!("CONNECT {0} HTTP/1.1\r\nHost: {0}\r\n\r\n", upstream_addr);
        tunnel_stream
            .write_all(connect_req.as_bytes())
            .await
            .unwrap();
        let mut head_buf = [0u8; 64];
        let n = tunnel_stream.read(&mut head_buf).await.unwrap();
        let head = std::str::from_utf8(head_buf.get(..n).unwrap_or(&[])).unwrap();
        assert!(head.starts_with("HTTP/1.1 200 Connection Established"));

        tunnel_stream.write_all(b"ping").await.unwrap();
        let mut pong = [0u8; 4];
        tunnel_stream.read_exact(&mut pong).await.unwrap();
        assert_eq!(&pong, b"pong");
        drop(tunnel_stream);
        let _ = tunnel_server.await;
        drop(proxy);

        // 2. SSRF resolved-IP classifier blocks metadata/RFC1918/IPv4-mapped/rebinding IPs
        assert!(is_disallowed_resolved_ip(
            "169.254.169.254".parse().unwrap(),
            false,
            false
        ));
        assert!(is_disallowed_resolved_ip(
            "10.0.0.1".parse().unwrap(),
            false,
            false
        ));
        assert!(is_disallowed_resolved_ip(
            "192.168.1.1".parse().unwrap(),
            false,
            false
        ));
        assert!(is_disallowed_resolved_ip(
            "127.0.0.1".parse().unwrap(),
            false,
            false
        ));
        assert!(is_disallowed_resolved_ip(
            "::ffff:169.254.169.254".parse().unwrap(),
            false,
            false
        ));
        assert!(is_disallowed_resolved_ip(
            "::ffff:127.0.0.1".parse().unwrap(),
            false,
            false
        ));
        assert!(is_disallowed_resolved_ip(
            "fe80::1".parse().unwrap(),
            false,
            false
        ));
        assert!(!is_disallowed_resolved_ip(
            "127.0.0.1".parse().unwrap(),
            true,
            false
        ));
        assert!(!is_disallowed_resolved_ip(
            "127.0.0.1".parse().unwrap(),
            false,
            true
        ));
        assert!(!is_disallowed_resolved_ip(
            "93.184.216.34".parse().unwrap(),
            false,
            false
        ));

        // 3. Stalled client triggers 408 Request Timeout
        let timeout_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let timeout_addr = timeout_listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let (sock, _) = timeout_listener.accept().await.unwrap();
            handle_proxy_client_with_header_timeout(sock, policy, Duration::from_millis(30))
                .await
                .unwrap();
        });
        let mut slow_client = TcpStream::connect(timeout_addr).await.unwrap();
        slow_client
            .write_all(b"GET http://127.0.0.1/slow HTTP/1.1\r\n")
            .await
            .unwrap();
        let mut timeout_resp = String::new();
        slow_client.read_to_string(&mut timeout_resp).await.unwrap();
        assert!(timeout_resp.starts_with("HTTP/1.1 408 Request Timeout"));
        server_task.await.unwrap();
    }
}
