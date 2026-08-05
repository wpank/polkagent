# Deployment

```mermaid
graph TB
    subgraph Local["Local Development"]
        LC["polkagent CLI"]
        LD["SQLite DB"]
        LC --> LD
    end

    subgraph Single["Single Instance (Docker)"]
        DC["Docker Container"]
        DS["SQLite Volume"]
        RP["Reverse Proxy<br/>(nginx/caddy)"]
        RP --> DC
        DC --> DS
    end

    subgraph Production["Production (Multi-Instance)"]
        LB["Load Balancer"]
        I1["Instance 1"]
        I2["Instance 2"]
        I3["Instance N"]
        PG["PostgreSQL"]
        OT["OTLP Collector"]
        LB --> I1
        LB --> I2
        LB --> I3
        I1 --> PG
        I2 --> PG
        I3 --> PG
        I1 --> OT
        I2 --> OT
        I3 --> OT
    end
```

## Running Locally

```bash
# Install from source
cargo install --path crates/polkagent-cli

# Run the CLI
polkagent run -a my-agent -p "Hello"

# Start the API server (the CLI default is port 8080)
polkagent serve --host 127.0.0.1 --port 8080

# Launch the TUI
polkagent tui
```

## Docker

The canonical image starts the current CLI contract explicitly:

```text
polkagent serve --host 0.0.0.0 --port 8080
```

It runs as the unprivileged `polkagent` user, writes SQLite state to
`/data/polkagent.db`, exposes container port `8080`, and uses
`GET /health/ready` for its Docker healthcheck. `Dockerfile.api` is retained
for compatibility with existing build commands and has the same runtime
contract. New deployments should use `Dockerfile`. Builds use Rust 1.91 and
`cargo build --locked`; this is compatible with the current dependency lock.
The workspace-wide Rust 1.89 MSRV is validated independently by the CI
workspace matrix.

### Docker Compose

Start the default single-instance SQLite deployment and wait for readiness:

```bash
docker compose up --detach --build --wait
curl --fail http://127.0.0.1:8080/health/ready
```

The host port can be changed without changing the port inside the container:

```bash
POLKAGENT_HTTP_PORT=9090 docker compose up --detach --build --wait
curl --fail http://127.0.0.1:9090/health/ready
```

Stop the service without deleting its named data volume:

```bash
docker compose down
```

Delete the named volume only when its SQLite data is intentionally disposable:

```bash
docker compose down --volumes
```

### Container smoke validation

Run the same automated smoke path used by CI:

```bash
./scripts/container-smoke.sh
```

The script validates the Compose model, builds and starts the image, waits for
the container healthcheck, probes `/health/live`, `/health/ready`, and
`/health/startup` through the published port, verifies that the configured
user and running process UID are non-root, and verifies that SQLite created
`/data/polkagent.db`. It uses an isolated Compose project and removes its test
container, volume, network, and locally tagged image on exit.

This is deliberately a boot/health smoke test. It does **not** prove that all
API routes use durable stores, that PostgreSQL works, or that auth, HA,
backup/restore, upgrades, and crash recovery are production-ready.

### Development Compose

`docker-compose.dev.yml` mounts the source directory and uses Cargo registry
and target caches. It serves the API on host port `8080` by default:

```bash
docker compose -f docker-compose.dev.yml up
```

Use `POLKAGENT_HTTP_PORT=9090` when another process already owns port `8080`.

### Current configuration contract

For the current `serve` implementation, bind host and port are explicit CLI
arguments. The container sets the one deployment environment variable that
the command directly resolves today:

```bash
POLKAGENT_DATABASE_SQLITE_PATH=/data/polkagent.db
```

Do not rely on the retired `serve --api` flag or on
`POLKAGENT_API_BIND`/`POLKAGENT_SERVER_BIND` to override the listener in this
image. Runtime-wide configuration and durable production composition remain
tracked by FND-01 and API-01 in the implementation backlog.

## Database Options

```mermaid
flowchart TD
    A{Deployment type?} -->|"Single instance"| B[SQLite]
    A -->|"Multi-instance"| C[PostgreSQL]
    A -->|"Development"| D[SQLite with WAL]

    B --> E{Data volume?}
    E -->|"< 10 GB"| F["SQLite with WAL mode<br/>busy_timeout_ms = 5000"]
    E -->|"> 10 GB"| G["Consider migrating<br/>to PostgreSQL"]

    C --> H{High availability?}
    H -->|"Yes"| I["PostgreSQL with<br/>ssl_mode = require<br/>max_connections = 20"]
    H -->|"No"| J["PostgreSQL single<br/>instance"]
```

### SQLite (default)

Suitable for single-instance deployments.

```toml
[database]
backend = "sqlite"

[database.sqlite]
path = "~/.local/share/polkagent/polkagent.db"
wal_mode = true
busy_timeout_ms = 5000
checkpoint_on_startup = false
```

### PostgreSQL

For production and multi-instance deployments.

```toml
[database]
backend = "postgres"

[database.postgres]
url = "postgresql://user:pass@host:5432/polkagent"
max_connections = 20
ssl_mode = "require"
```

Set the URL via environment variable: `POLKAGENT_DATABASE_POSTGRES_URL`

## Health Probes

Three health endpoints are available without authentication:

| Endpoint | Purpose |
|----------|---------|
| `GET /health/live` | Liveness — process is running |
| `GET /health/ready` | Readiness — can serve requests |
| `GET /health/startup` | Startup — initialization complete |

The shipped Dockerfiles and Compose service probe readiness over HTTP:

```dockerfile
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=6 \
  CMD ["curl", "--fail", "--silent", "--show-error", \
       "http://127.0.0.1:8080/health/ready"]
```

Or with HTTP probes in Kubernetes:

```yaml
livenessProbe:
  httpGet:
    path: /health/live
    port: 8080
readinessProbe:
  httpGet:
    path: /health/ready
    port: 8080
startupProbe:
  httpGet:
    path: /health/startup
    port: 8080
```

## Monitoring

```mermaid
graph LR
    subgraph Polkagent
        APP["polkagent<br/>Application"]
        MET["/metrics<br/>Prometheus"]
        HLT["/health/*<br/>Probes"]
    end

    subgraph Collectors
        OTLP["OTLP Collector"]
        PROM["Prometheus<br/>Scraper"]
    end

    subgraph Backends
        JAE["Jaeger<br/>(Traces)"]
        GRA["Grafana<br/>(Dashboards)"]
        LOG["Log Aggregator"]
    end

    APP -->|"gRPC traces + metrics"| OTLP
    MET -->|"scrape /metrics"| PROM
    APP -->|"JSON logs"| LOG
    OTLP --> JAE
    PROM --> GRA
    OTLP --> GRA
    HLT -->|"k8s probes"| GRA
```

### Prometheus Metrics

Available at `GET /metrics` in Prometheus exposition format.

### OpenTelemetry

```toml
[observability]
otlp_endpoint = "http://otel-collector:4317"
otlp_protocol = "grpc"
metrics_enabled = true
traces_enabled = true
service_name = "polkagent"
```

### CLI Monitoring

```bash
polkagent status     # Agent count, active runs, effect queue depth
polkagent doctor     # System health checks
polkagent logs       # Tail event log
```

## TLS

```toml
[server.tls]
cert_path = "/path/to/cert.pem"
key_path = "/path/to/key.pem"
ca_path = "/path/to/ca.pem"
```
