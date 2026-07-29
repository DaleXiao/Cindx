use reqwest::{redirect, Url};

const MAX_HTTP_REDIRECTS: usize = 10;

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

pub(crate) fn api_key_safe_redirect_policy() -> redirect::Policy {
    redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_HTTP_REDIRECTS {
            return attempt.error("too many redirects");
        }
        let Some(previous) = attempt.previous().last() else {
            return attempt.stop();
        };
        if crate::is_azure_openai_url(previous.as_str()) && !same_origin(previous, attempt.url()) {
            attempt.stop()
        } else {
            attempt.follow()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{run_http, ModelError};
    use reqwest::{Client, StatusCode};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn azure_api_key_redirects_stop_before_crossing_origins() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("redirect server should bind");
        let address = listener
            .local_addr()
            .expect("redirect server address should exist");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request should arrive");
            let mut request = [0_u8; 4096];
            let length = stream
                .read(&mut request)
                .expect("request should be readable");
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response should be writable");
            String::from_utf8_lossy(&request[..length]).into_owned()
        });
        let client = Client::builder()
            .no_proxy()
            .redirect(api_key_safe_redirect_policy())
            .resolve("cindx.openai.azure.com", address)
            .build()
            .expect("test client should build");
        let response = run_http(async move {
            client
                .get(format!(
                    "http://cindx.openai.azure.com:{}/start",
                    address.port()
                ))
                .header("api-key", "test-secret")
                .send()
                .await
                .map_err(|error| ModelError::new(error.to_string()))
        })
        .expect("cross-origin redirect should return the original response");
        let request = server.join().expect("redirect server should finish");

        assert_eq!(response.status(), StatusCode::FOUND);
        assert!(request
            .to_ascii_lowercase()
            .contains("api-key: test-secret"));
        assert!(same_origin(
            &Url::parse("https://cindx.openai.azure.com/openai/v1").unwrap(),
            &Url::parse("https://cindx.openai.azure.com/next").unwrap()
        ));
        assert!(!same_origin(
            &Url::parse("https://cindx.openai.azure.com/openai/v1").unwrap(),
            &Url::parse("https://attacker.example/next").unwrap()
        ));
    }
}
