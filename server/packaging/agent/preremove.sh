#!/bin/sh
# deb: $1 = remove or upgrade; rpm: $1 = 0 on erase, 1 on upgrade.
set -e
case "$1" in
    remove | 0)
        if [ -d /run/systemd/system ]; then
            systemctl disable --now opskills-agent.service >/dev/null 2>&1 || true
        fi
        ;;
esac
exit 0
