use super::test_support::temp_test_root;
use super::*;

#[test]
fn installed_app_data_is_user_scoped_and_overrideable() {
    let home = PathBuf::from("/Users/new-cindx-user");
    let expected = home
        .join("Library")
        .join("Application Support")
        .join("Cindx");
    assert_eq!(app_data_root_for(None, Some(home)), expected);

    let override_root = PathBuf::from("/tmp/cindx-portable-data");
    assert_eq!(
        app_data_root_for(Some(override_root.clone()), None),
        override_root
    );
    assert_eq!(database_path(), app_data_root().join("state.sqlite3"));
}

#[test]
fn persistent_app_store_creates_secures_and_reopens_database() {
    let root = temp_test_root("cindx-persistent-store");
    let database = root.join("state.sqlite3");

    let store = open_app_store_at(&database).expect("persistent store should open");
    drop(store);

    assert!(database.is_file());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&database)
            .expect("database metadata should load")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(
        SqliteStore::open_read_only(&database)
            .expect("created persistent store should reopen read-only"),
    );

    fs::remove_dir_all(root).expect("persistent store fixture should be removed");
}

#[test]
fn persistent_app_store_reports_database_path_when_open_fails() {
    let root = temp_test_root("cindx-persistent-store-failure");
    let database = root.join("state.sqlite3");
    fs::create_dir_all(&database).expect("database-path directory fixture should exist");

    let error = match open_app_store_at(&database) {
        Ok(_) => panic!("a directory must not be accepted as a persistent database"),
        Err(error) => error,
    };

    assert!(error
        .message
        .contains("failed to open Cindx state database"));
    assert!(error.message.contains(&database.display().to_string()));

    fs::remove_dir_all(root).expect("persistent store failure fixture should be removed");
}
