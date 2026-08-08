//! Static regression checks for the browser-facing database boundary.

use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    fs::read_to_string(root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

#[test]
fn package_and_docs_pin_the_read_only_shared_library_contract() {
    let zpkg = read(".zpkg.toml");
    assert!(zpkg.contains("\"zed-pkg/zed-lib\" = \"^0.1.0\""));
    assert!(zpkg.contains("dir = \".vendor/.zed\""));

    let boundary = read("docs/database-boundary.md");
    for contract in [
        "DbRole::ReadOnly",
        "assert_read_only",
        "queries::read",
        "default_transaction_read_only=on",
        "zed_pkg__web_ro",
        "sole request-serving writer",
    ] {
        assert!(
            boundary.contains(contract),
            "database boundary lost {contract}"
        );
    }
}

#[test]
fn pool_enforces_read_only_before_entering_application_state() {
    let server = read("src/server.rs");
    for contract in [
        "default_transaction_read_only",
        "current_setting('default_transaction_read_only')",
        "read_only != \"on\"",
        "serving in offline mode",
    ] {
        assert!(server.contains(contract), "server lost {contract}");
    }

    let manifest = read("Cargo.toml");
    assert!(manifest.contains("sea-orm ="));
    assert!(
        !manifest
            .lines()
            .any(|line| line.trim_start().starts_with("sqlx =")),
        "the web tier must not add an independent direct SQL dependency"
    );
}

#[test]
fn web_repository_contains_no_migration_crate_or_write_role() {
    let manifest = read("Cargo.toml");
    for forbidden in ["sea-orm-migration", "migration =", "DbRole::ReadWrite"] {
        assert!(
            !manifest.contains(forbidden),
            "web manifest crossed the database boundary with {forbidden}"
        );
    }
}
