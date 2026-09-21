//! Sonde de métadonnées — SPEC SF-01.
//!
//! Une **unique** invocation `yt-dlp -J` fournit tout ce dont le pipeline a
//! besoin : identité de la vidéo, durée (indispensable au calcul de progression
//! SF-04), statut de diffusion et inventaire des sous-titres. La v1 du cahier
//! des charges prévoyait deux appels réseau distincts ; celui-ci les remplace.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::error::{Result, ScriptaError, classify_sidecar_stderr};
use crate::url::CanonicalUrl;

/// Métadonnées retenues de la sortie `yt-dlp -J`.
///
/// Le document réel compte des centaines de champs : seuls ceux effectivement
/// exploités sont désérialisés.
#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub channel: Option<String>,
    /// Durée en secondes. Absente sur certains contenus.
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub upload_date: Option<String>,
    #[serde(default)]
    pub is_live: Option<bool>,
    #[serde(default)]
    pub live_status: Option<String>,
    #[serde(default)]
    pub age_limit: Option<u32>,
    #[serde(default)]
    pub subtitles: BTreeMap<String, Vec<SubtitleTrack>>,
    #[serde(default)]
    pub automatic_captions: BTreeMap<String, Vec<SubtitleTrack>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubtitleTrack {
    #[serde(default)]
    pub ext: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl Metadata {
    /// Vrai pour un direct, en cours ou à venir.
    ///
    /// `is_live` seul est insuffisant : une première programmée porte
    /// `is_live: false` et `live_status: "is_upcoming"`.
    pub fn is_live_content(&self) -> bool {
        if self.is_live == Some(true) {
            return true;
        }
        matches!(
            self.live_status.as_deref(),
            Some("is_live" | "is_upcoming" | "post_live")
        )
    }

    /// Langues disposant de sous-titres manuels, puis auto-générés.
    pub fn available_subtitle_langs(&self) -> Vec<&str> {
        let mut langs: Vec<&str> = self.subtitles.keys().map(String::as_str).collect();
        langs.extend(self.automatic_captions.keys().map(String::as_str));
        langs.dedup();
        langs
    }
}

/// Interroge `yt-dlp -J` et désérialise le résultat.
pub fn probe(ytdlp: &std::path::Path, url: &CanonicalUrl) -> Result<Metadata> {
    let output = Command::new(ytdlp)
        .args(["-J", "--no-warnings", "--no-playlist", "--"])
        .arg(url.as_str())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ScriptaError::SidecarMissing {
                    name: "yt-dlp".to_string(),
                }
            } else {
                ScriptaError::ExtractionFailed {
                    detail: format!("lancement de yt-dlp : {e}"),
                }
            }
        })?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        return Err(
            classify_sidecar_stderr(&stderr).unwrap_or(ScriptaError::ExtractionFailed {
                detail: format!("sonde yt-dlp en échec : {}", stderr.trim()),
            }),
        );
    }

    parse_metadata(&output.stdout)
}

pub fn parse_metadata(json: &[u8]) -> Result<Metadata> {
    serde_json::from_slice(json).map_err(|e| ScriptaError::ExtractionFailed {
        detail: format!("réponse yt-dlp illisible : {e}"),
    })
}

/// Contrôle de recevabilité avant toute extraction — SPEC SF-07.
///
/// Appelé **avant** le pipeline : un direct produit un flux de durée non bornée
/// qui épuiserait la mémoire (ADR-003).
pub fn check_admissible(meta: &Metadata, max_duration_min: u64) -> Result<()> {
    if meta.is_live_content() {
        return Err(ScriptaError::LiveNotSupported);
    }
    if let Some(d) = meta.duration {
        let actual_min = (d / 60.0).ceil() as u64;
        if actual_min > max_duration_min {
            return Err(ScriptaError::TooLong {
                actual_min,
                limit_min: max_duration_min,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &[u8] = br#"{"id":"dQw4w9WgXcQ","title":"Test","duration":212.0}"#;

    #[test]
    fn desiserialise_un_document_minimal() {
        let m = parse_metadata(MINIMAL).unwrap();
        assert_eq!(m.id, "dQw4w9WgXcQ");
        assert_eq!(m.duration, Some(212.0));
        assert!(!m.is_live_content());
    }

    #[test]
    fn tolere_les_champs_absents() {
        // yt-dlp omet de nombreux champs selon le contenu : aucun ne doit être
        // requis pour que la désérialisation aboutisse.
        let m = parse_metadata(br#"{"id":"abc12345678"}"#).unwrap();
        assert_eq!(m.duration, None);
        assert!(m.available_subtitle_langs().is_empty());
    }

    #[test]
    fn tolere_les_champs_supplementaires() {
        let m =
            parse_metadata(br#"{"id":"abc12345678","title":"T","formats":[{"x":1}],"inconnu":42}"#)
                .unwrap();
        assert_eq!(m.title, "T");
    }

    #[test]
    fn detecte_les_directs_sous_toutes_leurs_formes() {
        let live = parse_metadata(br#"{"id":"a","is_live":true}"#).unwrap();
        assert!(live.is_live_content());

        // Première programmée : is_live vaut false, seul live_status renseigne.
        let upcoming =
            parse_metadata(br#"{"id":"a","is_live":false,"live_status":"is_upcoming"}"#).unwrap();
        assert!(upcoming.is_live_content());

        let vod = parse_metadata(br#"{"id":"a","live_status":"not_live"}"#).unwrap();
        assert!(!vod.is_live_content());
    }

    #[test]
    fn refuse_les_directs() {
        let m = parse_metadata(br#"{"id":"a","is_live":true}"#).unwrap();
        assert!(matches!(
            check_admissible(&m, 240),
            Err(ScriptaError::LiveNotSupported)
        ));
    }

    #[test]
    fn refuse_au_dela_de_la_duree_maximale() {
        let m = parse_metadata(br#"{"id":"a","duration":18000.0}"#).unwrap(); // 5 h
        match check_admissible(&m, 240) {
            Err(ScriptaError::TooLong {
                actual_min,
                limit_min,
            }) => {
                assert_eq!(actual_min, 300);
                assert_eq!(limit_min, 240);
            }
            other => panic!("attendu TooLong, obtenu {other:?}"),
        }
        assert!(check_admissible(&m, 360).is_ok());
    }

    #[test]
    fn duree_absente_n_est_pas_bloquante() {
        let m = parse_metadata(br#"{"id":"a"}"#).unwrap();
        assert!(check_admissible(&m, 240).is_ok());
    }

    #[test]
    fn inventorie_les_sous_titres() {
        let m = parse_metadata(
            br#"{"id":"a","subtitles":{"fr":[{"ext":"vtt"}]},
                 "automatic_captions":{"en":[{"ext":"vtt"}]}}"#,
        )
        .unwrap();
        let langs = m.available_subtitle_langs();
        assert!(langs.contains(&"fr") && langs.contains(&"en"));
    }
}
