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

    #[test]
    fn codes_de_sortie_stables() {
        // Ces valeurs sont contractuelles : un changement casse les appelants.
        assert_eq!(ScriptaError::InvalidUrl("x".into()).exit_code(), 10);
        assert_eq!(ScriptaError::LiveNotSupported.exit_code(), 13);
        assert_eq!(ScriptaError::Interrupted.exit_code(), 130);
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
