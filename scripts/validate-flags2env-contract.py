#!/usr/bin/env python3
"""Validate the repository-owned flags2env contract without third-party Python deps."""

from __future__ import annotations

import argparse
import copy
import pathlib
import sys
import tomllib
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parent.parent
CONTRACT = ROOT / ".cli-flags.toml"
CANONICAL_TYPES = frozenset({"string", "bool", "integer", "double", "json", "array", "map"})
SECRET_ENVS = frozenset(
    {
        "DATABASE_URL",
        "SHARED_AUTH_HANDOFF_CLIENT_SECRET",
        "SHARED_AUTH_SERVICE_CREDENTIAL",
        "ZED_SESSION_SIGNING_SECRET",
    }
)
EXPECTED_PUBLIC_ENVS = frozenset(
    {
        "BIND_ADDR",
        "RUST_LOG",
        "DB_MAX_CONNECTIONS",
        "DB_STATEMENT_TIMEOUT_MS",
        "DB_CONNECT_MAX_WAIT_SECS",
        "PUBLIC_BASE_URL",
        "SHARED_AUTH_URL",
        "SHARED_AUTH_PUBLIC_URL",
        "ZED_API_URL",
        "SHARED_AUTH_HANDOFF_CLIENT_ID",
        "SHARED_AUTH_DELEGATE_CLIENT_ID",
        "SHARED_AUTH_AUDIENCE",
        "SHARED_AUTH_SCOPES",
    }
)


def load_contract(path: pathlib.Path = CONTRACT) -> dict[str, Any]:
    with path.open("rb") as source:
        value = tomllib.load(source)
    if not isinstance(value, dict):
        raise ValueError("contract root must be a TOML table")
    return value


def default_matches(flag_type: str, value: Any) -> bool:
    if flag_type == "bool":
        return isinstance(value, bool)
    if flag_type == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if flag_type == "double":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if flag_type == "string":
        return isinstance(value, str)
    return True


def validate(contract: dict[str, Any]) -> list[str]:
    errors: list[str] = []

    if "identity" in contract:
        errors.append("unsupported [identity] table must not be present")

    parse = contract.get("parse")
    if not isinstance(parse, dict) or parse.get("allow_unknown") is not False:
        errors.append("[parse].allow_unknown must be false")

    env = contract.get("env")
    if not isinstance(env, dict):
        errors.append("missing [env] table")
        env = {}
    if env.get("files") != []:
        errors.append("[env].files must be [] so caller working-directory dotenv is disabled")
    ignored = env.get("ignore", [])
    if not isinstance(ignored, list) or any(not isinstance(value, str) for value in ignored):
        errors.append("[env].ignore must be an array of strings")
        ignored_set: set[str] = set()
    else:
        ignored_set = set(ignored)
    missing_secret_ignores = sorted(SECRET_ENVS - ignored_set)
    if missing_secret_ignores:
        errors.append("secret envs missing from [env].ignore: " + ", ".join(missing_secret_ignores))

    flags = contract.get("flags")
    if not isinstance(flags, dict) or not flags:
        errors.append("missing nonempty [flags] table")
        return errors

    seen_envs: dict[str, str] = {}
    seen_aliases: dict[str, str] = {}
    seen_shorts: dict[str, str] = {}
    public_envs: set[str] = set()

    for name, raw_flag in sorted(flags.items()):
        if not isinstance(raw_flag, dict):
            errors.append(f"flags.{name} must be a table")
            continue
        if "long" in raw_flag or "switch" in raw_flag:
            errors.append(f"flags.{name} must use aliases, not legacy long/switch properties")

        env_name = raw_flag.get("env")
        if not isinstance(env_name, str) or not env_name.strip():
            errors.append(f"flags.{name}.env must be a nonblank string")
        else:
            if env_name in SECRET_ENVS:
                errors.append(f"secret env {env_name} must never be exposed as a CLI flag")
            previous = seen_envs.setdefault(env_name, name)
            if previous != name:
                errors.append(f"duplicate env {env_name} in flags.{previous} and flags.{name}")
            public_envs.add(env_name)

        aliases = raw_flag.get("aliases")
        if not isinstance(aliases, list) or not aliases or any(
            not isinstance(alias, str) or not alias.strip() for alias in aliases
        ):
            errors.append(f"flags.{name}.aliases must contain at least one nonblank string")
        else:
            if name not in aliases:
                errors.append(f"flags.{name}.aliases must include canonical public spelling {name!r}")
            for alias in aliases:
                previous = seen_aliases.setdefault(alias, name)
                if previous != name:
                    errors.append(f"duplicate alias {alias!r} in flags.{previous} and flags.{name}")

        short = raw_flag.get("short")
        if short is not None:
            if not isinstance(short, str) or len(short) != 1:
                errors.append(f"flags.{name}.short must be one character")
            else:
                previous = seen_shorts.setdefault(short, name)
                if previous != name:
                    errors.append(f"duplicate short {short!r} in flags.{previous} and flags.{name}")

        flag_type = raw_flag.get("type")
        if flag_type not in CANONICAL_TYPES:
            errors.append(f"flags.{name}.type must use canonical flags2env vocabulary")
        elif "default" in raw_flag and not default_matches(flag_type, raw_flag["default"]):
            errors.append(f"flags.{name}.default does not match type {flag_type}")

    missing_public = sorted(EXPECTED_PUBLIC_ENVS - public_envs)
    if missing_public:
        errors.append("current web runtime envs missing from flags contract: " + ", ".join(missing_public))

    return errors


def assert_rejected(contract: dict[str, Any], expected_fragment: str) -> None:
    errors = validate(contract)
    if not any(expected_fragment in error for error in errors):
        raise AssertionError(f"expected rejection containing {expected_fragment!r}; got {errors!r}")


def self_test() -> None:
    baseline = load_contract()
    if errors := validate(baseline):
        raise AssertionError(f"baseline contract is invalid: {errors!r}")

    ambient_dotenv = copy.deepcopy(baseline)
    ambient_dotenv["env"]["files"] = [".env"]
    assert_rejected(ambient_dotenv, "working-directory dotenv")

    legacy_property = copy.deepcopy(baseline)
    legacy_property["flags"]["bind-addr"]["long"] = "bind-addr"
    assert_rejected(legacy_property, "legacy long/switch")

    secret_flag = copy.deepcopy(baseline)
    secret_flag["flags"]["database-url"] = {
        "env": "DATABASE_URL",
        "aliases": ["database-url"],
        "type": "string",
    }
    assert_rejected(secret_flag, "must never be exposed as a CLI flag")

    loose_parse = copy.deepcopy(baseline)
    loose_parse["parse"]["allow_unknown"] = True
    assert_rejected(loose_parse, "allow_unknown must be false")

    noncanonical_type = copy.deepcopy(baseline)
    noncanonical_type["flags"]["help"]["type"] = "boolean"
    assert_rejected(noncanonical_type, "canonical flags2env vocabulary")

    print("flags2env contract validator self-test passed")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    try:
        if args.self_test:
            self_test()
            return
        contract = load_contract()
    except (OSError, UnicodeError, tomllib.TOMLDecodeError, ValueError, AssertionError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error

    errors = validate(contract)
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
    print("zed-web-server flags2env contract validated")


if __name__ == "__main__":
    main()
