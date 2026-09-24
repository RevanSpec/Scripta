//! Erreurs présentées à l'interface — SPEC SF-07, tâche 3.10.
//!
//! Les messages de `ScriptaError` s'adressent à la ligne de commande : ils
//! citent `--cookies-from-browser`, `--max-duration` ou `scripta doctor`, que
//! l'utilisateur de la fenêtre ne connaît pas. L'interface a donc ses propres
//! mots, par une correspondance **exhaustive** : une variante ajoutée au cœur
//! sans message pour la GUI ne compile pas.
//!
//! Le détail technique — la sortie de yt-dlp, le chemin en cause — n'est pas
//! perdu : il accompagne le message, replié par défaut.

use serde::Serialize;

use scripta_core::ScriptaError;

/// Code propre à l'interface, hors taxonomie SF-07 : une autre tâche occupe
/// déjà l'application.
pub const CODE_OCCUPE: i32 = 1;

/// Code propre à l'interface : défaut interne, qui relève d'un rapport de bogue.
pub const CODE_INTERNE: i32 = 3;

/// Erreur transmise à l'interface.
#[derive(Debug, Serialize)]
pub struct IpcError {
    /// Code de sortie contractuel (SF-07) : l'interface distingue ainsi une
    /// cause actionnable d'une panne, sans analyser de texte.
    pub code: i32,
    /// Ce qui s'est passé, en une phrase.
    pub message: String,
    /// Ce que l'utilisateur peut y faire, quand il y peut quelque chose.
    pub conseil: Option<String>,
    /// Détail technique, replié par défaut.
    pub detail: Option<String>,
}

pub type IpcResult<T> = std::result::Result<T, IpcError>;

impl IpcError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            conseil: None,
            detail: None,
        }
    }

    fn conseil(mut self, conseil: impl Into<String>) -> Self {
        self.conseil = Some(conseil.into());
        self
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if !detail.trim().is_empty() {
            self.detail = Some(detail);
        }
        self
    }

    pub fn occupe() -> Self {
        Self::new(CODE_OCCUPE, "Une autre opération est en cours.")
            .conseil("Attendez sa fin, ou annulez-la.")
    }

    pub fn interne(detail: impl std::fmt::Display) -> Self {
        Self::new(CODE_INTERNE, "Erreur interne de l'application.")
            .conseil("Relancez l'opération ; si le problème persiste, signalez-le.")
            .detail(detail.to_string())
    }
}

impl From<ScriptaError> for IpcError {
    fn from(e: ScriptaError) -> Self {
        let code = e.exit_code();
        match e {
            ScriptaError::InvalidUrl(detail) => {
                IpcError::new(code, "Cette adresse n'est pas celle d'une vidéo YouTube.")
                    .conseil("Collez l'adresse d'une vidéo : youtube.com/watch?v=…, youtu.be/… ou un Short.")
                    .detail(detail)
            }
            ScriptaError::Unavailable { detail } => IpcError::new(
                code,
                "Cette vidéo est indisponible : privée, supprimée, bloquée dans votre pays ou réservée aux membres.",
            )
            .detail(detail),
            ScriptaError::AuthRequired { detail } => IpcError::new(
                code,
                "YouTube exige une session connectée pour cette vidéo — vérification anti-robot ou limite d'âge.",
            )
            .conseil("Dans les options avancées, indiquez le navigateur où vous êtes connecté à YouTube.")
            .detail(detail),
            ScriptaError::LiveNotSupported => IpcError::new(
                code,
                "Les diffusions en direct ne sont pas prises en charge : leur durée n'est pas bornée.",
            )
            .conseil("Réessayez une fois la diffusion terminée."),
            ScriptaError::TooLong {
                actual_min,
                limit_min,
            } => IpcError::new(
                code,
                format!("Cette vidéo dure {actual_min} min, au-delà de la limite de {limit_min} min."),
            ),
            ScriptaError::ExtractionFailed { detail } => {
                IpcError::new(code, "L'audio de la vidéo n'a pas pu être extrait.")
                    .conseil("YouTube change régulièrement : mettre à jour yt-dlp règle le problème le plus souvent.")
                    .detail(detail)
            }
            ScriptaError::SidecarMissing { name } => {
                IpcError::new(code, format!("{name} est introuvable."))
                    .conseil("Scripta a besoin de yt-dlp et de ffmpeg : voir la section Installation du README.")
            }
            ScriptaError::ModelUnavailable { detail } => {
                IpcError::new(code, "Le modèle de transcription n'a pas pu être obtenu.")
                    .conseil("Vérifiez votre connexion : un modèle se télécharge à son premier usage.")
                    .detail(detail)
            }
            ScriptaError::InferenceFailed { detail } => {
                IpcError::new(code, "La transcription a échoué.")
                    .conseil("Un modèle plus petit demande moins de mémoire.")
                    .detail(detail)
            }
            ScriptaError::OutputFailed { path, source } => IpcError::new(
                code,
                format!("Impossible d'écrire « {} ».", path.display()),
            )
            .conseil("Choisissez un autre emplacement.")
            .detail(source.to_string()),
            ScriptaError::Interrupted => IpcError::new(code, "Opération annulée."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une instance de chaque variante. La correspondance de `From` est
    /// exhaustive, mais cette liste ne l'est que par soin : à compléter avec
    /// toute variante nouvelle.
    fn toutes() -> Vec<ScriptaError> {
        vec![
            ScriptaError::InvalidUrl("https://exemple.org".into()),
            ScriptaError::Unavailable {
                detail: "Private video".into(),
            },
            ScriptaError::AuthRequired {
                detail: "Sign in to confirm you're not a bot".into(),
            },
            ScriptaError::LiveNotSupported,
            ScriptaError::TooLong {
                actual_min: 300,
                limit_min: 240,
            },
            ScriptaError::ExtractionFailed {
                detail: "HTTP Error 403".into(),
            },
            ScriptaError::SidecarMissing {
                name: "yt-dlp".into(),
            },
            ScriptaError::ModelUnavailable {
                detail: "empreinte invalide".into(),
            },
            ScriptaError::InferenceFailed {
                detail: "failed to encode".into(),
            },
            ScriptaError::OutputFailed {
                path: "sortie.srt".into(),
                source: std::io::Error::other("accès refusé"),
            },
            ScriptaError::Interrupted,
        ]
    }

    #[test]
    fn aucun_message_ne_parle_la_langue_de_la_ligne_de_commande() {
        // Critère de sortie du J3 : aucun message technique brut dans
        // l'interface. Ni option, ni sous-commande, ni message anglais de
        // yt-dlp dans ce qui s'affiche en premier.
        for e in toutes() {
            let brut = e.to_string();
            let ipc = IpcError::from(e);
            let visible = format!("{} {}", ipc.message, ipc.conseil.unwrap_or_default());
            assert!(
                !ipc.message.trim().is_empty(),
                "message vide pour « {brut} »"
            );
            for interdit in ["--", "scripta ", "Sign in", "HTTP Error", "Private video"] {
                assert!(
                    !visible.contains(interdit),
                    "« {interdit} » affiché pour « {brut} » : {visible}"
                );
            }
        }
    }

    #[test]
    fn le_code_contractuel_est_conserve() {
        for e in toutes() {
            let attendu = e.exit_code();
            assert_eq!(IpcError::from(e).code, attendu);
        }
    }

    #[test]
    fn le_detail_technique_n_est_pas_perdu() {
        let ipc = IpcError::from(ScriptaError::AuthRequired {
            detail: "Sign in to confirm you're not a bot".into(),
        });
        assert_eq!(
            ipc.detail.as_deref(),
            Some("Sign in to confirm you're not a bot")
        );
        // Un détail vide n'encombre pas l'affichage.
        let ipc = IpcError::from(ScriptaError::ExtractionFailed {
            detail: "  ".into(),
        });
        assert_eq!(ipc.detail, None);
    }
}
