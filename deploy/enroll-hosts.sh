#!/bin/sh
# Enroll several hosts over SSH, with a fresh one-time token for each.
set -eu
OPSKILLS=https://opskills.example.com   # the address hosts use to reach OpSkills
HOSTS="web-1 web-2 db-1"                # SSH names of the hosts
TAGS=prod                               # tags for all of them (optional)

printf 'OpSkills admin password: '
stty -echo; read -r PW; stty echo; echo
JAR=$(mktemp)
trap 'rm -f "$JAR"' EXIT

# Sign in once. The session cookie goes into $JAR.
printf '{"username":"admin","password":"%s"}' "$PW" |
    curl -fsS -c "$JAR" -o /dev/null -H 'X-OpSkills-Request: 1' \
        -H 'Content-Type: application/json' --data-binary @- "$OPSKILLS/api/v1/auth/login"

for host in $HOSTS; do
    token=$(curl -fsS -b "$JAR" -X POST -H 'X-OpSkills-Request: 1' \
        "$OPSKILLS/api/v1/hosts/enrollment-tokens" | sed -E 's/.*"token":"([^"]+)".*/\1/')
    case "$token" in
        *[!0-9a-f]* | "") echo "Could not create an enrollment token. Check the address and password." >&2; exit 1 ;;
    esac
    echo "== $host"
    ssh "$host" "curl -fsSL $OPSKILLS/install-agent.sh | sudo sh -s -- --server $OPSKILLS --token $token --name $host --tags $TAGS"
done
