<div align="center">

<img src="ui/src-tauri/icons/source.svg" alt="FastWisper icon" width="96" height="96">

# FastWisper

**Private voice dictation for Linux. Hold a key, speak, let go: your words are typed into the app you were using, and nothing leaves your machine.**

[![CI](https://github.com/s3xz/FastWisper/actions/workflows/ci.yml/badge.svg)](https://github.com/s3xz/FastWisper/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/s3xz/FastWisper?include_prereleases&sort=semver)](https://github.com/s3xz/FastWisper/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Linux%20%C2%B7%20Hyprland%20%C2%B7%20Omarchy-8b5cf6)](#requirements)

**English** · [Português (Brasil)](README.pt-BR.md)

<img src="docs/images/hub-home.png" alt="The FastWisper hub: Home page with the sidebar, the week streak, recent voice notes and the note composer" width="860">

</div>

FastWisper is local voice dictation in English and Portuguese for [Omarchy](https://omarchy.org) and Hyprland, written in Rust. Whisper transcribes on your GPU or CPU, built-in rules and an optional local LLM tidy up the text, and the result is pasted into the window you were using. When pasting is not safe, the text is kept ready to copy instead. No account, no subscription, no cloud.

## Why FastWisper

- **100% local and private.** Speech recognition runs on your machine (whisper.cpp, Vulkan or CPU), and the optional LLM runs in your own Ollama over loopback. No account and no telemetry. Text and audio are not stored by default.
- **Pastes only where you meant.** The window you started dictating in is checked again before pasting. If the focus changed, the screen is locked or the app is copy-only, the text goes to the clipboard instead. FastWisper never presses Enter on its own.
- **Optional local LLM, never a point of failure.** A small local model fixes punctuation and style. If it is slow, missing or produces something you did not say, you get the rule-based text instead: dictation never fails because of the LLM.
- **Plays nice with games.** VRAM profiles load models on demand; the automatic profile notices games (GameMode, Steam) and frees the GPU for them.
- **Made for vibe coding.** Terminals and AI coding assistants get command-aware formatting, and the opt-in "send now" phrase pastes and submits your prompt hands-free.
- **Yours to configure.** Editable shortcuts, a local configuration hub, a CLI that covers every option, and an interface in English or Brazilian Portuguese.

## Quick start

On Omarchy, with the [build dependencies](#requirements) installed (one `pacman` line) and `~/.local/bin` on your `PATH`, run this from a terminal **inside your Hyprland session**:

```bash
git clone https://github.com/s3xz/FastWisper.git && cd FastWisper
packaging/omarchy/install.sh
fastwisper doctor
```

Then hold **Alt+Space**, speak, and let go. That's it.

The installer builds the current code (`cargo build --release`, Vulkan by default), configures the microphone, downloads the speech model with a pinned checksum, turns the local Ollama cloud features off, enables the user service and wires up the shortcuts, the bar icon, the recording pill and the configuration hub. It never needs `sudo` for FastWisper itself; it only asks for your password if a system-wide Ollama service has to be switched offline. Its messages are in English, or in Brazilian Portuguese when your locale is Portuguese (`LC_ALL`, `LC_MESSAGES` or `LANG` starting with `pt`, e.g. `LANG=pt_BR.UTF-8 packaging/omarchy/install.sh`). To update, pull and run the installer again.

<details>
<summary><b>Install from the release tarball (no compiling)</b></summary>

The release ships prebuilt binaries (`fastwisper`, `fastwisperd` and `fastwisper-hub`). Download the tarball and `SHA256SUMS` from the [releases page](https://github.com/s3xz/FastWisper/releases), check them and run the bundled installer:

```bash
sha256sum -c SHA256SUMS
tar -xzf fastwisper-0.1.0-x86_64-linux.tar.gz
cd fastwisper-0.1.0-x86_64-linux
./install.sh
fastwisper doctor
```

Keep the extracted folder: its `packaging/omarchy/rollback.sh` and `uninstall.sh` undo the installation. The release binaries are built with Vulkan on Arch Linux and fall back to the CPU when there is no working Vulkan driver; `vulkan-icd-loader` must still be installed, because the binaries link `libvulkan.so.1`. You need the runtime tools from [Requirements](#requirements), not the build dependencies.

</details>

<details>
<summary><b>CPU-only build</b></summary>

No Vulkan headers or `shaderc` needed. Build the binaries and the hub yourself and point the installer at them; it then installs without compiling:

```bash
cargo build --release --locked --no-default-features
(cd ui && npm ci && npm run tauri -- build --no-bundle -- --locked)
FASTWISPER_BUILD_DIR="$PWD/target/release" FASTWISPER_HUB_BUILD_DIR="$PWD/ui/src-tauri/target/release" \
  packaging/omarchy/install.sh
```

Without `FASTWISPER_HUB_BUILD_DIR`, an external build installs without the hub.

</details>

<details>
<summary><b>Installer options</b></summary>

- `--offline`: downloads no models, and the hub build only uses the npm/cargo cache.
- `--no-activate`: only installs files (no setup, downloads or activation).
- `--no-hub`: skips the hub (it does not remove an already installed hub).

They can be combined, and the tarball's `./install.sh` accepts the same options. If Node.js/npm or WebKit is missing, or the hub build fails, the installer warns and dictation keeps working without the hub. It migrates a recognized legacy integration automatically and restarts the shell when the active QML changed. `fastwisper models pull` without arguments downloads the configured speech model; the daemon and `fastwisper setup` never download models.

</details>

## Usage

| Shortcut (default) | Action |
|---|---|
| <kbd>Alt</kbd>+<kbd>Space</kbd>, hold | Records; releasing the key or a modifier stops, transcribes and pastes |
| <kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>Space</kbd> | Toggles hands-free recording |
| <kbd>Esc</kbd> while recording | Cancels (fixed) |
| <kbd>Super</kbd>+<kbd>Alt</kbd>+<kbd>V</kbd> | Pastes the last result again into the focused window |

Terminals get <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd>; other apps get <kbd>Ctrl</kbd>+<kbd>V</kbd>. The last result stays in RAM for 2 minutes (`fastwisper ctl copy-last` / `paste-last`). Transcription keeps the language you spoke (English or Portuguese) and never translates.

### The recording pill

<img src="docs/images/pill-recording.png" alt="The recording pill: a small dark capsule with a wave that follows the microphone level" width="320">

While recording, a pill appears at the bottom center of the focused monitor. The wave follows the microphone level and adapts to each microphone's background noise, without calibration; while transcribing it glows; if dictation fails (missing or muted microphone, for example) it turns amber for 1.5 s and disappears. It never takes focus or clicks and never shows the dictated text. It is the `fastwisper.pill` plugin of the Omarchy shell, fed by `fastwisper meter --follow`: one number between 0 and 1 per frame (~25 Hz), only while recording. No audio leaves the daemon. Hide it with `fastwisper config set overlay.enabled false`.

### Terminals and AI coding assistants

Claude Code, Codex and similar tools running inside Ghostty, Kitty, Alacritty and other terminals are recognized through the terminal (window class, Hyprland `terminal` tag or executable). With `llm.format_in_terminals = true` (the default):

- A **short command** (up to 12 words, one line, not a question) that starts with a known program is written as a command: lowercase, no trailing period, program names corrected (for example "git status", "cargo build --release").
- **Everything else is text**, formatted like in any other app, with a capital letter and punctuation.
- **Your speech is always dictated, never obeyed.** A request such as "write a function that adds two numbers" is typed as you said it, not answered. If the LLM output adds code, a preamble or words you did not say, FastWisper uses the rule-based text instead. Very long dictations (over 2,400 characters) skip the LLM.

Assistants with their own desktop window use <kbd>Ctrl</kbd>+<kbd>V</kbd>; list their window classes in `inject.harness_apps` to give them the same formatting (see [Configuration](#configuration)).

### "Send now" (opt-in)

End a dictation in a terminal or AI assistant with **"send now"** (or "enviar agora" in Portuguese) and FastWisper pastes the text and presses **one Enter** for you. It is off by default: turn it on in Settings › Vibe coding or with `fastwisper config set inject.send_now true`.

- The phrase is removed before the LLM; case, accents and trailing punctuation are ignored. In other apps it stays as ordinary text.
- Enter is sent only after an accepted paste, 200 ms later, into the same window, still focused and unlocked. Never in copy-only mode, after a focus change, on a clipboard fallback or for `paste-last`/`copy-last`.
- After a negation ("don't send now", "não vou enviar agora") nothing is sent: the whole text is pasted without Enter. If only the phrase is left, nothing is pasted or sent.
- Phrases are configurable per language: `fastwisper config set inject.send_now_phrases '{ pt = ["enviar agora"], en = ["send now"] }'` (up to 16 phrases per language, 64 characters each, letters, digits and spaces; an empty list turns that language off). The phrases of every language in `asr.languages` apply. Pick phrases that do not end your normal sentences.

### The hub

`fastwisper hub` opens the local configuration hub, also available from the app launcher and from a right click on the bar icon.

<table>
  <tr>
    <td width="50%"><img src="docs/images/hub-settings-general.png" alt="Settings › General: FastWisper language, theme and dictation shortcuts"></td>
    <td width="50%"><img src="docs/images/hub-themes.png" alt="Themes: Automatic, Dark, Light and the installed Omarchy palettes"></td>
  </tr>
  <tr>
    <td align="center">Settings › General</td>
    <td align="center">Themes</td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/images/hub-history.png" alt="History: dictation counts, latency percentiles and delivery, without any text"></td>
    <td width="50%"><img src="docs/images/hub-vibe-coding.png" alt="Settings › Vibe coding: terminal formatting, recognized terminals and send now"></td>
  </tr>
  <tr>
    <td align="center">History (metrics only, never text)</td>
    <td align="center">Settings › Vibe coding</td>
  </tr>
</table>

- **Home:** week streak (transcribed dictations only), words and WPM, recent voice notes and the note composer.
- **Customization:** Dictionary, Snippets and Style.
- **Settings:** General (language, theme, shortcuts, microphone, sounds and pill), System (transcription on GPU or CPU, AI, VRAM and maintenance), Vibe coding, Experimental, Data and privacy, Help and About. Fields show "Default: X" when your value differs from the default; fields that need a restart show a restart button.
- **History:** usage metrics over 7, 30 or 90 days: counts, p50/p95 latency and deliveries per app class. Never text.
- **Themes:** Automatic, Dark, Light and the installed Omarchy palettes; the choice only changes the hub's colors, live.

Deep links (page names are in Portuguese): `fastwisper hub inicio|notas|historico|temas|personalizacao|dicionario|snippets|estilo` opens a page; `fastwisper hub configuracoes|geral|sistema|vibe-coding|experimental|privacidade|ajuda|sobre` opens a settings tab. Pass **one** name per command. If the hub is not installed, `fastwisper hub` explains why (`--no-hub`, missing `node`/`npm` or `webkit2gtk-4.1`, a failed build or an external build without the hub).

The hub uses local fonts and icons and a strict CSP, and its webview has no network access. It never shows the text of regular dictation or window titles.

### Voice notes (experimental, opt-in)

Off by default (`experimental.notes = false`). Turn them on in Settings › Experimental or with `fastwisper config set experimental.notes true`. In the Home composer, click the microphone to start and again to save; Esc cancels. Each note can be copied or deleted, and the list has search, a compact view and refresh.

Notes are the only text FastWisper stores: in `$XDG_STATE_HOME/fastwisper/notes/notes.sqlite` (fallback `~/.local/state/fastwisper/notes/notes.sqlite`), a `0600` file in a `0700` directory, separate from the metrics and created only on the first save. **Audio is never stored, and Alt+Space dictation still never stores text.** Starting a note does not clear your last dictation. `fastwisper notes list` lists them; `fastwisper notes clear` deletes them all after confirmation (`--yes` in scripts). Turning the feature off hides the notes but keeps the database. Notes go through the same rules and formatting as dictation: if you consented to a remote provider, it may also receive the note text.

## Configuration

Everything can be changed in the hub or from the CLI. The config lives in `~/.config/fastwisper/config.toml` (or `$XDG_CONFIG_HOME/fastwisper/config.toml`); see the [annotated example](config/config.example.toml).

```bash
fastwisper config list                          # every key, and its default when different
fastwisper config get llm.model
fastwisper config set llm.style formal          # validated, keeps your comments, applied live
fastwisper config set hotkey.dictate SUPER+D    # several pairs at once: KEY VALUE KEY VALUE
fastwisper config reload                        # after editing the file by hand
fastwisper restart                              # for keys read only at startup
```

An invalid config is refused and the previous one stays active. Keys that only apply after a restart (LLM endpoint/engine/consent, Whisper model/GPU/threads, `hotkey.min_recording_ms`) are flagged.

<details>
<summary><b>Language, microphone and transcription</b></summary>

- **FastWisper language:** `ui.language = "en"` (default) or `"pt-BR"`, in Settings › General or with `fastwisper config set ui.language pt-BR`. It changes the hub live and, after a reload, notifications, the bar/pill and the CLI's human output. It does not change `asr.languages = ["pt", "en"]` (the languages accepted for transcription) or the "send now" phrases.
- **Microphone:** `fastwisper audio sources` lists the PipeWire nodes; `fastwisper setup --device NODE_NAME` picks an exact one. Without it, setup uses the system default; ambiguity is an error. FastWisper never switches microphones silently. `fastwisper audio test` records a few seconds and shows the level (nothing is transcribed or pasted); `fastwisper audio test-sound` plays the start and stop sounds.
- **Transcription on GPU or CPU:** `asr.gpu = "auto"` prefers dedicated Vulkan GPUs; `"cpu"` never opens the GPU driver. It also accepts the name or index of a Vulkan GPU (`fastwisper gpus` lists them). `fastwisper setup` picks the GPU model (`large-v3-turbo-q5_0`) or the lighter CPU model (`small-q5_1`); `fastwisper models use ID` switches models from the manifest.

</details>

<details>
<summary><b>Shortcuts</b></summary>

In Settings › General › Dictation (Change → press the combination → preview → Save; Restore default per row), or `fastwisper config set hotkey.dictate|hotkey.hands_free|hotkey.paste_last COMBO`. Format: modifiers (`SUPER`, `CTRL`, `ALT`, `SHIFT`) plus one key, with at least one modifier other than Shift; a bare key or Shift+key is refused because Hyprland would swallow your typing. The three shortcuts must differ, and Esc is fixed.

Before saving, FastWisper reads `hyprctl binds -j`: if another program uses the same combination, it warns and asks for confirmation (`--yes` in scripts). The change applies immediately: `$XDG_DATA_HOME/fastwisper/hotkeys.conf` is regenerated and Hyprland reloads. `fastwisper hotkeys list|check|apply|keys` shows, checks without writing, regenerates and lists the accepted keys. Reloading Hyprland while a recording is held cancels it (nothing is pasted).

</details>

<details>
<summary><b>LLM formatting (local or remote)</b></summary>

- **Local (default):** your Ollama at `http://127.0.0.1:11434`. Any installed model can be set in `llm.model`; we recommend `qwen3:4b-instruct` (`fastwisper ollama models` lists yours). A model outside the pin only produces a warning. `llm.enabled = false` uses only the rules; `llm.style` is `formal`, `casual`, `very_casual` or `excited`.
- **Remote provider, opt-in:** `llm.engine = "openai_compatible"`, an HTTPS endpoint, a model and `llm.remote_consent = true`. Provide the key with `llm.api_key_file` (a `0600` file) or `llm.api_key_env` in the service environment. The provider only receives text to format, never audio, window titles or app classes. Without consent, a non-loopback endpoint is refused.

</details>

<details>
<summary><b>Apps, terminals and AI assistants</b></summary>

- `inject.paste_apps = ["*"]` allows pasting into any validated app; a list restricts the targets. `inject.copy_only_apps` takes precedence and keeps those apps copy-only.
- Unrecognized terminal? Set `inject.terminal_apps` or give the window Hyprland's `terminal` tag.
- `inject.harness_apps` (empty by default) applies the terminal formatting to AI assistants with their own window, without changing the paste shortcut. Add only classes/executables you confirmed in your session, never titles, and do not use `electron`, which matches many unrelated apps: `fastwisper config set inject.harness_apps '["CONFIRMED_CLASS"]'`. It is also under Settings › Vibe coding › Edit list.
- `llm.format_in_terminals = false` pastes literal text in terminals and assistants.

</details>

<details>
<summary><b>VRAM profiles and game mode</b></summary>

`fastwisper vram` shows the profile and memory use; `fastwisper vram automatico|desempenho|sob_demanda|jogo` (automatic, performance, on demand, game) switches and saves it. The automatic default behaves like "on demand" (models unloaded after 5 idle minutes, `performance.unload_after_secs`) and switches to game mode while a game is running: the models load on the key press and are freed right after the dictation. `fastwisper ctl unload` frees the models now.

</details>

<details>
<summary><b>Pill and hub theme</b></summary>

- `fastwisper config set overlay.enabled false` hides the pill from the next recording; `true` shows it again. `overlay.position` only accepts `"bottom"` for now.
- `fastwisper config set hub.theme auto|dark|light|omarchy:<name>` (default `auto`, which follows the Omarchy theme, or dark without one). The gallery reads `colors.toml` from `/usr/share/omarchy/themes/` and `~/.config/omarchy/themes/`, with your themes taking priority. If the theme disappears, the hub uses Automatic and keeps your choice for when it comes back.

</details>

## Privacy

**What stays on your machine:** your audio, which is transcribed locally and never written to disk; the last result, kept in RAM for 2 minutes so you can paste it again; usage metrics (counts, timings, app class, never text); and, only if you turn them on, your voice notes.

**What never leaves it:** audio, never. Text, unless you explicitly configure and consent to a remote LLM provider, which then receives only the text to format.

- "Clear usage history" and "Delete all notes" (in Data and privacy, or `fastwisper history clear` / `fastwisper notes clear`) remove the rows with SQLite `secure_delete`, checkpoint the WAL and run `VACUUM`.
- The clipboard entry is marked as sensitive, so the Omarchy clipboard history skips it; other clipboard managers may still keep copies.
- When the kernel supports it, the daemon uses Landlock to limit its own TCP connections to the LLM port. This does not cover the separate Ollama process.
- By default FastWisper turns the local Ollama cloud features off (`llm.ollama_cloud = false`): `fastwisper ollama apply` (run by the installer) or `fastwisper ollama offline` adds `OLLAMA_NO_CLOUD=1` to a drop-in of the Ollama service and restarts it; `fastwisper ollama online` undoes it; `fastwisper ollama` shows the setting and the real state, also checked by `fastwisper doctor`. The daemon itself never touches Ollama.
- The hub's About page links to the repository and the developer. The links open in your browser only when you click them; the hub itself makes no requests to them.

## Requirements

- **Desktop:** [Omarchy](https://omarchy.org) with Hyprland 0.56+ (Lua config), a Wayland session, PipeWire/WirePlumber and a working microphone. Other Hyprland setups may work but are not tested.
- **Speech recognition:** a GPU with a Vulkan driver (`vulkan-radeon` for AMD, `vulkan-intel` for Intel, `nvidia-utils` for NVIDIA) plus `vulkan-icd-loader`, or just the CPU. With `asr.gpu = "cpu"`, setup picks the CPU model as long as the config still has the factory model and it has not been downloaded; a model you chose or already downloaded is never swapped.
- **Runtime tools:** `wl-clipboard`, `libnotify` and `jq`.
- **Optional, LLM formatting:** [Ollama](https://ollama.com) running locally with a model you installed yourself (`ollama pull qwen3:4b-instruct`). Without it, FastWisper uses its built-in rules and `fastwisper doctor` shows a warning, not an error.
- **Optional, configuration hub:** `webkit2gtk-4.1` (with GTK 3). Without it, dictation works and the hub is skipped.

To build from source you also need Rust stable 1.89+ (`rustup`), a C/C++ toolchain, `clang`, `cmake`, `pkgconf`, `git` and the PipeWire, ALSA and Vulkan development files (`vulkan-headers`, `shaderc`); the hub additionally needs Node.js and npm. On Omarchy/Arch:

```bash
sudo pacman -S --needed base-devel rustup clang cmake pkgconf git \
  pipewire alsa-lib vulkan-headers vulkan-icd-loader shaderc \
  wl-clipboard libnotify jq webkit2gtk-4.1
command -v npm >/dev/null || sudo pacman -S --needed npm  # pulls nodejs only if no Node.js (e.g. nodejs-lts-*) is installed
rustup default stable
```

To run the test suite (`cargo test --workspace` and the packaging tests) you also need `lua`, `python`, `socat` and `iproute2`: `sudo pacman -S --needed lua python socat iproute2`.

## Troubleshooting and FAQ

Start with these two:

```bash
fastwisper doctor
journalctl --user -u fastwisperd
```

<details>
<summary><b>Nothing happens when I hold Alt+Space</b></summary>

Run `fastwisper doctor` and check the installer warnings. If your `bindings.lua` already binds the same combination, the installer leaves your bind alone and both fire together: pick another shortcut with `fastwisper config set hotkey.dictate SUPER+D` (or in Settings › General › Dictation) or remove yours. The installer must run inside your Hyprland session to validate the shortcuts.

</details>

<details>
<summary><b>The text is only copied, not pasted</b></summary>

FastWisper copies instead of pasting when the focus changed during the dictation, the screen is locked, there was no focused window or the app is in `inject.copy_only_apps` (or outside `inject.paste_apps`). A notification tells you which. For a terminal that is not recognized, set `inject.terminal_apps` or the `terminal` tag in Hyprland.

</details>

<details>
<summary><b>Missing microphone or model</b></summary>

Check `fastwisper audio sources`, the config and the doctor's warnings. A microphone that only delivers digital silence produces a "microphone silent" warning. To prepare the speech model, run `fastwisper models pull`. Install the LLM model in your Ollama yourself (`ollama pull qwen3:4b-instruct`).

</details>

<details>
<summary><b>Broken GPU driver?</b></summary>

FastWisper probes the Vulkan driver in a separate process before using the GPU. If the driver hangs or crashes, dictation keeps working on the CPU (slower), you get a notification, and `fastwisper doctor` shows the reason. Fix or update the driver and run `fastwisper restart`. To skip the GPU entirely, set `asr.gpu = "cpu"`.

</details>

<details>
<summary><b>My game stutters while FastWisper is loaded</b></summary>

Use `fastwisper vram jogo` (game) or keep the default `automatico`, which detects games and frees VRAM. `fastwisper ctl unload` frees it right away.

</details>

<details>
<summary><b>Ollama cloud and network isolation</b></summary>

For a system-wide Ollama service, switching it offline needs `sudo` in a terminal, or the polkit password dialog with `--pkexec` (the path the hub uses, in Settings › Data and privacy). The doctor shows Landlock active with its ABI/ports, or unavailable. When supported, it restricts the daemon's TCP connections to the LLM endpoint port; it does not restrict IPs on that port, UDP/DNS or the filesystem. A kernel without support produces a warning and the daemon keeps running.

</details>

<details>
<summary><b>Does FastWisper work outside Omarchy?</b></summary>

It targets Omarchy with Hyprland 0.56+ and its Lua config; the bar icon and the pill are Omarchy shell plugins. Other Hyprland setups may work but are not tested, and the installer exits with an error when the Omarchy shell is missing.

</details>

## Known limitations

<details>
<summary>Things that work differently from what you might expect in v0.1.0</summary>

- **Ollama isolation:** Ollama is a separate process, outside the daemon's Landlock restrictions. `llm.ollama_cloud = false` sets `OLLAMA_NO_CLOUD=1`, but Ollama's own DNS lookups and update checks are beyond FastWisper's control.
- **Keyboard layouts:** shortcuts with letters or punctuation assume a US/ABNT2 key position; AZERTY, Dvorak and other layouts have not been validated. While you record a new shortcut in the hub, global compositor binds still fire.
- **"Send now" timing:** cancelling does not interrupt the ~200 ms between the paste and the Enter; the delay is fixed and does not prove the app consumed the paste. Turning the opt-in off in the middle of a dictation pastes without the phrase and without Enter.
- **Deleting history or notes:** deletion cannot guarantee physical removal on SSDs (wear leveling/TRIM), on copy-on-write file systems (btrfs, Omarchy's default), in snapshots, backups or swap. Use full-disk encryption if that matters to you.
- **Shortcut already bound:** if your `bindings.lua` already binds FastWisper's shortcut, the installer warns and leaves your bind alone, so both fire together.
- **Help in English:** Help opens the installed English README (without images), even with `ui.language = "pt-BR"`.
- **Hardware coverage:** daily use has been on one setup (NVIDIA GPU with Vulkan and a USB microphone). AMD, Intel and CPU-only setups are covered by tests but not by daily use. The pill has not been exercised with multiple monitors, other scale factors or a bottom bar.
- **Hub accessibility:** not yet verified with a screen reader or keyboard-only navigation in the real WebKitGTK; the theme gallery uses one Tab stop per card.
- **`fastwisper hub` feedback:** the command does not wait for the hub and reports success even if the hub dies while opening; an old hub stays installed after `--no-hub` or a failed hub build.
- **Restart notices are conservative:** changing the LLM endpoint together with `llm.model` or `llm.enabled` leaves both for the restart.
- **Follow streams:** `fastwisper meter --follow` and `fastwisper status --follow` only notice a closed output when they next write (the pill is covered separately).
- **Pill after rollback/uninstall:** reinstalling the same pill after an uninstall and a restart may leave the pill unloaded until the next shell restart; a very short press that goes straight to transcribing uses the previous pill settings.
- **Ollama offline without a terminal:** without a terminal, `fastwisper ollama apply` (and `install.sh` without a TTY) opens the polkit dialog instead of failing with the `sudo` commands; the polkit dialog shows `/bin/sh`.
- **Bench labels:** `fastwisper bench` and `fastwisper transcribe` print some labels in Portuguese even with `ui.language = "en"`.
- **Accepted edge cases:** a `HOME` path containing `$` or a backtick in the desktop entry's `Exec`; a config file with mode `0o0xx`; a FIFO in place of `hotkeys.conf`; the Lua side accepts `ESC` as a key name (it only refuses `ESCAPE`).
- **Hub details:** `vram_mb = 0` shows as "—"; total VRAM stays empty if `nvidia-smi` fails on the first call; "Release VRAM" stays enabled with the daemon stopped (it ends in an error toast).
- **Customization editors:** the substitutions editor overwrites an existing entry without warning; the list editor's duplicate check is case-sensitive. Deleting a note id that does not exist reports `deleted: 0`.
- **Generic settings path:** the hub's generic settings command accepts `hotkey.*` without the conflict confirmation of the shortcut editor (the UI does not use that path).
- **Hub with a custom daemon config:** the hub always saves to the default config file. If you start `fastwisperd --config` with another file, a VRAM mode change from the hub reaches the daemon, but it is saved to the default file instead.

</details>

## Uninstall and roll back

<details>
<summary>Commands</summary>

Run these from the source checkout or from the extracted release folder. To remove the integration:

```bash
systemctl --user disable --now fastwisperd.service
omarchy plugin disable fastwisper.dictation
omarchy plugin disable fastwisper.pill
packaging/omarchy/uninstall.sh
systemctl --user daemon-reload
hyprctl reload
```

To go back one installed generation:

```bash
packaging/omarchy/rollback.sh
systemctl --user daemon-reload
systemctl --user restart fastwisperd.service
hyprctl reload
```

`uninstall.sh` also removes the hub. Rollback and uninstall only restore files that still match what was installed, and keep your config, models and external edits. The rollback works on disk: if the QML changed, run `omarchy-restart-shell` to refresh the bar and the pill. Your config (`~/.config/fastwisper/`), models (`~/.local/share/fastwisper/models/`) and state such as usage history and notes (`~/.local/state/fastwisper/`) stay in place; delete them by hand for a clean slate.

</details>

## Roadmap

- An AUR package.
- Broader validation: AMD and Intel GPUs in daily use, multiple monitors and scale factors for the pill, more keyboard layouts.
- Hub accessibility: screen reader and keyboard-only navigation.
- Help in the interface language.

Ideas and bug reports are welcome in the issue tracker.

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for how to build and run the checks. Please report security issues privately as described in [SECURITY.md](SECURITY.md). This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). See the [changelog](CHANGELOG.md) for what changed in each version.

## License

[MIT](LICENSE)
