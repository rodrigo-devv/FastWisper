## What changed

<!-- A short description of the change and why it is needed. Link the issue if there is one. -->

## How to test

<!-- Commands or steps a reviewer can follow. -->

## Checklist

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --no-default-features -- -D warnings`
- [ ] `cargo test --workspace --locked --no-default-features`
- [ ] If I touched `ui/`: `cd ui && npm ci && npm run check && npm run build`, plus `cargo clippy` and `cargo test` with `--manifest-path ui/src-tauri/Cargo.toml`
- [ ] If I touched packaging or scripts: the matching `packaging/omarchy/tests/*.sh`, `scripts/*/tests/contract.sh`
- [ ] User-facing text is in both message catalogs (English and pt-BR)
- [ ] **Privacy:** this change never sends dictated text, audio, window titles or app names off the machine. A remote LLM provider stays explicit opt-in and text-only.
- [ ] No personal data in code, tests, logs or screenshots (use `/home/user`, `example-app`)
