# Security Policy

## Supported versions

Only the latest release receives security fixes.

## Reporting a vulnerability

Please **do not open a public issue**. Report it privately through GitHub Security Advisories:
[Report a vulnerability](https://github.com/s3xz/FastWisper/security/advisories/new) (repository **Security** tab → **Report a vulnerability**).

Include the affected version (`fastwisper --version`), what an attacker can do, and the steps to reproduce. Do not include real dictated text or other personal data; synthetic examples are enough.

You should get an acknowledgement within 7 days. Once a fix is released, the advisory is published with credit to you, unless you prefer to stay anonymous.

## Scope

In scope:

- Dictated text, audio, window titles or app classes leaving the machine without the explicit remote-provider opt-in, or being persisted when the configuration says they should not be.
- Pasting into a window other than the revalidated target, or sending keys (such as Enter) outside the documented opt-in behavior.
- The daemon's IPC socket, runtime directory and file permissions (for example access by other local users).
- The installer, rollback and uninstall scripts, and the release tarball and its checksums.
- The configuration hub (Tauri) and its IPC with the daemon.

Out of scope:

- Vulnerabilities in third-party software FastWisper uses, such as Ollama, whisper.cpp, Hyprland, PipeWire or a remote LLM provider you configured; please report those upstream.
- Clipboard managers or other local software keeping copies of pasted text.
- Attacks that require control of the user's account or root on the same machine.
