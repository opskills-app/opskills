#!/bin/sh
# Enroll several hosts over SSH, with a fresh one-time token for each.
set -eu
OPSKILLS=https://opskills.example.com   # the address hosts use to reach OpSkills
HOSTS="web-1 web-2 db-1"                # SSH names of the hosts
TAGS=prod                               # tags for all of them (optional)

# Enrolling hosts needs the Admin role. Use an admin's API token from Settings > API
# tokens if OPSKILLS_TOKEN is set; otherwise sign in as an admin (OPSKILLS_USER,
# default admin), keeping the session cookie in a temporary file.
JAR=$(mktemp)
trap 'rm -f "$JAR"' EXIT
if [ -n "${OPSKILLS_TOKEN:-}" ]; then
    new_token() {
        curl -fsS -X POST -H "Authorization: Bearer $OPSKILLS_TOKEN" "$OPSKILLS/api/v1/hosts/enrollment-tokens"
    }
else
    USER_NAME=${OPSKILLS_USER:-admin}
    printf 'OpSkills password for %s: ' "$USER_NAME"
    stty -echo; read -r PW; stty echo; echo
    printf '{"username":"%s","password":"%s"}' "$USER_NAME" "$PW" |
        curl -fsS -c "$JAR" -o /dev/null -H 'X-OpSkills-Request: 1' \
            -H 'Content-Type: application/json' --data-binary @- "$OPSKILLS/api/v1/auth/login"
    new_token() {
        curl -fsS -b "$JAR" -X POST -H 'X-OpSkills-Request: 1' "$OPSKILLS/api/v1/hosts/enrollment-tokens"
    }
fi

for host in $HOSTS; do
    token=$(new_token | sed -E 's/.*"token":"([^"]+)".*/\1/')
    case "$token" in
        *[!0-9a-f]* | "") echo "Could not create an enrollment token. Check the address and the token or password." >&2; exit 1 ;;
    esac
    echo "== $host"
    ssh "$host" "curl -fsSL $OPSKILLS/install-agent.sh | sudo sh -s -- --server $OPSKILLS --token $token --name $host --tags $TAGS"
done
