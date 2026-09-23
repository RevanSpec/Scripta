//! Tests d'intégration du moteur d'inférence — SPEC SF-04, §5.5.
//!
//! Ces tests nécessitent un modèle GGML et un échantillon audio, trop
//! volumineux pour le dépôt. Ils se **sautent proprement** quand
//! `SCRIPTA_TEST_MODEL` et `SCRIPTA_TEST_WAV` ne sont pas définis, afin que la
//! CI reste verte sans eux. Les tests du VAD demandent en outre le modèle
//! Silero, désigné par `SCRIPTA_TEST_VAD`.
//!
//! ```text
//! SCRIPTA_TEST_MODEL=/chemin/ggml-tiny.bin \
//! SCRIPTA_TEST_WAV=/chemin/jfk.wav \
//! SCRIPTA_TEST_VAD=/chemin/ggml-silero-v5.1.2.bin \
//! cargo test -p scripta-core --test inference -- --test-threads=1
//! ```
//!
//! `--test-threads=1` n'est pas cosmétique : chaque test charge le modèle et
//! lance une inférence multithread ; en parallèle ils se disputent le CPU et la
//! durée totale est multipliée par quarante.
//!
//! Conformément au §5.5, les assertions portent sur la présence de termes
//! attendus, jamais sur une égalité de chaîne : la sortie varie selon le
//! backend, la version de ggml et la quantification du modèle.

use std::path::PathBuf;

use std::sync::{Arc, Mutex};

use scripta_core::audio::{PcmDecoder, SAMPLE_RATE};
use scripta_core::transcribe::{CancelToken, Engine, Hooks, NewSegment, Options};

fn fixture(var: &str) -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os(var)?);
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("{var} pointe sur un fichier absent : {}", p.display());
        None
    }
}

/// Lecteur WAV minimal : PCM 16 bits mono, tel que produit par le pipeline
/// SF-02. Écrit à la main pour éviter une dépendance de test supplémentaire.
fn read_wav_16k_mono(path: &std::path::Path) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("lecture du WAV");
    assert!(
        bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "fichier non RIFF/WAVE"
    );

    // Parcours des chunks jusqu'à « data ».
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"data" {
            let end = (body + size).min(bytes.len());
            let mut out = Vec::new();
            PcmDecoder::new().push(&bytes[body..end], &mut out);
            return out;
        }
        // Les chunks sont alignés sur 2 octets.
        pos = body + size + (size % 2);
    }
    panic!("chunk « data » introuvable");
}

struct Fixtures {
    engine: Engine,
    samples: Vec<f32>,
}

fn load() -> Option<Fixtures> {
    let model = fixture("SCRIPTA_TEST_MODEL")?;
    let wav = fixture("SCRIPTA_TEST_WAV")?;
    let engine = Engine::load(&model).expect("chargement du modèle");
    Some(Fixtures {
        engine,
        samples: read_wav_16k_mono(&wav),
    })
}

macro_rules! fixtures_or_skip {
    () => {
        match load() {
            Some(f) => f,
            None => {
                eprintln!("ignoré : SCRIPTA_TEST_MODEL / SCRIPTA_TEST_WAV non définis");
                return;
            }
        }
    };
}

#[test]
fn transcrit_l_echantillon_de_reference() {
    let f = fixtures_or_skip!();

    let transcript = f
        .engine
        .transcribe(f.samples, &Options::default(), Hooks::default())
        .expect("inférence");

    assert!(!transcript.is_empty(), "aucun segment produit");
    assert_eq!(transcript.language.as_deref(), Some("en"));

    let texte = scripta_core::format::txt::render(&transcript).to_lowercase();
    eprintln!("transcription obtenue : {texte}");

    // Termes distinctifs de l'échantillon, robustes au modèle `tiny`.
    for attendu in ["fellow", "americans", "country"] {
        assert!(
            texte.contains(attendu),
            "terme « {attendu} » absent : {texte}"
        );
    }
}

#[test]
fn les_horodatages_sont_coherents() {
    let f = fixtures_or_skip!();
    // Relevée avant l'appel : `transcribe` consomme le tampon.
    let duree_audio = scripta_core::transcribe::duration_of(&f.samples);

    let transcript = f
        .engine
        .transcribe(f.samples, &Options::default(), Hooks::default())
        .expect("inférence");

    let mut precedent = 0.0f64;

    for seg in &transcript.segments {
        assert!(seg.start >= 0.0, "début négatif : {seg:?}");
        assert!(seg.end >= seg.start, "fin avant début : {seg:?}");
        assert!(
            seg.start >= precedent - 0.01,
            "segments non ordonnés : {seg:?}"
        );
        // Marge d'une seconde : whisper.cpp travaille par fenêtres de 30 s et
        // peut déborder légèrement de la durée réelle sur le dernier segment.
        assert!(
            seg.end <= duree_audio + 1.0,
            "horodatage {} au-delà de l'audio ({duree_audio:.1} s)",
            seg.end
        );
        precedent = seg.start;
    }
}

#[test]
fn la_progression_est_rapportee() {
    let f = fixtures_or_skip!();

    let vus = Arc::new(Mutex::new(Vec::<i32>::new()));
    let collecteur = Arc::clone(&vus);

    f.engine
        .transcribe(
            f.samples,
            &Options::default(),
            Hooks {
                on_progress: Some(Box::new(move |p| collecteur.lock().unwrap().push(p))),
                ..Default::default()
            },
        )
        .expect("inférence");

    let vus = vus.lock().unwrap();
    assert!(!vus.is_empty(), "aucune progression rapportée");
    assert!(
        vus.windows(2).all(|w| w[1] >= w[0]),
        "progression non monotone : {vus:?}"
    );
    assert!(
        vus.iter().all(|p| (0..=100).contains(p)),
        "progression hors bornes : {vus:?}"
    );
}

/// Un jeton **vivant mais non armé** ne doit pas interrompre l'inférence.
///
/// Ce cas n'était couvert par aucun test — tous passaient `cancel: None` ou un
/// jeton pré-annulé — et c'est précisément lui qui échouait : le binding
/// `set_abort_callback_safe` de whisper-rs 0.16.0 confond les types et faisait
/// renvoyer n'importe quoi au rappel, avortant l'encodage avec le code -6. Sans
/// ce test, le défaut ne se voyait que sur une exécution réelle.
#[test]
fn un_jeton_non_arme_n_interrompt_pas() {
    let f = fixtures_or_skip!();

    let transcript = f
        .engine
        .transcribe(
            f.samples,
            &Options::default(),
            Hooks {
                cancel: Some(CancelToken::new()), // vivant, jamais armé
                ..Default::default()
            },
        )
        .expect("un jeton non armé ne doit pas faire échouer l'inférence");

    assert!(
        !transcript.is_empty(),
        "transcription vide alors que l'annulation n'a pas été demandée"
    );
}

/// L'annulation doit produire `Interrupted`, jamais une transcription tronquée
/// présentée comme complète (SPEC SF-04).
#[test]
fn l_annulation_interrompt_l_inference() {
    let f = fixtures_or_skip!();

    let token = CancelToken::new();
    token.cancel(); // armé avant le démarrage : abandon déterministe

    match f.engine.transcribe(
        f.samples,
        &Options::default(),
        Hooks {
            cancel: Some(token),
            ..Default::default()
        },
    ) {
        Err(e) => assert_eq!(e.exit_code(), 130),
        Ok(t) => panic!(
            "une inférence annulée a rendu {} segments",
            t.segments.len()
        ),
    }
}

/// Les mots sont reconstruits depuis les tokens BPE de whisper — « bonjour »
/// peut arriver en « bon » + « jour ». Sans cette reconstruction, le champ
/// `words` du JSON resterait vide alors que le schéma le promet (SF-05).
#[test]
fn reconstruit_les_mots_depuis_les_tokens() {
    let f = fixtures_or_skip!();

    let transcript = f
        .engine
        .transcribe(
            f.samples,
            &Options {
                word_timestamps: true,
                ..Default::default()
            },
            Hooks::default(),
        )
        .expect("inférence");

    let segment = transcript.segments.first().expect("au moins un segment");
    assert!(!segment.words.is_empty(), "aucun mot reconstruit");

    for mot in &segment.words {
        assert!(!mot.word.is_empty(), "mot vide");
        assert!(
            !mot.word.starts_with(' '),
            "espace initiale non retirée : {:?}",
            mot.word
        );
        assert!(mot.end >= mot.start, "bornes inversées : {mot:?}");
        assert!(
            mot.start >= segment.start - 0.01 && mot.end <= segment.end + 0.01,
            "mot hors des bornes du segment : {mot:?}"
        );
        assert!(
            mot.probability.is_some_and(|p| (0.0..=1.0).contains(&p)),
            "probabilité hors bornes : {mot:?}"
        );
    }

    // Les mots recomposés doivent restituer le texte du segment.
    let recompose = segment
        .words
        .iter()
        .map(|w| w.word.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        recompose.to_lowercase().contains("fellow"),
        "recomposition incohérente : {recompose}"
    );
}

/// Sans `word_timestamps`, le champ reste vide : le coût de la reconstruction
/// n'est payé que sur demande.
#[test]
fn pas_de_mots_sans_horodatage_demande() {
    let f = fixtures_or_skip!();

    let transcript = f
        .engine
        .transcribe(f.samples, &Options::default(), Hooks::default())
        .expect("inférence");

    assert!(transcript.segments.iter().all(|s| s.words.is_empty()));
}

#[test]
fn un_buffer_vide_est_refuse() {
    let f = fixtures_or_skip!();

    match f
        .engine
        .transcribe(Vec::new(), &Options::default(), Hooks::default())
    {
        Err(e) => assert_eq!(e.exit_code(), 40),
        Ok(_) => panic!("un buffer vide ne devrait pas produire de transcription"),
    }
}

/// Les segments émis en cours d'inférence sont ceux de la transcription
/// finale, dans l'ordre : c'est ce qui permet à la GUI de les afficher au fil
/// de l'eau, et à la CLI d'en tirer position et vitesse.
#[test]
fn les_segments_sont_emis_au_fil_de_l_eau() {
    let f = fixtures_or_skip!();

    let emis = Arc::new(Mutex::new(Vec::<NewSegment>::new()));
    let collecteur = Arc::clone(&emis);

    let transcript = f
        .engine
        .transcribe(
            f.samples,
            &Options::default(),
            Hooks {
                on_segment: Some(Box::new(move |s| {
                    collecteur.lock().unwrap().push(s.clone())
                })),
                ..Default::default()
            },
        )
        .expect("inférence");

    let emis = emis.lock().unwrap();
    assert_eq!(emis.len(), transcript.segments.len(), "segments manquants");
    for (e, s) in emis.iter().zip(&transcript.segments) {
        assert_eq!(e.id, s.id);
        assert_eq!(e.text, s.text);
        assert!((e.start - s.start).abs() < 1e-9, "{e:?} / {s:?}");
        assert!((e.end - s.end).abs() < 1e-9, "{e:?} / {s:?}");
    }
}

// ------------------------------------------------------------------ VAD ------

fn modele_vad() -> Option<std::path::PathBuf> {
    let m = fixture("SCRIPTA_TEST_VAD");
    if m.is_none() {
        eprintln!("ignoré : SCRIPTA_TEST_VAD non défini");
    }
    m
}

fn silence(secondes: f64) -> Vec<f32> {
    vec![0.0; (secondes * SAMPLE_RATE as f64) as usize]
}

/// Test d'exactitude du VAD : la même phrase, précédée et séparée de longs
/// silences, doit retrouver ses **vraies** positions — segments et mots.
///
/// C'est précisément ce qu'échoue le VAD intégré de whisper.cpp 1.8.3 : il
/// replace les segments, pas les tokens, si bien que les mots de la seconde
/// occurrence y arriveraient plusieurs secondes trop tôt, hors de leur propre
/// segment.
#[test]
fn le_vad_conserve_la_chronologie_d_origine() {
    let f = fixtures_or_skip!();
    let Some(vad) = modele_vad() else { return };

    let duree_phrase = scripta_core::transcribe::duration_of(&f.samples);
    let (avant, entre) = (5.0, 7.0);
    let debut_second = avant + duree_phrase + entre;

    let mut audio = silence(avant);
    audio.extend_from_slice(&f.samples);
    audio.extend(silence(entre));
    audio.extend_from_slice(&f.samples);

    let transcript = f
        .engine
        .transcribe(
            audio,
            &Options {
                vad_model: Some(vad),
                word_timestamps: true,
                ..Default::default()
            },
            Hooks::default(),
        )
        .expect("inférence avec VAD");

    let premier = transcript.segments.first().expect("au moins un segment");
    assert!(
        premier.start >= avant - 0.5,
        "la parole commence à {avant} s, pas à {} s",
        premier.start
    );

    // Chaque mot reste dans les bornes de son segment : c'est ce que casse un
    // décalage des tokens.
    for seg in &transcript.segments {
        for mot in &seg.words {
            assert!(
                mot.start >= seg.start - 0.05 && mot.end <= seg.end + 0.05,
                "mot hors de son segment : {mot:?} dans [{}, {}]",
                seg.start,
                seg.end
            );
        }
    }

    // Les deux occurrences de « fellow » : la seconde doit tomber dans la
    // seconde phrase, décalée de la durée réelle qui les sépare.
    let fellow: Vec<f64> = transcript
        .segments
        .iter()
        .flat_map(|s| &s.words)
        .filter(|m| m.word.to_lowercase().contains("fellow"))
        .map(|m| m.start)
        .collect();
    assert_eq!(fellow.len(), 2, "occurrences de « fellow » : {fellow:?}");
    assert!(
        fellow[1] >= debut_second - 0.5,
        "seconde occurrence à {:.2} s, attendue après {debut_second:.2} s",
        fellow[1]
    );
    let ecart = fellow[1] - fellow[0];
    let attendu = duree_phrase + entre;
    assert!(
        (ecart - attendu).abs() < 1.0,
        "écart de {ecart:.2} s entre les occurrences, attendu {attendu:.2} s"
    );
}

/// Sans parole, la transcription est vide — ce n'est pas une panne.
#[test]
fn sans_parole_la_transcription_est_vide() {
    let f = fixtures_or_skip!();
    let Some(vad) = modele_vad() else { return };

    let transcript = f
        .engine
        .transcribe(
            silence(5.0),
            &Options {
                vad_model: Some(vad),
                ..Default::default()
            },
            Hooks::default(),
        )
        .expect("un silence n'est pas une erreur");
    assert!(transcript.is_empty(), "{:?}", transcript.segments);
}

/// Un chemin de modèle VAD erroné est un modèle indisponible (code 30), pas
/// une désactivation silencieuse du VAD.
#[test]
fn un_modele_vad_absent_est_signale() {
    let f = fixtures_or_skip!();

    match f.engine.transcribe(
        f.samples,
        &Options {
            vad_model: Some("vad-qui-n-existe-pas.bin".into()),
            ..Default::default()
        },
        Hooks::default(),
    ) {
        Err(e) => assert_eq!(e.exit_code(), 30),
        Ok(_) => panic!("le VAD a été ignoré en silence"),
    }
}
