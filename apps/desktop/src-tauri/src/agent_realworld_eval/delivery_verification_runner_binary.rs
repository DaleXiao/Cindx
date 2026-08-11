use super::delivery_verification_protocol::DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::process::Command;

pub(super) const MAX_RUNNER_BYTES: u64 = 512 * 1024 * 1024;
const CODESIGN_PATH: &str = "/usr/bin/codesign";
const CODE_DIRECTORY_PREFIX: &str = "CandidateCDHashFull sha256=";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryVerificationRunnerBinary {
    pub(super) bytes: Vec<u8>,
    pub(super) code_directory_sha256: String,
}

pub(super) fn read_delivery_execute_sibling() -> Result<DeliveryVerificationRunnerBinary, String> {
    let control_plane = std::env::current_exe()
        .map_err(|error| format!("failed to locate delivery control-plane binary: {error}"))?;
    let directory = control_plane
        .parent()
        .ok_or_else(|| "delivery control-plane binary has no parent directory".to_string())?;
    read_exact_binary(
        &directory.join(format!(
            "{}{}",
            DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME,
            std::env::consts::EXE_SUFFIX
        )),
        DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME,
    )
}

pub(super) fn read_current_delivery_execute() -> Result<DeliveryVerificationRunnerBinary, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("failed to locate delivery execute binary: {error}"))?;
    let runner = read_exact_binary(&executable, DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME)?;
    validate_running_code_directory(&runner.code_directory_sha256)?;
    Ok(runner)
}

fn read_exact_binary(
    path: &Path,
    expected_stem: &str,
) -> Result<DeliveryVerificationRunnerBinary, String> {
    let expected_name = format!("{}{}", expected_stem, std::env::consts::EXE_SUFFIX);
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err("delivery execute binary has an unexpected fixed name".into());
    }
    let before = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect delivery execute binary: {error}"))?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err("delivery execute binary must be a regular non-symlink file".into());
    }
    let mut file = File::open(path)
        .map_err(|error| format!("failed to open delivery execute binary: {error}"))?;
    let after = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to re-inspect delivery execute binary: {error}"))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("failed to stat delivery execute binary: {error}"))?;
    if !after.is_file() || after.file_type().is_symlink() {
        return Err("delivery execute binary changed to a non-regular file".into());
    }
    #[cfg(unix)]
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || after.dev() != opened.dev()
        || after.ino() != opened.ino()
    {
        return Err("delivery execute binary changed while it was opened".into());
    }
    if opened.len() == 0 || opened.len() > MAX_RUNNER_BYTES {
        return Err("delivery execute binary size is outside its bound".into());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read delivery execute binary: {error}"))?;
    if u64::try_from(bytes.len()).ok() != Some(opened.len()) {
        return Err("delivery execute binary changed while it was read".into());
    }
    let code_directory_sha256 = code_directory_sha256_for_bytes(&bytes)?;
    Ok(DeliveryVerificationRunnerBinary {
        bytes,
        code_directory_sha256,
    })
}

pub(super) fn code_directory_sha256_for_bytes(bytes: &[u8]) -> Result<String, String> {
    if bytes.is_empty() || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RUNNER_BYTES {
        return Err("delivery execute binary size is outside its bound".into());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bytes;
        return Err("delivery execute code-directory identity requires macOS".into());
    }
    #[cfg(target_os = "macos")]
    {
        let directory = tempfile::Builder::new()
            .prefix("cindx-delivery-runner-identity.")
            .tempdir()
            .map_err(|error| format!("failed to stage delivery execute identity: {error}"))?;
        let path = directory.path().join(format!(
            "{}{}",
            DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME,
            std::env::consts::EXE_SUFFIX
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o500);
        let mut file = options
            .open(&path)
            .map_err(|error| format!("failed to create delivery execute identity copy: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("failed to write delivery execute identity copy: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("failed to sync delivery execute identity copy: {error}"))?;
        drop(file);

        let verification = Command::new(CODESIGN_PATH)
            .env("LC_ALL", "C")
            .args(["--verify", "--strict"])
            .arg(&path)
            .output()
            .map_err(|error| {
                format!("failed to verify delivery execute code signature: {error}")
            })?;
        if !verification.status.success() {
            return Err("delivery execute code signature is invalid".into());
        }
        code_directory_sha256(path.as_os_str(), "delivery execute identity copy")
    }
}

pub(super) fn running_code_directory_sha256() -> Result<String, String> {
    #[cfg(not(target_os = "macos"))]
    {
        Err("delivery execute running-image identity requires macOS".into())
    }
    #[cfg(target_os = "macos")]
    {
        let process = format!("+{}", std::process::id());
        code_directory_sha256(OsStr::new(&process), "running delivery execute image")
    }
}

pub(super) fn validate_running_code_directory(expected: &str) -> Result<(), String> {
    validate_code_directory_sha256(expected)?;
    #[cfg(target_os = "macos")]
    verify_running_code_signature()?;
    let running = running_code_directory_sha256()?;
    #[cfg(target_os = "macos")]
    verify_running_code_signature()?;
    if running != expected {
        return Err(
            "running delivery execute image differs from the frozen code-directory identity".into(),
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_running_code_signature() -> Result<(), String> {
    let process = format!("+{}", std::process::id());
    let verification = Command::new(CODESIGN_PATH)
        .env("LC_ALL", "C")
        .args(["--verify", "--strict"])
        .arg(&process)
        .output()
        .map_err(|error| format!("failed to verify running delivery execute image: {error}"))?;
    if !verification.status.success() {
        return Err(
            "running delivery execute image differs from the frozen code-directory identity".into(),
        );
    }
    Ok(())
}

fn code_directory_sha256(target: &OsStr, label: &str) -> Result<String, String> {
    let output = Command::new(CODESIGN_PATH)
        .env("LC_ALL", "C")
        .args(["--display", "--verbose=5"])
        .arg(target)
        .output()
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !output.status.success() {
        return Err(format!("failed to inspect {label} code-directory identity"));
    }
    let text = std::str::from_utf8(&output.stderr)
        .map_err(|_| format!("{label} code-directory output is not UTF-8"))?;
    let mut candidates = text
        .lines()
        .filter_map(|line| line.strip_prefix(CODE_DIRECTORY_PREFIX));
    let candidate = candidates
        .next()
        .ok_or_else(|| format!("{label} has no full SHA-256 code-directory identity"))?;
    if candidates.next().is_some() {
        return Err(format!("{label} has multiple code-directory identities"));
    }
    validate_code_directory_sha256(candidate)?;
    Ok(candidate.to_string())
}

fn validate_code_directory_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err("delivery execute code-directory identity is not lowercase SHA-256".into());
    }
    Ok(())
}
