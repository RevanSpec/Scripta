//! Représentation d'une transcription — SPEC SF-05.

use serde::{Deserialize, Serialize};

/// Segment horodaté produit par le moteur d'inférence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: usize,
    /// Début, en secondes depuis le début du média.
    pub start: f64,
    /// Fin, en secondes.
    pub end: f64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub no_speech_prob: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub avg_logprob: Option<f32>,
    /// Renseigné uniquement si l'horodatage au mot est demandé (SF-04).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub word: String,
    pub start: f64,
    pub end: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub probability: Option<f32>,
}

/// Transcription complète.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Transcript {
    pub segments: Vec<Segment>,
    /// Langue détectée ou imposée (ISO 639-1).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language_probability: Option<f32>,
    #[serde(default)]
    pub translated: bool,
}

impl Transcript {
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Durée couverte par le dernier segment, en secondes.
    pub fn covered_duration(&self) -> f64 {
        self.segments.last().map(|s| s.end).unwrap_or(0.0)
    }
}

impl Segment {
    /// Constructeur concis, utile aux tests et aux formateurs.
    pub fn new(id: usize, start: f64, end: f64, text: impl Into<String>) -> Self {
        Self {
            id,
            start,
            end,
            text: text.into(),
            no_speech_prob: None,
            avg_logprob: None,
            words: Vec::new(),
        }
    }
}
