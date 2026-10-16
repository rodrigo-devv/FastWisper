# Contributing to FastWisper

Thanks for your interest! FastWisper is local voice dictation for Linux (Hyprland), written in Rust, with a Tauri/Svelte configuration hub. Bug reports, fixes and ideas are welcome.

## Ground rules

- **Everything runs locally.** Speech recognition and the default LLM run on the user's machine (the LLM over loopback). No telemetry, no account, no network at run time by default.
- **Dictated text and audio never leave the machine.** Audio is never sent anywhere. A remote LLM provider is explicit opt-in (`llm.engine = "openai_compatible"` plus `llm.remote_consent = true`) and only receives the text to format, never audio, window titles or app classes. A pull request that changes this will not be merged.
- **Nothing is stored by default.** Text and audio are not persisted unless the user opts in (voice notes); usage history keeps metrics only.
- **Dictation never fails because of the LLM:** on timeout or error it falls back to the built-in text rules.
- Never send Enter or other keys automatically outside the documented opt-in "send now" behavior, and never paste into a window that was not revalidated.
- User-facing messages exist in English and Brazilian Portuguese; add new strings to both catalogs. Code identifiers are in English.

## Building

Requirements are listed in the [README](README.md#requirements). On Omarchy/Arch:

```bash
sudo pacman -S --needed base-devel rustup clang cmake pkgconf git \
  pipewire alsa-lib vulkan-headers vulkan-icd-loader shaderc \
  wl-clipboard libnotify jq nodejs npm webkit2gtk-4.1 \
  lua python socat iproute2
rustup default stable

cargo build                          # GPU build (Vulkan, the default)
cargo build --no-default-features    # CPU only, what CI uses
```

## Running the checks

CI runs these on every pull request; please run them locally first:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace --locked --no-default-features
bash scripts/network/tests/contract.sh
bash scripts/release/tests/contract.sh
bash scripts/public/leak-scan.sh .
```

If you touched the hub (`ui/`):

```bash
cd ui && npm ci && npm run check && npm run build && cd ..
cargo fmt --manifest-path ui/src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path ui/src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path ui/src-tauri/Cargo.toml --locked
bash packaging/omarchy/tests/hub.sh
```

Tests marked `#[ignore]` need a real speech model, GPU, Ollama or PipeWire and do not run in CI. Tests never touch your real config: they use temporary `HOME`, `XDG_*` and `XDG_RUNTIME_DIR` directories, and they must never send keys to a real window.

## Commits and pull requests

- Use [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): summary`, for example `fix(paste): revalidate the target window after the LLM`. Common types: `feat`, `fix`, `docs`, `test`, `refactor`, `ci`, `chore`.
- Keep pull requests focused on one change and fill in the template checklist.
- A maintainer review and green CI are required before merge. CI for pull requests from new contributors starts after a maintainer approves it.

## Reporting bugs without personal data

Open an issue with the bug template and include `fastwisper --version` and the output of `fastwisper doctor`. **Do not paste dictated text, audio, window titles or app names.** Describe the situation in general terms instead ("pasting into a terminal"), and review logs before posting to remove your user name or device names.

Security issues should be reported privately; see [SECURITY.md](SECURITY.md).

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By participating you agree to uphold it.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
