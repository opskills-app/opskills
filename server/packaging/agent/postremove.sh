#!/bin/sh
# deb: $1 = remove, purge or upgrade; rpm: $1 = 0 on erase, 1 on upgrade.
set -e
case "$1" in
    purge)
        # Forget the enrollment and any leftover step folders.
        rm -rf /etc/opskills-agent /var/lib/opskills-agent
        ;;
    remove | 0)
        rm -rf /var/lib/opskills-agent
        ;;
esac
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
fi
exit 0
