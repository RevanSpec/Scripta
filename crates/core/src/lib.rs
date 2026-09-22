//! Cœur applicatif de Scripta.
//!
//! Ce crate ne dépend d'aucune couche de présentation : la CLI et la future
//! application Tauri en sont deux consommateurs symétriques (SPEC §2.2).
//!
//! # État — Jalon 1
//!
//! Livré : validation d'URL, sonde de métadonnées, pipeline d'extraction audio,
//! inférence Whisper, taxonomie d'erreurs, format `txt`.
//!
//! Formats `txt`, `srt`, `vtt` et `json` livrés (tâche 2.5).
//!
//! Restent au Jalon 2 : téléchargement des modèles, cache de transcriptions,
//! sous-titres officiels, cookies.

pub mod audio;
pub mod document;
pub mod error;
pub mod format;
pub mod models;
pub mod probe;
pub mod transcribe;
pub mod transcript;
pub mod url;

pub use document::{Document, Run, Source};
pub use error::{Result, ScriptaError};
pub use models::ModelSpec;
pub use transcribe::{Backend, CancelToken, Engine};
pub use transcript::{Segment, Transcript, Word};
pub use url::CanonicalUrl;
