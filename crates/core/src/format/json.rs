//! Format JSON enrichi — SPEC SF-05.
//!
//! `schema_version` permet de faire évoluer la structure sans casser les
//! consommateurs : un script qui lit ce fichier doit pouvoir refuser une
//! version qu'il ne connaît pas plutôt que d'en mésinterpréter les champs.

use serde::Serialize;

use crate::document::{Document, Run, Source};
use crate::transcript::Segment;

/// Incrémenter à chaque changement incompatible de la structure.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
struct JsonDocument<'a> {
    schema_version: u32,
    source: &'a Source,
    transcription: &'a Run,
    segments: &'a [Segment],
}

pub fn render(doc: &Document) -> String {
    let out = JsonDocument {
        schema_version: SCHEMA_VERSION,
        source: &doc.source,
        transcription: &doc.run,
        segments: &doc.transcript.segments,
    };

    // La sérialisation d'une structure sans boucle de références ne peut pas
    // échouer ; le repli évite néanmoins un `unwrap` sur un chemin public.
    match serde_json::to_string_pretty(&out) {
        Ok(mut s) => {
            s.push('\n');
            s
        }
        Err(e) => format!("{{\"error\":\"sérialisation impossible: {e}\"}}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Transcript, Word};

    fn document() -> Document {
        Document {
            source: Source {
                url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
                video_id: "dQw4w9WgXcQ".into(),
                title: "Titre".into(),
                channel: Some("Chaîne".into()),
                duration_s: Some(212.0),
                upload_date: Some("20251104".into()),
            },
            run: Run {
                model: "ggml-tiny".into(),
                backend: "cpu".into(),
                language: Some("fr".into()),
                duration_ms: 4200,
                speed_realtime: 8.6,
                vad: true,
                ..Default::default()
            },
            transcript: Transcript {
                segments: vec![Segment {
                    id: 0,
                    start: 12.5,
                    end: 15.0,
                    text: "Bonjour à tous.".into(),
                    no_speech_prob: Some(0.01),
                    avg_logprob: None,
                    words: vec![Word {
                        word: "Bonjour".into(),
                        start: 12.5,
                        end: 12.91,
                        probability: Some(0.99),
                    }],
                }],
                language: Some("fr".into()),
                language_probability: None,
                translated: false,
            },
        }
    }

    fn parse(s: &str) -> serde_json::Value {
        serde_json::from_str(s).expect("le JSON produit doit être valide")
    }

    #[test]
    fn structure_conforme_au_schema() {
        let v = parse(&render(&document()));
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["source"]["video_id"], "dQw4w9WgXcQ");
        assert_eq!(v["source"]["channel"], "Chaîne");
        assert_eq!(v["transcription"]["engine"], "whisper.cpp");
        assert_eq!(v["transcription"]["model"], "ggml-tiny");
        assert_eq!(v["transcription"]["backend"], "cpu");
        assert_eq!(v["transcription"]["vad"], true);
        assert_eq!(v["segments"][0]["start"], 12.5);
        assert_eq!(v["segments"][0]["text"], "Bonjour à tous.");
        assert_eq!(v["segments"][0]["words"][0]["word"], "Bonjour");
    }

    #[test]
    fn les_champs_absents_sont_omis_et_non_nuls() {
        // Un consommateur doit pouvoir tester la présence d'une clé ; `null`
        // l'obligerait à distinguer deux formes d'absence.
        let mut d = document();
        d.source.channel = None;
        d.transcript.segments[0].words.clear();
        d.transcript.segments[0].avg_logprob = None;

        let v = parse(&render(&d));
        assert!(v["source"].get("channel").is_none());
        assert!(v["segments"][0].get("words").is_none());
        assert!(v["segments"][0].get("avg_logprob").is_none());
    }

    #[test]
    fn le_json_est_valide_meme_vide() {
        let v = parse(&render(&Document::default()));
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["segments"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn preserve_les_caracteres_non_ascii() {
        let mut d = document();
        d.transcript.segments[0].text = "日本語 · Ελληνικά · émoji 🎙".into();
        let v = parse(&render(&d));
        assert_eq!(v["segments"][0]["text"], "日本語 · Ελληνικά · émoji 🎙");
    }

    #[test]
    fn se_termine_par_un_saut_de_ligne() {
        // Convention Unix : un fichier texte se termine par un saut de ligne,
        // et `scripta … -f json | jq` s'en trouve mieux.
        assert!(render(&document()).ends_with("}\n"));
    }
}
