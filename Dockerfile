# Build context is the parent source-graph directory used by the publish
# workflow. Cargo.lock independently pins both cross-repository Rust inputs:
#
#   docker build -f zed-web-server.rs/Dockerfile -t ghcr.io/zed-pkg/zed-web-server:dev .
#
# The toolchain must satisfy `edition = "2024"` (>= 1.85) and the shared
# workspace dependencies' MSRV, so the base is pinned to 1.97.1.
# RUSTUP_TOOLCHAIN overrides the repo's floating rust-toolchain.toml channel so
# the build uses the toolchain already present in the image.
# `-bookworm` keeps the build glibc compatible with the Debian 12 runtime stage.
FROM rust:1.97-slim-bookworm AS build
ENV RUSTUP_TOOLCHAIN=1.97.1
WORKDIR /work
COPY zed-web-server.rs ./zed-web-server.rs
WORKDIR /work/zed-web-server.rs
RUN cargo build --release --locked

FROM debian:12-slim
ARG ZED_WEB_REVISION=unknown
ARG ZED_INTERFACES_REVISION=unknown
ARG ZED_LIB_CORE_REVISION=unknown
LABEL org.opencontainers.image.title="Zed registry web" \
      org.opencontainers.image.description="Read-only Zed package registry web interface" \
      org.opencontainers.image.source="https://github.com/zed-pkg/zed-web-server.rs" \
      org.opencontainers.image.revision="$ZED_WEB_REVISION" \
      org.opencontainers.image.licenses="MIT" \
      io.zpkg.interfaces.revision="$ZED_INTERFACES_REVISION" \
      io.zpkg.core.revision="$ZED_LIB_CORE_REVISION"
RUN useradd --system --uid 10001 zed \
    && apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /work/zed-web-server.rs/target/release/zed-web-server /usr/local/bin/zed-web-server
COPY --from=build /work/zed-web-server.rs/static ./static
USER zed
ENV BIND_ADDR=0.0.0.0:8081
EXPOSE 8081
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \

# --- sops: decrypt at `docker run`, never at `docker build` ------------------
# The image carries only CIPHERTEXT (env/enc/<SOPS_ENV>.env.enc) and the sops
# binary. The age key arrives at run time (SOPS_AGE_KEY / SOPS_AGE_KEY_FILE);
# scripts/sops-entrypoint.sh decrypts into the process environment and execs
# the real command, so no plaintext ever lands in a layer or on disk.
# See env/README.md.
ARG SOPS_ENV=prod
COPY --chmod=0755 --from=ghcr.io/getsops/sops:v3.10.2-alpine /usr/local/bin/sops /usr/local/bin/sops
COPY --chmod=0755 scripts/sops-entrypoint.sh /usr/local/bin/sops-entrypoint.sh
COPY --chmod=0644 env/enc/${SOPS_ENV}.env.enc /app/secrets/app.env
ENV SOPS_SECRETS_FILE=/app/secrets/app.env

    CMD curl -fsS http://127.0.0.1:8081/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/sops-entrypoint.sh", "/usr/local/bin/zed-web-server"]
