//! Tests d'intégration du moteur d'inférence — SPEC SF-04, §5.5.
//!
//! Ces tests nécessitent un modèle GGML et un échantillon audio, trop
//! volumineux pour le dépôt. Ils se **sautent proprement** quand
//! `SCRIPTA_TEST_MODEL` et `SCRIPTA_TEST_WAV` ne sont pas définis, afin que la
//! CI reste verte sans eux.
//!
//! ```text
//! SCRIPTA_TEST_MODEL=/chemin/ggml-tiny.bin \
//! SCRIPTA_TEST_WAV=/chemin/jfk.wav \
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

use scripta_core::audio::PcmDecoder;
use scripta_core::transcribe::{CancelToken, Engine, Hooks, Options};

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
        .transcribe(&f.samples, &Options::default(), Hooks::default())
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

    let transcript = f
        .engine
        .transcribe(&f.samples, &Options::default(), Hooks::default())
        .expect("inférence");

    let duree_audio = scripta_core::transcribe::duration_of(&f.samples);
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

    let vus = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i32>::new()));
    let collecteur = std::sync::Arc::clone(&vus);

    f.engine
        .transcribe(
            &f.samples,
            &Options::default(),
            Hooks {
                on_progress: Some(Box::new(move |p| collecteur.lock().unwrap().push(p))),
                cancel: None,
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
            &f.samples,
            &Options::default(),
            Hooks {
                on_progress: None,
                cancel: Some(CancelToken::new()), // vivant, jamais armé
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
        &f.samples,
        &Options::default(),
        Hooks {
            on_progress: None,
            cancel: Some(token),
        },
    ) {
        Err(e) => assert_eq!(e.exit_code(), 130),
        Ok(t) => panic!(
            "une inférence annulée a rendu {} segments",
            t.segments.len()
        ),
    }
}

#[test]
fn un_buffer_vide_est_refuse() {
    let f = fixtures_or_skip!();

    match f
        .engine
        .transcribe(&[], &Options::default(), Hooks::default())
    {
        Err(e) => assert_eq!(e.exit_code(), 40),
        Ok(_) => panic!("un buffer vide ne devrait pas produire de transcription"),
    }
}
