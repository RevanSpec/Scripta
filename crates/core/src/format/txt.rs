//! Format texte brut — SPEC SF-05 : « transcription continue, nettoyée, sans
//! horodatage ».

use crate::transcript::Transcript;

/// Concatène les segments en un texte continu.
///
/// Whisper produit des segments dont le texte porte des espaces de bordure
/// irréguliers et, sur certains contenus, des espaces internes multiples : la
/// simple concaténation donnerait un rendu sale.
pub fn render(transcript: &Transcript) -> String {
    let mut out = String::new();
    for segment in &transcript.segments {
        let text = collapse_whitespace(&segment.text);
        if text.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&text);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Réduit toute suite d'espaces — y compris les sauts de ligne internes — à un
/// espace unique, et supprime les bordures.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Segment;

    fn transcript(textes: &[&str]) -> Transcript {
        Transcript {
            segments: textes
                .iter()
                .enumerate()
                .map(|(i, t)| Segment::new(i, i as f64, i as f64 + 1.0, *t))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn concatene_avec_un_espace_unique() {
        // Whisper préfixe habituellement ses segments d'une espace.
        let t = transcript(&[" Bonjour à tous", " et bienvenue."]);
        assert_eq!(render(&t), "Bonjour à tous et bienvenue.\n");
    }

    #[test]
    fn normalise_les_espaces_internes() {
        let t = transcript(&["Un  texte\tavec\ndes espaces"]);
        assert_eq!(render(&t), "Un texte avec des espaces\n");
    }

    #[test]
    fn ignore_les_segments_vides() {
        let t = transcript(&["Premier", "   ", "", "Second"]);
        assert_eq!(render(&t), "Premier Second\n");
    }

    #[test]
    fn transcription_vide_ne_produit_pas_de_saut_de_ligne() {
        assert_eq!(render(&Transcript::default()), "");
        assert_eq!(render(&transcript(&["", "  "])), "");
    }

    #[test]
    fn preserve_les_caracteres_non_latins() {
        let t = transcript(&["日本語のテスト", "Ελληνικά"]);
        assert_eq!(render(&t), "日本語のテスト Ελληνικά\n");
    }
}
