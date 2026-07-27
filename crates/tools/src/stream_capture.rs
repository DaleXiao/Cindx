use std::io::Read;

#[derive(Default)]
pub(crate) struct LimitedStreamCapture {
    pub(crate) bytes: Vec<u8>,
    pub(crate) total_bytes: u64,
    pub(crate) truncated: bool,
    pub(crate) error: Option<String>,
}

pub(crate) fn capture_stream_limited(
    mut stream: impl Read,
    max_bytes: usize,
) -> LimitedStreamCapture {
    let mut capture = LimitedStreamCapture {
        bytes: Vec::with_capacity(max_bytes.min(64 * 1024)),
        ..LimitedStreamCapture::default()
    };
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                capture.error = Some(error.to_string());
                break;
            }
        };
        capture.total_bytes = capture.total_bytes.saturating_add(count as u64);
        let remaining = max_bytes.saturating_sub(capture.bytes.len());
        let retained = remaining.min(count);
        capture.bytes.extend_from_slice(&buffer[..retained]);
        capture.truncated |= retained < count;
    }
    capture
}
