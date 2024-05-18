
PYTHON=python3
down:
	docker-compose down
stop:
	make down
up:
	docker-compose up -d
restart:
	make down && make up
start:
	make start
enter:
	docker exec -it subspace bash
chmod_scripts:
	chmod +x ./scripts/*.sh

compose:
	docker-compose up -d ${service}

RUST_LOG ?= info,pallet_subspace::migrations=debug

try-runtime-upgrade:
	cargo build --release --features try-runtime
	RUST_BACKTRACE=1; RUST_LOG="${RUST_LOG}"; try-runtime --runtime target/release/wbuild/node-subspace-runtime/node_subspace_runtime.compact.compressed.wasm on-runtime-upgrade live --uri wss://commune-api-node-0.communeai.net:443

generate-benchmark:
	cargo build -r --features runtime-benchmarks
	./target/release/node-subspace build-spec --disable-default-bootnode --chain local > specs/benchmark.json
	./target/release/node-subspace benchmark pallet --chain specs/benchmark.json --pallet pallet_subspace --extrinsic "*" --steps 20 --repeat 10 --output pallets/subspace/src/autogen_benchmark.rs