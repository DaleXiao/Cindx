use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(super) struct HttpFixtureReceipt {
    pub(super) target_sha256: String,
    pub(super) body_sha256: String,
    pub(super) successful_requests: usize,
}

pub(super) struct HttpFixture {
    url: String,
    target_sha256: String,
    body_sha256: String,
    successful_requests: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl HttpFixture {
    pub(super) fn start(root: &Path, route_key: &str) -> Result<Self, String> {
        validate_route_key(route_key)?;
        let body_path = root.join("site/index.html");
        let body = std::fs::read(&body_path).map_err(|error| {
            format!(
                "failed to read HTTP fixture {}: {error}",
                body_path.display()
            )
        })?;
        let body_sha256 = crate::sha256_hex(&body);
        let route = format!("/fixture/{route_key}/site/index.html");
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("failed to bind HTTP fixture: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("failed to configure HTTP fixture listener: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("failed to resolve HTTP fixture address: {error}"))?;
        let url = format!("http://{address}{route}");
        let target_sha256 = crate::sha256_hex(url.as_bytes());
        let successful_requests = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let server_requests = Arc::clone(&successful_requests);
        let server_shutdown = Arc::clone(&shutdown);
        let server = thread::Builder::new()
            .name("cindx-agent-eval-http-fixture".to_string())
            .spawn(move || {
                serve(listener, &route, &body, &server_requests, &server_shutdown);
            })
            .map_err(|error| format!("failed to start HTTP fixture: {error}"))?;

        Ok(Self {
            url,
            target_sha256,
            body_sha256,
            successful_requests,
            shutdown,
            server: Some(server),
        })
    }

    pub(super) fn url(&self) -> &str {
        &self.url
    }

    pub(super) fn receipt(&self) -> HttpFixtureReceipt {
        HttpFixtureReceipt {
            target_sha256: self.target_sha256.clone(),
            body_sha256: self.body_sha256.clone(),
            successful_requests: self.successful_requests.load(Ordering::Acquire),
        }
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn validate_route_key(route_key: &str) -> Result<(), String> {
    if route_key.is_empty()
        || route_key.len() > 128
        || !route_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(
            "HTTP fixture route key must contain only ASCII letters, digits, '-' or '_'"
                .to_string(),
        );
    }
    Ok(())
}

fn serve(
    listener: TcpListener,
    route: &str,
    body: &[u8],
    successful_requests: &AtomicUsize,
    shutdown: &AtomicBool,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let _ = respond(stream, route, body, successful_requests);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
        }
    }
}

fn respond(
    mut stream: TcpStream,
    route: &str,
    body: &[u8],
    successful_requests: &AtomicUsize,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
    let mut request = [0_u8; 8192];
    let mut received = 0;
    while received < request.len() {
        let read = stream.read(&mut request[received..])?;
        if read == 0 {
            break;
        }
        received += read;
        if request[..received]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
    }

    let request_line = String::from_utf8_lossy(&request[..received]);
    let mut parts = request_line
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let (status, content_type, response_body, allow, successful) =
        if method != "GET" && method != "HEAD" {
            (
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                b"Method Not Allowed\n".as_slice(),
                true,
                false,
            )
        } else if target != route {
            (
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"Not Found\n".as_slice(),
                false,
                false,
            )
        } else {
            ("200 OK", "text/html; charset=utf-8", body, false, true)
        };
    let allow_header = if allow { "Allow: GET, HEAD\r\n" } else { "" };
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{allow_header}Connection: close\r\n\r\n",
        response_body.len()
    );
    stream.write_all(headers.as_bytes())?;
    if method != "HEAD" {
        stream.write_all(response_body)?;
    }
    stream.flush()?;
    if successful {
        successful_requests.fetch_add(1, Ordering::AcqRel);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("fixture tempdir");
        std::fs::create_dir_all(root.path().join("site")).expect("site directory");
        std::fs::write(
            root.path().join("site/index.html"),
            b"<!doctype html><title>Cindx fixture</title>",
        )
        .expect("fixture body");
        root
    }

    fn request(url: &str, method: &str, target: &str) -> String {
        let authority = url
            .strip_prefix("http://")
            .expect("loopback URL")
            .split('/')
            .next()
            .expect("URL authority");
        let mut stream = TcpStream::connect(authority).expect("connect fixture");
        write!(
            stream,
            "{method} {target} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
        )
        .expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    }

    fn path(url: &str) -> &str {
        let without_scheme = url.strip_prefix("http://").expect("loopback URL");
        &without_scheme[without_scheme.find('/').expect("URL path")..]
    }

    #[test]
    fn serves_only_the_exact_fixture_target_and_records_receipt() {
        let root = fixture_root();
        let fixture = HttpFixture::start(root.path(), "case_digest").expect("start fixture");
        let response = request(fixture.url(), "GET", path(fixture.url()));

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("<!doctype html><title>Cindx fixture</title>"));
        let receipt = fixture.receipt();
        assert_eq!(
            receipt.target_sha256,
            crate::sha256_hex(fixture.url().as_bytes())
        );
        assert_eq!(
            receipt.body_sha256,
            crate::sha256_hex(b"<!doctype html><title>Cindx fixture</title>")
        );
        assert_eq!(receipt.successful_requests, 1);

        let missing = request(
            fixture.url(),
            "GET",
            "/fixture/case_digest/site/missing.html",
        );
        assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"));
        let rejected = request(fixture.url(), "POST", path(fixture.url()));
        assert!(rejected.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        assert!(rejected.contains("\r\nAllow: GET, HEAD\r\n"));
        assert_eq!(fixture.receipt().successful_requests, 1);
    }

    #[test]
    fn rejects_route_keys_that_could_escape_the_single_route() {
        let root = fixture_root();
        assert!(HttpFixture::start(root.path(), "../escape").is_err());
        assert!(HttpFixture::start(root.path(), "nested/route").is_err());
    }

    #[test]
    fn drop_closes_the_loopback_listener() {
        let root = fixture_root();
        let fixture = HttpFixture::start(root.path(), "drop_case").expect("start fixture");
        let address = fixture
            .url()
            .strip_prefix("http://")
            .expect("loopback URL")
            .split('/')
            .next()
            .expect("URL authority")
            .parse()
            .expect("socket address");
        assert!(TcpStream::connect(address).is_ok());

        drop(fixture);

        assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
    }
}
