//! Document de sortie — SPEC SF-05.
//!
//! Regroupe la provenance de l'audio, les conditions d'exécution et la
//! transcription. C'est l'entrée unique de tous les formateurs : `txt`, `srt` et
//! `vtt` n'en exploitent que la transcription, `json` la totalité.

use serde::Serialize;

use crate::transcript::Transcript;

/// Provenance de l'audio.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Source {
    pub url: String,
    pub video_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_date: Option<String>,
}

/// Conditions dans lesquelles la transcription a été produite.
///
/// Ces champs rendent un résultat reproductible et comparable : sans le modèle
/// ni le backend, deux fichiers JSON divergents sont inexploitables.
#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub engine: &'static str,
    pub model: String,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_probability: Option<f32>,
    pub translated: bool,
    pub vad: bool,
    /// Durée de l'inférence.
    pub duration_ms: u64,
    /// Rapport entre la durée de l'audio et celle de l'inférence.
    pub speed_realtime: f64,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            engine: "whisper.cpp",
            model: String::new(),
            backend: String::new(),
            language: None,
            language_probability: None,
            translated: false,
            vad: false,
            duration_ms: 0,
            speed_realtime: 0.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Document {
    pub source: Source,
    pub run: Run,
    pub transcript: Transcript,
}

impl Document {
    /// Construit un document sans métadonnées de provenance, pour les tests et
    /// les usages où seule la transcription importe.
    pub fn from_transcript(transcript: Transcript) -> Self {
        Self {
            transcript,
            ..Default::default()
        }
    }
}
