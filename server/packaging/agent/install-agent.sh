#!/bin/sh
# Install the OpSkills agent on this host, enroll it and start it.
#
#   curl -fsSL https://OPSKILLS/install-agent.sh | sudo sh -s -- --server https://OPSKILLS --token TOKEN [--name NAME] [--tags web,prod]
#
# Picks the .deb or .rpm package for this system and architecture, downloads it from
# the OpSkills server, installs it, enrolls the host and starts the opskills-agent
# service. The token can also come from OPSKILLS_ENROLL_TOKEN. Running it again on an
# enrolled host upgrades the agent and keeps the enrollment.
set -eu

VERSION="@VERSION@"
SERVER=""
TOKEN="${OPSKILLS_ENROLL_TOKEN:-}"
NAME=""
TAGS=""

die() {
    echo "install-agent: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --server) SERVER=${2:-}; shift 2 ;;
        --token) TOKEN=${2:-}; shift 2 ;;
        --name) NAME=${2:-}; shift 2 ;;
        --tags) TAGS=${2:-}; shift 2 ;;
        *) die "unknown option $1 (use --server, --token, --name, --tags)" ;;
    esac
done

[ -n "$SERVER" ] || die "--server is required, for example --server https://opskills.example.com"
SERVER=${SERVER%/}
[ "$(id -u)" = 0 ] || die "run it as root, for example with sudo"

case "$(uname -m)" in
    x86_64 | amd64) DEB_ARCH=amd64; RPM_ARCH=x86_64 ;;
    aarch64 | arm64) DEB_ARCH=arm64; RPM_ARCH=aarch64 ;;
    *) die "unsupported architecture $(uname -m); the agent is built for x86_64 and aarch64" ;;
esac

if command -v apt-get >/dev/null 2>&1; then
    FILE="opskills-agent_${VERSION}_${DEB_ARCH}.deb"
elif command -v dnf >/dev/null 2>&1 || command -v yum >/dev/null 2>&1; then
    FILE="opskills-agent-${VERSION}-1.${RPM_ARCH}.rpm"
else
    die "found neither apt-get nor dnf/yum; install the package by hand"
fi

CURRENT=$(opskills-agent --version 2>/dev/null | awk '{print $2}' || true)
if [ "$CURRENT" = "$VERSION" ]; then
    echo "opskills-agent $VERSION is already installed."
else
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
chmod 0755 "$TMP"

echo "Downloading $FILE from $SERVER"
if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$TMP/$FILE" "$SERVER/downloads/$FILE" || die "download failed: $SERVER/downloads/$FILE"
elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$TMP/$FILE" "$SERVER/downloads/$FILE" || die "download failed: $SERVER/downloads/$FILE"
else
    die "needs curl or wget"
fi

echo "Installing $FILE"
case "$FILE" in
    *.deb) DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$TMP/$FILE" >/dev/null ;;
    *.rpm)
        if command -v dnf >/dev/null 2>&1; then
            dnf install -y -q "$TMP/$FILE" >/dev/null
        else
            yum install -y -q "$TMP/$FILE" >/dev/null
        fi
        ;;
esac
if [ -n "$CURRENT" ]; then
    echo "Upgraded opskills-agent from $CURRENT to $VERSION."
else
    echo "Installed opskills-agent $VERSION."
fi
fi

if [ -f /etc/opskills-agent/agent.toml ] && opskills-agent status >/dev/null 2>&1; then
    echo "Already enrolled; keeping the current enrollment."
else
    [ -n "$TOKEN" ] || die "--token is required to enroll this host (create one on the Hosts page)"
    set -- enroll --server "$SERVER"
    [ -z "$NAME" ] || set -- "$@" --name "$NAME"
    [ -z "$TAGS" ] || set -- "$@" --tags "$TAGS"
    OPSKILLS_ENROLL_TOKEN="$TOKEN" opskills-agent "$@"
fi

if [ -d /run/systemd/system ]; then
    systemctl enable --now opskills-agent >/dev/null 2>&1
    systemctl restart opskills-agent
    echo "The opskills-agent service is running."
else
    echo "No systemd: start the agent with: opskills-agent run"
fi
