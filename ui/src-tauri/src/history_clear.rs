//! A mesma limpeza transacional da CLI, sem shell e sem remover o banco vivo.
use fw_core::history;
use tauri::{AppHandle, Emitter};

#[tauri::command]
pub async fn history_clear(app: AppHandle) -> Result<usize, String> {
    let deleted = tauri::async_runtime::spawn_blocking(|| {
        let path = history::default_path().ok_or("Sem HOME: histórico indisponível")?;
        history::clear(&path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "Falha ao apagar o histórico".to_owned())??;
    // Os consumidores podem reler apenas métricas, inclusive quando o total já era zero.
    let _ = app.emit("history-cleared", ());
    Ok(deleted)
}
