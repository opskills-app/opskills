#!/bin/sh
# deb: $1 = configure; rpm: $1 = 1 on install, 2 on upgrade.
set -e
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    # On upgrade, restart a running agent on the new version.
    systemctl try-restart opskills-agent.service >/dev/null 2>&1 || true
fi
if [ ! -f /etc/opskills-agent/agent.toml ]; then
    echo "OpSkills agent installed. Enroll this host with the command from the Hosts page:"
    echo "  sudo opskills-agent enroll --server https://YOUR-OPSKILLS-SERVER --token TOKEN"
    echo "  sudo systemctl enable --now opskills-agent"
fi
exit 0
