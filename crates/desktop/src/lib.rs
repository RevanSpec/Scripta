//! Backend de l'application de bureau — SPEC §4.2.
//!
//! Les commandes IPC ne contiennent aucune logique métier : elles traduisent
//! entre le frontend et `scripta-core`, qui reste l'unique dépositaire du
//! comportement. La CLI et la GUI en sont deux consommateurs symétriques.

mod commands;
mod state;

pub use state::AppState;

/// Point d'entrée, partagé avec les cibles mobiles.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::backend_info,
            commands::list_models,
            commands::probe_url,
            commands::transcribe,
            commands::cancel,
            commands::render_as,
            commands::save_output,
            commands::update_extractor,
        ])
        .run(tauri::generate_context!())
        .expect("démarrage de l'application");
}
