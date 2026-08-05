#!/bin/sh
# Prove the default single-instance Compose deployment starts unprivileged,
# honours a read-only bind-mounted config file, drains on SIGTERM, recovers one
# HTTP-created durable interaction/run after replacing the container while
# retaining /data, and restores an offline whole-volume SQLite backup into a
# fresh Compose project/volume. The configured real local provider is
# intentionally unreachable: terminal failure persistence is proven,
# successful model output is not.
set -eu

repo_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
compose_file="$repo_dir/docker-compose.yml"
recovery_compose_file="$repo_dir/tests/deployment/docker-compose.recovery.yml"
project_name="polkagent-smoke-$$"
restore_project_name="polkagent-restore-smoke-$$"
smoke_port=${POLKAGENT_SMOKE_PORT:-18080}
artifact_dir=${POLKAGENT_SMOKE_ARTIFACT_DIR:-}
keep_resources=${POLKAGENT_SMOKE_KEEP:-0}
base_url="http://127.0.0.1:$smoke_port"
container_id=
restore_container_id=
restore_project_initialized=0
backup_copy_container_id=
backup_transfer_volume="${project_name}_backup-transfer"
request_dir=$(mktemp -d /tmp/polkagent-container-smoke.XXXXXX)
request_dir=$(CDPATH='' cd -- "$request_dir" && pwd -P)
backup_dir="$request_dir/backup"
integrity_dir="$request_dir/integrity"
backup_archive="$backup_dir/polkagent-volume.tar"
backup_checksum_file="$backup_dir/polkagent-volume.tar.sha256"
backup_manifest="$backup_dir/polkagent-volume.manifest.txt"
agent_name=container-recovery-agent
agent_model=container-smoke-local/container-smoke-model
interaction_title="Container recovery interaction"
prompt_text="prove durable container replacement without simulated model output"

export COMPOSE_PROJECT_NAME="$project_name"
export POLKAGENT_HTTP_PORT="$smoke_port"
export COMPOSE_PROGRESS=plain

compose() {
    docker compose -f "$compose_file" -f "$recovery_compose_file" "$@"
}

restore_compose() {
    docker compose --project-name "$restore_project_name" \
        -f "$compose_file" -f "$recovery_compose_file" "$@"
}

sha256_digest() {
    digest_file=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$digest_file" | awk '{print $1}'
    else
        shasum -a 256 "$digest_file" | awk '{print $1}'
    fi
}

fail() {
    if [ -n "$artifact_dir" ]; then
        echo "$*" >"$artifact_dir/failure.txt"
    fi
    echo "container smoke failed: $*" >&2
    exit 1
}

http_json() {
    expected_status=$1
    request_method=$2
    request_path=$3
    request_label=$4
    response_file="$request_dir/$request_label.json"

    if [ "$#" -eq 5 ]; then
        request_body=$5
        if ! response_status=$(curl --silent --show-error --max-time 10 --connect-timeout 3 \
            --request "$request_method" \
            --header 'accept: application/json' \
            --header 'content-type: application/json' \
            --data "$request_body" \
            --output "$response_file" \
            --write-out '%{http_code}' \
            "$base_url$request_path"); then
            fail "$request_method $request_path did not complete; response file: $response_file"
        fi
    else
        if ! response_status=$(curl --silent --show-error --max-time 10 --connect-timeout 3 \
            --request "$request_method" \
            --header 'accept: application/json' \
            --output "$response_file" \
            --write-out '%{http_code}' \
            "$base_url$request_path"); then
            fail "$request_method $request_path did not complete; response file: $response_file"
        fi
    fi

    if [ "$response_status" != "$expected_status" ]; then
        response_excerpt=$(head -c 4000 "$response_file" | tr '\n' ' ')
        fail "$request_method $request_path returned HTTP $response_status, expected $expected_status: $response_excerpt"
    fi
    if ! jq --exit-status . "$response_file" >/dev/null 2>&1; then
        response_excerpt=$(head -c 4000 "$response_file" | tr '\n' ' ')
        fail "$request_method $request_path returned non-JSON: $response_excerpt"
    fi
    cat "$response_file"
}

compact_json() {
    projection_label=$1
    projection_filter=$2
    projection_json=$3
    if ! printf '%s\n' "$projection_json" | jq --exit-status --sort-keys --compact-output "$projection_filter"; then
        fail "could not project $projection_label from its HTTP response"
    fi
}

assert_same_projection() {
    projection_label=$1
    projection_before=$2
    projection_after=$3
    [ "$projection_before" = "$projection_after" ] || \
        fail "$projection_label changed across container replacement"
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
    if [ "$restore_project_initialized" = "1" ]; then
        echo "restore smoke diagnostics (project $restore_project_name):" >&2
        restore_compose ps --all >&2 || true
        restore_compose logs --timestamps --no-color --tail 300 >&2 || true
    fi

    if [ -n "$artifact_dir" ]; then
        compose ps --all >"$artifact_dir/compose-ps-failure.txt" 2>&1 || true
        compose logs --timestamps --no-color --tail 300 >"$artifact_dir/compose-failure.log" 2>&1 || true
        if [ -n "$container_id" ]; then
            save_container_state "$container_id" failure-container
        fi
        if [ "$restore_project_initialized" = "1" ]; then
            restore_compose ps --all >"$artifact_dir/restore-compose-ps-failure.txt" 2>&1 || true
            restore_compose logs --timestamps --no-color --tail 300 \
                >"$artifact_dir/restore-compose-failure.log" 2>&1 || true
            if [ -n "$restore_container_id" ]; then
                save_container_state "$restore_container_id" failure-restore-container
            fi
        fi
        mkdir -p "$artifact_dir/http"
        cp "$request_dir"/*.json "$artifact_dir/http/" 2>/dev/null || true
        if [ -s "$backup_archive" ]; then
            mkdir -p "$artifact_dir/backup"
            cp "$backup_archive" "$backup_checksum_file" "$backup_manifest" \
                "$artifact_dir/backup/" 2>/dev/null || true
        fi
    fi
}

cleanup() {
    cleanup_status=$?
    trap - 0 1 2 15
    set +e

    if [ -n "$backup_copy_container_id" ]; then
        docker rm --force "$backup_copy_container_id" >/dev/null 2>&1 || true
    fi

    if [ "$cleanup_status" -ne 0 ]; then
        if [ -n "$artifact_dir" ] && [ ! -e "$artifact_dir/failure.txt" ]; then
            echo "unexpected command failure (exit $cleanup_status)" >"$artifact_dir/failure.txt"
        fi
        collect_failure_diagnostics
    fi

    if [ "$keep_resources" = "1" ]; then
        echo "container smoke retained Compose project: $project_name" >&2
        if [ "$restore_project_initialized" = "1" ]; then
            echo "container smoke retained restore project: $restore_project_name" >&2
        fi
        echo "container smoke retained backup transfer volume: $backup_transfer_volume" >&2
        echo "container smoke retained temporary backup: $backup_dir" >&2
    else
        if [ "$restore_project_initialized" = "1" ]; then
            restore_compose down --volumes --remove-orphans --rmi local >/dev/null 2>&1 || true
        fi
        compose down --volumes --remove-orphans --rmi local >/dev/null 2>&1 || true
        case "$backup_transfer_volume" in
            polkagent-smoke-*_backup-transfer)
                docker volume rm --force "$backup_transfer_volume" >/dev/null 2>&1 || true
                ;;
            *) echo "refusing to remove unexpected backup volume: $backup_transfer_volume" >&2 ;;
        esac
        case "$request_dir" in
            /tmp/polkagent-container-smoke.*|/private/tmp/polkagent-container-smoke.*)
                rm -rf -- "$request_dir"
                ;;
        esac
    fi

    case "$request_dir" in
        /tmp/polkagent-container-smoke.*|/private/tmp/polkagent-container-smoke.*) ;;
        *) echo "refusing to accept unexpected temporary path: $request_dir" >&2 ;;
    esac

    exit "$cleanup_status"
}

assert_http_contract() {
    for endpoint in live ready startup; do
        response=$(curl --fail --silent --show-error --max-time 10 --connect-timeout 3 \
            "$base_url/health/$endpoint")
        case "$response" in
            *'"status":"ok"'*) ;;
            *) fail "/health/$endpoint did not report status=ok: $response" ;;
        esac
    done

    system_info=$(curl --fail --silent --show-error --max-time 10 --connect-timeout 3 \
        "$base_url/api/v1alpha1/system/info")
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

    cors_headers=$(curl --silent --show-error --max-time 10 --connect-timeout 3 \
        --dump-header - --output /dev/null \
        --header 'Origin: https://container-recovery.invalid' \
        "$base_url/api/v1alpha1/system/info" | tr -d '\r')
    case "$cors_headers" in
        *'access-control-allow-origin: https://container-recovery.invalid'*) ;;
        *) fail "bind-mounted CORS allow-list was not honoured" ;;
    esac

    case "$system_info" in
        *'container-smoke-placeholder'*) fail "safe system info leaked the provider token sentinel" ;;
    esac
}

if [ -n "$artifact_dir" ]; then
    mkdir -p "$artifact_dir"
fi

trap cleanup 0
trap 'exit 130' 1 2 15

for required_command in docker curl jq sqlite3 tar awk; do
    command -v "$required_command" >/dev/null 2>&1 || \
        fail "required command is unavailable: $required_command"
done
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    fail "required SHA-256 command is unavailable: install sha256sum or shasum"
fi

mkdir -m 0777 "$backup_dir"
mkdir -m 0700 "$integrity_dir"
smoke_started_at=$(date +%s)

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
runtime_gid=$(compose exec --no-TTY polkagent id -g | tr -d '\r')
[ "$runtime_gid" != "0" ] || fail "container process is running with gid 0"

config_mount=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/etc/polkagent/recovery-config.toml"}}{{.Type}}:{{.RW}}{{end}}{{end}}' "$container_id")
[ "$config_mount" = "bind:false" ] || fail "recovery config mount is not a read-only bind mount: $config_mount"

volume_before=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Name}}{{end}}{{end}}' "$container_id")
[ -n "$volume_before" ] || fail "container has no named volume mounted at /data"

compose exec --no-TTY polkagent test -s /data/polkagent.db

agent_payload=$(jq --null-input --compact-output \
    --arg name "$agent_name" \
    --arg model "$agent_model" \
    '{name: $name, model: $model, description: "container replacement API proof"}')
agent_created=$(http_json 201 POST /api/v1alpha1/agents first-agent-create "$agent_payload")
agent_id=$(printf '%s\n' "$agent_created" | jq --exit-status --raw-output '.id') || \
    fail "created agent response has no id"
printf '%s\n' "$agent_created" | jq --exit-status \
    --arg id "$agent_id" --arg name "$agent_name" --arg model "$agent_model" \
    '.id == $id and .spec.id == $id and .spec.name == $name and .spec.model == $model and .status.state == "created"' \
    >/dev/null || \
    fail "created agent projection is inconsistent"

interaction_payload=$(jq --null-input --compact-output \
    --arg title "$interaction_title" \
    --arg agent_id "$agent_id" \
    '{title: $title, target: {kind: "agent", id: $agent_id}, working_directory: "/data", client_name: "container-smoke", client_session_id: "container-smoke-session-v1"}')
interaction_created=$(http_json 201 POST /api/v1alpha1/interactions first-interaction-create "$interaction_payload")
conversation_id=$(printf '%s\n' "$interaction_created" | \
    jq --exit-status --raw-output '.interaction.conversation_id') || \
    fail "created interaction response has no conversation id"

config_payload=$(jq --null-input --compact-output --arg model "$agent_model" \
    '{option: "model", value: $model}')
config_before_json=$(http_json 200 PUT \
    "/api/v1alpha1/interactions/$conversation_id/config" \
    first-interaction-config-update "$config_payload")
printf '%s\n' "$config_before_json" | jq --exit-status \
    --arg agent_id "$agent_id" --arg model "$agent_model" \
    '.config.target.kind == "agent" and .config.target.id == $agent_id and .config.model == $model' \
    >/dev/null || fail "durable interaction config did not retain target/model"

prompt_payload=$(jq --null-input --compact-output --arg prompt "$prompt_text" \
    '{prompt: $prompt, working_directory: "/data", client_name: "container-smoke", client_session_id: "container-smoke-session-v1"}')
prompt_started=$(http_json 202 POST \
    "/api/v1alpha1/interactions/$conversation_id/prompt" \
    first-prompt "$prompt_payload")
turn_id=$(printf '%s\n' "$prompt_started" | jq --exit-status --raw-output '.handle.turn_id') || \
    fail "prompt response has no turn id"
run_id=$(printf '%s\n' "$prompt_started" | jq --exit-status --raw-output \
    '.handle.run_ids | if length == 1 then .[0] else error("expected exactly one run") end') || \
    fail "prompt response did not contain exactly one run id"
handle_conversation_id=$(printf '%s\n' "$prompt_started" | \
    jq --exit-status --raw-output '.handle.conversation_id') || \
    fail "prompt response has no conversation id"
[ "$handle_conversation_id" = "$conversation_id" ] || \
    fail "prompt handle conversation $handle_conversation_id differs from $conversation_id"

turn_state=
poll_attempt=0
while [ "$poll_attempt" -lt 30 ]; do
    turns_poll=$(http_json 200 GET \
        "/api/v1alpha1/interactions/$conversation_id/turns" first-turns-poll)
    turn_state=$(printf '%s\n' "$turns_poll" | jq --exit-status --raw-output \
        --arg turn_id "$turn_id" \
        '.data[] | select(.handle.turn_id == $turn_id) | .state') || \
        fail "turn $turn_id disappeared while waiting for terminal state"
    case "$turn_state" in
        completed|failed|cancelled|timed_out) break ;;
        pending|running|awaiting_approval) ;;
        *) fail "turn $turn_id reported unknown state: $turn_state" ;;
    esac
    poll_attempt=$((poll_attempt + 1))
    sleep 1
done
[ "$turn_state" = "failed" ] || \
    fail "unreachable real provider produced terminal state '$turn_state', expected 'failed'"

agent_before_json=$(http_json 200 GET "/api/v1alpha1/agents/$agent_id" first-agent)
interaction_before_json=$(http_json 200 GET \
    "/api/v1alpha1/interactions/$conversation_id" first-interaction)
config_before_json=$(http_json 200 GET \
    "/api/v1alpha1/interactions/$conversation_id/config" first-interaction-config)
turns_before_json=$(http_json 200 GET \
    "/api/v1alpha1/interactions/$conversation_id/turns" first-turns)
run_before_json=$(http_json 200 GET "/api/v1alpha1/runs/$run_id" first-run)
transcript_before_json=$(http_json 200 GET \
    "/api/v1alpha1/conversations/$conversation_id" first-transcript)

printf '%s\n' "$run_before_json" | jq --exit-status --arg run_id "$run_id" \
    '.id == $run_id and .status.state == "failed" and .completed_at != null and (.terminal_reason | type == "string")' \
    >/dev/null || fail "run $run_id did not reach a reason-bearing failed terminal state"
printf '%s\n' "$turns_before_json" | jq --exit-status \
    --arg turn_id "$turn_id" --arg run_id "$run_id" \
    '.data | length == 1 and .[0].handle.turn_id == $turn_id and .[0].handle.run_ids == [$run_id] and .[0].state == "failed" and .[0].completed_at != null' \
    >/dev/null || fail "turn/run correlation was not durably terminal"
printf '%s\n' "$transcript_before_json" | jq --exit-status --arg prompt "$prompt_text" \
    '.message_count == 2 and .messages[0].role == "user" and .messages[0].content == $prompt and .messages[1].role == "assistant" and .messages[1].content == ""' \
    >/dev/null || fail "failed prompt transcript is incomplete or contains simulated output"

agent_before=$(compact_json agent '.' "$agent_before_json")
interaction_before=$(compact_json interaction '.interaction' "$interaction_before_json")
config_before=$(compact_json interaction-config '.config' "$config_before_json")
turns_before=$(compact_json interaction-turns '.data' "$turns_before_json")
run_before=$(compact_json run '.' "$run_before_json")
transcript_before=$(compact_json transcript '.' "$transcript_before_json")

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

agent_after_json=$(http_json 200 GET "/api/v1alpha1/agents/$agent_id" replacement-agent)
interaction_after_json=$(http_json 200 GET \
    "/api/v1alpha1/interactions/$conversation_id" replacement-interaction)
config_after_json=$(http_json 200 GET \
    "/api/v1alpha1/interactions/$conversation_id/config" replacement-interaction-config)
turns_after_json=$(http_json 200 GET \
    "/api/v1alpha1/interactions/$conversation_id/turns" replacement-turns)
run_after_json=$(http_json 200 GET "/api/v1alpha1/runs/$run_id" replacement-run)
transcript_after_json=$(http_json 200 GET \
    "/api/v1alpha1/conversations/$conversation_id" replacement-transcript)

agent_after=$(compact_json agent '.' "$agent_after_json")
interaction_after=$(compact_json interaction '.interaction' "$interaction_after_json")
config_after=$(compact_json interaction-config '.config' "$config_after_json")
turns_after=$(compact_json interaction-turns '.data' "$turns_after_json")
run_after=$(compact_json run '.' "$run_after_json")
transcript_after=$(compact_json transcript '.' "$transcript_after_json")

assert_same_projection agent "$agent_before" "$agent_after"
assert_same_projection interaction "$interaction_before" "$interaction_after"
assert_same_projection interaction-config "$config_before" "$config_after"
assert_same_projection interaction-turns "$turns_before" "$turns_after"
assert_same_projection run "$run_before" "$run_after"
assert_same_projection transcript "$transcript_before" "$transcript_after"

save_container_state "$container_id" replacement-running

# Capture a whole-volume backup only after the source has closed SQLite and
# exited cleanly. Copying the mounted volume read-only includes the main DB and
# any WAL/SHM sidecars as one offline filesystem snapshot.
backup_started_at=$(date +%s)
docker stop --time 20 "$container_id" >/dev/null

backup_source_exit_code=$(docker inspect --format '{{.State.ExitCode}}' "$container_id")
[ "$backup_source_exit_code" = "0" ] || \
    fail "backup source SIGTERM exit code was $backup_source_exit_code, expected 0"
backup_source_logs=$(docker logs "$container_id" 2>&1)
case "$backup_source_logs" in
    *'Shutdown signal received. Draining active HTTP connections...'*) ;;
    *) fail "backup source did not report graceful HTTP drain before capture" ;;
esac
save_container_state "$container_id" backup-source-stopped

running_volume_users=$(docker ps --filter "volume=$volume_after" --format '{{.ID}}')
[ -z "$running_volume_users" ] || \
    fail "source volume still has a running container during backup: $running_volume_users"

source_image_id=$(docker inspect --format '{{.Image}}' "$container_id")
[ -n "$source_image_id" ] || fail "could not resolve the exact source image id"

capture_started_at=$(date +%s)
docker volume create "$backup_transfer_volume" >/dev/null
docker run --rm --network none --read-only \
    --user "$runtime_uid:$runtime_gid" \
    --entrypoint /bin/sh \
    --env EXPECTED_UID="$runtime_uid" \
    --env EXPECTED_GID="$runtime_gid" \
    --mount "type=volume,source=$volume_after,target=/source,readonly" \
    --mount "type=volume,source=$backup_transfer_volume,target=/data" \
    "$source_image_id" -eu -c '
        [ "$(id -u)" = "$EXPECTED_UID" ]
        [ "$(id -g)" = "$EXPECTED_GID" ]
        [ -s /source/polkagent.db ]
        [ -w /data ]
        umask 022
        tar -C /source -cf /data/polkagent-volume.tar .
        [ -s /data/polkagent-volume.tar ]
    '

backup_copy_container_id=$(docker create --network none --read-only \
    --user "$runtime_uid:$runtime_gid" \
    --entrypoint /bin/true \
    --mount "type=volume,source=$backup_transfer_volume,target=/data,readonly" \
    "$source_image_id")
[ -n "$backup_copy_container_id" ] || fail "could not create the bounded backup export container"
docker cp "$backup_copy_container_id:/data/polkagent-volume.tar" "$backup_archive"
docker rm "$backup_copy_container_id" >/dev/null
backup_copy_container_id=
capture_finished_at=$(date +%s)
backup_capture_seconds=$((capture_finished_at - capture_started_at))

tar -tf "$backup_archive" >"$backup_manifest"
[ -s "$backup_manifest" ] || fail "offline volume backup has an empty manifest"
awk '$0 == "./polkagent.db" { found = 1 } END { exit !found }' "$backup_manifest" || \
    fail "offline volume backup does not contain ./polkagent.db"
if awk '$0 ~ /^\// || $0 ~ /(^|\/)\.\.($|\/)/ { unsafe = 1 } END { exit !unsafe }' \
    "$backup_manifest"; then
    fail "offline volume backup contains an unsafe archive path"
fi

backup_checksum=$(sha256_digest "$backup_archive")
printf '%s  %s\n' "$backup_checksum" "polkagent-volume.tar" >"$backup_checksum_file"
[ "$(sha256_digest "$backup_archive")" = "$backup_checksum" ] || \
    fail "offline volume backup failed its SHA-256 verification"
backup_bytes=$(wc -c <"$backup_archive" | tr -d ' ')
[ "$backup_bytes" -gt 0 ] || fail "offline volume backup has zero bytes"

tar -xf "$backup_archive" -C "$integrity_dir"
[ -s "$integrity_dir/polkagent.db" ] || \
    fail "integrity-check copy does not contain a SQLite database"
integrity_output=$(sqlite3 "$integrity_dir/polkagent.db" \
    'PRAGMA integrity_check; PRAGMA foreign_key_check;')
[ "$integrity_output" = "ok" ] || \
    fail "offline SQLite backup failed integrity checks: $integrity_output"
printf '%s\n' "$integrity_output" >"$backup_dir/sqlite-integrity.txt"

# Compose creates and labels a distinct project volume from the exact source
# image. The same non-root UID then extracts the verified archive before the
# normal service starts against it.
restore_started_at=$(date +%s)
restore_image="${restore_project_name}-polkagent"
restore_project_initialized=1
docker tag "$source_image_id" "$restore_image"
restore_compose create --no-build polkagent >/dev/null
restore_container_id=$(restore_compose ps --all --quiet polkagent)
[ -n "$restore_container_id" ] || fail "restore Compose project created no container"

restore_volume=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Name}}{{end}}{{end}}' \
    "$restore_container_id")
[ -n "$restore_volume" ] || fail "restore container has no named volume mounted at /data"
[ "$restore_volume" != "$volume_after" ] || \
    fail "restore project reused the source volume instead of a fresh volume"

[ "$(sha256_digest "$backup_archive")" = "$backup_checksum" ] || \
    fail "offline volume backup changed before restore"
docker run --rm --network none --read-only \
    --user "$runtime_uid:$runtime_gid" \
    --entrypoint /bin/sh \
    --env EXPECTED_UID="$runtime_uid" \
    --env EXPECTED_GID="$runtime_gid" \
    --mount "type=volume,source=$restore_volume,target=/data" \
    --mount "type=volume,source=$backup_transfer_volume,target=/backup,readonly" \
    "$source_image_id" -eu -c '
        [ "$(id -u)" = "$EXPECTED_UID" ]
        [ "$(id -g)" = "$EXPECTED_GID" ]
        [ -w /data ]
        [ ! -e /data/polkagent.db ]
        tar -xpf /backup/polkagent-volume.tar -C /data
        [ -s /data/polkagent.db ]
        [ -r /data/polkagent.db ]
        [ -w /data/polkagent.db ]
        [ "$(stat -c %u /data/polkagent.db)" = "$EXPECTED_UID" ]
        printf "non-root restore write probe\n" >/data/.restore-write-probe
        rm /data/.restore-write-probe
    '

restore_compose up --detach --no-build --wait --wait-timeout 180
restore_container_id=$(restore_compose ps --quiet polkagent)
[ -n "$restore_container_id" ] || fail "restore Compose project did not start a container"

restore_runtime_uid=$(restore_compose exec --no-TTY polkagent id -u | tr -d '\r')
restore_runtime_gid=$(restore_compose exec --no-TTY polkagent id -g | tr -d '\r')
[ "$restore_runtime_uid:$restore_runtime_gid" = "$runtime_uid:$runtime_gid" ] || \
    fail "restored runtime user changed from $runtime_uid:$runtime_gid to $restore_runtime_uid:$restore_runtime_gid"
restore_compose exec --no-TTY polkagent test -s /data/polkagent.db
assert_http_contract

restored_agent_json=$(http_json 200 GET "/api/v1alpha1/agents/$agent_id" restored-agent)
restored_interaction_json=$(http_json 200 GET \
    "/api/v1alpha1/interactions/$conversation_id" restored-interaction)
restored_config_json=$(http_json 200 GET \
    "/api/v1alpha1/interactions/$conversation_id/config" restored-interaction-config)
restored_turns_json=$(http_json 200 GET \
    "/api/v1alpha1/interactions/$conversation_id/turns" restored-turns)
restored_run_json=$(http_json 200 GET "/api/v1alpha1/runs/$run_id" restored-run)
restored_transcript_json=$(http_json 200 GET \
    "/api/v1alpha1/conversations/$conversation_id" restored-transcript)

restored_agent=$(compact_json restored-agent '.' "$restored_agent_json")
restored_interaction=$(compact_json restored-interaction '.interaction' "$restored_interaction_json")
restored_config=$(compact_json restored-interaction-config '.config' "$restored_config_json")
restored_turns=$(compact_json restored-interaction-turns '.data' "$restored_turns_json")
restored_run=$(compact_json restored-run '.' "$restored_run_json")
restored_transcript=$(compact_json restored-transcript '.' "$restored_transcript_json")

assert_same_projection restored-agent "$agent_before" "$restored_agent"
assert_same_projection restored-interaction "$interaction_before" "$restored_interaction"
assert_same_projection restored-interaction-config "$config_before" "$restored_config"
assert_same_projection restored-interaction-turns "$turns_before" "$restored_turns"
assert_same_projection restored-run "$run_before" "$restored_run"
assert_same_projection restored-transcript "$transcript_before" "$restored_transcript"

save_container_state "$restore_container_id" restore-running
restore_finished_at=$(date +%s)
backup_restore_seconds=$((restore_finished_at - restore_started_at))
backup_total_seconds=$((restore_finished_at - backup_started_at))
smoke_total_seconds=$((restore_finished_at - smoke_started_at))

if [ -n "$artifact_dir" ]; then
    {
        echo "result=passed"
        echo "project=$project_name"
        echo "restore_project=$restore_project_name"
        echo "first_container=$first_container_id"
        echo "replacement_container=$container_id"
        echo "restore_container=$restore_container_id"
        echo "persistent_volume=$volume_after"
        echo "restored_volume=$restore_volume"
        echo "runtime_user=$container_user"
        echo "runtime_uid=$runtime_uid"
        echo "runtime_gid=$runtime_gid"
        echo "config_mount=$config_mount"
        echo "agent_id=$agent_id"
        echo "conversation_id=$conversation_id"
        echo "turn_id=$turn_id"
        echo "run_id=$run_id"
        echo "terminal_state=$turn_state"
        echo "backup_sha256=$backup_checksum"
        echo "backup_bytes=$backup_bytes"
        echo "sqlite_integrity=$integrity_output"
        echo "backup_capture_seconds=$backup_capture_seconds"
        echo "backup_restore_seconds=$backup_restore_seconds"
        echo "backup_total_seconds=$backup_total_seconds"
        echo "smoke_total_seconds=$smoke_total_seconds"
        echo "execution_backend=real-local-provider-intentionally-unreachable"
        echo "successful_model_output_proven=false"
        echo "online_backup_proven=false"
    } >"$artifact_dir/summary.txt"
    mkdir -p "$artifact_dir/http"
    cp "$request_dir"/*.json "$artifact_dir/http/"
    mkdir -p "$artifact_dir/backup"
    cp "$backup_archive" "$backup_checksum_file" "$backup_manifest" \
        "$backup_dir/sqlite-integrity.txt" "$artifact_dir/backup/"
fi

echo "container recovery and offline backup/restore smoke passed"
echo "  HTTP probes: live, ready, startup"
echo "  read-only config mount: CORS allow-list, execution limit, local provider"
echo "  runtime user: $container_user (uid $runtime_uid)"
echo "  SIGTERM exit: 0 with graceful drain"
echo "  replaced container: $first_container_id -> $container_id"
echo "  retained volume: $volume_after"
echo "  durable IDs: agent=$agent_id conversation=$conversation_id turn=$turn_id run=$run_id"
echo "  terminal state: $turn_state (real local provider, intentionally unreachable)"
echo "  exact restart projections: agent, interaction config, turn, run, transcript"
echo "  offline backup: $backup_bytes bytes, sha256=$backup_checksum, integrity=$integrity_output"
echo "  restored project/volume: $restore_project_name / $restore_volume"
echo "  exact restore projections: agent, interaction config, turn, run, transcript"
echo "  timings: capture=${backup_capture_seconds}s restore=${backup_restore_seconds}s backup-total=${backup_total_seconds}s smoke-total=${smoke_total_seconds}s"
echo "  not proven: successful model output, online/encrypted backup, run/worker/effect drain, Postgres, auth, HA, upgrade/rollback"
