//! Détection d'activité vocale — SPEC SF-04.
//!
//! Whisper hallucine sur les silences prolongés, les génériques et les bruits
//! de fond. Le VAD Silero les écarte avant l'inférence : seule la parole est
//! transcrite, et le temps de calcul baisse d'autant sur les contenus peu
//! denses.
//!
//! # Pourquoi ne pas déléguer à whisper.cpp
//!
//! whisper.cpp sait appliquer le VAD lui-même (`FullParams::enable_vad`), mais
//! cette voie est fermée, pour trois raisons :
//!
//! - **elle est inopérante via whisper-rs.** whisper.cpp 1.8.3 n'applique le
//!   VAD que dans `whisper_full`, sur l'état par défaut du contexte ; or
//!   `WhisperState::full` de whisper-rs 0.16 appelle `whisper_full_with_state`,
//!   qui ignore `params.vad`. Les paramètres sont acceptés, et rien ne se
//!   passe — c'était le cas de `--vad-model` jusqu'ici ;
//! - **les horodatages de mots seraient faux.** Même par `whisper_full`, seules
//!   les bornes des *segments* sont replacées sur la chronologie d'origine
//!   (`whisper_full_get_segment_t0_from_state`) ; celles des *tokens*
//!   (`whisper_full_get_token_data_from_state`) restent dans la chronologie
//!   compactée. Chaque silence retiré décalerait d'autant tous les mots
//!   suivants — et le JSON porte toujours les mots ;
//! - **l'audio serait recopié.** whisper.cpp assemble la parole dans un second
//!   tampon : jusqu'à 223 Mo de plus par heure d'audio, au moment même du pic
//!   mémoire (ADR-003).
//!
//! Scripta détecte donc la parole avec le modèle Silero de whisper.cpp, compacte
//! l'audio **sur place**, et conserve une table de correspondance exacte entre
//! les deux chronologies. Le test `le_vad_conserve_la_chronologie_d_origine`
//! (`tests/inference.rs`) le vérifie, et échoue si l'on revient au VAD intégré.

use std::path::Path;

use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use crate::audio::SAMPLE_RATE;
use crate::error::{Result, ScriptaError};

/// Échantillons par centiseconde, l'unité des horodatages de whisper.cpp.
const SAMPLES_PER_CS: usize = SAMPLE_RATE as usize / 100;

/// Silence inséré entre deux plages de parole : 0,1 s, comme whisper.cpp.
/// Sans lui, la fin d'une phrase et le début de la suivante se touchent, et
/// le modèle peine à placer la frontière entre les deux segments.
const SILENCE: usize = SAMPLE_RATE as usize / 10;

/// Threads de la détection : **un seul**. Silero traite des fenêtres de 32 ms
/// par un réseau minuscule ; chaque fenêtre est un petit graphe ggml, et au-delà
/// d'un thread la synchronisation coûte plus que le calcul. Mesuré sur 10 min
/// d'audio : 1,3 s avec 1 thread, 3,4 s avec 4, 52 s avec 20 — la valeur qui
/// aurait suivi `--threads` sur la machine de mesure, et qui ajoutait cinq
/// minutes par heure d'audio.
const THREADS: i32 = 1;

/// Prolongement de chaque plage de parole : 0,1 s, comme le `samples_overlap`
/// de whisper.cpp. Silero coupe parfois avant l'extinction de la dernière
/// syllabe ; ce reliquat d'audio réel la préserve.
const OVERLAP: usize = SAMPLE_RATE as usize / 10;

/// Plage de parole, en échantillons, sur la chronologie d'origine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// Tronçon de la table de correspondance.
///
/// Un tronçon de parole a la même longueur dans les deux chronologies ; un
/// tronçon de silence est comprimé, et sa correspondance est linéaire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Piece {
    processed: usize,
    original: usize,
    processed_len: usize,
    original_len: usize,
}

/// Correspondance entre la chronologie de l'audio compacté, sur laquelle
/// whisper.cpp horodate, et celle de la vidéo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimeMap {
    /// Vide pour l'identité, c'est-à-dire sans VAD.
    pieces: Vec<Piece>,
}

impl TimeMap {
    pub fn identity() -> Self {
        Self::default()
    }

    pub fn is_identity(&self) -> bool {
        self.pieces.is_empty()
    }

    /// Convertit un horodatage whisper.cpp — en centisecondes, sur l'audio
    /// compacté — en secondes sur la chronologie d'origine.
    pub fn to_original_s(&self, processed_cs: i64) -> f64 {
        let cs = processed_cs.max(0);
        if self.is_identity() {
            // Chemin sans VAD : strictement le calcul d'avant son introduction.
            return cs as f64 / 100.0;
        }
        let original = self.to_original_samples(cs as usize * SAMPLES_PER_CS);
        original as f64 / SAMPLE_RATE as f64
    }

    fn to_original_samples(&self, processed: usize) -> usize {
        let Some(premier) = self.pieces.first() else {
            return processed;
        };
        if processed < premier.processed {
            return premier.original;
        }

        // Dernier tronçon commençant au plus tard à `processed`.
        let i = self
            .pieces
            .partition_point(|p| p.processed <= processed)
            .saturating_sub(1);
        let p = &self.pieces[i];
        let dedans = processed - p.processed;

        if dedans >= p.processed_len {
            // Au-delà du dernier tronçon : whisper.cpp déborde parfois de
            // quelques centisecondes sur la fin de l'audio.
            return p.original + p.original_len + (dedans - p.processed_len);
        }
        if p.processed_len == p.original_len {
            return p.original + dedans;
        }
        let etire = dedans as u128 * p.original_len as u128 / p.processed_len as u128;
        p.original + etire as usize
    }
}

/// Détecte la parole avec le modèle Silero de whisper.cpp.
///
/// Rend les plages dans l'ordre, en échantillons. Une liste vide signifie
/// qu'aucune parole n'a été détectée — ce n'est pas une erreur.
pub fn detect(model: &Path, samples: &[f32]) -> Result<Vec<Span>> {
    crate::transcribe::silence_native_logs();

    let chemin = model
        .to_str()
        .ok_or_else(|| ScriptaError::ModelUnavailable {
            detail: format!("chemin du modèle VAD non UTF-8 : {}", model.display()),
        })?;

    let mut params = WhisperVadContextParams::new();
    params.set_n_threads(THREADS);
    // Silero pèse moins d'un mégaoctet : le CPU suffit largement, et le GPU
    // reste libre pour l'inférence.
    params.set_use_gpu(false);

    let mut contexte =
        WhisperVadContext::new(chemin, params).map_err(|e| ScriptaError::ModelUnavailable {
            detail: format!("chargement du modèle VAD {} : {e}", model.display()),
        })?;

    let segments = contexte
        .segments_from_samples(WhisperVadParams::new(), samples)
        .map_err(|e| ScriptaError::InferenceFailed {
            detail: format!("détection d'activité vocale : {e}"),
        })?;

    let total = samples.len();
    Ok(segments
        .map(|s| Span {
            start: cs_to_samples(s.start).min(total),
            end: cs_to_samples(s.end).min(total),
        })
        .filter(|s| s.end > s.start)
        .collect())
}

fn cs_to_samples(cs: f32) -> usize {
    if cs.is_finite() && cs > 0.0 {
        (f64::from(cs) * SAMPLES_PER_CS as f64).round() as usize
    } else {
        0
    }
}

/// Ne conserve que la parole, **sur place**, et rend la correspondance des
/// chronologies.
///
/// Les plages sont recopiées vers le début du tampon, séparées par un court
/// silence. La position d'écriture ne dépasse jamais celle de lecture — le
/// silence inséré n'excède jamais celui qu'il remplace —, ce qui rend l'opération
/// sûre sans second tampon. La capacité du `Vec` est conservée : la réduire
/// imposerait justement la recopie que ce module évite.
pub fn compact(samples: &mut Vec<f32>, spans: &[Span]) -> TimeMap {
    let plages = normalize(spans, samples.len());
    let mut pieces = Vec::with_capacity(plages.len() * 2);
    let mut ecrit = 0usize;

    for (i, plage) in plages.iter().enumerate() {
        let longueur = plage.end - plage.start;
        samples.copy_within(plage.start..plage.end, ecrit);
        pieces.push(Piece {
            processed: ecrit,
            original: plage.start,
            processed_len: longueur,
            original_len: longueur,
        });
        ecrit += longueur;

        if let Some(suivante) = plages.get(i + 1) {
            // Positif : les plages qui se touchent ont été fusionnées.
            let trou = suivante.start - plage.end;
            let silence = trou.min(SILENCE);
            samples[ecrit..ecrit + silence].fill(0.0);
            pieces.push(Piece {
                processed: ecrit,
                original: plage.end,
                processed_len: silence,
                original_len: trou,
            });
            ecrit += silence;
        }
    }

    samples.truncate(ecrit);
    TimeMap { pieces }
}

/// Bornes ramenées dans le tampon, plages vides écartées, ordre rétabli,
/// prolongement appliqué, et plages qui se chevauchent ou se touchent fusionnées.
fn normalize(spans: &[Span], len: usize) -> Vec<Span> {
    let mut triees: Vec<Span> = spans
        .iter()
        .map(|s| Span {
            start: s.start.min(len),
            end: s.end.min(len),
        })
        .filter(|s| s.end > s.start)
        .collect();
    triees.sort_by_key(|s| s.start);

    let mut fusionnees: Vec<Span> = Vec::with_capacity(triees.len());
    for s in triees {
        let s = Span {
            start: s.start,
            end: (s.end + OVERLAP).min(len),
        };
        match fusionnees.last_mut() {
            Some(precedente) if s.start <= precedente.end => {
                precedente.end = precedente.end.max(s.end);
            }
            _ => fusionnees.push(s),
        }
    }
    fusionnees
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tampon dont chaque échantillon vaut son propre indice : après
    /// compactage, la valeur lue dit d'où elle vient. Exact en `f32` jusqu'à
    /// 2²⁴ échantillons, soit 17 minutes.
    fn tampon_indexe(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32).collect()
    }

    /// Implémentation de référence, hors place : ce que `compact` doit
    /// produire, calculé sans la contrainte de travailler dans un seul tampon.
    fn compact_reference(source: &[f32], spans: &[Span]) -> Vec<f32> {
        let plages = normalize(spans, source.len());
        let mut out = Vec::new();
        for (i, p) in plages.iter().enumerate() {
            out.extend_from_slice(&source[p.start..p.end]);
            if let Some(s) = plages.get(i + 1) {
                out.extend(std::iter::repeat_n(0.0, (s.start - p.end).min(SILENCE)));
            }
        }
        out
    }

    fn span(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    #[test]
    fn l_identite_conserve_le_calcul_historique() {
        let carte = TimeMap::identity();
        assert!(carte.is_identity());
        for cs in [0, 1, 1234, 366_400] {
            assert_eq!(carte.to_original_s(cs), cs as f64 / 100.0);
        }
        // Une valeur négative, jamais émise en pratique, est ramenée à zéro.
        assert_eq!(carte.to_original_s(-5), 0.0);
    }

    #[test]
    fn compacte_deux_plages_et_retrouve_la_chronologie() {
        // 8 s d'audio, parole de 1 à 2 s puis de 5 à 6 s.
        let mut audio = tampon_indexe(128_000);
        let spans = [span(16_000, 32_000), span(80_000, 96_000)];
        let carte = compact(&mut audio, &spans);

        // Plage 1 prolongée de 0,1 s : 1,0 → 2,1 s, soit 17 600 échantillons.
        assert_eq!(audio[0], 16_000.0);
        assert_eq!(audio[17_599], 33_599.0);
        // Puis 0,1 s de silence inséré.
        assert!(audio[17_600..19_200].iter().all(|&x| x == 0.0));
        // Puis la plage 2, prolongée elle aussi.
        assert_eq!(audio[19_200], 80_000.0);
        assert_eq!(audio[36_799], 97_599.0);
        assert_eq!(audio.len(), 36_800);

        // Correspondance exacte dans la parole.
        assert_eq!(carte.to_original_samples(0), 16_000);
        assert_eq!(carte.to_original_samples(17_599), 33_599);
        assert_eq!(carte.to_original_samples(19_200), 80_000);
        assert_eq!(carte.to_original_samples(36_799), 97_599);
        // Le silence inséré s'étire sur le silence retiré : son milieu tombe au
        // milieu de ce dernier.
        assert_eq!(carte.to_original_samples(18_400), 56_800);
        // Au-delà de la fin, prolongement à l'identique.
        assert_eq!(carte.to_original_samples(36_900), 97_700);

        // En centisecondes, l'unité de whisper.cpp : 1,2 s compactée = 5 s réelle.
        assert_eq!(carte.to_original_s(120), 5.0);
        assert_eq!(carte.to_original_s(0), 1.0);
    }

    #[test]
    fn le_compactage_sur_place_egale_la_reference() {
        let source = tampon_indexe(200_000);
        let cas: [&[Span]; 5] = [
            &[span(0, 1_000), span(3_000, 4_000)], // trou plus court que le silence
            &[span(50_000, 60_000)],               // plage unique
            &[span(10, 20), span(15, 5_000)],      // chevauchement
            &[span(100_000, 110_000), span(1_000, 2_000)], // désordre
            &[span(190_000, 250_000), span(5, 5)], // débordement, plage vide
        ];
        for spans in cas {
            let mut audio = source.clone();
            compact(&mut audio, spans);
            assert_eq!(audio, compact_reference(&source, spans), "{spans:?}");
        }
    }

    #[test]
    fn les_plages_proches_fusionnent() {
        // Un trou inférieur au prolongement disparaît : pas de silence inséré
        // au milieu d'une phrase à peine suspendue.
        let n = normalize(&[span(0, 1_000), span(1_500, 3_000)], 10_000);
        assert_eq!(n, vec![span(0, 3_000 + OVERLAP)]);
    }

    #[test]
    fn la_correspondance_est_croissante() {
        let mut audio = tampon_indexe(160_000);
        let carte = compact(
            &mut audio,
            &[
                span(8_000, 20_000),
                span(40_000, 41_000),
                span(90_000, 150_000),
            ],
        );
        let mut precedent = 0;
        for p in 0..audio.len() + 1_000 {
            let o = carte.to_original_samples(p);
            assert!(o >= precedent, "régression en {p} : {o} < {precedent}");
            precedent = o;
        }
    }

    #[test]
    fn la_capacite_est_conservee() {
        // Réduire la capacité recopierait le tampon — précisément ce que le
        // compactage sur place évite.
        let mut audio = tampon_indexe(64_000);
        let capacite = audio.capacity();
        compact(&mut audio, &[span(1_000, 2_000)]);
        assert_eq!(audio.capacity(), capacite);
    }

    #[test]
    fn conversion_des_centisecondes() {
        assert_eq!(cs_to_samples(0.0), 0);
        assert_eq!(cs_to_samples(1.0), 160);
        assert_eq!(cs_to_samples(-3.0), 0);
        assert_eq!(cs_to_samples(f32::NAN), 0);
    }
}
