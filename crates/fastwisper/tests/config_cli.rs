//! `fastwisper config` real contra o actor real (pipeline nulo), com
//! runtime e config temporários.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use fw_core::config::Config;
use fw_core::daemon::{Daemon, DaemonPaths};
use fw_core::inject::NoDesktop;
use fw_core::pipeline::NullPipeline;

async fn cli(runtime: &Path, args: &[&str]) -> Output {
    cli_in(runtime, &std::env::current_dir().expect("cwd"), args).await
}

/// [`cli`] rodando na pasta `cwd`.
async fn cli_in(runtime: &Path, cwd: &Path, args: &[&str]) -> Output {
    let runtime = runtime.to_owned();
    let cwd = cwd.to_owned();
    let args: Vec<_> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        // Config e dados descartáveis, sem Hyprland: `config reload` também
        // regera atalhos e nunca pode tocar na sessão de quem roda o teste.
        Command::new(env!("CARGO_BIN_EXE_fastwisper"))
            .current_dir(cwd)
            .env("XDG_CONFIG_HOME", runtime.join("xdg-config"))
            .env("XDG_DATA_HOME", runtime.join("xdg-data"))
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .env("XDG_RUNTIME_DIR", runtime)
            .args(args)
            .output()
            .expect("CLI")
    })
    .await
    .expect("worker")
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("pt-BR");
    println!("{text}");
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_list_set_reload_and_refusals() {
    let root = tempfile::tempdir().expect("temporário");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("privado");
    let config = root.path().join("config.toml");
    let original = "# config de teste\n[llm]\n# estilo pessoal\nstyle = \"casual\" # meu estilo\n\n[audio]\nsounds = false # silêncio\n";
    fs::write(&config, original).expect("config");
    let boot = Config::load(Some(&config)).expect("válida");
    let daemon = Daemon::bind(DaemonPaths::in_dir(root.path()))
        .expect("daemon")
        .with_config(&config, true, boot);
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(
        daemon.run(Arc::new(NullPipeline), Arc::new(NoDesktop), async {
            let _ = stopped.await;
        }),
    );
    let path = config.to_str().expect("path");

    // get / list, com padrões e [overlay].
    assert_eq!(
        success(
            cli(
                root.path(),
                &["config", "get", "llm.style", "--config", path]
            )
            .await
        ),
        "\"casual\"\n"
    );
    assert_eq!(
        success(
            cli(
                root.path(),
                &[
                    "config",
                    "get",
                    "overlay.position",
                    "--json",
                    "--config",
                    path
                ]
            )
            .await
        ),
        "\"bottom\"\n"
    );
    let list = success(cli(root.path(), &["config", "list", "--config", path]).await);
    assert!(list.contains("audio.sounds = false  # default: true"));
    assert!(list.contains("overlay.enabled = true\n"));
    let json = success(cli(root.path(), &["config", "list", "--json", "--config", path]).await);
    let rows: serde_json::Value = serde_json::from_str(&json).expect("JSON");
    let endpoint = rows
        .as_array()
        .expect("lista")
        .iter()
        .find(|r| r["key"] == "llm.endpoint")
        .expect("llm.endpoint");
    assert_eq!(endpoint["restart"], true);
    assert!(
        !cli(
            root.path(),
            &["config", "get", "llm.estilo", "--config", path]
        )
        .await
        .status
        .success()
    );

    // set seguro: comentários preservados e aplicado no daemon.
    let out = success(
        cli(
            root.path(),
            &["config", "set", "llm.style", "formal", "--config", path],
        )
        .await,
    );
    assert!(out.contains("Saved: llm.style = \"formal\""));
    assert!(out.contains("Applied to the daemon."));
    assert_eq!(
        fs::read_to_string(&config).expect("config"),
        original.replace("\"casual\"", "\"formal\"")
    );

    // set inválido não grava nada.
    let before = fs::read_to_string(&config).expect("config");
    for args in [
        ["hotkey.max_recording_secs", "0"],
        ["audio.sounds", "talvez"],
        ["overlay.position", "top"],
        ["llm.estilo", "formal"],
    ] {
        let output = cli(
            root.path(),
            &["config", "set", args[0], args[1], "--config", path],
        )
        .await;
        assert!(!output.status.success(), "{args:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
        assert_eq!(
            fs::read_to_string(&config).expect("config"),
            before,
            "{args:?}"
        );
    }

    // Chave que exige reinício é informada.
    let out = success(
        cli(
            root.path(),
            &["config", "set", "asr.threads", "4", "--config", path],
        )
        .await,
    );
    assert!(out.contains("Applies only after restarting the daemon: asr.threads"));
    assert!(!out.contains("Applied to the daemon."));

    // Edição à mão + reload.
    fs::write(
        &config,
        fs::read_to_string(&config).expect("config") + "\n[inject]\nnotify = false\n",
    )
    .expect("config");
    // Outra config (`--config` que não é a do daemon): grava, mas não
    // sinaliza o daemon (A-23). Se sinalizasse, ele já teria relido a edição
    // à mão acima e o `reload` abaixo não veria mudança.
    let other = root.path().join("outra.toml");
    fs::write(&other, "# outra\n").expect("outra");
    let out = success(
        cli(
            root.path(),
            &[
                "config",
                "set",
                "llm.style",
                "casual",
                "--config",
                other.to_str().expect("path"),
            ],
        )
        .await,
    );
    assert!(
        out.contains(&format!(
            "Saved to {}; the daemon uses {}: nothing applied.",
            other.display(),
            config.display()
        )),
        "{out}"
    );
    assert!(!out.contains("Applied to the daemon."), "{out}");
    assert_eq!(
        Config::load(Some(&other)).expect("válida").llm.style,
        fw_core::config::Style::Casual
    );
    let out = success(cli(root.path(), &["config", "reload"]).await);
    assert!(
        out.contains("Changed: asr.threads, inject.notify."),
        "{out}"
    );
    // O mesmo arquivo por outro caminho (relativo à pasta) é a config dele.
    let relative = cli_in(
        root.path(),
        root.path(),
        &[
            "config",
            "set",
            "llm.style",
            "formal",
            "--config",
            "./config.toml",
        ],
    )
    .await;
    assert!(
        !success(relative).contains("nothing applied"),
        "mesmo arquivo, outro caminho"
    );

    stop.send(()).expect("stop");
    worker.await.expect("daemon");

    // Sem daemon: grava e avisa.
    let out = success(
        cli(
            root.path(),
            &[
                "config",
                "set",
                "audio.sound_volume",
                "0.5",
                "--config",
                path,
            ],
        )
        .await,
    );
    assert!(out.contains("changes apply on the next start"));
    assert_eq!(
        Config::load(Some(&config))
            .expect("válida")
            .audio
            .sound_volume,
        0.5
    );
    assert!(
        !cli(root.path(), &["config", "reload"])
            .await
            .status
            .success()
    );
}

/// Daemon que não informa a config em uso (antigo ou sem config): só a
/// config padrão sinaliza a recarga, como antes; outra fica só gravada.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_without_config_path_is_only_signalled_for_the_default_config() {
    let root = tempfile::tempdir().expect("temporário");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("privado");
    let daemon = Daemon::bind(DaemonPaths::in_dir(root.path())).expect("daemon");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(
        daemon.run(Arc::new(NullPipeline), Arc::new(NoDesktop), async {
            let _ = stopped.await;
        }),
    );
    let other = root.path().join("outra.toml");
    let out = success(
        cli(
            root.path(),
            &[
                "config",
                "set",
                "llm.style",
                "formal",
                "--config",
                other.to_str().expect("path"),
            ],
        )
        .await,
    );
    assert!(
        out.contains("the running daemon does not report which config it uses: nothing applied"),
        "{out}"
    );
    // A padrão (XDG_CONFIG_HOME temporário) é sinalizada: este daemon de
    // teste não tem config para reler e recusa, como um antigo faria.
    let default = cli(root.path(), &["config", "set", "llm.style", "formal"]).await;
    assert!(!default.status.success());
    let stderr = String::from_utf8_lossy(&default.stderr);
    assert!(stderr.contains("the daemon rejected reload"), "{stderr}");
    stop.send(()).expect("stop");
    worker.await.expect("daemon");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_many_keys_at_once_validates_them_together() {
    let root = tempfile::tempdir().expect("temporário");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("privado");
    let config = root.path().join("config.toml");
    fs::write(&config, "# minha config\n").expect("config");
    let path = config.to_str().expect("path");
    // Endpoint remoto sozinho é recusado; com o consentimento junto, vale.
    let alone = cli(
        root.path(),
        &[
            "config",
            "set",
            "llm.endpoint",
            "https://api.example.com",
            "--config",
            path,
        ],
    )
    .await;
    assert!(!alone.status.success());
    assert_eq!(
        fs::read_to_string(&config).expect("config"),
        "# minha config\n"
    );
    let out = success(
        cli(
            root.path(),
            &[
                "config",
                "set",
                "llm.engine",
                "openai_compatible",
                "llm.endpoint",
                "https://api.example.com",
                "llm.remote_consent",
                "true",
                "--config",
                path,
            ],
        )
        .await,
    );
    assert!(out.contains("The daemon is stopped"), "{out}");
    let saved = Config::load(Some(&config)).expect("válida");
    assert!(saved.llm.remote_consent);
    assert_eq!(saved.llm.endpoint, "https://api.example.com");
    let odd = cli(
        root.path(),
        &["config", "set", "llm.style", "--config", path],
    )
    .await;
    assert!(!odd.status.success(), "par incompleto é recusado");
}

/// `hub.theme` (Hub v2): a mesma chave do catálogo do hub, com as paletas e os temas Omarchy,
/// e a recusa de nomes inseguros, também sem daemon.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hub_theme_is_a_cli_key() {
    let root = tempfile::tempdir().expect("temporário");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("privado");
    let config = root.path().join("config.toml");
    fs::write(&config, "# minha config\n").expect("config");
    let path = config.to_str().expect("path");
    let get = ["config", "get", "hub.theme", "--config", path];
    assert_eq!(success(cli(root.path(), &get).await), "\"auto\"\n");
    for theme in [
        "dark",
        "light",
        "auto",
        "omarchy:tokyo-night",
        "omarchy:Custom_82",
        "light",
    ] {
        let set = ["config", "set", "hub.theme", theme, "--config", path];
        success(cli(root.path(), &set).await);
        assert_eq!(
            success(cli(root.path(), &get).await),
            format!("\"{theme}\"\n")
        );
    }
    let list = success(cli(root.path(), &["config", "list", "--config", path]).await);
    assert!(
        list.contains("hub.theme = \"light\"  # default: \"auto\""),
        "{list}"
    );
    let before = fs::read_to_string(&config).expect("config");
    assert!(before.contains("# minha config\n"), "{before}");
    for theme in [
        "sepia",
        "omarchy:",
        "omarchy:../x",
        "omarchy:a/b",
        "omarchy:x..y",
    ] {
        let bad = ["config", "set", "hub.theme", theme, "--config", path];
        assert!(!cli(root.path(), &bad).await.status.success());
    }
    assert_eq!(fs::read_to_string(&config).expect("config"), before);
}

/// "Enviar agora": opt-in e frases por idioma pela CLI, com a mesma
/// validação do hub; sem daemon.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_now_is_a_cli_key() {
    let root = tempfile::tempdir().expect("temporário");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("privado");
    let config = root.path().join("config.toml");
    fs::write(&config, "# minha config\n").expect("config");
    let path = config.to_str().expect("path");
    let get = |key: &'static str| ["config", "get", key, "--json", "--config", path];
    assert_eq!(
        success(cli(root.path(), &get("inject.send_now")).await),
        "false\n"
    );
    assert_eq!(
        success(cli(root.path(), &get("inject.send_now_phrases")).await),
        "{\"en\":[\"send now\"],\"pt\":[\"enviar agora\"]}\n"
    );
    let set = ["config", "set", "inject.send_now", "true", "--config", path];
    success(cli(root.path(), &set).await);
    let phrases = r#"{ pt = ["enviar agora", "manda ver"], en = ["send now", "ship it"] }"#;
    let set = [
        "config",
        "set",
        "inject.send_now_phrases",
        phrases,
        "--config",
        path,
    ];
    success(cli(root.path(), &set).await);
    assert_eq!(
        success(cli(root.path(), &get("inject.send_now_phrases")).await),
        "{\"en\":[\"send now\",\"ship it\"],\"pt\":[\"enviar agora\",\"manda ver\"]}\n"
    );
    let list = success(cli(root.path(), &["config", "list", "--config", path]).await);
    assert!(
        list.contains("inject.send_now = true  # default: false"),
        "{list}"
    );
    let loaded = Config::load(Some(&config)).expect("válida");
    assert!(loaded.inject.send_now);
    assert_eq!(loaded.inject.send_now_phrases.en, ["send now", "ship it"]);

    let before = fs::read_to_string(&config).expect("config");
    assert!(before.contains("# minha config\n"), "{before}");
    for bad in [
        r#"{ pt = [""] }"#,
        r#"{ pt = ["enviar, agora"] }"#,
        r#"{ fr = ["envoyer"] }"#,
        r#"["enviar agora"]"#,
    ] {
        let set = [
            "config",
            "set",
            "inject.send_now_phrases",
            bad,
            "--config",
            path,
        ];
        assert!(!cli(root.path(), &set).await.status.success(), "{bad}");
    }
    assert_eq!(fs::read_to_string(&config).expect("config"), before);
}
