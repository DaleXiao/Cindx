use std::fs;
use std::io::{self, Write};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub fn write_private_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), io::Error> {
    write_private_file_atomically_with(path, bytes, |temporary, target| {
        temporary
            .persist(target)
            .map_err(|error| error.error)
            .map(|_| ())
    })
}

fn write_private_file_atomically_with(
    path: &Path,
    bytes: &[u8],
    publish: impl FnOnce(tempfile::NamedTempFile, &Path) -> Result<(), io::Error>,
) -> Result<(), io::Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("cindx-private");
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
    publish(temporary, path)?;

    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_no_staging_files(directory: &Path, target_name: &str) {
        let prefix = format!(".{target_name}.");
        let leftovers = fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "staging files remain: {leftovers:?}");
    }

    #[test]
    fn atomically_replaces_existing_file_with_exact_private_contents() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, b"old-partial-contents").unwrap();

        write_private_file_atomically(&path, b"{\"enabled\":true}").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"{\"enabled\":true}");
        assert_no_staging_files(directory.path(), "settings.json");
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn failed_publish_preserves_target_and_cleans_staging_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let original = b"{\"enabled\":false}";
        fs::write(&path, original).unwrap();

        let result = write_private_file_atomically_with(
            &path,
            b"{\"enabled\":true}",
            |_temporary, _target| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected publish failure",
                ))
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_no_staging_files(directory.path(), "settings.json");
    }

    #[test]
    fn creates_missing_parent_without_changing_payload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/settings.conf");

        write_private_file_atomically(&path, b"key=value\n").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"key=value\n");
    }
}
