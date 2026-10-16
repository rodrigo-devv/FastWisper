//! O plugin single-instance do hub cuida de focar a janela existente.
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Destinos que `fastwisper hub <página>` abre: as páginas e as abas de
/// Configurações do hub, e as páginas antigas, que o hub leva para o lugar
/// novo (`ui/src-tauri/src/navigation.rs`).
pub const PAGES: [&str; 21] = [
    "inicio",
    "notas",
    "historico",
    "temas",
    "personalizacao",
    "dicionario",
    "snippets",
    "estilo",
    "configuracoes",
    "geral",
    "sistema",
    "vibe-coding",
    "experimental",
    "privacidade",
    "ajuda",
    "sobre",
    "microfone",
    "transcricao",
    "ia",
    "desempenho",
    "colagem",
];

const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

pub fn run(page: Option<String>) -> anyhow::Result<()> {
    let binary: PathBuf = std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow::anyhow!("diretório da CLI indisponível"))?
        .join("fastwisper-hub");
    let mut command = Command::new(binary);
    // O renderizador DMABUF do WebKitGTK derruba a janela com "Error 71
    // (Protocol error)" no Wayland com NVIDIA (visto no Hyprland). Sem ele o
    // hub abre em qualquer placa; quem definiu a variável mantém a escolha.
    if std::env::var_os(DMABUF).is_none() {
        command.env(DMABUF, "1");
    }
    if let Some(page) = page {
        command.args(["--page", &page]);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| missing_hub(&error, install_status().as_deref()))?;
    Ok(())
}

/// Motivo que o `install.sh` gravou em
/// `$XDG_STATE_HOME/fastwisper/packaging/hub-status` (ex.: `missing_node`).
fn install_status() -> Option<String> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    let text = std::fs::read_to_string(state.join("fastwisper/packaging/hub-status")).ok()?;
    Some(text.trim().to_owned())
}

/// Hub ausente: diz o motivo da instalação; sem marcador (ou outro erro),
/// a mensagem genérica.
fn missing_hub(error: &std::io::Error, status: Option<&str>) -> anyhow::Error {
    let status = status.filter(|_| error.kind() == std::io::ErrorKind::NotFound);
    match status {
        Some("no_hub") => anyhow::anyhow!(
            "o hub não foi instalado (install.sh --no-hub); rode packaging/omarchy/install.sh sem --no-hub"
        ),
        Some("missing_node") => anyhow::anyhow!(
            "o hub não foi instalado: faltou node/npm; instale-os e rode packaging/omarchy/install.sh de novo"
        ),
        Some("missing_webkit") => anyhow::anyhow!(
            "o hub não foi instalado: faltou webkit2gtk-4.1; instale-o e rode packaging/omarchy/install.sh de novo"
        ),
        Some("build_failed") => anyhow::anyhow!(
            "o hub não foi instalado: a compilação dele falhou; veja a saída do install.sh e rode-o de novo"
        ),
        Some("external_build") => anyhow::anyhow!(
            "o hub não foi instalado: o build externo não trazia o hub (FASTWISPER_HUB_BUILD_DIR)"
        ),
        _ => anyhow::anyhow!(
            "não foi possível abrir o hub ({error}); rode packaging/omarchy/install.sh de novo"
        ),
    }
}
