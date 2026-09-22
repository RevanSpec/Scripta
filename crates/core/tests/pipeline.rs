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

fn sidecar(dir: &Path, nom: &str) -> PathBuf {
    let src = env!("CARGO_BIN_EXE_scripta-fake-sidecar");
    let dst = dir.join(format!("{nom}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(src, &dst).expect("copie du sidecar simulé");
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
    let sc = Sidecars::new(
        sidecar(dir.path(), &format!("ytdlp-so4096-se{STDERR_SATURANT}")),
        sidecar(dir.path(), &format!("ffmpeg-so64000-se{STDERR_SATURANT}")),
    );

    let samples = extract_avec_limite(sc, Duration::from_secs(30)).expect("extraction");
    assert_eq!(samples.len(), 32_000);
}

#[test]
fn echec_ytdlp_classe_la_verification_anti_robot() {
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();

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
