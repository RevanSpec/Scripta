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
//! Restent au Jalon 2 : téléchargement des modèles, formats `srt`/`vtt`/`json`,
//! cache de transcriptions, sous-titres officiels.

pub mod audio;
pub mod error;
pub mod format;
pub mod probe;
pub mod transcribe;
pub mod transcript;
pub mod url;

pub use error::{Result, ScriptaError};
pub use transcribe::{Backend, CancelToken, Engine};
pub use transcript::{Segment, Transcript, Word};
pub use url::CanonicalUrl;
