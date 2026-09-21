//! Formateurs de sortie — SPEC SF-05.
//!
//! Jalon 1 : `txt` seul. `srt`, `vtt` et `json` relèvent du Jalon 2 (tâche 2.5).

pub mod txt;

use crate::transcript::Transcript;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Txt,
}

impl OutputFormat {
    pub fn render(&self, transcript: &Transcript) -> String {
        match self {
            Self::Txt => txt::render(transcript),
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Txt => "txt",
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "txt" | "text" => Ok(Self::Txt),
            // Message explicite plutôt qu'un « valeur invalide » générique :
            // ces formats sont spécifiés et attendus, simplement pas encore livrés.
            "srt" | "vtt" | "json" => {
                Err(format!("le format « {s} » arrive au Jalon 2 (SPEC SF-05)"))
            }
            other => Err(format!("format inconnu : « {other} »")),
        }
    }
}
