//! Formateurs de sortie — SPEC SF-05.

pub mod cues;
pub mod json;
pub mod srt;
pub mod txt;
pub mod vtt;

pub use cues::SubtitleOptions;

use crate::document::Document;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Txt,
    Srt,
    Vtt,
    Json,
}

impl OutputFormat {
    pub fn render(&self, doc: &Document, opts: &SubtitleOptions) -> String {
        match self {
            Self::Txt => txt::render(&doc.transcript),
            Self::Srt => srt::render(doc, opts),
            Self::Vtt => vtt::render(doc, opts),
            Self::Json => json::render(doc),
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Txt => "txt",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::Json => "json",
        }
    }

    /// Vrai si le format s'appuie sur le découpage en cues, donc sur les
    /// contraintes de lisibilité.
    pub fn uses_subtitle_options(&self) -> bool {
        matches!(self, Self::Srt | Self::Vtt)
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "txt" | "text" => Ok(Self::Txt),
            "srt" => Ok(Self::Srt),
            "vtt" | "webvtt" => Ok(Self::Vtt),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "format inconnu : « {other} » (attendu : txt, srt, vtt, json)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Segment, Transcript};

    #[test]
    fn analyse_les_noms_de_format() {
        for (entree, attendu) in [
            ("txt", OutputFormat::Txt),
            ("TXT", OutputFormat::Txt),
            ("srt", OutputFormat::Srt),
            ("vtt", OutputFormat::Vtt),
            ("webvtt", OutputFormat::Vtt),
            ("json", OutputFormat::Json),
        ] {
            assert_eq!(entree.parse::<OutputFormat>().unwrap(), attendu, "{entree}");
        }
        assert!("xml".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn chaque_format_produit_une_sortie_distincte() {
        let doc = Document::from_transcript(Transcript {
            segments: vec![Segment::new(0, 0.0, 2.0, "Bonjour")],
            ..Default::default()
        });
        let opts = SubtitleOptions::default();

        let rendus: Vec<String> = [
            OutputFormat::Txt,
            OutputFormat::Srt,
            OutputFormat::Vtt,
            OutputFormat::Json,
        ]
        .iter()
        .map(|f| f.render(&doc, &opts))
        .collect();

        for r in &rendus {
            assert!(r.contains("Bonjour"), "texte absent : {r}");
        }
        // Aucun format ne doit produire le rendu d'un autre.
        for i in 0..rendus.len() {
            for j in (i + 1)..rendus.len() {
                assert_ne!(rendus[i], rendus[j]);
            }
        }
    }

    #[test]
    fn extensions_coherentes() {
        assert_eq!(OutputFormat::Srt.extension(), "srt");
        assert!(OutputFormat::Srt.uses_subtitle_options());
        assert!(!OutputFormat::Json.uses_subtitle_options());
    }
}
