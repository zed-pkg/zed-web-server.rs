#!/bin/sh
set -eu

target="${ZED_PKG_TEST_TARGET:?ZED_PKG_TEST_TARGET is required}"

test -f "$target/src/routes/console.rs"
test -f "$target/src/session.rs"
