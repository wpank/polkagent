#!/bin/sh
# Prove the default single-instance Compose deployment starts unprivileged,
# honours a read-only bind-mounted config file, drains on SIGTERM, and recovers
# one HTTP-created durable interaction/run after replacing the container while
# retaining /data. The configured real local provider is intentionally
# unreachable: terminal failure persistence is proven, successful model output
# is not.
set -eu

repo_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
compose_file="$repo_dir/docker-compose.yml"
recovery_compose_file="$repo_dir/tests/deployment/docker-compose.recovery.yml"
project_name="polkagent-smoke-$$"
smoke_port=${POLKAGENT_SMOKE_PORT:-18080}
artifact_dir=${POLKAGENT_SMOKE_ARTIFACT_DIR:-}
keep_resources=${POLKAGENT_SMOKE_KEEP:-0}
base_url="http://127.0.0.1:$smoke_port"
container_id=
request_dir=$(mktemp -d /tmp/polkagent-container-smoke.XXXXXX)
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

    if [ -n "$artifact_dir" ]; then
        compose ps --all >"$artifact_dir/compose-ps-failure.txt" 2>&1 || true
        compose logs --timestamps --no-color --tail 300 >"$artifact_dir/compose-failure.log" 2>&1 || true
        if [ -n "$container_id" ]; then
            save_container_state "$container_id" failure-container
        fi
        mkdir -p "$artifact_dir/http"
        cp "$request_dir"/*.json "$artifact_dir/http/" 2>/dev/null || true
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

    case "$request_dir" in
        /tmp/polkagent-container-smoke.*) rm -rf -- "$request_dir" ;;
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

for required_command in docker curl jq; do
    command -v "$required_command" >/dev/null 2>&1 || \
        fail "required command is unavailable: $required_command"
done

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
        echo "agent_id=$agent_id"
        echo "conversation_id=$conversation_id"
        echo "turn_id=$turn_id"
        echo "run_id=$run_id"
        echo "terminal_state=$turn_state"
        echo "execution_backend=real-local-provider-intentionally-unreachable"
        echo "successful_model_output_proven=false"
    } >"$artifact_dir/summary.txt"
    mkdir -p "$artifact_dir/http"
    cp "$request_dir"/*.json "$artifact_dir/http/"
fi

echo "container recovery smoke passed"
echo "  HTTP probes: live, ready, startup"
echo "  read-only config mount: CORS allow-list, execution limit, local provider"
echo "  runtime user: $container_user (uid $runtime_uid)"
echo "  SIGTERM exit: 0 with graceful drain"
echo "  replaced container: $first_container_id -> $container_id"
echo "  retained volume: $volume_after"
echo "  durable IDs: agent=$agent_id conversation=$conversation_id turn=$turn_id run=$run_id"
echo "  terminal state: $turn_state (real local provider, intentionally unreachable)"
echo "  exact restart projections: agent, interaction config, turn, run, transcript"
echo "  not proven: successful model output, run/worker drain, Postgres, auth, HA, backup/restore, rolling upgrades"
