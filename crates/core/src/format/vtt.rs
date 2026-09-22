//! Format WebVTT (`.vtt`) — SPEC SF-05.

use super::cues::{self, SubtitleOptions};
use crate::document::Document;

pub fn render(doc: &Document, opts: &SubtitleOptions) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues::build(&doc.transcript, opts) {
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            cues::timestamp(cue.start, '.'),
            cues::timestamp(cue.end, '.'),
            escape(&cue.text())
        ));
    }
    out
}

/// WebVTT interprète `<` et `&` comme du balisage : un texte transcrit
/// contenant « R&D » ou « <inaudible> » corromprait le fichier sans
/// échappement.
fn escape(s: &str) -> String {
    // L'esperluette d'abord, sans quoi les entités produites seraient
    // elles-mêmes réécrites.
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
    fn commence_par_l_en_tete_obligatoire() {
        let out = render(&Document::default(), &SubtitleOptions::default());
        assert!(out.starts_with("WEBVTT\n\n"), "{out:?}");
    }

    #[test]
    fn utilise_le_point_decimal_et_omet_la_numerotation() {
        let d = doc(vec![Segment::new(0, 12.5, 15.0, "Bonjour.")]);
        let out = render(&d, &SubtitleOptions::default());
        assert!(out.contains("00:00:12.500 --> 00:00:15.000"), "{out}");
        assert!(!out.contains("\n1\n"), "numérotation SRT parasite : {out}");
    }

    #[test]
    fn echappe_les_entites_xml() {
        let d = doc(vec![Segment::new(0, 0.0, 2.0, "R&D <inaudible> a>b")]);
        let out = render(&d, &SubtitleOptions::default());
        assert!(out.contains("R&amp;D &lt;inaudible&gt; a&gt;b"), "{out}");
    }

    #[test]
    fn n_echappe_pas_deux_fois_l_esperluette() {
        let d = doc(vec![Segment::new(0, 0.0, 2.0, "a < b")]);
        let out = render(&d, &SubtitleOptions::default());
        assert!(out.contains("a &lt; b"), "{out}");
        assert!(!out.contains("&amp;lt;"), "double échappement : {out}");
    }
}
