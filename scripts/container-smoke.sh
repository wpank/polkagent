#!/bin/sh
# Prove the default single-instance Compose deployment starts unprivileged,
# honours a read-only bind-mounted config, drains on SIGTERM, and recovers a
# durable SQLite marker after replacing the container while retaining /data.
# This intentionally does not claim durable HTTP API stores, Postgres, HA,
# backup/restore, or rolling upgrades.
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
compose_file="$repo_dir/docker-compose.yml"
recovery_compose_file="$repo_dir/tests/deployment/docker-compose.recovery.yml"
project_name="polkagent-smoke-$$"
smoke_port=${POLKAGENT_SMOKE_PORT:-18080}
artifact_dir=${POLKAGENT_SMOKE_ARTIFACT_DIR:-}
keep_resources=${POLKAGENT_SMOKE_KEEP:-0}
base_url="http://127.0.0.1:$smoke_port"
container_id=

export COMPOSE_PROJECT_NAME="$project_name"
export POLKAGENT_HTTP_PORT="$smoke_port"
export COMPOSE_PROGRESS=plain

compose() {
    docker compose -f "$compose_file" -f "$recovery_compose_file" "$@"
}

fail() {
    if [ -n "$artifact_dir" ]; then
        echo "$*" >"$artifact_dir/failure.txt"
    fi
    echo "container smoke failed: $*" >&2
    exit 1
}

save_container_state() {
    state_container_id=$1
    state_label=$2

    [ -n "$artifact_dir" ] || return 0
    docker inspect --format 'id={{.Id}}
name={{.Name}}
image={{.Image}}
user={{.Config.User}}
status={{.State.Status}}
exit_code={{.State.ExitCode}}
oom_killed={{.State.OOMKilled}}
error={{json .State.Error}}
health={{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}
{{range .Mounts}}mount={{.Type}}:{{.Destination}}:rw={{.RW}}
{{end}}' "$state_container_id" >"$artifact_dir/$state_label-state.txt" 2>&1 || true
    docker logs --timestamps "$state_container_id" >"$artifact_dir/$state_label.log" 2>&1 || true
}

collect_failure_diagnostics() {
    echo "container smoke diagnostics (project $project_name):" >&2
    compose ps --all >&2 || true
    compose logs --timestamps --no-color --tail 300 >&2 || true

    if [ -n "$artifact_dir" ]; then
        compose ps --all >"$artifact_dir/compose-ps-failure.txt" 2>&1 || true
        compose logs --timestamps --no-color --tail 300 >"$artifact_dir/compose-failure.log" 2>&1 || true
        if [ -n "$container_id" ]; then
            save_container_state "$container_id" failure-container
        fi
    fi
}

cleanup() {
    cleanup_status=$?
    trap - 0 1 2 15
    set +e

    if [ "$cleanup_status" -ne 0 ]; then
        collect_failure_diagnostics
    fi

    if [ "$keep_resources" = "1" ]; then
        echo "container smoke retained Compose project: $project_name" >&2
    else
        compose down --volumes --remove-orphans --rmi local >/dev/null 2>&1 || true
    fi

    exit "$cleanup_status"
}

assert_http_contract() {
    for endpoint in live ready startup; do
        response=$(curl --fail --silent --show-error "$base_url/health/$endpoint")
        case "$response" in
            *'"status":"ok"'*) ;;
            *) fail "/health/$endpoint did not report status=ok: $response" ;;
        esac
    done

    system_info=$(curl --fail --silent --show-error "$base_url/api/v1alpha1/system/info")
    case "$system_info" in
        *'"bind_address":"127.0.0.1:19090"'*) ;;
        *) fail "bind-mounted config sentinel is absent from system info: $system_info" ;;
    esac
    case "$system_info" in
        *'"database_backend":"sqlite"'*) ;;
        *) fail "system info did not report the SQLite backend: $system_info" ;;
    esac
    case "$system_info" in
        *'"max_concurrent_runs":37'*) ;;
        *) fail "bind-mounted execution limit is absent from system info: $system_info" ;;
    esac

    readonly_status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
        --request POST --header 'content-type: application/json' --data '{}' \
        "$base_url/api/v1alpha1/agents")
    [ "$readonly_status" = "405" ] || fail "read-only config returned HTTP $readonly_status, expected 405"

    readonly_body=$(curl --silent --show-error \
        --request POST --header 'content-type: application/json' --data '{}' \
        "$base_url/api/v1alpha1/agents")
    case "$readonly_body" in
        *'server is in read-only mode'*) ;;
        *) fail "read-only middleware returned an unexpected response: $readonly_body" ;;
    esac

    cors_headers=$(curl --silent --show-error --dump-header - --output /dev/null \
        --header 'Origin: https://container-recovery.invalid' \
        "$base_url/api/v1alpha1/system/info" | tr -d '\r')
    case "$cors_headers" in
        *'access-control-allow-origin: https://container-recovery.invalid'*) ;;
        *) fail "bind-mounted CORS allow-list was not honoured" ;;
    esac
}

if [ -n "$artifact_dir" ]; then
    mkdir -p "$artifact_dir"
fi

trap cleanup 0
trap 'exit 130' 1 2 15

compose config --quiet
compose up --detach --build --wait --wait-timeout 180

container_id=$(compose ps --quiet polkagent)
[ -n "$container_id" ] || fail "Compose did not return a polkagent container id"

assert_http_contract

container_user=$(docker inspect --format '{{.Config.User}}' "$container_id")
if [ -z "$container_user" ] || [ "$container_user" = "0" ] || [ "$container_user" = "root" ]; then
    fail "container is not configured with a non-root user"
fi

runtime_uid=$(compose exec --no-TTY polkagent id -u | tr -d '\r')
[ "$runtime_uid" != "0" ] || fail "container process is running with uid 0"

config_mount=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/etc/polkagent/recovery-config.toml"}}{{.Type}}:{{.RW}}{{end}}{{end}}' "$container_id")
[ "$config_mount" = "bind:false" ] || fail "recovery config mount is not a read-only bind mount: $config_mount"

volume_before=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Name}}{{end}}{{end}}' "$container_id")
[ -n "$volume_before" ] || fail "container has no named volume mounted at /data"

compose exec --no-TTY polkagent test -s /data/polkagent.db

marker_name=container-recovery-marker
marker_create=$(compose exec --no-TTY polkagent /usr/local/bin/polkagent --log-file - \
    agent create "$marker_name" --model fake/recovery --description 'container restart persistence proof' --json)
case "$marker_create" in
    *'"name":"container-recovery-marker"'*|*'"name": "container-recovery-marker"'*) ;;
    *) fail "durable agent marker was not created: $marker_create" ;;
esac

save_container_state "$container_id" first-running

docker stop --time 20 "$container_id" >/dev/null

exit_code=$(docker inspect --format '{{.State.ExitCode}}' "$container_id")
[ "$exit_code" = "0" ] || fail "SIGTERM shutdown exit code was $exit_code, expected 0"

shutdown_logs=$(docker logs "$container_id" 2>&1)
case "$shutdown_logs" in
    *'Shutdown signal received. Draining active HTTP connections...'*) ;;
    *) fail "SIGTERM shutdown log did not report connection draining" ;;
esac

save_container_state "$container_id" first-stopped
first_container_id=$container_id

compose rm --force polkagent >/dev/null
compose up --detach --no-build --wait --wait-timeout 180

container_id=$(compose ps --quiet polkagent)
[ -n "$container_id" ] || fail "Compose did not return the replacement container id"
[ "$container_id" != "$first_container_id" ] || fail "container was not replaced during restart test"

volume_after=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Name}}{{end}}{{end}}' "$container_id")
[ "$volume_after" = "$volume_before" ] || fail "replacement container used volume $volume_after, expected $volume_before"

assert_http_contract

marker_show=$(compose exec --no-TTY polkagent /usr/local/bin/polkagent --log-file - \
    agent show "$marker_name" --json)
case "$marker_show" in
    *'"name":"container-recovery-marker"'*|*'"name": "container-recovery-marker"'*) ;;
    *) fail "durable marker did not survive replacement: $marker_show" ;;
esac
case "$marker_show" in
    *'"model":"fake/recovery"'*|*'"model": "fake/recovery"'*) ;;
    *) fail "durable marker survived but its model changed: $marker_show" ;;
esac

save_container_state "$container_id" replacement-running

if [ -n "$artifact_dir" ]; then
    {
        echo "result=passed"
        echo "project=$project_name"
        echo "first_container=$first_container_id"
        echo "replacement_container=$container_id"
        echo "persistent_volume=$volume_after"
        echo "runtime_user=$container_user"
        echo "runtime_uid=$runtime_uid"
        echo "config_mount=$config_mount"
        echo "durable_marker=$marker_name"
    } >"$artifact_dir/summary.txt"
fi

echo "container recovery smoke passed"
echo "  HTTP probes: live, ready, startup"
echo "  bind-mounted config: read-only mode, CORS allow-list, execution limit"
echo "  runtime user: $container_user (uid $runtime_uid)"
echo "  SIGTERM exit: 0 with graceful drain"
echo "  replaced container: $first_container_id -> $container_id"
echo "  retained volume: $volume_after"
echo "  durable marker: $marker_name"
echo "  not proven: durable HTTP API stores, Postgres, auth, HA, backup/restore, rolling upgrades"
