//! Tests d'intégration du pipeline d'extraction — SPEC SF-02, §5.5.
//!
//! Aucun accès réseau, aucun binaire externe : les sidecars sont simulés par
//! `scripta-fake-sidecar`, dont le comportement est encodé dans le nom de
//! fichier. Chaque test dispose de son propre répertoire temporaire, ce qui
//! garantit l'isolation sous exécution parallèle.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use scripta_core::audio::{self, Sidecars};
use scripta_core::url;
use scripta_core::{Access, ScriptaError};

/// Au-delà de la taille d'un tampon de pipe (≈64 Kio) : de quoi bloquer à coup
/// sûr un sidecar dont le `stderr` ne serait pas drainé.
const STDERR_SATURANT: usize = 1024 * 1024;

/// Répertoire temporaire des fixtures.
///
/// Le répertoire temporaire **système**, et non un sous-dossier de `target/` :
/// le chemin du dépôt est déjà long, les noms de fixtures encodent un message
/// en hexadécimal, et l'ensemble dépassait `MAX_PATH` sous Windows.
fn fixtures() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("scripta-")
        .tempdir()
        .expect("répertoire de fixtures")
}

/// Installe une copie du sidecar simulé sous un nom porteur de configuration.
///
/// **Lien physique et non copie.** Copier un exécutable puis l'exécuter
/// aussitôt expose à `ETXTBSY` sous Linux : `execve` refuse un fichier qu'un
/// autre thread tient ouvert en écriture, et les tests s'exécutent en
/// parallèle dans un même processus. Un lien physique ne crée qu'un nom
/// supplémentaire pour un inode que personne n'écrit — la course disparaît.
///
/// La copie reste en repli si les liens échouent, par exemple lorsque le
/// répertoire temporaire est monté sur un autre système de fichiers.
fn sidecar(dir: &Path, nom: &str) -> PathBuf {
    let src = env!("CARGO_BIN_EXE_scripta-fake-sidecar");
    let dst = dir.join(format!("{nom}{}", std::env::consts::EXE_SUFFIX));

    if std::fs::hard_link(src, &dst).is_err() {
        std::fs::copy(src, &dst).expect("copie du sidecar simulé");
    }
    dst
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn url_test() -> scripta_core::CanonicalUrl {
    url::parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap()
}

/// Exécute `extract` sous contrainte de temps. Un dépassement signale un
/// interblocage, jamais une lenteur : les sidecars simulés sont instantanés.
fn extract_avec_limite(sidecars: Sidecars, limite: Duration) -> Result<Vec<f32>, ScriptaError> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(audio::extract(
            &sidecars,
            &url_test(),
            Some(2.0),
            &Access::default(),
        ));
    });
    match rx.recv_timeout(limite) {
        Ok(res) => res,
        Err(_) => panic!(
            "INTERBLOCAGE : extract() n'a pas rendu la main en {limite:?}. \
             Cause attendue : un stderr de sidecar non drainé (SPEC SF-02)."
        ),
    }
}

#[test]
fn pipeline_nominal_produit_les_echantillons_attendus() {
    let dir = fixtures();
    // 64 000 octets s16le = 32 000 échantillons = 2 s à 16 kHz.
    let sc = Sidecars::new(
        sidecar(dir.path(), "ytdlp-so4096"),
        sidecar(dir.path(), "ffmpeg-so64000"),
    );

    let samples =
        audio::extract(&sc, &url_test(), Some(2.0), &Access::default()).expect("extraction");

    assert_eq!(samples.len(), 32_000);
    // Motif déterministe du sidecar : compteur 16 bits little-endian.
    assert_eq!(samples[0], 0.0);
    assert!((samples[1] - 1.0 / 32768.0).abs() < 1e-9);
    assert!((samples[31_999] - 31_999.0 / 32768.0).abs() < 1e-6);
}

/// Test de non-régression de l'interblocage `stderr` — critère de sortie du
/// Jalon 1. Les deux sidecars déversent 1 Mio sur `stderr` **avant** d'écrire
/// quoi que ce soit sur `stdout`. Sans drainage concurrent, le pipeline se fige
/// définitivement.
#[test]
fn stderr_saturant_ne_provoque_pas_d_interblocage() {
    let dir = fixtures();
    let sc = Sidecars::new(
        sidecar(dir.path(), &format!("ytdlp-so4096-se{STDERR_SATURANT}")),
        sidecar(dir.path(), &format!("ffmpeg-so64000-se{STDERR_SATURANT}")),
    );

    let samples = extract_avec_limite(sc, Duration::from_secs(30)).expect("extraction");
    assert_eq!(samples.len(), 32_000);
}

#[test]
fn echec_ytdlp_classe_la_verification_anti_robot() {
    let dir = fixtures();
    let msg = hex("ERROR: Sign in to confirm you're not a bot");
    let sc = Sidecars::new(
        sidecar(dir.path(), &format!("ytdlp-x1-msg{msg}")),
        sidecar(dir.path(), "ffmpeg-so0"),
    );

    match audio::extract(&sc, &url_test(), None, &Access::default()) {
        Err(e @ ScriptaError::AuthRequired { .. }) => assert_eq!(e.exit_code(), 12),
        other => panic!("attendu AuthRequired, obtenu {other:?}"),
    }
}

#[test]
fn echec_ytdlp_classe_l_indisponibilite() {
    let dir = fixtures();
    let msg = hex("ERROR: Private video. Sign in if you've been granted access");
    let sc = Sidecars::new(
        sidecar(dir.path(), &format!("ytdlp-x1-msg{msg}")),
        sidecar(dir.path(), "ffmpeg-so0"),
    );

    match audio::extract(&sc, &url_test(), None, &Access::default()) {
        Err(e @ ScriptaError::Unavailable { .. }) => assert_eq!(e.exit_code(), 11),
        other => panic!("attendu Unavailable, obtenu {other:?}"),
    }
}

/// Un motif inconnu doit se dégrader en `ExtractionFailed` en exposant le
/// `stderr` brut, jamais en classification erronée.
#[test]
fn echec_ytdlp_non_reconnu_expose_le_stderr_brut() {
    let dir = fixtures();
    let msg = hex("ERROR: un message totalement inedit de yt-dlp");
    let sc = Sidecars::new(
        sidecar(dir.path(), &format!("ytdlp-x1-msg{msg}")),
        sidecar(dir.path(), "ffmpeg-so0"),
    );

    match audio::extract(&sc, &url_test(), None, &Access::default()) {
        Err(ScriptaError::ExtractionFailed { detail }) => {
            assert!(
                detail.contains("totalement inedit"),
                "stderr brut absent du diagnostic : {detail}"
            );
        }
        other => panic!("attendu ExtractionFailed, obtenu {other:?}"),
    }
}

#[test]
fn sidecar_absent_est_signale_explicitement() {
    let dir = fixtures();
    let sc = Sidecars::new(
        dir.path().join("binaire-qui-n-existe-pas"),
        sidecar(dir.path(), "ffmpeg-so0"),
    );

    match audio::extract(&sc, &url_test(), None, &Access::default()) {
        Err(e @ ScriptaError::SidecarMissing { .. }) => assert_eq!(e.exit_code(), 21),
        other => panic!("attendu SidecarMissing, obtenu {other:?}"),
    }
}

#[test]
fn flux_audio_vide_est_une_erreur() {
    let dir = fixtures();
    let sc = Sidecars::new(
        sidecar(dir.path(), "ytdlp-so4096"),
        sidecar(dir.path(), "ffmpeg-so0"),
    );

    assert!(matches!(
        audio::extract(&sc, &url_test(), None, &Access::default()),
        Err(ScriptaError::ExtractionFailed { .. })
    ));
}

#[test]
fn echec_ffmpeg_est_remonte() {
    let dir = fixtures();
    let msg = hex("Invalid data found when processing input");
    let sc = Sidecars::new(
        sidecar(dir.path(), "ytdlp-so4096"),
        sidecar(dir.path(), &format!("ffmpeg-x1-msg{msg}")),
    );

    match audio::extract(&sc, &url_test(), None, &Access::default()) {
        Err(ScriptaError::ExtractionFailed { detail }) => {
            assert!(detail.contains("Invalid data"), "diagnostic : {detail}");
        }
        other => panic!("attendu ExtractionFailed, obtenu {other:?}"),
    }
}

/// Invariant « zero-disk » (SPEC §5.2) : le pipeline ne doit produire aucun
/// fichier intermédiaire.
#[test]
fn aucun_fichier_temporaire_n_est_cree() {
    let dir = fixtures();
    let travail = tempfile::tempdir().unwrap();
    let sc = Sidecars::new(
        sidecar(dir.path(), "ytdlp-so8192"),
        sidecar(dir.path(), "ffmpeg-so64000"),
    );

    let avant = std::fs::read_dir(travail.path()).unwrap().count();
    audio::extract(&sc, &url_test(), Some(2.0), &Access::default()).expect("extraction");
    let apres = std::fs::read_dir(travail.path()).unwrap().count();

    assert_eq!(
        avant, apres,
        "des fichiers ont été créés pendant l'extraction"
    );
}

/// Le tampon audio ne doit pas être réalloué quand le flux dépasse légèrement
/// la durée annoncée — ce qui est le cas systématique, `yt-dlp` arrondissant à
/// la seconde inférieure.
///
/// Une réallocation double la capacité : sur une heure d'audio, 223 Mo
/// deviennent 446 Mo, et l'ancien tampon coexiste avec le nouveau le temps de
/// la copie. C'est une vérification directe de l'allocation, là où la mémoire
/// résidente du processus est un instrument trop grossier.
#[test]
fn le_tampon_audio_n_est_pas_realloue() {
    let dir = fixtures();

    // 1,0 s annoncée ; le flux en livre 1,002 — même écart relatif que celui
    // mesuré en conditions réelles (3 664 s décodées pour 3 657 annoncées).
    let duree_annoncee = 1.0_f64;
    let octets = 16_032 * 2; // 16 032 échantillons s16le
    let sc = Sidecars::new(
        sidecar(dir.path(), "ytdlp-so4096"),
        sidecar(dir.path(), &format!("ffmpeg-so{octets}")),
    );

    let samples = audio::extract(&sc, &url_test(), Some(duree_annoncee), &Access::default())
        .expect("extraction");

    assert_eq!(samples.len(), 16_032, "flux tronqué");
    assert!(
        samples.len() > (duree_annoncee * 16_000.0) as usize,
        "le test doit bien dépasser la durée annoncée"
    );
    assert_eq!(
        samples.capacity(),
        audio::preallocation_len(Some(duree_annoncee)),
        "le tampon a été réalloué : la marge de pré-allocation ne joue plus"
    );
}

// ---------------------------------------------------------------- cache -----

/// Aller-retour complet du cache de transcriptions — SPEC SF-08.
///
/// Le cache stocke le JSON enrichi, dont tous les autres formats se dérivent :
/// une perte d'information au passage serait invisible en `txt` mais ruinerait
/// le `json` et les sous-titres.
#[test]
fn le_cache_restitue_le_document_a_l_identique() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY : variable propre à ce test, lue une seule fois par cache_dir.
    let garde = EnvGuard::set("SCRIPTA_CACHE_DIR", dir.path());

    let clef = scripta_core::cache::Key {
        video_id: "dQw4w9WgXcQ".into(),
        model: "ggml-base".into(),
        lang: Some("fr".into()),
        translate: false,
        vad: true,
        word_timestamps: true,
    };

    assert!(scripta_core::cache::get(&clef).is_none(), "cache non vide");

    let doc = scripta_core::Document {
        source: scripta_core::Source {
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
            video_id: "dQw4w9WgXcQ".into(),
            title: "Un titre avec des accents é à ü".into(),
            channel: Some("Chaîne".into()),
            duration_s: Some(3657.0),
            upload_date: None,
        },
        run: scripta_core::Run {
            model: "ggml-base".into(),
            backend: "cpu".into(),
            language: Some("fr".into()),
            speed_realtime: 4.8,
            vad: true,
            ..Default::default()
        },
        transcript: scripta_core::Transcript {
            segments: vec![scripta_core::Segment {
                id: 0,
                start: 12.5,
                end: 15.0,
                text: "Bonjour à tous.".into(),
                no_speech_prob: Some(0.01),
                avg_logprob: None,
                words: vec![scripta_core::Word {
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
    };

    scripta_core::cache::put(&clef, &doc).expect("écriture");

    let relu = scripta_core::cache::get(&clef).expect("entrée absente après écriture");
    assert_eq!(relu.source.title, doc.source.title);
    assert_eq!(relu.source.channel, doc.source.channel);
    assert_eq!(relu.run.model, doc.run.model);
    assert_eq!(relu.run.engine, "whisper.cpp");
    assert!((relu.run.speed_realtime - 4.8).abs() < 1e-9);
    assert_eq!(relu.transcript.segments.len(), 1);
    assert_eq!(relu.transcript.segments[0].text, "Bonjour à tous.");
    // Les mots sont le premier poste qu'une sérialisation bâclée perdrait.
    assert_eq!(relu.transcript.segments[0].words.len(), 1);
    assert_eq!(relu.transcript.segments[0].words[0].word, "Bonjour");

    // Un paramètre différent ne doit jamais resservir cette entrée.
    let autre = scripta_core::cache::Key {
        translate: true,
        ..clef.clone()
    };
    assert!(
        scripta_core::cache::get(&autre).is_none(),
        "entrée resservie pour des paramètres différents"
    );

    assert_eq!(scripta_core::cache::list().unwrap().len(), 1);
    assert_eq!(scripta_core::cache::clear().unwrap(), 1);
    assert!(scripta_core::cache::get(&clef).is_none());

    drop(garde);
}

/// Garde-fou pour les variables d'environnement : les tests d'intégration
/// s'exécutent en parallèle dans le même processus, et une variable laissée
/// en place contaminerait les suivants.
struct EnvGuard(&'static str);

impl EnvGuard {
    fn set(nom: &'static str, valeur: &std::path::Path) -> Self {
        // SAFETY : aucun autre thread ne lit cette variable pendant le test.
        unsafe { std::env::set_var(nom, valeur) };
        Self(nom)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY : idem.
        unsafe { std::env::remove_var(self.0) };
    }
}
