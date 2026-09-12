# Agent instructions

## Scope and hierarchy

- These instructions apply to the whole `zed-pkg/zed-web-server.rs` repository unless a deeper lowercase `agents.md` adds narrower rules.
- Before editing, resolve the current working directory and load every readable ancestor `agents.md` from the filesystem root to the working directory. Do not search siblings. Resolve symlinks, deduplicate resolved files, and report unreadable or cyclic instruction files.
- `.claude/CLAUDE.md`, `.gemini/GEMINI.md`, and `.openai/AGENTS.md` are pointers only. Never duplicate instructions in tool-specific files.

## Repository role

This Rust service provides the Zed registry web experience and server-rendered UI. It coordinates presentation, authenticated browser flows, API integration, accessibility, and deployment-safe web behavior without redefining registry contracts.

## Working rules

- Reuse `zed-interfaces` and API-server contracts; do not create divergent manifest, registry, authentication, or error models in the UI layer.
- Keep server-rendered HTML semantic, accessible, progressively enhanced, and usable without unnecessary client-side state.
- Preserve CSRF, session, redirect, cookie, and authorization boundaries. Fail closed for administrative and publish-related actions.
- Escape untrusted content and validate uploads, URLs, and user-controlled metadata at the appropriate boundary.
- Keep cache headers, health/readiness behavior, and observability intentional and free of secrets or personal data.
- Prefer the repository's established Rust/HTML/HTMX and ORM patterns over parallel framework or raw-SQL implementations.
- Never commit tokens, session secrets, database URLs, cloud credentials, or production environment files.
- Exercise focused rendering, route, security, integration, formatting, compilation, Clippy, and container checks relevant to the change.

## Validation

The pinned `agents policy` workflow validates this hierarchy and the three tool pointers. Follow `README.md` and existing CI for service-specific validation before requesting review.

## Repository-local Git worktrees

- Create or use a Git worktree only when the human operator explicitly authorizes it for the current task. Concurrency or a dirty checkout is not permission by itself.
- Put every authorized worktree at `<repository-root>/tmp/worktrees/<name>`; from the repository root, use `./tmp/worktrees/<name>`. Never place worktrees beside repositories or organization directories.
- Keep `tmp`, `temp`, `tmp/worktrees`, and `temp/worktrees` ignored in the repository-root `.gitignore`. Do not commit files from those directories.
- Relocate or remove a worktree only when the operator explicitly requests it. Before removal, preserve and publish intended changes, verify its commit is represented on the target branch, and confirm there are no tracked, untracked, ignored-sensitive, or in-use files that must survive. Remove it with `git worktree remove <path>` without `--force`; never delete a worktree directory with `rm`.
