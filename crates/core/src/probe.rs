//! Sonde de métadonnées — SPEC SF-01.
//!
//! Une **unique** invocation `yt-dlp -J` fournit tout ce dont le pipeline a
//! besoin : identité de la vidéo, durée (indispensable au calcul de progression
//! SF-04), statut de diffusion et inventaire des sous-titres. La v1 du cahier
//! des charges prévoyait deux appels réseau distincts ; celui-ci les remplace.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use serde::Deserialize;

use crate::cancel::{self, CancelToken};
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
    /// Langue déclarée de la vidéo, quand YouTube la renseigne.
    ///
    /// Indispensable au choix d'une piste de sous-titres : YouTube expose des
    /// traductions automatiques dans une centaine de langues, et seule celle-ci
    /// désigne l'originale.
    #[serde(default)]
    pub language: Option<String>,
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

    /// Langues disposant de sous-titres, manuels puis auto-générés.
    ///
    /// `dedup` ne supprime que les doublons **consécutifs** : les deux sources
    /// étant concaténées, chaque langue présente dans les deux apparaissait
    /// deux fois dans les messages d'erreur.
    pub fn available_subtitle_langs(&self) -> Vec<&str> {
        let mut vus = std::collections::BTreeSet::new();
        self.subtitles
            .keys()
            .chain(self.automatic_captions.keys())
            .map(String::as_str)
            .filter(|l| vus.insert(*l))
            .collect()
    }
}

/// Options d'accès communes à la sonde et à l'extraction — SPEC SF-09.
///
/// Certaines vidéos exigent une session authentifiée : limite d'âge, contenu
/// réservé, ou vérification anti-robot. Les cookies restent **désactivés par
/// défaut** et ne sont jamais activés automatiquement.
#[derive(Debug, Clone, Default)]
pub struct Access {
    /// Navigateur dont lire les cookies (`firefox`, `chrome`, `edge`…).
    pub cookies_from_browser: Option<String>,
}

impl Access {
    /// Arguments à insérer dans une invocation de `yt-dlp`.
    pub fn args(&self) -> Vec<String> {
        match &self.cookies_from_browser {
            Some(nav) => vec!["--cookies-from-browser".to_string(), nav.clone()],
            None => Vec::new(),
        }
    }
}

/// Période de surveillance du jeton d'annulation pendant la sonde.
const SURVEILLANCE: Duration = Duration::from_millis(100);

/// Interroge `yt-dlp -J` et désérialise le résultat.
///
/// Un jeton armé tue `yt-dlp` et rend [`ScriptaError::Interrupted`] : la
/// sonde dure quelques secondes d'ordinaire, mais bien davantage sur un
/// réseau qui ne répond plus, et le bouton « Annuler » de la GUI ne dispose
/// pas, comme `Ctrl-C` dans une console, d'un signal délivré au sidecar
/// lui-même.
pub fn probe(
    ytdlp: &Path,
    url: &CanonicalUrl,
    access: &Access,
    cancel: Option<&CancelToken>,
) -> Result<Metadata> {
    let mut enfant = Command::new(ytdlp)
        .args(["-J", "--no-warnings", "--no-playlist"])
        .args(access.args())
        .arg("--")
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
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

    // Les deux flux sont lus dans des threads dédiés : la réponse d'une vidéo
    // pèse souvent plusieurs centaines de Kio, bien plus qu'un tampon de pipe,
    // et ce thread-ci reste libre de surveiller l'annulation.
    let sortie = lire_en_tache(enfant.stdout.take());
    let erreurs = lire_en_tache(enfant.stderr.take());

    let statut = loop {
        if cancel::is_cancelled(cancel) {
            let _ = enfant.kill();
            let _ = enfant.wait();
            // Les lecteurs ne sont pas attendus : sous Windows, yt-dlp est un
            // exécutable autoextractible dont le processus enfant, qui survit
            // au parent, peut garder les pipes ouverts jusqu'à sa propre fin.
            return Err(ScriptaError::Interrupted);
        }
        match enfant.try_wait() {
            Ok(Some(statut)) => break statut,
            Ok(None) => thread::sleep(SURVEILLANCE),
            Err(e) => {
                let _ = enfant.kill();
                let _ = enfant.wait();
                return Err(ScriptaError::ExtractionFailed {
                    detail: format!("attente de yt-dlp : {e}"),
                });
            }
        }
    };

    let stdout = sortie.join().unwrap_or_default();
    let stderr = erreurs.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr);

    if !statut.success() {
        return Err(
            classify_sidecar_stderr(&stderr).unwrap_or(ScriptaError::ExtractionFailed {
                detail: format!("sonde yt-dlp en échec : {}", stderr.trim()),
            }),
        );
    }

    parse_metadata(&stdout)
}

/// Lit un flux jusqu'à sa fin dans un thread dédié.
fn lire_en_tache(flux: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut contenu = Vec::new();
        if let Some(mut flux) = flux {
            let _ = flux.read_to_end(&mut contenu);
        }
        contenu
    })
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
    fn n_inventorie_chaque_langue_qu_une_fois() {
        // `fr` est présent dans les deux sources : il ne doit apparaître
        // qu'une fois dans la liste proposée à l'utilisateur.
        let m = parse_metadata(
            br#"{"id":"a","subtitles":{"fr":[],"en":[]},
                 "automatic_captions":{"fr":[],"de":[]}}"#,
        )
        .unwrap();
        let mut langs = m.available_subtitle_langs();
        langs.sort_unstable();
        assert_eq!(langs, vec!["de", "en", "fr"]);
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

#[cfg(test)]
mod tests_access {
    use super::*;

    #[test]
    fn aucun_argument_sans_cookies() {
        // Invariant SF-09 : les cookies ne sont jamais activés d'office.
        assert!(Access::default().args().is_empty());
    }

    #[test]
    fn les_cookies_produisent_les_arguments_attendus() {
        let a = Access {
            cookies_from_browser: Some("firefox".into()),
        };
        assert_eq!(a.args(), vec!["--cookies-from-browser", "firefox"]);
    }
}
