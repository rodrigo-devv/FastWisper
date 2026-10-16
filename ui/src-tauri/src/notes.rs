//! Único conteúdo persistente permitido na webview: notas explicitamente opt-in.
use fw_core::{
    client,
    daemon::DaemonPaths,
    ipc::{Request, Response},
    notes::Note,
    session::Command,
};
use tauri::{AppHandle, Emitter};

fn send(socket: &std::path::Path, command: Command, id: Option<u64>) -> Result<Response, String> {
    let mut request = Request::new("hub-notes", command);
    request.args.id = id;
    let response =
        client::send(socket, &request).map_err(|_| "Não foi possível comunicar com o daemon.")?;
    if !response.ok {
        return Err(response
            .error
            .map(|e| e.message)
            .unwrap_or_else(|| "O daemon recusou a ação.".into()));
    }
    Ok(response)
}
async fn command(command: Command, id: Option<u64>) -> Result<Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = DaemonPaths::from_env().map_err(|_| "Runtime do daemon indisponível.")?;
        send(&paths.socket, command, id)
    })
    .await
    .map_err(|_| "Falha na ação de notas.".to_owned())?
}
#[tauri::command]
pub async fn note_start() -> Result<(), String> {
    command(Command::NoteStart, None).await.map(|_| ())
}
#[tauri::command]
pub async fn note_save() -> Result<(), String> {
    command(Command::NoteSave, None).await.map(|_| ())
}
#[tauri::command]
pub async fn note_cancel() -> Result<(), String> {
    command(Command::NoteCancel, None).await.map(|_| ())
}
#[tauri::command]
pub async fn notes_list() -> Result<Vec<Note>, String> {
    Ok(command(Command::NotesList, None)
        .await?
        .notes
        .unwrap_or_default())
}
#[tauri::command]
pub async fn notes_copy(id: u64) -> Result<(), String> {
    command(Command::NotesCopy, Some(id)).await.map(|_| ())
}
#[tauri::command]
pub async fn notes_delete(app: AppHandle, id: u64) -> Result<(), String> {
    command(Command::NotesDelete, Some(id)).await?;
    let _ = app.emit("notes-changed", ());
    Ok(())
}
#[tauri::command]
pub async fn notes_clear(app: AppHandle) -> Result<(), String> {
    command(Command::NotesClear, None).await?;
    let _ = app.emit("notes-changed", ());
    Ok(())
}
