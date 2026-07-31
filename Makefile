.PHONY: build test check lint fmt clean docker

build:
	cargo build --workspace

test:
	cargo test --workspace

check:
	cargo check --workspace

lint:
	cargo clippy --workspace -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clean:
	cargo clean

docker:
	docker build -t polkagent .

docker-api:
	docker build -t polkagent-api -f Dockerfile.api .

docker-compose:
	docker compose up -d

release:
	cargo build --release -p polkagent-cli

install:
	cargo install --path crates/polkagent-cli

docs:
	cargo doc --workspace --no-deps --open
