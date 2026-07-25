# Build context must be the PARENT directory (side-by-side checkout) because
# of the ../zed-interfaces path dependency:
#
#   docker build -f zed-web-server.rs/Dockerfile -t ghcr.io/zed-pkg/zed-web-server:dev .
# rust >= 1.85 for the crate's `edition = "2024"`. RUSTUP_TOOLCHAIN pins the
# Docker build to the base image's toolchain and overrides the repo's
# rust-toolchain.toml (channel = "stable"), so the build never downloads a
# floating stable toolchain — reproducible, no build-time CDN dependency.
FROM rust:1.90-slim AS build
ENV RUSTUP_TOOLCHAIN=1.90.0
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
