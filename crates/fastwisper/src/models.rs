//! `fastwisper models`: downloads explícitos ASR (`pull`) e, como ferramenta
//! opcional, o snapshot Ollama fixado (`pull-llm`); os demais subcomandos não
//! usam rede. Baixa só URLs do manifesto, por HTTPS, e só instala depois de
//! conferir tamanho e SHA-256 fixados no manifesto.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::Subcommand;
use fw_core::models::{self, MANIFEST, ModelSpec, Verification};
use sha2::{Digest, Sha256};

#[derive(Debug, Subcommand)]
pub enum ModelsCommand {
    /// Mostra o pin Ollama e a proveniência (sem acessar a rede).
    LlmManifest,
    /// Opcional: baixa o snapshot fixado para um NOVO store Ollama. O caminho
    /// normal é o Ollama que você já usa (`ollama pull qwen3:4b-instruct`).
    PullLlm {
        /// Store separado: não aponta automaticamente para o Ollama do sistema.
        #[arg(long)]
        directory: PathBuf,
    },
    /// Mostra o manifesto e o que já está instalado.
    List,
    /// Baixa o modelo configurado (ou o perfil informado), conferindo o SHA-256.
    Pull {
        id: Option<String>,
        /// Progresso como linhas JSON no stdout (`{"progress":42}`), para o hub.
        #[arg(long)]
        json: bool,
    },
    /// Passa a usar um modelo do manifesto (`asr.model`; vale ao reiniciar o daemon).
    Use {
        id: String,
        #[command(flatten)]
        config: crate::ConfigArgs,
    },
    /// Recalcula o SHA-256 de um modelo instalado.
    Verify {
        /// Sem id, o modelo configurado em `asr.model` (o `small` numa máquina só com CPU).
        id: Option<String>,
    },
}

pub fn run(command: ModelsCommand) -> anyhow::Result<()> {
    match command {
        ModelsCommand::LlmManifest => {
            crate::human_print!(
                "{} sha256:{}\nlicença: Apache-2.0\norigem observada: {}/manifests/4b-instruct\n{}",
                models::OLLAMA_MODEL,
                models::OLLAMA_DIGEST,
                models::OLLAMA_REGISTRY,
                models::OLLAMA_MANIFEST
            );
            return Ok(());
        }
        ModelsCommand::PullLlm { directory } => return pull_llm(&directory),
        _ => {}
    }
    let dir = models::models_dir().context("não foi possível localizar a pasta de dados (HOME)")?;
    match command {
        ModelsCommand::LlmManifest | ModelsCommand::PullLlm { .. } => unreachable!("handled above"),
        ModelsCommand::List => {
            crate::human_print!("pasta: {}", dir.display());
            for m in MANIFEST {
                let installed = match fs::metadata(dir.join(m.file_name)) {
                    Ok(meta) if meta.len() == m.size => "instalado",
                    Ok(_) => "tamanho diferente",
                    Err(_) => "ausente",
                };
                crate::human_print!(
                    "{:<22} {:>6} MB  {:<18} {} ({})",
                    m.id,
                    m.size / 1_000_000,
                    crate::human(installed),
                    crate::human(m.note),
                    m.license
                );
            }
            Ok(())
        }
        ModelsCommand::Pull { id, json } => {
            if let Some(id) = id {
                pull(spec(&id)?, &dir, json)
            } else {
                let config = fw_core::config::Config::load(None)?;
                let path = config.asr.model_path();
                if path.is_file() && configured_spec(&path).is_err() {
                    crate::human_print!("modelo configurado já existe: {}", path.display());
                    return Ok(());
                }
                let model = configured_spec(&path)?;
                pull_configured(model, &path, &dir, json)
            }
        }
        ModelsCommand::Use { id, config } => use_model(spec(&id)?, &dir, &config),
        ModelsCommand::Verify { id: Some(id) } => {
            let spec = spec(&id)?;
            let path = dir.join(spec.file_name);
            report(spec, &path, spec.verify(&path)?)
        }
        ModelsCommand::Verify { id: None } => {
            let path = fw_core::config::Config::load(None)?.asr.model_path();
            let spec = configured_spec(&path)?;
            report(spec, &path, spec.verify(&path)?)
        }
    }
}

// Caminhos externos são escolhas do usuário: verificar não exige chmod nem escrita.
fn pull_configured(spec: &ModelSpec, path: &Path, store: &Path, json: bool) -> anyhow::Result<()> {
    if spec.verify(path)? == Verification::Ok {
        crate::human_print!("{} já está instalado e confere.", spec.id);
        return Ok(());
    }
    let parent = path.parent().context("modelo sem pasta de destino")?;
    if parent != store
        || parent
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        bail!(
            "modelo configurado ausente ou inválido fora do store FastWisper: {}; instale-o nesse caminho sem alterar as permissões da pasta, ou configure o store {}",
            path.display(),
            store.display()
        );
    }
    pull(spec, store, json)
}

/// `asr.model` passa a apontar para o arquivo do manifesto no store. Não
/// baixa nada: avisa se ainda falta `models pull`.
fn use_model(spec: &ModelSpec, store: &Path, args: &crate::ConfigArgs) -> anyhow::Result<()> {
    let path = args
        .config
        .clone()
        .map_or_else(fw_core::config::Config::default_path, Ok)?;
    let file = store.join(spec.file_name);
    let value = file.to_str().context("caminho do modelo não é UTF-8")?;
    fw_core::config_edit::update(&path, vec![("asr.model".into(), value.into())])?;
    crate::human_print!("Salvo: asr.model = {value}");
    if !file.is_file() {
        crate::human_print!(
            "O modelo ainda não está instalado: rode `fastwisper models pull {}`.",
            spec.id
        );
    }
    let paths = fw_core::daemon::DaemonPaths::from_env()?;
    match crate::config::reload_for(&paths, &path)? {
        crate::config::ReloadFor::Reload(crate::config::Reload::Done(report)) => {
            if let Some(notice) = crate::config::restart_notice(&report) {
                crate::human_print!("{notice}");
            }
        }
        crate::config::ReloadFor::Elsewhere(daemon) => {
            crate::config::print_elsewhere(&path, daemon.as_deref())
        }
        crate::config::ReloadFor::Reload(_) => {}
    }
    Ok(())
}

fn configured_spec(path: &Path) -> anyhow::Result<&'static ModelSpec> {
    MANIFEST
        .iter()
        .find(|model| path.file_name() == Some(std::ffi::OsStr::new(model.file_name)))
        .with_context(|| {
            format!(
                "modelo configurado ausente fora do manifesto: {}",
                path.display()
            )
        })
}

fn spec(id: &str) -> anyhow::Result<&'static ModelSpec> {
    models::find(id).with_context(|| {
        let ids: Vec<_> = MANIFEST.iter().map(|m| m.id).collect();
        format!("modelo desconhecido \"{id}\"; opções: {}", ids.join(", "))
    })
}

fn report(spec: &ModelSpec, path: &Path, verification: Verification) -> anyhow::Result<()> {
    match verification {
        Verification::Ok => {
            crate::human_print!("ok: {} confere (sha256 {})", path.display(), spec.sha256);
            Ok(())
        }
        Verification::Missing => bail!(
            "{} não está instalado; rode `fastwisper models pull {}`",
            path.display(),
            spec.id
        ),
        Verification::WrongSize { actual } => bail!(
            "{} tem {actual} bytes, esperado {}; rode `fastwisper models pull {}`",
            path.display(),
            spec.size,
            spec.id
        ),
        Verification::WrongHash { actual } => bail!(
            "{} não confere: sha256 {actual}, esperado {}",
            path.display(),
            spec.sha256
        ),
    }
}

pub(crate) fn pull(spec: &ModelSpec, dir: &Path, json: bool) -> anyhow::Result<()> {
    let dest = dir.join(spec.file_name);

    match spec.verify(&dest)? {
        Verification::Ok => {
            crate::human_print!("{} já está instalado e confere.", spec.id);
            return Ok(());
        }
        Verification::Missing => {}
        other => crate::human_error!(
            "aviso: o arquivo existente não confere ({other:?}); só será substituído depois de um download verificado"
        ),
    }

    fs::create_dir_all(dir).with_context(|| format!("falha ao criar {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    crate::human_error!(
        "baixando {} ({} MB, {}) de {}",
        spec.id,
        spec.size / 1_000_000,
        spec.license,
        spec.url
    );
    let tmp = dir.join(format!(".{}.part-{}", spec.file_name, process::id()));
    let _partial = Partial(&tmp);
    download(spec, &tmp, json)?;
    // Rename atômico na mesma pasta: um modelo válido nunca é trocado por um parcial.
    fs::rename(&tmp, &dest).with_context(|| format!("falha ao instalar {}", dest.display()))?;
    File::open(dir)?.sync_all()?;
    crate::human_print!("ok: {} instalado em {}", spec.id, dest.display());
    Ok(())
}

/// Apaga o download parcial em qualquer saída: erro, sucesso (o rename já o
/// levou, nada a apagar) ou pipe da saída fechado no meio.
struct Partial<'a>(&'a Path);

impl Drop for Partial<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0);
    }
}

fn download(spec: &ModelSpec, tmp: &PathBuf, json: bool) -> anyhow::Result<()> {
    let agent = ureq::Agent::config_builder()
        .proxy(None)
        .https_only(true)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(60 * 30)))
        .build()
        .new_agent();
    let mut response = agent
        .get(spec.url)
        .call()
        .with_context(|| format!("falha ao baixar {}", spec.url))?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(spec.size + 1)
        .reader();

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp)
        .with_context(|| format!("falha ao criar {}", tmp.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total: u64 = 0;
    let mut last_pct = 0;
    loop {
        let n = reader.read(&mut buf).context("download interrompido")?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > spec.size {
            bail!(
                "o servidor enviou mais que os {} bytes esperados",
                spec.size
            );
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .context("falha ao gravar (disco cheio?)")?;
        let pct = total * 100 / spec.size;
        if json && pct > last_pct {
            last_pct = pct;
            crate::outln!("{{\"progress\":{pct}}}");
        } else if !json && pct >= last_pct + 5 {
            last_pct = pct;
            crate::eout!("\r  {pct:>3}%");
        }
    }
    if !json {
        crate::human_error!();
    }
    file.sync_all()?;

    if total != spec.size {
        bail!("download incompleto: {total} de {} bytes", spec.size);
    }
    let actual = models::hex(&hasher.finalize());
    if actual != spec.sha256 {
        bail!("SHA-256 não confere: {actual}, esperado {}", spec.sha256);
    }
    Ok(())
}

/// A pasta final precisa estar ausente: nunca substitui um store instalado.
/// Todos os blobs conferem antes do rename atômico do store completo.
fn pull_llm(destination: &Path) -> anyhow::Result<()> {
    if destination.exists() || destination.symlink_metadata().is_ok() {
        bail!("store já existe: escolha uma pasta nova; nenhum modelo será substituído");
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".fastwisper-ollama-{}", process::id()));
    fs::DirBuilder::new().mode(0o700).create(&staging)?;
    let result = download_llm_store(&staging).and_then(|()| {
        models::publish_new_store(&staging, destination)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    crate::human_print!(
        "ok: {} digest {} em {}. Ferramenta opcional: para servir este store, aponte OLLAMA_MODELS de um Ollama para ele; o FastWisper não altera serviços.",
        models::OLLAMA_MODEL,
        models::OLLAMA_DIGEST,
        destination.display()
    );
    Ok(())
}

fn download_llm_store(staging: &Path) -> anyhow::Result<()> {
    let manifest: serde_json::Value = serde_json::from_str(models::OLLAMA_MANIFEST)?;
    let blobs = staging.join("blobs");
    fs::create_dir(&blobs)?;
    for blob in manifest["layers"]
        .as_array()
        .context("layers")?
        .iter()
        .chain(std::iter::once(&manifest["config"]))
    {
        let digest = blob["digest"].as_str().context("digest")?;
        let hash = digest.strip_prefix("sha256:").context("sha256")?;
        let size = blob["size"].as_u64().context("size")?;
        let url = format!("{}/blobs/{digest}", models::OLLAMA_REGISTRY);
        let agent = ureq::Agent::config_builder()
            .proxy(None)
            .https_only(true)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(1800)))
            .build()
            .new_agent();
        let mut response = agent.get(&url).call()?;
        let mut reader = response.body_mut().with_config().limit(size + 1).reader();
        let path = blobs.join(format!("sha256-{hash}"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        copy_verified_blob(&mut reader, &mut file, size, hash)?;
        file.sync_all()?;
    }
    let manifests = staging.join("manifests/registry.ollama.ai/library/qwen3");
    fs::create_dir_all(&manifests)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(manifests.join("4b-instruct"))?;
    file.write_all(models::OLLAMA_MANIFEST.as_bytes())?;
    file.sync_all()?;
    File::open(&blobs)?.sync_all()?;
    File::open(&manifests)?.sync_all()?;
    File::open(staging)?.sync_all()?;
    Ok(())
}

fn copy_verified_blob(
    reader: &mut impl Read,
    writer: &mut impl Write,
    size: u64,
    hash: &str,
) -> anyhow::Result<()> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buffer)
            .context("download Ollama interrompido")?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > size {
            bail!("blob maior que manifesto");
        }
        writer.write_all(&buffer[..n])?;
        hasher.update(&buffer[..n]);
    }
    if total != size || models::hex(&hasher.finalize()) != hash {
        bail!("tamanho ou SHA-256 divergente; store não publicado");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_external_model_never_changes_directory_permissions() {
        let root = std::env::temp_dir().join(format!("fw-model-permissions-{}", process::id()));
        fs::create_dir_all(&root).expect("pasta de teste");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("modo");
        let model = ModelSpec {
            id: "fixture",
            file_name: "fixture.bin",
            url: "https://invalid.invalid/",
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            size: 3,
            license: "MIT",
            note: "teste",
        };
        let path = root.join(model.file_name);
        let store = root.join("xdg/fastwisper/models");
        assert!(pull_configured(&model, &path, &store, false).is_err());
        assert!(!store.exists());
        assert_eq!(
            fs::metadata(&root).expect("pasta").permissions().mode() & 0o777,
            0o755
        );
        fs::write(&path, b"abc").expect("modelo válido");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).expect("modo");
        pull_configured(&model, &path, &store, false).expect("modelo válido externo");
        pull(&model, &root, false).expect("modelo válido sem preparação da pasta");
        assert_eq!(
            fs::metadata(&root).expect("pasta").permissions().mode() & 0o777,
            0o555
        );
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("modo");
        fs::write(&path, b"abd").expect("modelo divergente");
        assert!(pull_configured(&model, &path, &store, false).is_err());
        assert_eq!(fs::read(&path).expect("modelo intacto"), b"abd");
        fs::remove_dir_all(root).expect("limpeza");
    }

    #[test]
    fn verified_model_in_system_directory_needs_no_chmod() {
        // Fixture somente leitura: /etc pertence ao sistema, não ao FastWisper.
        let path = Path::new("/etc/os-release");
        let bytes = fs::read(path).expect("identificação do sistema Linux");
        let hash = models::hex(&Sha256::digest(&bytes));
        let model = ModelSpec {
            id: "system-fixture",
            file_name: "os-release",
            url: "https://invalid.invalid/",
            sha256: Box::leak(hash.into_boxed_str()),
            size: bytes.len() as u64,
            license: "teste",
            note: "teste",
        };
        let mode = fs::metadata("/etc")
            .expect("pasta do sistema")
            .permissions()
            .mode();
        pull_configured(&model, path, Path::new("/unused/fastwisper/models"), false)
            .expect("verificação sem alterar pasta de outro dono");
        assert_eq!(
            fs::metadata("/etc")
                .expect("pasta do sistema")
                .permissions()
                .mode(),
            mode
        );
    }

    #[test]
    fn configured_model_uses_manifest_and_daemon_default() {
        let default =
            fw_core::config::Config::parse("[audio]\ndevice = 'mic'", Path::new("config.toml"))
                .expect("config/modelo válido no teste");
        assert_eq!(
            configured_spec(&default.asr.model_path())
                .expect("config/modelo válido no teste")
                .id,
            fw_core::models::DEFAULT_MODEL_ID
        );
        for model in MANIFEST {
            assert_eq!(
                configured_spec(&Path::new("/custom").join(model.file_name))
                    .expect("config/modelo válido no teste"),
                model
            );
        }
        assert!(configured_spec(Path::new("/custom/unknown.bin")).is_err());
    }

    #[test]
    fn blob_download_requires_exact_size_and_hash_and_propagates_io_failures() {
        const HASH: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        for bytes in [b"ab".as_slice(), b"abd", b"abcd"] {
            assert!(copy_verified_blob(&mut &bytes[..], &mut Vec::new(), 3, HASH).is_err());
        }
        let mut output = Vec::new();
        copy_verified_blob(&mut b"abc".as_slice(), &mut output, 3, HASH).expect("correct blob");
        assert_eq!(output, b"abc");
        struct FullDisk;
        impl Write for FullDisk {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(copy_verified_blob(&mut b"abc".as_slice(), &mut FullDisk, 3, HASH).is_err());
    }
}
