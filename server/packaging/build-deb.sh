#!/bin/sh
# Build an OpSkills .deb from a prebuilt static binary.
# Usage: build-deb.sh <version> <amd64|arm64> <path/to/opskills> <output-dir>
set -eu

VERSION=$1
ARCH=$2
BINARY=$3
OUT=$4
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

install -D -m 0755 "$BINARY" "$ROOT/usr/bin/opskills"
install -D -m 0640 "$HERE/opskills.toml" "$ROOT/etc/opskills/opskills.toml"
install -D -m 0644 "$HERE/opskills.service" "$ROOT/usr/lib/systemd/system/opskills.service"
install -D -m 0644 "$REPO/LICENSE" "$ROOT/usr/share/doc/opskills/copyright"

mkdir -p "$ROOT/DEBIAN"
for script in postinst prerm postrm; do
    install -m 0755 "$HERE/debian/$script" "$ROOT/DEBIAN/$script"
done
echo "/etc/opskills/opskills.toml" > "$ROOT/DEBIAN/conffiles"

SIZE=$(du -sk "$ROOT/usr" "$ROOT/etc" | awk '{s += $1} END {print s}')
cat > "$ROOT/DEBIAN/control" <<CONTROL
Package: opskills
Version: $VERSION
Architecture: $ARCH
Maintainer: Thomas Kim <admin@megahash.net>
Installed-Size: $SIZE
Depends: adduser
Section: admin
Priority: optional
Homepage: https://opskills.app
Description: Runbooks your whole team can run
 OpSkills turns the procedures your team repeats into safe, self-service
 runbooks with live output, approvals and a full audit trail. This package
 installs the opskills server as a sandboxed systemd service.
CONTROL

mkdir -p "$OUT"
dpkg-deb --root-owner-group -Zxz --build "$ROOT" "$OUT/opskills_${VERSION}_${ARCH}.deb" >/dev/null
echo "$OUT/opskills_${VERSION}_${ARCH}.deb"
