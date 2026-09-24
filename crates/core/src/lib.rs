//! Cœur applicatif de Scripta.
//!
//! Ce crate ne dépend d'aucune couche de présentation : la CLI et
//! l'application de bureau en sont deux consommateurs symétriques (SPEC §2.2).
//!
//! # État — Jalon 2
//!
//! Chaîne complète URL → transcription : validation d'URL, sonde de
//! métadonnées, pipeline d'extraction audio, VAD, inférence Whisper, formats
//! `txt`, `srt`, `vtt` et `json`. Autour d'elle : téléchargement vérifié des
//! modèles, cache de transcriptions, sous-titres officiels, cookies, mise à
//! jour de l'extracteur, annulation de toutes les opérations longues.
//!
//! [`pipeline`] enchaîne ces étapes pour les deux interfaces, qui n'en
//! reçoivent que les événements : aucune règle d'orchestration n'est écrite
//! deux fois.

pub mod audio;
pub mod cache;
pub mod cancel;
pub mod diagnostic;
pub mod document;
pub mod error;
pub mod format;
pub mod models;
pub mod output;
pub mod paths;
pub mod pipeline;
pub mod probe;
pub mod sidecar;
pub mod subtitles;
pub mod transcribe;
pub mod transcript;
pub mod url;
pub mod vad;

pub use cancel::CancelToken;
pub use document::{Document, Run, Source};
pub use error::{Result, ScriptaError};
pub use models::ModelSpec;
pub use probe::Access;
pub use transcribe::{Backend, Engine};
pub use transcript::{Segment, Transcript, Word};
pub use url::CanonicalUrl;
