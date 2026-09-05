//! HTTP wire-response parsing for the egress tools.
//!
//! Owns the strict separation of a raw `curl --include` transcript into
//! (status, headers, body). Keeping this apart from the fetch policy in
//! `web_fetch` means the parser can never be tempted by policy concerns, and
//! body content that merely quotes HTTP examples can never change a fetch
//! outcome or a redirect target.

use super::ToolError;

/// Split one raw `--include` response into (status, Location, body).
///
/// Header and body are separated strictly: the walk starts at the first status
/// line and skips interim responses (such as `100 Continue`) block by block.
/// The status and `Location` are only ever parsed inside the final header
/// block, so body text that merely contains `HTTP/` or `Location:` examples
/// (documentation pages, error samples) can never change the fetch outcome or
/// the redirect target.
pub(crate) fn parse_http_response(raw: &str) -> Result<(u16, Option<String>, String), ToolError> {
    let mut rest = raw;
    if !rest.starts_with("HTTP/") {
        // Tolerate proxy noise before the first status line, anchored on the
        // first occurrence only: real headers always precede the body.
        let start = rest
            .find("\nHTTP/")
            .map(|index| index + 1)
            .ok_or_else(|| ToolError::new("web fetch returned no response headers"))?;
        rest = &rest[start..];
    }
    loop {
        let (head, body) = rest
            .split_once("\r\n\r\n")
            .or_else(|| rest.split_once("\n\n"))
            .ok_or_else(|| ToolError::new("web fetch returned no response headers"))?;
        let status = parse_status_line(head)?;
        if (100..200).contains(&status) {
            // An interim response owns no body; the next block is the real one.
            rest = body;
            if !rest.starts_with("HTTP/") {
                return Err(ToolError::new(
                    "web fetch returned no parseable status line after an interim response",
                ));
            }
            continue;
        }
        let location = head.lines().skip(1).find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("location")
                .then(|| value.trim().trim_end_matches('\r').to_string())
        });
        return Ok((status, location, body.to_string()));
    }
}

fn parse_status_line(head: &str) -> Result<u16, ToolError> {
    head.lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| ToolError::new("web fetch returned no parseable status line"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http_response_extracts_status_location_and_body() {
        let redirect = "HTTP/1.1 301 Moved Permanently\r\nLocation: https://example.com/next\r\nContent-Length: 0\r\n\r\n";
        let (status, location, body) = parse_http_response(redirect).expect("redirect parses");
        assert_eq!(status, 301);
        assert_eq!(location.as_deref(), Some("https://example.com/next"));
        assert_eq!(body, "");

        let ok = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\npage body";
        let (status, location, body) = parse_http_response(ok).expect("200 parses");
        assert_eq!(status, 200);
        assert!(location.is_none());
        assert_eq!(body, "page body");

        let interim = "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n\r\ndone";
        let (status, _, body) = parse_http_response(interim).expect("interim skipped");
        assert_eq!(status, 200);
        assert_eq!(body, "done");
    }

    #[test]
    fn parse_http_response_never_reads_status_or_location_from_the_body() {
        // A body that quotes a status line must not become the response status.
        let quoted = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nDocs often show:\r\nHTTP/1.1 404 Not Found\r\n\r\nmore text";
        let (status, location, body) = parse_http_response(quoted).expect("quoted body parses");
        assert_eq!(status, 200);
        assert!(location.is_none());
        assert!(body.contains("HTTP/1.1 404 Not Found"));

        // A body line that looks like a Location header must not override the
        // real redirect target.
        let redirect = "HTTP/1.1 302 Found\r\nLocation: https://example.com/real\r\n\r\nLocation: https://evil.example/from-body\r\n";
        let (status, location, _) = parse_http_response(redirect).expect("redirect parses");
        assert_eq!(status, 302);
        assert_eq!(location.as_deref(), Some("https://example.com/real"));

        // A 200 body quoting a full redirect response must not trigger a hop.
        let example =
            "HTTP/1.1 200 OK\r\n\r\nHTTP/1.1 302 Found\r\nLocation: https://evil.example\r\n\r\n";
        let (status, location, _) = parse_http_response(example).expect("example parses");
        assert_eq!(status, 200);
        assert!(location.is_none());

        // LF-only framing keeps the same strict separation.
        let lf_only = "HTTP/1.1 200 OK\nContent-Type: text/plain\n\nbody\nHTTP/1.1 500 Boom\n\nx";
        let (status, _, body) = parse_http_response(lf_only).expect("lf-only parses");
        assert_eq!(status, 200);
        assert!(body.contains("HTTP/1.1 500 Boom"));
    }
}
