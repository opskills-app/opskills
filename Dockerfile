# syntax=docker/dockerfile:1
#
# Build everything from source:
#   Image (both architectures):  docker buildx build --platform linux/amd64,linux/arm64 -t opskills .
#   Debian packages:             docker buildx build --target debs --output type=local,dest=dist .

ARG RUST_VERSION=1.100
ARG NODE_VERSION=24

# 1. Web app
FROM --platform=$BUILDPLATFORM node:${NODE_VERSION}-slim AS ui
WORKDIR /src/ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY ui/ ./
RUN npm run build

# 2. Static binaries for amd64 and arm64, cross-compiled once on the build machine
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim AS build
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
WORKDIR /src/server
COPY server/Cargo.toml server/Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo fetch --locked && rm -r src
COPY server/ ./
COPY --from=ui /src/ui/dist /src/ui/dist
# Link the static musl binaries with Rust's bundled lld; no C cross toolchain needed.
# Scoped per target so build-time helpers (proc macros) still build for the host.
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C linker-flavor=ld.lld -C target-feature=+crt-static" \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C linker-flavor=ld.lld -C target-feature=+crt-static"
RUN cargo test --locked --release --quiet \
 && for arch in amd64:x86_64 arm64:aarch64; do \
      cargo build --locked --release --target "${arch#*:}-unknown-linux-musl" \
      && install -D "target/${arch#*:}-unknown-linux-musl/release/opskills" "/out/${arch%%:*}/opskills"; \
    done

# 3. Debian packages
FROM --platform=$BUILDPLATFORM build AS deb-build
COPY LICENSE /src/LICENSE
RUN VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1) \
 && for arch in amd64 arm64; do packaging/build-deb.sh "$VERSION" "$arch" "/out/$arch/opskills" /debs; done

FROM scratch AS debs
COPY --from=deb-build /debs/ /

# 4. Runtime image: Alpine provides /bin/sh and BusyBox tools for runbook steps
FROM alpine:3.22
ARG TARGETARCH
LABEL org.opencontainers.image.title="OpSkills" \
      org.opencontainers.image.description="Runbooks your whole team can run" \
      org.opencontainers.image.url="https://opskills.app" \
      org.opencontainers.image.source="https://github.com/opskills-app/opskills" \
      org.opencontainers.image.licenses="Apache-2.0"
RUN addgroup -S -g 65532 opskills \
 && adduser -S -D -H -u 65532 -G opskills -h /var/lib/opskills -s /sbin/nologin opskills \
 && install -d -o opskills -g opskills -m 0750 /var/lib/opskills
COPY --from=build /out/${TARGETARCH}/opskills /usr/bin/opskills
USER 65532:65532
EXPOSE 8080
VOLUME ["/var/lib/opskills"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/usr/bin/opskills", "healthcheck"]
ENTRYPOINT ["/usr/bin/opskills"]
CMD ["serve"]
