//! Backend de l'application de bureau — SPEC §4.2.
//!
//! Les commandes IPC ne contiennent aucune logique métier : elles traduisent
//! entre l'interface et `scripta-core`, qui reste l'unique dépositaire du
//! comportement. La CLI et la GUI en sont deux consommateurs symétriques.

mod commands;
mod erreur;
mod relais;
mod state;

pub use state::AppState;

/// Point d'entrée de l'application.
pub fn run() {
    tauri::Builder::default()
        // Employés côté Rust uniquement : l'interface n'a aucun accès direct
        // au système de fichiers ni au presse-papiers (voir capabilities/).
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::backend_info,
            commands::list_models,
            commands::download_model,
            commands::remove_model,
            commands::probe_url,
            commands::transcribe,
            commands::cancel,
            commands::export,
            commands::copy_text,
            commands::update_extractor,
        ])
        .run(tauri::generate_context!())
        .expect("démarrage de l'application");
}
