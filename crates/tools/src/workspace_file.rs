use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug)]
pub(crate) enum AtomicReplaceError {
    Conflict { observed_sha256: Option<String> },
    Io,
}

pub(super) fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn write_immutable_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), io::Error> {
    if path.exists() {
        return exact_existing_file_or_conflict(path, bytes);
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("cindx-workspace-snapshot");
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            exact_existing_file_or_conflict(path, bytes)?;
        }
        Err(error) => return Err(error.error),
    }
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn exact_existing_file_or_conflict(path: &Path, bytes: &[u8]) -> Result<(), io::Error> {
    if fs::read(path).is_ok_and(|existing| existing == bytes) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "immutable workspace snapshot already exists with different contents",
        ))
    }
}

pub(crate) fn atomic_replace_preserving_permissions_if_sha256(
    path: &Path,
    bytes: &[u8],
    expected_sha256: &str,
    max_recheck_bytes: usize,
) -> Result<(), AtomicReplaceError> {
    atomic_replace_preserving_permissions_if_sha256_with(
        path,
        bytes,
        expected_sha256,
        max_recheck_bytes,
        |_| Ok(()),
        |temporary, target| {
            temporary
                .persist(target)
                .map_err(|error| error.error)
                .map(|_| ())
        },
    )
}

pub(crate) fn atomic_replace_preserving_permissions_if_sha256_with<BeforeRecheck, Publish>(
    path: &Path,
    bytes: &[u8],
    expected_sha256: &str,
    max_recheck_bytes: usize,
    before_recheck: BeforeRecheck,
    publish: Publish,
) -> Result<(), AtomicReplaceError>
where
    BeforeRecheck: FnOnce(&Path) -> Result<(), io::Error>,
    Publish: FnOnce(tempfile::NamedTempFile, &Path) -> Result<(), io::Error>,
{
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("cindx-workspace-file");
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|_| AtomicReplaceError::Io)?;
    temporary
        .write_all(bytes)
        .map_err(|_| AtomicReplaceError::Io)?;

    let mut locked_target = fs::File::open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            AtomicReplaceError::Conflict {
                observed_sha256: None,
            }
        } else {
            AtomicReplaceError::Io
        }
    })?;
    locked_target
        .try_lock()
        .map_err(|_| AtomicReplaceError::Conflict {
            observed_sha256: None,
        })?;
    before_recheck(path).map_err(|_| AtomicReplaceError::Io)?;
    let locked_metadata = locked_target
        .metadata()
        .map_err(|_| AtomicReplaceError::Io)?;
    let path_metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => {
            return Err(AtomicReplaceError::Conflict {
                observed_sha256: None,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(AtomicReplaceError::Conflict {
                observed_sha256: None,
            })
        }
        Err(_) => return Err(AtomicReplaceError::Io),
    };
    if !same_file_identity(&locked_metadata, &path_metadata) {
        return Err(AtomicReplaceError::Conflict {
            observed_sha256: None,
        });
    }
    let observed_sha256 = sha256_reader_bounded(&mut locked_target, max_recheck_bytes)
        .map_err(|_| AtomicReplaceError::Io)?;
    if observed_sha256.as_deref() != Some(expected_sha256) {
        return Err(AtomicReplaceError::Conflict { observed_sha256 });
    }
    let final_path_metadata = fs::metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            AtomicReplaceError::Conflict {
                observed_sha256: None,
            }
        } else {
            AtomicReplaceError::Io
        }
    })?;
    if !final_path_metadata.is_file() || !same_file_identity(&locked_metadata, &final_path_metadata)
    {
        return Err(AtomicReplaceError::Conflict {
            observed_sha256: None,
        });
    }

    temporary
        .as_file()
        .set_permissions(locked_metadata.permissions())
        .map_err(|_| AtomicReplaceError::Io)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| AtomicReplaceError::Io)?;
    publish(temporary, path).map_err(|_| AtomicReplaceError::Io)?;

    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

pub(super) fn sha256_file_bounded(
    path: &Path,
    max_bytes: usize,
) -> Result<Option<String>, io::Error> {
    let mut file = fs::File::open(path)?;
    sha256_reader_bounded(&mut file, max_bytes)
}

fn sha256_reader_bounded(
    file: &mut fs::File,
    max_bytes: usize,
) -> Result<Option<String>, io::Error> {
    file.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut total = 0usize;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        if total > max_bytes {
            return Ok(None);
        }
        digest.update(&buffer[..read]);
    }
    Ok(Some(format!("{:x}", digest.finalize())))
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.permissions().readonly() == right.permissions().readonly()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_race_preserves_the_racing_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.txt");
        fs::write(&path, "base").unwrap();
        let expected = sha256_bytes(b"base");

        let result = atomic_replace_preserving_permissions_if_sha256_with(
            &path,
            b"patch",
            &expected,
            1024,
            |target| fs::write(target, "racing writer"),
            |temporary, target| {
                temporary
                    .persist(target)
                    .map_err(|error| error.error)
                    .map(|_| ())
            },
        );

        assert!(matches!(result, Err(AtomicReplaceError::Conflict { .. })));
        assert_eq!(fs::read_to_string(path).unwrap(), "racing writer");
    }

    #[test]
    fn advisory_lock_contention_fails_without_publishing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.txt");
        fs::write(&path, "base").unwrap();
        let locked = fs::File::open(&path).unwrap();
        locked.try_lock().unwrap();

        let result = atomic_replace_preserving_permissions_if_sha256(
            &path,
            b"patch",
            &sha256_bytes(b"base"),
            1024,
        );

        assert!(matches!(result, Err(AtomicReplaceError::Conflict { .. })));
        assert_eq!(fs::read_to_string(path).unwrap(), "base");
    }

    #[test]
    fn atomic_path_replacement_during_recheck_is_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.txt");
        fs::write(&path, "base").unwrap();
        let expected = sha256_bytes(b"base");

        let result = atomic_replace_preserving_permissions_if_sha256_with(
            &path,
            b"patch",
            &expected,
            1024,
            |target| {
                let replacement = target.with_extension("racing");
                fs::write(&replacement, "racing replacement")?;
                fs::rename(replacement, target)
            },
            |temporary, target| {
                temporary
                    .persist(target)
                    .map_err(|error| error.error)
                    .map(|_| ())
            },
        );

        assert!(matches!(result, Err(AtomicReplaceError::Conflict { .. })));
        assert_eq!(fs::read_to_string(path).unwrap(), "racing replacement");
    }

    #[test]
    fn injected_publish_failure_preserves_the_original_and_cleans_staging() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.txt");
        fs::write(&path, "base").unwrap();
        let expected = sha256_bytes(b"base");

        let result = atomic_replace_preserving_permissions_if_sha256_with(
            &path,
            b"patch",
            &expected,
            1024,
            |_| Ok(()),
            |_temporary, _target| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected publish failure",
                ))
            },
        );

        assert!(matches!(result, Err(AtomicReplaceError::Io)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "base");
        let staging = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(".note.txt.") && name.ends_with(".tmp"))
            .collect::<Vec<_>>();
        assert!(staging.is_empty(), "staging files remain: {staging:?}");
    }

    #[test]
    fn immutable_snapshot_is_atomic_and_rejects_conflicting_reuse() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history/note.txt");

        write_immutable_file_atomically(&path, b"first").unwrap();
        write_immutable_file_atomically(&path, b"first").unwrap();
        let error = write_immutable_file_atomically(&path, b"second").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(path).unwrap(), b"first");
    }
}
