//! Format SubRip (`.srt`) — SPEC SF-05.

use super::cues::{self, SubtitleOptions};
use crate::document::Document;

pub fn render(doc: &Document, opts: &SubtitleOptions) -> String {
    let mut out = String::new();
    for cue in cues::build(&doc.transcript, opts) {
        // SRT n'a pas d'échappement standardisé : le texte est écrit tel quel,
        // contrairement à WebVTT qui exige des entités XML.
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            cue.index,
            cues::timestamp(cue.start, ','),
            cues::timestamp(cue.end, ','),
            cue.text()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Segment, Transcript};

    fn doc(segments: Vec<Segment>) -> Document {
        Document::from_transcript(Transcript {
            segments,
            ..Default::default()
        })
    }

    #[test]
    fn rendu_conforme_au_format() {
        let d = doc(vec![
            Segment::new(0, 12.5, 15.0, "Bonjour à tous."),
            Segment::new(1, 15.2, 18.4, "Deuxième réplique."),
        ]);
        assert_eq!(
            render(&d, &SubtitleOptions::default()),
            "1\n00:00:12,500 --> 00:00:15,000\nBonjour à tous.\n\n\
             2\n00:00:15,200 --> 00:00:18,400\nDeuxième réplique.\n\n"
        );
    }

    #[test]
    fn utilise_la_virgule_decimale() {
        let d = doc(vec![Segment::new(0, 1.25, 3.5, "test")]);
        let out = render(&d, &SubtitleOptions::default());
        assert!(out.contains("00:00:01,250"), "{out}");
        assert!(!out.contains("00:00:01.250"), "{out}");
    }

    #[test]
    fn transcription_vide_ne_produit_rien() {
        assert_eq!(
            render(&Document::default(), &SubtitleOptions::default()),
            ""
        );
    }
}
