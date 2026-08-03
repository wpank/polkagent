# Deployment

## Running Locally

```bash
# Install from source
cargo install --path crates/polkagent-cli

# Run the CLI
polkagent run -a my-agent -p "Hello"

# Start the API server
polkagent serve --port 9090

# Launch the TUI
polkagent tui
```

## Docker

### Dockerfile

The project includes two Dockerfiles.

**`Dockerfile`** (general purpose):
- Build stage: `rust:1.80-bookworm`, builds `polkagent-cli` in release mode
- Runtime stage: `debian:bookworm-slim` with `ca-certificates`
- Runs as non-root `polkagent` user
- Data volume at `/data`
- Default SQLite path: `/data/polkagent.db`
- Exposes port 9090
- Default command: `polkagent serve`

```bash
docker build -t polkagent .
docker run -p 9090:9090 -v polkagent-data:/data polkagent
```

**`Dockerfile.api`** (API-focused):
- Same build process
- Enables API by default (`POLKAGENT_API_ENABLED=true`, `POLKAGENT_API_BIND=0.0.0.0:9090`)
- Includes health check: `polkagent doctor` every 30s
- Default command: `polkagent serve --api`

### docker-compose.yml

```yaml
services:
  polkagent:
    build: .
    ports:
      - "9090:9090"
    volumes:
      - polkagent-data:/data
      - ./fixtures:/fixtures:ro
    environment:
      - POLKAGENT_LOG_LEVEL=info
      - POLKAGENT_API_ENABLED=true
      - POLKAGENT_API_BIND=0.0.0.0:9090
    restart: unless-stopped

volumes:
  polkagent-data:
```

```bash
docker compose up -d
```

### Development Compose

`docker-compose.dev.yml` mounts the source directory and uses cargo caching:

```yaml
services:
  polkagent:
    build:
      context: .
      dockerfile: Dockerfile
      target: builder
    command: cargo run -p polkagent-cli -- serve --api
    volumes:
      - .:/build
      - cargo-cache:/usr/local/cargo/registry
      - target-cache:/build/target
    environment:
      - RUST_LOG=debug
      - POLKAGENT_LOG_FORMAT=pretty
```

```bash
docker compose -f docker-compose.dev.yml up
```

## Environment Variables for Production

```bash
# Logging
POLKAGENT_LOG_LEVEL=info
POLKAGENT_LOG_FORMAT=json

# Database
POLKAGENT_DATABASE_BACKEND=postgres
POLKAGENT_DATABASE_POSTGRES_URL=postgresql://user:pass@host:5432/polkagent

# Server
POLKAGENT_SERVER_BIND=0.0.0.0:9090
POLKAGENT_AUTH_ENABLED=true

# API keys (at least one provider)
ANTHROPIC_API_KEY=sk-ant-...

# Budget limits
POLKAGENT_EXECUTION_BUDGET_MAX_USD_PER_RUN=5.00
POLKAGENT_EXECUTION_BUDGET_MAX_USD_PER_DAY=50.00

# Rate limiting
POLKAGENT_SERVER_RATE_LIMIT_RPS=100
POLKAGENT_SERVER_RATE_LIMIT_BURST=200

# Observability
POLKAGENT_OTLP_ENDPOINT=http://otel-collector:4317
POLKAGENT_OBSERVABILITY_SERVICE_NAME=polkagent-prod
```

## Database Options

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

Use in Docker:

```dockerfile
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD ["/usr/local/bin/polkagent", "doctor"]
```

Or with HTTP probes in Kubernetes:

```yaml
livenessProbe:
  httpGet:
    path: /health/live
    port: 9090
readinessProbe:
  httpGet:
    path: /health/ready
    port: 9090
startupProbe:
  httpGet:
    path: /health/startup
    port: 9090
```

## Monitoring

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
