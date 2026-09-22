//! Découpage en cues de sous-titres — SPEC SF-05.
//!
//! Whisper produit des segments dont la longueur suit sa propre logique de
//! décodage, sans rapport avec ce qu'un œil humain peut lire en une fois. Ce
//! module les redécoupe selon les contraintes de lisibilité, et répartit les
//! horodatages en conséquence. Le résultat alimente à la fois `srt` et `vtt`.

use crate::transcript::Transcript;

/// Contraintes de lisibilité, paramétrables en CLI — SPEC SF-05.
#[derive(Debug, Clone, Copy)]
pub struct SubtitleOptions {
    /// Largeur maximale d'une ligne, en caractères.
    pub max_line_width: usize,
    /// Nombre maximal de lignes par cue.
    pub max_line_count: usize,
    /// Durée d'affichage minimale, en secondes.
    pub min_duration: f64,
    /// Durée d'affichage maximale, en secondes.
    pub max_duration: f64,
}

impl Default for SubtitleOptions {
    fn default() -> Self {
        Self {
            max_line_width: 42,
            max_line_count: 2,
            min_duration: 1.0,
            max_duration: 7.0,
        }
    }
}

/// Un sous-titre affichable.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    /// Numérotation séquentielle, à partir de 1 (exigence du format SRT).
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub lines: Vec<String>,
}

impl Cue {
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// Redécoupe une transcription en cues respectant les contraintes.
pub fn build(transcript: &Transcript, opts: &SubtitleOptions) -> Vec<Cue> {
    let mut cues: Vec<Cue> = Vec::new();

    for segment in &transcript.segments {
        let lines = wrap(&collapse_whitespace(&segment.text), opts.max_line_width);
        if lines.is_empty() {
            continue;
        }

        let groups: Vec<Vec<String>> = lines
            .chunks(opts.max_line_count.max(1))
            .map(|c| c.to_vec())
            .collect();

        // Répartition de la durée du segment au prorata du volume de texte :
        // un groupe deux fois plus long reste affiché deux fois plus longtemps.
        let total_chars: usize = groups
            .iter()
            .map(|g| g.iter().map(|l| l.chars().count()).sum::<usize>().max(1))
            .sum();
        let span = (segment.end - segment.start).max(0.0);

        let mut curseur = segment.start;
        for (i, group) in groups.iter().enumerate() {
            let chars: usize = group
                .iter()
                .map(|l| l.chars().count())
                .sum::<usize>()
                .max(1);
            let part = span * (chars as f64 / total_chars as f64);
            // Le dernier groupe reprend la borne exacte du segment : cumuler
            // des fractions laisserait dériver la fin de quelques millisecondes.
            let fin = if i + 1 == groups.len() {
                segment.end
            } else {
                curseur + part
            };

            cues.push(Cue {
                index: cues.len() + 1,
                start: curseur,
                end: fin,
                lines: group.clone(),
            });
            curseur = fin;
        }
    }

    enforce_durations(&mut cues, opts);
    cues
}

/// Applique les bornes de durée d'affichage.
///
/// L'allongement d'un cue trop bref ne doit jamais empiéter sur le suivant :
/// deux sous-titres simultanés sont pires qu'un sous-titre fugace.
fn enforce_durations(cues: &mut [Cue], opts: &SubtitleOptions) {
    for i in 0..cues.len() {
        let limite = cues.get(i + 1).map(|c| c.start);

        if cues[i].end - cues[i].start > opts.max_duration {
            cues[i].end = cues[i].start + opts.max_duration;
        }

        if cues[i].end - cues[i].start < opts.min_duration {
            let souhaite = cues[i].start + opts.min_duration;
            cues[i].end = match limite {
                Some(suivant) => souhaite.min(suivant).max(cues[i].end),
                None => souhaite,
            };
        }
    }
}

/// Découpe un texte en lignes d'au plus `width` caractères, aux frontières de
/// mots.
///
/// Un mot plus long que la largeur — URL, ou langue sans espaces comme le
/// japonais — est coupé sans ménagement : le laisser déborder produirait une
/// ligne illisible, voire tronquée par le lecteur.
fn wrap(texte: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lignes = Vec::new();
    let mut courante = String::new();
    let mut courante_len = 0usize;

    for mot in texte.split_whitespace() {
        let mot_len = mot.chars().count();

        if mot_len > width {
            if courante_len > 0 {
                lignes.push(std::mem::take(&mut courante));
                courante_len = 0;
            }
            for morceau in split_chars(mot, width) {
                lignes.push(morceau);
            }
            // Le dernier morceau peut encore accueillir du texte.
            if let Some(dernier) = lignes.pop() {
                courante_len = dernier.chars().count();
                courante = dernier;
            }
            continue;
        }

        let besoin = if courante_len == 0 {
            mot_len
        } else {
            courante_len + 1 + mot_len
        };
        if besoin > width {
            lignes.push(std::mem::take(&mut courante));
            courante.push_str(mot);
            courante_len = mot_len;
        } else {
            if courante_len > 0 {
                courante.push(' ');
            }
            courante.push_str(mot);
            courante_len = besoin;
        }
    }

    if !courante.is_empty() {
        lignes.push(courante);
    }
    lignes
}

fn split_chars(mot: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for (i, c) in mot.chars().enumerate() {
        if i > 0 && i % width == 0 {
            out.push(std::mem::take(&mut buf));
        }
        buf.push(c);
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

pub(crate) fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            pending = !out.is_empty();
        } else {
            if pending {
                out.push(' ');
                pending = false;
            }
            out.push(ch);
        }
    }
    out
}

/// Horodatage `HH:MM:SS<sep>mmm`. SRT emploie la virgule, WebVTT le point.
pub fn timestamp(secondes: f64, separateur: char) -> String {
    let total_ms = (secondes.max(0.0) * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let s = (total_ms / 1_000) % 60;
    let m = (total_ms / 60_000) % 60;
    let h = total_ms / 3_600_000;
    format!("{h:02}:{m:02}:{s:02}{separateur}{ms:03}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Segment;

    fn transcript(segments: Vec<Segment>) -> Transcript {
        Transcript {
            segments,
            ..Default::default()
        }
    }

    #[test]
    fn horodatage_au_format_attendu() {
        assert_eq!(timestamp(0.0, ','), "00:00:00,000");
        assert_eq!(timestamp(12.5, ','), "00:00:12,500");
        assert_eq!(timestamp(12.5, '.'), "00:00:12.500");
        assert_eq!(timestamp(3661.125, ','), "01:01:01,125");
        // Un horodatage négatif ne doit pas produire de valeur aberrante.
        assert_eq!(timestamp(-5.0, ','), "00:00:00,000");
    }

    #[test]
    fn decoupe_aux_frontieres_de_mots() {
        let lignes = wrap("le petit chat dort sur le tapis rouge", 15);
        assert!(lignes.iter().all(|l| l.chars().count() <= 15), "{lignes:?}");
        assert_eq!(lignes.join(" "), "le petit chat dort sur le tapis rouge");
    }

    #[test]
    fn coupe_un_mot_plus_long_que_la_ligne() {
        // Une URL, ou une langue sans espaces : laisser déborder produirait une
        // ligne illisible.
        let lignes = wrap("https://example.com/un/chemin/tres/tres/long", 12);
        assert!(lignes.iter().all(|l| l.chars().count() <= 12), "{lignes:?}");
        assert_eq!(
            lignes.concat(),
            "https://example.com/un/chemin/tres/tres/long"
        );
    }

    #[test]
    fn compte_en_caracteres_et_non_en_octets() {
        // « é » pèse deux octets : un comptage en octets couperait trop tôt.
        let lignes = wrap("éééééééééé", 10);
        assert_eq!(lignes, vec!["éééééééééé"]);
    }

    #[test]
    fn respecte_le_nombre_de_lignes_par_cue() {
        let long = "un deux trois quatre cinq six sept huit neuf dix onze douze";
        let t = transcript(vec![Segment::new(0, 0.0, 12.0, long)]);
        let opts = SubtitleOptions {
            max_line_width: 12,
            max_line_count: 2,
            ..Default::default()
        };
        let cues = build(&t, &opts);
        assert!(cues.len() > 1, "le segment aurait dû être scindé");
        assert!(cues.iter().all(|c| c.lines.len() <= 2), "{cues:?}");
    }

    #[test]
    fn numerotation_sequentielle_depuis_un() {
        let t = transcript(vec![
            Segment::new(0, 0.0, 2.0, "premier"),
            Segment::new(1, 2.0, 4.0, "second"),
        ]);
        let cues = build(&t, &SubtitleOptions::default());
        assert_eq!(cues.iter().map(|c| c.index).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn les_cues_ne_se_chevauchent_pas() {
        // Deux segments très brefs et collés : l'allongement du premier ne doit
        // pas empiéter sur le second.
        let t = transcript(vec![
            Segment::new(0, 0.0, 0.2, "bref"),
            Segment::new(1, 0.3, 0.5, "encore"),
            Segment::new(2, 0.6, 5.0, "enfin un segment plus long"),
        ]);
        let cues = build(&t, &SubtitleOptions::default());
        for paire in cues.windows(2) {
            assert!(
                paire[0].end <= paire[1].start + 1e-9,
                "chevauchement : {:?} puis {:?}",
                paire[0],
                paire[1]
            );
        }
    }

    #[test]
    fn plafonne_la_duree_d_affichage() {
        let t = transcript(vec![Segment::new(0, 0.0, 30.0, "court")]);
        let opts = SubtitleOptions {
            max_duration: 7.0,
            ..Default::default()
        };
        let cues = build(&t, &opts);
        assert!((cues[0].end - cues[0].start - 7.0).abs() < 1e-9);
    }

    #[test]
    fn allonge_un_cue_trop_bref_quand_la_place_existe() {
        let t = transcript(vec![Segment::new(0, 0.0, 0.2, "bref")]);
        let cues = build(&t, &SubtitleOptions::default());
        assert!((cues[0].end - cues[0].start - 1.0).abs() < 1e-9);
    }

    #[test]
    fn la_duree_totale_est_preservee() {
        let t = transcript(vec![Segment::new(
            0,
            10.0,
            16.0,
            "un texte assez long pour être réparti sur plusieurs cues successifs",
        )]);
        let opts = SubtitleOptions {
            max_line_width: 20,
            max_line_count: 1,
            min_duration: 0.0,
            max_duration: 60.0,
        };
        let cues = build(&t, &opts);
        assert!(cues.len() > 1);
        assert!((cues[0].start - 10.0).abs() < 1e-9);
        // Le dernier cue reprend la borne exacte : pas de dérive cumulée.
        assert!((cues.last().unwrap().end - 16.0).abs() < 1e-9);
    }

    #[test]
    fn ignore_les_segments_vides() {
        let t = transcript(vec![
            Segment::new(0, 0.0, 2.0, "   "),
            Segment::new(1, 2.0, 4.0, "utile"),
        ]);
        let cues = build(&t, &SubtitleOptions::default());
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].lines, vec!["utile"]);
    }
}
