//! Sous-titres YouTube officiels — SPEC SF-01.
//!
//! Quand une vidéo en possède, les récupérer est instantané là où Whisper
//! demanderait plusieurs dizaines de minutes. C'est le seul chemin du projet
//! qui ne passe pas par l'inférence.
//!
//! **Désactivé par défaut.** Les sous-titres auto-générés de YouTube sont
//! dépourvus de ponctuation dans de nombreuses langues et restent en deçà de
//! Whisper `small` : l'utilisateur doit les demander explicitement.

use crate::error::{Result, ScriptaError};
use crate::probe::Metadata;
use crate::transcript::{Segment, Transcript};

/// Une piste de sous-titres retenue pour téléchargement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub lang: String,
    /// Vrai pour une piste auto-générée, de qualité moindre.
    pub auto: bool,
    pub url: String,
}

/// Choisit la meilleure piste disponible.
///
/// # Choix de la langue
///
/// Prendre la première piste venue serait une faute : YouTube expose des
/// **traductions automatiques** dans une centaine de langues, et l'ordre
/// alphabétique fait remonter l'afar ou l'abkhaze. L'ordre de préférence est
/// donc :
///
/// 1. la langue demandée explicitement ;
/// 2. la langue déclarée de la vidéo ;
/// 3. l'unique piste disponible, s'il n'y en a qu'une ;
/// 4. sinon rien — l'appelant demandera une langue plutôt que de deviner.
///
/// Les sous-titres rédigés priment sur les auto-générés à langue égale :
/// ponctuation, casse et noms propres y sont corrects.
pub fn best_track(meta: &Metadata, lang: Option<&str>) -> Option<Track> {
    let demande = lang.or(meta.language.as_deref());

    choisir_dans(&meta.subtitles, demande, false)
        .or_else(|| choisir_dans(&meta.automatic_captions, demande, true))
}

fn choisir_dans(
    source: &std::collections::BTreeMap<String, Vec<crate::probe::SubtitleTrack>>,
    demande: Option<&str>,
    auto: bool,
) -> Option<Track> {
    let (code, pistes) = match demande {
        // `fr-FR` doit satisfaire une demande de `fr`.
        Some(l) => {
            let clef = source
                .keys()
                .find(|k| k.as_str() == l || k.split('-').next() == Some(l))?;
            (clef.clone(), source.get(clef)?)
        }
        // Sans langue connue, une piste unique ne laisse pas d'ambiguïté ;
        // au-delà, deviner reviendrait à tirer au sort.
        None if source.len() == 1 => source.iter().next().map(|(k, v)| (k.clone(), v))?,
        None => return None,
    };

    // WebVTT de préférence : c'est le format que sait lire `parse_vtt`.
    let piste = pistes
        .iter()
        .find(|p| p.ext.as_deref() == Some("vtt"))
        .or_else(|| pistes.first())?;

    Some(Track {
        lang: code,
        auto,
        url: piste.url.clone()?,
    })
}

/// Vrai si la piste est une traduction automatique et non la transcription
/// d'origine — sa qualité cumule alors deux passages machine.
pub fn is_translation(track: &Track, meta: &Metadata) -> bool {
    match meta.language.as_deref() {
        Some(origine) => track.lang.split('-').next() != Some(origine),
        None => false,
    }
}

/// Télécharge une piste et la convertit en transcription.
pub fn fetch(track: &Track) -> Result<Transcript> {
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(30)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(60)))
        .build()
        .new_agent();

    let corps = agent
        .get(&track.url)
        .call()
        .and_then(|r| r.into_body().read_to_string())
        .map_err(|e| ScriptaError::ExtractionFailed {
            detail: format!("récupération des sous-titres ({}) : {e}", track.lang),
        })?;

    let segments = parse_vtt(&corps)?;
    if segments.is_empty() {
        return Err(ScriptaError::ExtractionFailed {
            detail: "piste de sous-titres vide".to_string(),
        });
    }

    Ok(Transcript {
        segments,
        language: Some(
            track
                .lang
                .split('-')
                .next()
                .unwrap_or(&track.lang)
                .to_string(),
        ),
        language_probability: None,
        translated: false,
    })
}

/// Analyse un document WebVTT en segments.
///
/// Le format produit par YouTube pose deux difficultés absentes du WebVTT
/// ordinaire :
///
/// - des **balises de chronométrage intra-ligne** (`<00:00:12.500><c>mot</c>`),
///   qui doivent être retirées sans supprimer le texte qu'elles encadrent ;
/// - un **défilement** : chaque cue répète les lignes de la précédente pour
///   simuler un texte qui monte. Sans déduplication, la transcription
///   contiendrait chaque phrase deux ou trois fois.
pub fn parse_vtt(source: &str) -> Result<Vec<Segment>> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut lignes_emises: Vec<String> = Vec::new();

    let lignes: Vec<&str> = source.lines().collect();
    let mut i = 0;

    while i < lignes.len() {
        let ligne = lignes[i];
        i += 1;

        let Some((debut, fin)) = parse_cue_timing(ligne) else {
            continue;
        };

        // Le corps du cue court jusqu'à la prochaine ligne **réellement** vide.
        //
        // Le `trim()` serait ici une faute : YouTube fait précéder le texte
        // d'une ligne ne contenant qu'une espace. La traiter comme un
        // séparateur vidait le cue de son contenu, qui était alors récupéré
        // par le cue de transition suivant — long de dix millisecondes.
        let mut contenu: Vec<String> = Vec::new();
        while i < lignes.len() && !lignes[i].is_empty() {
            let texte = strip_tags(lignes[i]);
            let texte = texte.trim();
            if !texte.is_empty() {
                contenu.push(texte.to_string());
            }
            i += 1;
        }

        // Déduplication du défilement : on ne garde que ce qui n'a pas déjà
        // été émis par les cues précédents.
        let nouvelles: Vec<String> = contenu
            .into_iter()
            .filter(|l| !lignes_emises.contains(l))
            .collect();

        if nouvelles.is_empty() {
            continue;
        }

        lignes_emises.extend(nouvelles.iter().cloned());
        // Fenêtre glissante : comparer à tout l'historique coûterait un temps
        // quadratique sur une heure de sous-titres, et le défilement ne
        // recouvre jamais plus de quelques lignes.
        if lignes_emises.len() > 8 {
            let exces = lignes_emises.len() - 8;
            lignes_emises.drain(..exces);
        }

        segments.push(Segment::new(
            segments.len(),
            debut,
            fin,
            nouvelles.join(" "),
        ));
    }

    Ok(segments)
}

/// Analyse `00:00:12.500 --> 00:00:15.000 align:start position:0%`.
fn parse_cue_timing(ligne: &str) -> Option<(f64, f64)> {
    let (gauche, reste) = ligne.split_once("-->")?;
    let debut = parse_timestamp(gauche.trim())?;
    // Les réglages de placement suivent la borne de fin, séparés par une espace.
    let fin = parse_timestamp(reste.split_whitespace().next()?)?;
    Some((debut, fin))
}

/// `HH:MM:SS.mmm` ou `MM:SS.mmm`, la virgule décimale du SRT étant tolérée.
fn parse_timestamp(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, reste) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, *s),
        [m, s] => (0.0, m.parse::<f64>().ok()?, *s),
        _ => return None,
    };
    let secondes = reste.parse::<f64>().ok()?;
    Some(h * 3600.0 + m * 60.0 + secondes)
}

/// Retire les balises `<...>` en conservant le texte encadré.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dans_balise = false;
    for c in s.chars() {
        match c {
            '<' => dans_balise = true,
            '>' => dans_balise = false,
            _ if !dans_balise => out.push(c),
            _ => {}
        }
    }
    // WebVTT échappe les entités XML ; le texte restitué doit être lisible.
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::parse_metadata;

    #[test]
    fn analyse_les_horodatages() {
        assert_eq!(parse_timestamp("00:00:12.500"), Some(12.5));
        assert_eq!(parse_timestamp("01:01:01.125"), Some(3661.125));
        assert_eq!(parse_timestamp("02:30.000"), Some(150.0));
        // Virgule décimale du SRT.
        assert_eq!(parse_timestamp("00:00:12,500"), Some(12.5));
        assert_eq!(parse_timestamp("pas un horodatage"), None);
    }

    #[test]
    fn ignore_les_reglages_de_placement() {
        let t = parse_cue_timing("00:00:12.500 --> 00:00:15.000 align:start position:0%");
        assert_eq!(t, Some((12.5, 15.0)));
    }

    #[test]
    fn retire_les_balises_sans_perdre_le_texte() {
        assert_eq!(
            strip_tags("bonjour<00:00:00.539><c> à</c><00:00:00.840><c> tous</c>"),
            "bonjour à tous"
        );
        assert_eq!(strip_tags("R&amp;D et a&lt;b"), "R&D et a<b");
    }

    #[test]
    fn analyse_un_vtt_simple() {
        let vtt = "WEBVTT\n\n\
                   00:00:12.500 --> 00:00:15.000\n\
                   Bonjour à tous\n\n\
                   00:00:15.200 --> 00:00:18.400\n\
                   Deuxième réplique\n";
        let segs = parse_vtt(vtt).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "Bonjour à tous");
        assert_eq!(segs[0].start, 12.5);
        assert_eq!(segs[1].text, "Deuxième réplique");
        assert_eq!(segs[1].id, 1);
    }

    /// Cas réel des sous-titres auto-générés : chaque cue répète la ligne
    /// précédente pour simuler un texte défilant. Sans déduplication, chaque
    /// phrase apparaîtrait deux ou trois fois.
    #[test]
    fn deduplique_le_defilement_des_sous_titres_auto() {
        let vtt = "WEBVTT\n\n\
            00:00:00.030 --> 00:00:02.040 align:start position:0%\n\
            bonjour<00:00:00.539><c> à</c><00:00:00.840><c> tous</c>\n\n\
            00:00:02.040 --> 00:00:02.050 align:start position:0%\n\
            bonjour à tous\n\n\
            00:00:02.050 --> 00:00:04.560 align:start position:0%\n\
            bonjour à tous\n\
            et bienvenue<00:00:03.120><c> ici</c>\n\n\
            00:00:04.560 --> 00:00:06.000 align:start position:0%\n\
            et bienvenue ici\n";

        let segs = parse_vtt(vtt).unwrap();
        let texte = segs
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        assert_eq!(texte, "bonjour à tous et bienvenue ici");
        assert_eq!(segs.len(), 2, "cues redondants non éliminés : {segs:?}");
    }

    /// Reproduit à l'octet près l'en-tête d'un fichier réel de YouTube : le
    /// corps de chaque cue commence par une ligne ne contenant qu'une espace.
    /// Traitée comme un séparateur, elle vidait le cue, dont le texte était
    /// alors porté par le cue de transition suivant — dix millisecondes
    /// d'affichage au lieu de trois secondes.
    #[test]
    fn la_ligne_d_espace_de_youtube_ne_termine_pas_le_cue() {
        // `concat!` et non des continuations `\` : celles-ci suppriment
        // l'indentation de la ligne suivante, et la ligne d'espace — l'objet
        // même du test — se transformerait en ligne vide.
        let vtt = concat!(
            "WEBVTT\nKind: captions\nLanguage: fr\n\n",
            "00:00:00.120 --> 00:00:03.230 align:start position:0%\n",
            " \n",
            "Cette<00:00:00.399><c> vidéo</c><00:00:02.360><c> est</c>\n\n",
            "00:00:03.230 --> 00:00:03.240 align:start position:0%\n",
            "Cette vidéo est\n",
            " \n"
        );

        let segs = parse_vtt(vtt).unwrap();
        assert_eq!(segs.len(), 1, "cue de transition non dédupliqué : {segs:?}");
        assert_eq!(segs[0].text, "Cette vidéo est");
        assert_eq!(segs[0].start, 0.120, "texte reporté sur le mauvais cue");
        assert!(
            segs[0].end - segs[0].start > 3.0,
            "durée d'affichage aberrante : {:?}",
            segs[0]
        );
    }

    #[test]
    fn ignore_l_en_tete_et_les_blocs_note() {
        let vtt = "WEBVTT\n\
                   Kind: captions\n\
                   Language: fr\n\n\
                   NOTE ceci est un commentaire\n\n\
                   00:00:01.000 --> 00:00:02.000\n\
                   utile\n";
        let segs = parse_vtt(vtt).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "utile");
    }

    #[test]
    fn document_vide_ou_sans_cue() {
        assert!(parse_vtt("").unwrap().is_empty());
        assert!(parse_vtt("WEBVTT\n\nNOTE rien ici\n").unwrap().is_empty());
    }

    fn meta(json: &str) -> Metadata {
        parse_metadata(json.as_bytes()).unwrap()
    }

    #[test]
    fn prefere_les_sous_titres_rediges_aux_auto_generes() {
        let m = meta(
            r#"{"id":"a",
                "subtitles":{"fr":[{"ext":"vtt","url":"https://manuel"}]},
                "automatic_captions":{"fr":[{"ext":"vtt","url":"https://auto"}]}}"#,
        );
        let t = best_track(&m, Some("fr")).unwrap();
        assert!(!t.auto, "une piste auto a été préférée à une piste rédigée");
        assert_eq!(t.url, "https://manuel");
    }

    #[test]
    fn se_rabat_sur_les_auto_generes() {
        let m =
            meta(r#"{"id":"a","automatic_captions":{"en":[{"ext":"vtt","url":"https://auto"}]}}"#);
        let t = best_track(&m, None).unwrap();
        assert!(t.auto);
        assert_eq!(t.lang, "en");
    }

    /// YouTube expose des traductions automatiques dans une centaine de
    /// langues. Retenir la première par ordre alphabétique livrait de l'afar
    /// sur une vidéo française, avec un code de sortie 0.
    #[test]
    fn suit_la_langue_declaree_et_non_l_ordre_alphabetique() {
        let m = meta(
            r#"{"id":"a","language":"fr","automatic_captions":{
                "aa":[{"ext":"vtt","url":"https://afar"}],
                "ab":[{"ext":"vtt","url":"https://abkhaze"}],
                "fr":[{"ext":"vtt","url":"https://francais"}],
                "en":[{"ext":"vtt","url":"https://anglais"}]}}"#,
        );
        let t = best_track(&m, None).unwrap();
        assert_eq!(t.lang, "fr", "traduction automatique retenue à tort");
        assert_eq!(t.url, "https://francais");
    }

    /// Sans langue déclarée ni demandée, deviner parmi plusieurs pistes
    /// reviendrait à tirer au sort : « Me at the zoo » rendait de l'allemand
    /// parce que `de` précède `en`.
    #[test]
    fn refuse_de_deviner_entre_plusieurs_pistes() {
        let m = meta(
            r#"{"id":"a","subtitles":{
                "de":[{"ext":"vtt","url":"https://de"}],
                "en":[{"ext":"vtt","url":"https://en"}]}}"#,
        );
        assert!(best_track(&m, None).is_none());
        // Une demande explicite lève l'ambiguïté.
        assert_eq!(best_track(&m, Some("en")).unwrap().url, "https://en");
    }

    #[test]
    fn une_piste_unique_ne_laisse_pas_d_ambiguite() {
        let m = meta(r#"{"id":"a","subtitles":{"en":[{"ext":"vtt","url":"https://en"}]}}"#);
        assert_eq!(best_track(&m, None).unwrap().lang, "en");
    }

    #[test]
    fn signale_une_traduction_automatique() {
        let m = meta(
            r#"{"id":"a","language":"fr","automatic_captions":{
                "fr":[{"ext":"vtt","url":"https://fr"}],
                "en":[{"ext":"vtt","url":"https://en"}]}}"#,
        );
        let origine = best_track(&m, Some("fr")).unwrap();
        assert!(!is_translation(&origine, &m));

        let traduite = best_track(&m, Some("en")).unwrap();
        assert!(is_translation(&traduite, &m), "traduction non signalée");
    }

    #[test]
    fn une_demande_de_fr_accepte_fr_fr() {
        // YouTube expose parfois `fr-FR` là où l'utilisateur demande `fr`.
        let m = meta(r#"{"id":"a","subtitles":{"fr-FR":[{"ext":"vtt","url":"https://x"}]}}"#);
        assert!(best_track(&m, Some("fr")).is_some());
        assert!(best_track(&m, Some("de")).is_none());
    }

    #[test]
    fn prefere_le_format_vtt() {
        let m = meta(
            r#"{"id":"a","subtitles":{"fr":[
                {"ext":"json3","url":"https://json"},
                {"ext":"vtt","url":"https://vtt"}]}}"#,
        );
        assert_eq!(best_track(&m, Some("fr")).unwrap().url, "https://vtt");
    }

    #[test]
    fn aucune_piste_disponible() {
        let m = meta(r#"{"id":"a"}"#);
        assert!(best_track(&m, None).is_none());
    }
}
