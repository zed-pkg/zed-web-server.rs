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
    CMD curl -fsS http://127.0.0.1:8081/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/zed-web-server"]
