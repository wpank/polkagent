#!/bin/sh
# Prove the default single-instance Compose container starts, becomes healthy,
# serves all three HTTP probes, runs unprivileged, and creates its SQLite DB.
# This intentionally does not claim API durability, Postgres, HA, or recovery.
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
compose_file="$repo_dir/docker-compose.yml"
project_name="polkagent-smoke-$$"
smoke_port=${POLKAGENT_SMOKE_PORT:-18080}
base_url="http://127.0.0.1:$smoke_port"

export COMPOSE_PROJECT_NAME="$project_name"
export POLKAGENT_HTTP_PORT="$smoke_port"
export COMPOSE_PROGRESS=plain

cleanup() {
    docker compose -f "$compose_file" down --volumes --remove-orphans --rmi local >/dev/null 2>&1 || true
}
trap cleanup EXIT HUP INT TERM

docker compose -f "$compose_file" config --quiet
docker compose -f "$compose_file" up --detach --build --wait --wait-timeout 180

container_id=$(docker compose -f "$compose_file" ps --quiet polkagent)
if [ -z "$container_id" ]; then
    echo "smoke failed: Compose did not return a polkagent container id" >&2
    exit 1
fi

for endpoint in live ready startup; do
    response=$(curl --fail --silent --show-error "$base_url/health/$endpoint")
    case "$response" in
        *'"status":"ok"'*) ;;
        *)
            echo "smoke failed: /health/$endpoint did not report status=ok: $response" >&2
            exit 1
            ;;
    esac
done

container_user=$(docker inspect --format '{{.Config.User}}' "$container_id")
if [ -z "$container_user" ] || [ "$container_user" = "0" ] || [ "$container_user" = "root" ]; then
    echo "smoke failed: container is not configured with a non-root user" >&2
    exit 1
fi

runtime_uid=$(docker compose -f "$compose_file" exec --no-TTY polkagent id -u | tr -d '\r')
if [ "$runtime_uid" = "0" ]; then
    echo "smoke failed: container process is running with uid 0" >&2
    exit 1
fi

docker compose -f "$compose_file" exec --no-TTY polkagent test -s /data/polkagent.db

echo "container smoke passed"
echo "  HTTP probes: live, ready, startup"
echo "  runtime user: $container_user (uid $runtime_uid)"
echo "  SQLite file: /data/polkagent.db"
echo "  not proven: durable API stores, Postgres, auth, HA, backup/restore, upgrades"
