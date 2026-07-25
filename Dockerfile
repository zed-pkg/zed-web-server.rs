# Build context must be the PARENT directory (side-by-side checkout) because
# of the ../zed-interfaces path dependency:
#
#   docker build -f zed-web-server.rs/Dockerfile -t ghcr.io/zed-pkg/zed-web-server:dev .
# Toolchain must satisfy `edition = "2024"` (>= 1.85) and the shared workspace
# deps' MSRV, so the base is pinned to 1.97.1. RUSTUP_TOOLCHAIN (the base image's
# exact version) overrides the repo's rust-toolchain.toml (channel = "stable"),
# so the build uses the installed toolchain and never downloads one at build
# time — reproducible, no build-time CDN dependency.
# -bookworm (not the default trixie) so the build glibc matches the
# debian:12-slim (bookworm) runtime stage below.
FROM rust:1.97-slim-bookworm AS build
ENV RUSTUP_TOOLCHAIN=1.97.1
WORKDIR /work
COPY zed-interfaces ./zed-interfaces
COPY zed-web-server.rs ./zed-web-server.rs
WORKDIR /work/zed-web-server.rs
RUN cargo build --release --locked

FROM debian:12-slim
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
