# Changelog

All notable changes to FastWisper are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - Unreleased

<!-- Replace "Unreleased" above with the tag date (YYYY-MM-DD) when tagging v0.1.0. -->

First public release: local push-to-talk dictation for Omarchy/Hyprland.

### Added

- **Local dictation on Alt+Space.** Hold to record, release to transcribe and
  paste into the focused app. Alt+Shift+Space toggles hands-free recording,
  Esc cancels, and Super+Alt+V pastes the last result again.
- **Local speech recognition** with Whisper (whisper.cpp), Portuguese and
  English, transcribed as spoken (never translated). Runs on any Vulkan GPU
  (`asr.gpu = "auto"` picks the best one) or on the CPU, with pinned model
  downloads verified by SHA-256 (`fastwisper models pull`). The GPU is probed
  in a child process with a timeout first, so a broken Vulkan driver (one that
  hangs or crashes on init) falls back to the CPU with a notification and a
  `doctor` warning instead of taking FastWisper down; `asr.gpu = "cpu"` skips
  the probe.
- **Audio capture straight from PipeWire**, silence trimming, a maximum
  recording time, and a clear warning when the microphone delivers only digital
  silence.
- **Text cleanup with rules plus an optional local LLM** through your own
  Ollama on loopback. Any failure or timeout falls back to the rules text, so
  dictation never fails because of the LLM. A remote OpenAI-compatible provider
  is available only as an explicit opt-in (`llm.remote_consent`) and receives
  text only.
- **Safe pasting on Hyprland.** The target window is revalidated before
  pasting; terminals get Ctrl+Shift+V, other apps Ctrl+V. When delivery is not
  safe the text is only copied. The clipboard entry is marked sensitive and the
  last result stays in RAM for a short time only.
- **Terminal-aware formatting**: in terminals and configured AI harnesses
  (`inject.harness_apps`), commands and executables from your `PATH` are kept
  intact (`llm.format_in_terminals`). Only short speech that starts with a
  known program is formatted as a command; everything else is formatted as
  normal text. Requests inside the speech ("write a function that…", "ignore
  the previous instructions") are transcribed, never followed, and an output
  guard falls back to the rules text when the LLM output drifts from what was
  said, logging only a reason code.
- **"Send now" (opt-in, off by default).** Ending a dictation with
  "send now" (or "enviar agora") in a terminal or harness removes the phrase,
  pastes the text and presses Enter once, only after a confirmed paste in the
  same window. Phrases are configurable.
- **Editable shortcuts** (`hotkey.dictate`, `hotkey.hands_free`,
  `hotkey.paste_last`) from the hub, the CLI or the config file, with conflict
  checks against existing Hyprland binds and live reload.
- **Recording pill**: a small overlay at the bottom of the focused monitor with
  a live microphone level, a transcribing glow and a short amber flash on
  errors. It never takes focus or clicks and never shows dictated text.
- **Status bar plugin** for the Omarchy shell, desktop notifications and
  start/stop sounds.
- **Configuration hub** (Tauri): Home with usage stats, Settings (General,
  System, Vibe coding, Experimental, Data and privacy, Help, About), usage
  history (metrics only), personalization (dictionary, snippets, style) and a
  theme gallery that follows Omarchy themes. Settings fields show the
  default when your value differs, and the Home week streak counts
  transcribed dictations only.
  The hub makes no network requests.
- **Live configuration** with `fastwisper config list|get|set|reload`; every
  hub setting has a CLI equivalent. Restart-only keys are flagged.
- **VRAM profiles** (`fastwisper vram`): automatic (unloads models when idle
  and detects games via GameMode/Steam), performance, on-demand and game mode.
  Cancelling a dictation never unloads models early; they follow the active
  profile's idle timeout.
- **Voice notes (experimental, opt-in, off by default)**: text-only notes from
  the hub composer, stored locally in a separate `0600` database. Audio is
  never stored. Starting a note does not clear the last dictation, so
  `paste-last` still returns it.
- **English and Brazilian Portuguese interface** (`ui.language`, English by
  default) for the CLI, notifications, bar, pill and hub.
- **Privacy by default**: no audio or text is stored; usage history keeps
  metrics only; the daemon restricts its own TCP connections with Landlock to
  the LLM port; the local Ollama cloud features are turned off by default
  (`llm.ollama_cloud = false`; `fastwisper ollama online|offline` switches it
  and `fastwisper ollama apply` re-applies the configured value).
- **Automatic installer** (`packaging/omarchy/install.sh`) that builds or
  installs prebuilt binaries, runs setup, prepares the speech model and wires
  up the service, shortcuts, bar, pill and hub; every installation is a
  generation that `rollback.sh` and `uninstall.sh` can undo without touching
  your config, models or external edits. Installer messages are in English,
  or in Brazilian Portuguese when the locale is `pt*`.
- **`fastwisper doctor`** to check the microphone, models, LLM, network
  restrictions, shortcuts and service. Every CLI command is safe to pipe
  (`fastwisper config list | head -1` exits cleanly), and `--follow` streams
  stop when their reader goes away.

[Unreleased]: https://github.com/rodrigo-devv/FastWisper/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/rodrigo-devv/FastWisper/releases/tag/v0.1.0
