//! Cache de transcriptions — SPEC SF-08.
//!
//! Retraiter une vidéo déjà transcrite est fréquent : changement de format
//! d'export, ajustement des sous-titres, simple erreur de manipulation. Sans
//! cache, chacune de ces reprises coûte l'inférence complète — treize minutes
//! pour une heure de contenu.
//!
//! Le JSON enrichi est stocké tel quel : **tous les autres formats s'en
//! dérivent** sans réinférence.

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::document::Document;
use crate::error::{Result, ScriptaError};

/// Plafond par défaut du cache, en octets.
pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Incrémenter à tout changement incompatible de `Document` : les entrées
/// d'une version antérieure sont alors ignorées plutôt que mal interprétées.
const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    document: Document,
}

/// Tout ce qui influe sur le résultat d'une transcription.
///
/// Omettre un seul de ces paramètres ferait resservir une transcription
/// obtenue dans d'autres conditions — le pire défaut possible pour un cache,
/// puisqu'il est silencieux.
#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    pub video_id: String,
    pub model: String,
    pub lang: Option<String>,
    pub translate: bool,
    pub vad: bool,
    pub word_timestamps: bool,
    /// Contexte fourni au modèle : il change la graphie des noms propres, donc
    /// le résultat.
    pub initial_prompt: Option<String>,
    pub no_speech_thold: Option<f32>,
    pub entropy_thold: Option<f32>,
}

impl Key {
    /// Empreinte servant de nom de fichier.
    pub fn digest(&self) -> String {
        let mut h = Sha256::new();
        // Séparateur explicite : sans lui, ("ab", "c") et ("a", "bc")
        // produiraient la même empreinte.
        for champ in [
            self.video_id.as_str(),
            self.model.as_str(),
            self.lang.as_deref().unwrap_or("auto"),
        ] {
            h.update(champ.as_bytes());
            h.update(b"\x1f");
        }
        h.update([
            self.translate as u8,
            self.vad as u8,
            self.word_timestamps as u8,
        ]);

        // Champs facultatifs : étiquetés, pour qu'aucun ne puisse passer pour
        // un autre, et omis quand ils sont absents. Une clé qui n'en porte
        // aucun garde ainsi l'empreinte d'avant leur introduction, et les
        // entrées déjà en cache restent valables.
        if let Some(p) = &self.initial_prompt {
            h.update(b"prompt\x1f");
            h.update(p.as_bytes());
            h.update(b"\x1f");
        }
        if let Some(v) = self.no_speech_thold {
            h.update(b"no_speech_thold\x1f");
            h.update(v.to_bits().to_le_bytes());
        }
        if let Some(v) = self.entropy_thold {
            h.update(b"entropy_thold\x1f");
            h.update(v.to_bits().to_le_bytes());
        }

        h.finalize()
            .iter()
            .map(|o| format!("{o:02x}"))
            .collect::<String>()
    }
}

pub fn cache_dir() -> Result<PathBuf> {
    crate::paths::sub_dir("transcripts", "SCRIPTA_CACHE_DIR")
}

fn path_of(key: &Key) -> Result<PathBuf> {
    Ok(cache_dir()?.join(format!("{}.json", key.digest())))
}

/// Relit une transcription mise en cache.
///
/// Une entrée illisible ou d'une version antérieure est traitée comme absente
/// et supprimée : un cache n'a pas à faire échouer une commande qui aurait
/// abouti sans lui.
pub fn get(key: &Key) -> Option<Document> {
    let chemin = path_of(key).ok()?;
    let brut = fs::read_to_string(&chemin).ok()?;

    match serde_json::from_str::<Envelope>(&brut) {
        Ok(e) if e.version == CACHE_VERSION => {
            // L'horodatage de modification sert d'heure de dernier accès :
            // c'est lui qui ordonne l'éviction LRU.
            let _ = filetime_touch(&chemin);
            Some(e.document)
        }
        _ => {
            let _ = fs::remove_file(&chemin);
            None
        }
    }
}

pub fn put(key: &Key, document: &Document) -> Result<()> {
    let chemin = path_of(key)?;
    let dossier = chemin.parent().ok_or_else(|| ScriptaError::OutputFailed {
        path: chemin.clone(),
        source: std::io::Error::other("chemin de cache invalide"),
    })?;
    fs::create_dir_all(dossier).map_err(|source| ScriptaError::OutputFailed {
        path: dossier.to_path_buf(),
        source,
    })?;

    let contenu = serde_json::to_string(&Envelope {
        version: CACHE_VERSION,
        document: document.clone(),
    })
    .map_err(|e| ScriptaError::OutputFailed {
        path: chemin.clone(),
        source: std::io::Error::other(e),
    })?;

    // Écriture puis renommage atomique : une interruption ne laisse jamais une
    // entrée tronquée, que `get` devrait ensuite détecter.
    let partiel = chemin.with_extension("part");
    fs::write(&partiel, contenu).map_err(|source| ScriptaError::OutputFailed {
        path: partiel.clone(),
        source,
    })?;
    fs::rename(&partiel, &chemin).map_err(|source| ScriptaError::OutputFailed {
        path: chemin.clone(),
        source,
    })?;

    Ok(())
}

/// Met à jour la date de modification, faute d'API stable pour l'heure d'accès.
fn filetime_touch(chemin: &std::path::Path) -> std::io::Result<()> {
    let f = fs::OpenOptions::new().append(true).open(chemin)?;
    f.set_modified(SystemTime::now())
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: SystemTime,
    /// Titre de la vidéo, lu dans l'entrée — un nom de fichier est une
    /// empreinte, illisible pour un humain.
    pub title: String,
}

pub fn list() -> Result<Vec<Entry>> {
    let dir = match cache_dir() {
        Ok(d) if d.is_dir() => d,
        _ => return Ok(Vec::new()),
    };

    let mut entrees = Vec::new();
    let lecture = fs::read_dir(&dir).map_err(|source| ScriptaError::OutputFailed {
        path: dir.clone(),
        source,
    })?;

    for e in lecture.flatten() {
        let chemin = e.path();
        if chemin.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };

        let titre = fs::read_to_string(&chemin)
            .ok()
            .and_then(|s| serde_json::from_str::<Envelope>(&s).ok())
            .map(|env| env.document.source.title)
            .unwrap_or_else(|| "(illisible)".to_string());

        entrees.push(Entry {
            path: chemin,
            bytes: meta.len(),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            title: titre,
        });
    }

    // Plus récent d'abord : c'est l'ordre attendu à l'affichage.
    entrees.sort_by_key(|e| std::cmp::Reverse(e.modified));
    Ok(entrees)
}

pub fn total_bytes() -> Result<u64> {
    Ok(list()?.iter().map(|e| e.bytes).sum())
}

/// Vide le cache. Retourne le nombre d'entrées supprimées.
pub fn clear() -> Result<usize> {
    let mut n = 0;
    for e in list()? {
        if fs::remove_file(&e.path).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

/// Éviction LRU au-delà du plafond. Retourne le nombre d'entrées supprimées.
///
/// Les plus anciennement utilisées partent en premier : une transcription
/// consultée hier a plus de chances de resservir qu'une autre oubliée depuis
/// des mois.
pub fn evict(max_bytes: u64) -> Result<usize> {
    let mut entrees = list()?;
    let mut total: u64 = entrees.iter().map(|e| e.bytes).sum();
    if total <= max_bytes {
        return Ok(0);
    }

    // `list` rend le plus récent d'abord ; l'éviction procède en sens inverse.
    entrees.reverse();

    let mut n = 0;
    for e in entrees {
        if total <= max_bytes {
            break;
        }
        if fs::remove_file(&e.path).is_ok() {
            total = total.saturating_sub(e.bytes);
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clef() -> Key {
        Key {
            video_id: "dQw4w9WgXcQ".into(),
            model: "ggml-base".into(),
            lang: Some("fr".into()),
            translate: false,
            vad: false,
            word_timestamps: false,
            initial_prompt: None,
            no_speech_thold: None,
            entropy_thold: None,
        }
    }

    #[test]
    fn l_empreinte_est_stable() {
        assert_eq!(clef().digest(), clef().digest());
        assert_eq!(clef().digest().len(), 64);
    }

    /// Les champs facultatifs, absents, laissent l'empreinte inchangée : les
    /// transcriptions mises en cache avant leur introduction restent servies.
    /// Valeur relevée avant l'ajout de ces champs.
    #[test]
    fn l_empreinte_historique_est_preservee() {
        assert_eq!(
            clef().digest(),
            "3a540706689b914cf0bfeeb6dbb50309e74cb9d8684c5d42738e2cd6f5da3d3b"
        );
    }

    /// Chaque paramètre influant sur le résultat doit changer l'empreinte.
    /// En omettre un ferait resservir une transcription obtenue dans d'autres
    /// conditions — un défaut silencieux, donc le pire.
    #[test]
    fn chaque_parametre_distingue_l_empreinte() {
        let base = clef().digest();

        let variantes = [
            Key {
                video_id: "autre_video".into(),
                ..clef()
            },
            Key {
                model: "ggml-large-v3".into(),
                ..clef()
            },
            Key {
                lang: Some("en".into()),
                ..clef()
            },
            Key {
                lang: None,
                ..clef()
            },
            Key {
                translate: true,
                ..clef()
            },
            Key {
                vad: true,
                ..clef()
            },
            Key {
                word_timestamps: true,
                ..clef()
            },
            Key {
                initial_prompt: Some("Etienne Klein".into()),
                ..clef()
            },
            // Un contexte vide n'est pas une absence de contexte : c'est à
            // l'appelant de normaliser, la clé ne doit pas deviner.
            Key {
                initial_prompt: Some(String::new()),
                ..clef()
            },
            Key {
                no_speech_thold: Some(0.5),
                ..clef()
            },
            Key {
                entropy_thold: Some(0.5),
                ..clef()
            },
        ];

        for v in &variantes {
            assert_ne!(v.digest(), base, "empreinte non distinguée : {v:?}");
        }

        // Et les variantes doivent aussi différer entre elles.
        let mut vues = std::collections::HashSet::new();
        for v in &variantes {
            assert!(vues.insert(v.digest()), "collision entre variantes : {v:?}");
        }
    }

    /// Sans séparateur entre les champs, ("ab", "c") et ("a", "bc")
    /// produiraient la même empreinte.
    #[test]
    fn les_champs_ne_se_confondent_pas() {
        let a = Key {
            video_id: "ab".into(),
            model: "c".into(),
            ..clef()
        };
        let b = Key {
            video_id: "a".into(),
            model: "bc".into(),
            ..clef()
        };
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn le_plafond_par_defaut_est_raisonnable() {
        // Deux gigaoctets : quelques centaines d'heures de transcription, le
        // JSON d'une heure pesant de l'ordre du mégaoctet.
        assert_eq!(DEFAULT_MAX_BYTES, 2 * 1024 * 1024 * 1024);
    }
}
