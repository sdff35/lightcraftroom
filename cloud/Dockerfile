# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM rust:1.95.0-slim-bookworm AS build
ARG TARGETARCH
ARG CARGO_BUILD_JOBS=2
WORKDIR /build
COPY . .
RUN --mount=type=secret,id=proxy_ca \
    set -eu; \
    if [ -f /run/secrets/proxy_ca ]; then export SSL_CERT_FILE=/run/secrets/proxy_ca CARGO_HTTP_CAINFO=/run/secrets/proxy_ca; fi; \
    case "$TARGETARCH" in amd64) target=x86_64-unknown-linux-musl;; arm64) target=aarch64-unknown-linux-musl;; *) exit 1;; esac; \
    rustup target add "$target"; \
    export PATH="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin:$PATH"; \
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld; \
    cargo build --locked --release --target "$target"; \
    cp "target/$target/release/lightcraft-cloud" /lightcraft-cloud
FROM scratch
COPY --from=build --chmod=0555 /lightcraft-cloud /lightcraft-cloud
COPY --chown=10001:10001 docker/runtime/ /data/
USER 10001:10001
ENV LC_STORAGE_ROOT=/data TMPDIR=/tmp
EXPOSE 8080
ENTRYPOINT ["/lightcraft-cloud"]
