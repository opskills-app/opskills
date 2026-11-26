#!/bin/sh
# Build the agent .deb and .rpm for one architecture.
# Usage: build-agent-packages.sh <version> <amd64|arm64> <path/to/opskills> <output-dir>
set -eu

VERSION=$1
ARCH=$2
BINARY=$3
OUT=$4
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

cp "$HERE/nfpm.yaml" "$HERE/opskills-agent.service" "$HERE"/*.sh "$STAGE/"
install -m 0755 "$BINARY" "$STAGE/opskills-agent"
install -m 0644 "$REPO/LICENSE" "$STAGE/LICENSE"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

cd "$STAGE"
for packager in deb rpm; do
    VERSION=$VERSION ARCH=$ARCH nfpm package --config nfpm.yaml --packager "$packager" --target "$OUT/" >/dev/null
done
ls "$OUT"/opskills-agent*"$VERSION"*
