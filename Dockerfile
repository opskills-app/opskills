# syntax=docker/dockerfile:1
#
# Build everything from source:
#   Image (both architectures):  docker buildx build --platform linux/amd64,linux/arm64 -t opskills .
#   Packages (server .deb, agent .deb and .rpm):
#                                docker buildx build --target packages --output type=local,dest=dist .

ARG RUST_VERSION=1.100
ARG NODE_VERSION=24

# 1. Web app
FROM --platform=$BUILDPLATFORM node:${NODE_VERSION}-slim AS ui
WORKDIR /src/ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY ui/ ./
RUN npm run build

# 2. Static binaries for amd64 and arm64, cross-compiled once on the build machine.
#    Zig is the C compiler and linker for the musl targets (needed by the TLS library).
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim AS build
ARG BUILDARCH
ARG ZIG_VERSION=0.17.1
ARG CARGO_ZIGBUILD_VERSION=0.24.1
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends curl ca-certificates xz-utils >/dev/null && rm -rf /var/lib/apt/lists/*
RUN case "$BUILDARCH" in \
      amd64) arch=x86_64;  sum=3581863e3cbcfd0fce0efc682ce7e9e93da96668a977791d356a26d2a7e9b17a ;; \
      arm64) arch=aarch64; sum=a1546d5646eb184437918d6635ca6b762a0e889eac09cef720ee7a1ed3fe7795 ;; \
      *) echo "unsupported build platform $BUILDARCH" >&2; exit 1 ;; \
    esac \
 && curl -fsSL -o /tmp/zig.tar.xz "https://ziglang.org/download/${ZIG_VERSION}/zig-${arch}-linux-${ZIG_VERSION}.tar.xz" \
 && echo "${sum}  /tmp/zig.tar.xz" | sha256sum -c - \
 && mkdir -p /opt/zig && tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1 && rm /tmp/zig.tar.xz
ENV PATH=/opt/zig:$PATH
RUN cargo install cargo-zigbuild --version ${CARGO_ZIGBUILD_VERSION} --locked --quiet
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
WORKDIR /src/server
COPY server/Cargo.toml server/Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo fetch --locked && rm -r src
COPY server/ ./
COPY --from=ui /src/ui/dist /src/ui/dist
RUN cargo test --locked --release --quiet \
 && for arch in amd64:x86_64 arm64:aarch64; do \
      cargo zigbuild --locked --release --target "${arch#*:}-unknown-linux-musl" \
      && install -D "target/${arch#*:}-unknown-linux-musl/release/opskills" "/out/${arch%%:*}/opskills"; \
    done

# 3. Packages: the server .deb, and the agent .deb and .rpm, for both architectures
FROM --platform=$BUILDPLATFORM build AS package-build
ARG BUILDARCH
ARG NFPM_VERSION=2.49.0
RUN case "$BUILDARCH" in \
      amd64) arch=x86_64; sum=2afaa5ffb5e3493402e31e7c67a5a1fb42133e3f66fc315ca0051934125561c7 ;; \
      arm64) arch=arm64;  sum=d4eee71839e92c68cbd3b101640bad32d2856fc551acae0635be9118e48c9d9c ;; \
      *) echo "unsupported build platform $BUILDARCH" >&2; exit 1 ;; \
    esac \
 && curl -fsSL -o /tmp/nfpm.tar.gz "https://github.com/goreleaser/nfpm/releases/download/v${NFPM_VERSION}/nfpm_${NFPM_VERSION}_Linux_${arch}.tar.gz" \
 && echo "${sum}  /tmp/nfpm.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/nfpm.tar.gz -C /usr/local/bin nfpm && rm /tmp/nfpm.tar.gz
COPY LICENSE /src/LICENSE
RUN VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1) \
 && for arch in amd64 arm64; do \
      packaging/build-deb.sh "$VERSION" "$arch" "/out/$arch/opskills" /packages \
      && packaging/agent/build-agent-packages.sh "$VERSION" "$arch" "/out/$arch/opskills" /packages; \
    done

FROM scratch AS packages
COPY --from=package-build /packages/ /

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
# Agent packages for the Hosts page to offer for download.
COPY --from=package-build /packages/opskills-agent* /usr/share/opskills/packages/
USER 65532:65532
EXPOSE 8080
VOLUME ["/var/lib/opskills"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/usr/bin/opskills", "healthcheck"]
ENTRYPOINT ["/usr/bin/opskills"]
CMD ["serve"]
