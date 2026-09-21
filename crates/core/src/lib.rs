//! Cœur applicatif de Scripta.
//!
//! Ce crate ne dépend d'aucune couche de présentation : la CLI et la future
//! application Tauri en sont deux consommateurs symétriques (SPEC §2.2).
//!
//! # État — Jalon 1
//!
//! Livré : validation d'URL, sonde de métadonnées, pipeline d'extraction audio,
//! taxonomie d'erreurs, format `txt`.
//!
//! Manquant : l'inférence (`whisper-rs`) est bloquée sur l'absence de CMake dans
//! l'environnement de développement — voir `docs/ROADMAP.md`, Jalon 0, tâche 0.1.

pub mod audio;
pub mod error;
pub mod format;
pub mod probe;
pub mod transcript;
pub mod url;

pub use error::{Result, ScriptaError};
pub use transcript::{Segment, Transcript, Word};
pub use url::CanonicalUrl;
