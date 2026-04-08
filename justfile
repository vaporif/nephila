default:
    @just --list

check: clippy test check-fmt lint

lint: lint-toml check-typos check-nix-fmt lint-actions

fmt: fmt-rust fmt-toml fmt-nix

build *args:
    cargo build --workspace {{args}}

clippy:
    cargo clippy --workspace -- -D warnings

test:
    cargo nextest run --workspace

coverage:
    cargo llvm-cov nextest --workspace --lcov --output-path lcov.info

coverage-html:
    cargo llvm-cov nextest --workspace --html

check-fmt:
    cargo fmt --all -- --check

fmt-rust:
    cargo fmt --all

lint-toml:
    taplo check

fmt-toml:
    taplo fmt

check-nix-fmt:
    alejandra --check flake.nix nix/

fmt-nix:
    alejandra flake.nix nix/

check-typos:
    typos

lint-actions:
    actionlint

run *args:
    cargo run -p nephila -- {{args}}
