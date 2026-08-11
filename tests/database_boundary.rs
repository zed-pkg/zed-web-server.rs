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
fn package_manifest_and_lock_pin_the_cross_repository_contracts() {
    let zpkg = read(".zpkg.toml");
    assert!(zpkg.contains("\"zed-pkg/zed-lib-core\" = \"^0.1.0\""));
    assert!(zpkg.contains("dir = \".vendor/.zed\""));
    assert!(
        !zpkg.contains("\"zed-pkg/zed-orm-core\""),
        "package metadata must follow the repository that supplies the locked Cargo source"
    );

    let manifest = read("Cargo.toml");
    let lock = read("Cargo.lock");
    for (repository, revision) in [
        (
            "https://github.com/zed-pkg/zed-interfaces.git",
            "b795c92126202cd1e4bd2365eb58d8cefa2095c3",
        ),
        (
            "https://github.com/zed-pkg/zed-lib-core.git",
            "fc4f2677a2315d522913bef05a1c0d23f8208960",
        ),
    ] {
        assert!(
            manifest.contains(repository),
            "Cargo manifest lost {repository}"
        );
        assert!(
            manifest.contains(&format!("rev = \"{revision}\"")),
            "Cargo manifest lost {revision}"
        );
        assert!(
            lock.contains(&format!("git+{repository}?rev={revision}#{revision}")),
            "Cargo lock lost {repository}@{revision}"
        );
    }

    let boundary = read("docs/database-boundary.md");
    for contract in [
        "zed-orm-core",
        "zed-lib-core",
        "read-only",
        "opaque `ReadContext`",
        "named policy-aware functions",
        "default_transaction_read_only=on",
        "zed_pkg__web_ro",
        "sole request-serving",
        "writer and the owner",
    ] {
        assert!(
            boundary.contains(contract),
            "database boundary lost {contract}"
        );
    }
}

#[test]
fn pool_creation_stays_inside_the_opaque_read_only_boundary() {
    let server = read("src/server.rs");
    for contract in [
        "ConnectPolicy::default()",
        "connect_read_only_with_policy",
        "Result<ReadContext>",
        "serving in offline mode",
    ] {
        assert!(server.contains(contract), "server lost {contract}");
    }
    for forbidden in [
        "PgPoolOptions::new",
        "SqlxPostgresConnector::from",
        "use sea_orm::",
        "use sqlx::",
        "sea_orm::sqlx::",
    ] {
        assert!(
            !server.contains(forbidden),
            "web server recreated the ORM boundary with {forbidden}"
        );
    }

    let manifest = read("Cargo.toml");
    for forbidden_prefix in ["sea-orm =", "sqlx ="] {
        assert!(
            !manifest
                .lines()
                .any(|line| line.trim_start().starts_with(forbidden_prefix)),
            "the web tier added an independent {forbidden_prefix} dependency"
        );
    }
}

#[test]
fn kubernetes_uses_the_dedicated_read_only_secret() {
    let deployment = read("k8s/base/deployment.yaml");
    for contract in [
        "name: dd-zed-web-secrets",
        "key: ZED_WEB_DATABASE_URL",
        "dedicated SELECT-only web principal",
    ] {
        assert!(deployment.contains(contract), "deployment lost {contract}");
    }
    assert!(
        !deployment.contains("postgres://zed@"),
        "the web Deployment must not embed the API bootstrap DSN"
    );

    let external_secret = read("k8s/externalsecret.yaml");
    assert!(external_secret.contains("secretKey: ZED_WEB_DATABASE_URL"));
    assert!(external_secret.contains("never contain the API or migrator credential"));

    let kustomization = read("k8s/kustomization.yaml");
    assert!(kustomization.contains("externalsecret.yaml"));
}

#[test]
fn web_repository_contains_no_migration_crate_or_write_role() {
    let manifest = read("Cargo.toml");
    for forbidden in [
        "sea-orm-migration",
        "features = [\"read-write\"]",
        "features = [\"migrate\"]",
        "DbRole::ReadWrite",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "web manifest crossed the database boundary with {forbidden}"
        );
    }
}
