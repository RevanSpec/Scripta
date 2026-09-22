//! Taxonomie d'erreurs et codes de sortie — SPEC SF-07.
//!
//! Chaque variante porte un message actionnable et un code de sortie stable,
//! contractuel pour les scripts appelants.

use std::path::PathBuf;

/// Erreur applicative. Le code de sortie associé est contractuel : voir
/// [`ScriptaError::exit_code`] et la table SF-07 de la spécification.
#[derive(Debug, thiserror::Error)]
pub enum ScriptaError {
    #[error("URL non reconnue ou domaine non supporté : {0}")]
    InvalidUrl(String),

    #[error("Vidéo indisponible (privée, supprimée, géo-bloquée ou réservée aux membres).")]
    Unavailable { detail: String },

    #[error(
        "Connexion requise (vérification anti-robot ou limite d'âge). \
         Réessayez avec --cookies-from-browser <navigateur>."
    )]
    AuthRequired { detail: String },

    #[error("Les diffusions en direct ne sont pas prises en charge.")]
    LiveNotSupported,

    #[error(
        "Durée de {actual_min} min supérieure à la limite de {limit_min} min (--max-duration)."
    )]
    TooLong { actual_min: u64, limit_min: u64 },

    #[error("Échec de l'extraction audio : {detail}")]
    ExtractionFailed { detail: String },

    #[error("Binaire « {name} » introuvable. Lancez `scripta doctor` pour un diagnostic.")]
    SidecarMissing { name: String },

    #[error("Modèle indisponible : {detail}")]
    ModelUnavailable { detail: String },

    #[error("Échec de l'inférence : {detail}")]
    InferenceFailed { detail: String },

    #[error("Écriture impossible dans « {path} » : {source}")]
    OutputFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Interrompu par l'utilisateur.")]
    Interrupted,
}

impl ScriptaError {
    /// Code de sortie contractuel (table SF-07). Ne jamais réattribuer une
    /// valeur déjà publiée : des scripts en dépendent.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::InvalidUrl(_) => 10,
            Self::Unavailable { .. } => 11,
            Self::AuthRequired { .. } => 12,
            Self::LiveNotSupported => 13,
            Self::TooLong { .. } => 14,
            Self::ExtractionFailed { .. } => 20,
            Self::SidecarMissing { .. } => 21,
            Self::ModelUnavailable { .. } => 30,
            Self::InferenceFailed { .. } => 40,
            Self::OutputFailed { .. } => 50,
            Self::Interrupted => 130,
        }
    }
}

pub type Result<T> = std::result::Result<T, ScriptaError>;

/// Classe le `stderr` d'un sidecar en une erreur typée.
///
/// La correspondance est volontairement **large et tolérante** : les messages
/// de yt-dlp changent au fil des versions. Aucune logique de contrôle ne doit
/// dépendre d'une chaîne exacte. Un motif non reconnu retourne `None`, et
/// l'appelant se rabat sur [`ScriptaError::ExtractionFailed`] en exposant le
/// `stderr` brut — dégradation lisible plutôt que classification erronée.
pub fn classify_sidecar_stderr(stderr: &str) -> Option<ScriptaError> {
    let haystack = stderr.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| haystack.contains(n));

    // Ordre significatif : l'authentification est testée avant l'indisponibilité,
    // car « Sign in to confirm you're not a bot » contient aussi « video
    // unavailable » dans certaines versions de yt-dlp.
    if has(&[
        "not a bot",
        "sign in to confirm",
        "confirm your age",
        "age-restricted",
        "age restricted",
        "login required",
        "requires authentication",
        "use --cookies",
    ]) {
        return Some(ScriptaError::AuthRequired {
            detail: tail(stderr),
        });
    }

    if has(&[
        "private video",
        "video unavailable",
        "this video is unavailable",
        "removed by the uploader",
        "has been removed",
        "not available in your country",
        "blocked it in your country",
        "members-only",
        "join this channel",
        "video has been terminated",
        "account associated with this video has been terminated",
    ]) {
        return Some(ScriptaError::Unavailable {
            detail: tail(stderr),
        });
    }

    if has(&[
        "is a live event",
        "live event will begin",
        "premieres in",
        "this live event",
    ]) {
        return Some(ScriptaError::LiveNotSupported);
    }

    None
}

/// Conserve la fin du message, seule portion réellement informative quand
/// yt-dlp a déversé des avertissements en amont.
fn tail(s: &str) -> String {
    let trimmed = s.trim();
    const MAX: usize = 400;
    if trimmed.len() <= MAX {
        return trimmed.to_string();
    }
    let start = trimmed
        .char_indices()
        .rev()
        .nth(MAX)
        .map(|(i, _)| i)
        .unwrap_or(0);
    format!("…{}", &trimmed[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second énoncé, indépendant, de la table SF-07.
    ///
    /// Ce `match` est **exhaustif** : ajouter une variante sans lui attribuer
    /// de code ici casse la compilation. C'est l'effet recherché — ces codes
    /// sont contractuels, des scripts en dépendent, et une variante silencieuse
    /// qui hériterait d'un code voisin passerait inaperçue.
    fn code_attendu(e: &ScriptaError) -> i32 {
        match e {
            ScriptaError::InvalidUrl(_) => 10,
            ScriptaError::Unavailable { .. } => 11,
            ScriptaError::AuthRequired { .. } => 12,
            ScriptaError::LiveNotSupported => 13,
            ScriptaError::TooLong { .. } => 14,
            ScriptaError::ExtractionFailed { .. } => 20,
            ScriptaError::SidecarMissing { .. } => 21,
            ScriptaError::ModelUnavailable { .. } => 30,
            ScriptaError::InferenceFailed { .. } => 40,
            ScriptaError::OutputFailed { .. } => 50,
            ScriptaError::Interrupted => 130,
        }
    }

    /// Une instance de chaque variante.
    fn toutes_les_variantes() -> Vec<ScriptaError> {
        vec![
            ScriptaError::InvalidUrl("x".into()),
            ScriptaError::Unavailable {
                detail: String::new(),
            },
            ScriptaError::AuthRequired {
                detail: String::new(),
            },
            ScriptaError::LiveNotSupported,
            ScriptaError::TooLong {
                actual_min: 300,
                limit_min: 240,
            },
            ScriptaError::ExtractionFailed {
                detail: String::new(),
            },
            ScriptaError::SidecarMissing {
                name: "yt-dlp".into(),
            },
            ScriptaError::ModelUnavailable {
                detail: String::new(),
            },
            ScriptaError::InferenceFailed {
                detail: String::new(),
            },
            ScriptaError::OutputFailed {
                path: std::path::PathBuf::from("/x"),
                source: std::io::Error::other("x"),
            },
            ScriptaError::Interrupted,
        ]
    }

    #[test]
    fn tous_les_codes_de_sortie_sont_figes() {
        for e in toutes_les_variantes() {
            assert_eq!(e.exit_code(), code_attendu(&e), "{e:?}");
        }
    }

    #[test]
    fn aucun_code_de_sortie_n_est_partage() {
        // Deux variantes partageant un code rendraient le diagnostic ambigu
        // pour un script appelant.
        let mut vus = std::collections::HashMap::new();
        for e in toutes_les_variantes() {
            let code = e.exit_code();
            if let Some(precedent) = vus.insert(code, format!("{e:?}")) {
                panic!("code {code} partagé par {precedent} et {e:?}");
            }
        }
    }

    #[test]
    fn les_codes_restent_dans_les_bornes_posix() {
        // Un code supérieur à 255 serait tronqué par le shell.
        for e in toutes_les_variantes() {
            let c = e.exit_code();
            assert!((1..=255).contains(&c), "code hors bornes : {c} pour {e:?}");
        }
    }

    #[test]
    fn chaque_variante_porte_un_message_utile() {
        for e in toutes_les_variantes() {
            let msg = e.to_string();
            assert!(!msg.is_empty(), "message vide : {e:?}");
            // Un message qui se réduit au nom de la variante n'apprend rien
            // à l'utilisateur.
            assert!(msg.len() > 15, "message trop laconique : {msg}");
        }
    }

    #[test]
    fn classe_la_verification_anti_robot() {
        let msg = "ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm you're not a bot.";
        assert!(matches!(
            classify_sidecar_stderr(msg),
            Some(ScriptaError::AuthRequired { .. })
        ));
    }

    #[test]
    fn auth_prioritaire_sur_indisponible() {
        // Certaines versions de yt-dlp combinent les deux formulations : la
        // classification doit retenir la cause actionnable.
        let msg = "ERROR: Video unavailable. Sign in to confirm you're not a bot";
        assert!(matches!(
            classify_sidecar_stderr(msg),
            Some(ScriptaError::AuthRequired { .. })
        ));
    }

    #[test]
    fn classe_les_indisponibilites() {
        for msg in [
            "ERROR: Private video. Sign in if you've been granted access",
            "ERROR: This video is unavailable",
            "ERROR: Video unavailable. This video is not available in your country",
            "ERROR: Join this channel to get access to members-only content",
        ] {
            assert!(
                matches!(
                    classify_sidecar_stderr(msg),
                    Some(ScriptaError::Unavailable { .. })
                ),
                "non classé : {msg}"
            );
        }
    }

    #[test]
    fn classe_les_directs() {
        let msg = "ERROR: This live event will begin in 2 hours";
        assert!(matches!(
            classify_sidecar_stderr(msg),
            Some(ScriptaError::LiveNotSupported)
        ));
    }

    #[test]
    fn motif_inconnu_non_classe() {
        // Repli explicite : mieux vaut ExtractionFailed + stderr brut qu'une
        // classification fausse.
        assert!(classify_sidecar_stderr("ERROR: something entirely new").is_none());
    }

    #[test]
    fn tail_coupe_sur_une_frontiere_utf8() {
        let long = "é".repeat(1000);
        let out = tail(&long); // ne doit pas paniquer
        assert!(out.starts_with('…'));
    }
}
