//! Bridge local com projeções explícitas, nunca Response/Config completos.
mod commands;
mod history;
mod history_clear;
mod home;
mod hotkeys;
mod navigation;
mod notes;
mod settings;
#[cfg(test)]
mod test_support;
mod theme;

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
            if let Some(destination) = navigation::destination_from_args(&args) {
                let _ = app.emit("navigate", destination);
            }
        }))
        .manage(home::Live::default())
        .setup(|app| {
            home::start_watcher(app.handle().clone(), &app.state::<home::Live>());
            theme::start_watcher(app.handle().clone());
            Ok(())
        })
        .manage(navigation::InitialDestination(Mutex::new(
            navigation::destination_from_args(&std::env::args().collect::<Vec<_>>()),
        )))
        .manage(Arc::new(settings::MeterState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::hub_status,
            commands::config_summary,
            commands::hub_version,
            commands::open_guide,
            commands::open_about_link,
            theme::theme_colors,
            theme::omarchy_themes,
            theme::hub_theme,
            theme::ui_language,
            home::daemon_view,
            home::release_vram,
            home::set_vram_mode,
            home::doctor_summary,
            home::vram_total,
            history::history_stats,
            history_clear::history_clear,
            notes::note_start,
            notes::note_save,
            notes::note_cancel,
            notes::notes_list,
            notes::notes_copy,
            notes::notes_delete,
            notes::notes_clear,
            navigation::initial_destination,
            hotkeys::hotkeys_get,
            hotkeys::hotkey_preview,
            hotkeys::hotkey_save,
            settings::settings_get,
            settings::settings_set,
            settings::daemon_restart,
            settings::microphones,
            settings::sound_test,
            settings::mic_test,
            settings::meter_start,
            settings::meter_stop,
            settings::asr_models,
            settings::asr_gpus,
            settings::asr_model_pull,
            settings::ollama_models,
            settings::ollama_cloud,
            settings::ollama_cloud_set
        ])
        .run(tauri::generate_context!())
        .expect("não foi possível abrir o hub FastWisper");
}
