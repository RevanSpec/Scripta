//! Tests d'intégration de l'orchestration — SPEC §2.1, §5.5.
//!
//! Les règles d'enchaînement de `pipeline::run`, que la CLI et l'application
//! de bureau partagent : ce qui court-circuite le reste, dans quel ordre les
//! étapes s'exécutent, ce qu'une annulation interrompt.
//!
//! Aucun accès réseau ni modèle : `yt-dlp` est simulé par
//! `scripta-fake-sidecar`, qui répond à la sonde par un JSON encodé dans son
//! nom, et chaque scénario s'arrête avant l'inférence — ou la rend inutile. Le
//! cache n'est consulté que par le test qui le détourne vers un répertoire
//! temporaire : les autres ne touchent jamais au cache de l'utilisateur.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use scripta_core::audio::Sidecars;
use scripta_core::pipeline::{
    self, EngineSlot, Event, ModelChoice, Observer, Origin, Request, VadChoice,
};
use scripta_core::{Access, CancelToken, ScriptaError, probe, url};

/// Répertoire des fixtures : voir `tests/pipeline.rs` pour le choix du
/// répertoire temporaire système.
fn fixtures() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("scripta-")
        .tempdir()
        .expect("répertoire de fixtures")
}

/// Installe le sidecar simulé sous un nom porteur de configuration — lien
/// physique plutôt que copie, pour la raison exposée dans `tests/pipeline.rs`.
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

/// `yt-dlp` simulé dont la sonde rend `json`.
fn ytdlp_repondant(dir: &Path, json: &str) -> PathBuf {
    sidecar(dir, &format!("ytdlp-txt{}", hex(json)))
}

const URL: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

/// Requête qui ne sollicite ni le cache ni un modèle réel : VAD désactivé,
/// modèle désigné par un chemin absent.
fn requete(ytdlp: PathBuf) -> Request {
    let mut r = Request::new(
        url::parse(URL).unwrap(),
        Sidecars::new(ytdlp, "ffmpeg-absent-du-test"),
    );
    r.model = ModelChoice::File("modele-absent-du-test.bin".into());
    r.vad = VadChoice::Off;
    r.use_cache = false;
    r
}

/// Noms des événements reçus, dans l'ordre.
#[derive(Clone, Default)]
struct Trace(Arc<Mutex<Vec<String>>>);

impl Trace {
    fn observateur(&self) -> Arc<dyn Observer> {
        let noms = Arc::clone(&self.0);
        Arc::new(move |e: Event<'_>| {
            let brut = format!("{e:?}");
            let nom = brut
                .split(|c: char| !c.is_alphanumeric())
                .next()
                .unwrap_or_default()
                .to_string();
            noms.lock().unwrap().push(nom);
        })
    }

    fn noms(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

fn executer(
    req: &Request,
    cancel: &CancelToken,
) -> (Result<pipeline::Outcome, ScriptaError>, Trace) {
    let trace = Trace::default();
    let issue = pipeline::run(req, &EngineSlot::new(), &trace.observateur(), cancel);
    (issue, trace)
}

// ------------------------------------------------------------------ ordre ---

#[test]
fn un_modele_absent_echoue_avant_toute_sonde() {
    // Sans `prefer_subs`, le modèle est résolu d'abord : un modèle absent doit
    // échouer tout de suite, pas après la sonde et l'extraction. yt-dlp est
    // lui-même absent : l'atteindre produirait une autre erreur (code 21).
    let (issue, trace) = executer(&requete("yt-dlp-absent".into()), &CancelToken::new());
    let e = issue.expect_err("modèle absent");
    assert_eq!(e.exit_code(), 30, "{e}");
    assert_eq!(trace.noms(), ["CacheKey", "Loading"]);
}

#[test]
fn avec_prefer_subs_la_sonde_precede_le_modele() {
    // Des sous-titres rendraient le modèle inutile : il n'est chargé qu'après
    // leur échec. Ici la vidéo n'en a aucun.
    let dir = fixtures();
    let mut req = requete(ytdlp_repondant(
        dir.path(),
        r#"{"id":"dQw4w9WgXcQ","title":"Essai","duration":2.0}"#,
    ));
    req.prefer_subs = true;

    let (issue, trace) = executer(&req, &CancelToken::new());
    assert_eq!(issue.expect_err("modèle absent").exit_code(), 30);
    assert_eq!(
        trace.noms(),
        [
            "CacheKey",
            "Probing",
            "Probed",
            "SubtitlesFallback",
            "Loading"
        ]
    );
}

#[test]
fn une_video_trop_longue_est_refusee_avant_l_extraction() {
    let dir = fixtures();
    let mut req = requete(ytdlp_repondant(
        dir.path(),
        r#"{"id":"dQw4w9WgXcQ","title":"Long","duration":20000}"#,
    ));
    req.prefer_subs = true;

    let (issue, trace) = executer(&req, &CancelToken::new());
    let e = issue.expect_err("vidéo trop longue");
    assert_eq!(e.exit_code(), 14, "{e}");
    assert_eq!(trace.noms(), ["CacheKey", "Probing"]);
}

#[test]
fn un_direct_est_refuse() {
    let dir = fixtures();
    let mut req = requete(ytdlp_repondant(
        dir.path(),
        r#"{"id":"dQw4w9WgXcQ","title":"Direct","is_live":true}"#,
    ));
    req.prefer_subs = true;

    let (issue, _) = executer(&req, &CancelToken::new());
    assert_eq!(issue.expect_err("direct").exit_code(), 13);
}

#[test]
fn la_traduction_avec_turbo_est_refusee_avant_tout_travail() {
    // Refusée avant de télécharger 570 Mo pour rien — et avant même de
    // consulter le cache.
    let mut req = requete("yt-dlp-absent".into());
    req.model = ModelChoice::Alias("turbo".into());
    req.translate = true;

    let (issue, trace) = executer(&req, &CancelToken::new());
    assert_eq!(issue.expect_err("turbo ne traduit pas").exit_code(), 40);
    assert!(trace.noms().is_empty(), "{:?}", trace.noms());
}

// ---------------------------------------------------------------- annulation -

#[test]
fn un_jeton_deja_arme_n_engage_aucun_travail() {
    let dir = fixtures();
    let mut req = requete(ytdlp_repondant(dir.path(), r#"{"id":"x"}"#));
    req.prefer_subs = true;
    let cancel = CancelToken::new();
    cancel.cancel();

    let (issue, trace) = executer(&req, &cancel);
    assert!(matches!(issue, Err(ScriptaError::Interrupted)), "{issue:?}");
    assert_eq!(trace.noms(), ["CacheKey"]);
}

#[test]
fn l_annulation_interrompt_une_sonde_figee() {
    // Réseau muet : yt-dlp ne répond plus. Le bouton « Annuler » de la GUI,
    // qui ne dispose pas de `Ctrl-C`, doit rendre la main quand même.
    let dir = fixtures();
    let mut req = requete(sidecar(dir.path(), "ytdlp-sl60000"));
    req.prefer_subs = true;

    let cancel = CancelToken::new();
    let armeur = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel.cancel();
        })
    };

    let debut = Instant::now();
    let (issue, trace) = executer(&req, &cancel);
    let ecoule = debut.elapsed();
    armeur.join().unwrap();

    assert!(matches!(issue, Err(ScriptaError::Interrupted)), "{issue:?}");
    assert!(ecoule < Duration::from_secs(3), "annulation en {ecoule:?}");
    assert_eq!(trace.noms(), ["CacheKey", "Probing"]);
}

// ------------------------------------------------------------------ sonde ---

#[test]
fn la_sonde_rend_les_metadonnees() {
    let dir = fixtures();
    let ytdlp = ytdlp_repondant(
        dir.path(),
        r#"{"id":"dQw4w9WgXcQ","title":"Essai","duration":2.0}"#,
    );
    let meta =
        probe::probe(&ytdlp, &url::parse(URL).unwrap(), &Access::default(), None).expect("sonde");
    assert_eq!(meta.title, "Essai");
    assert_eq!(meta.duration, Some(2.0));
}

#[test]
fn la_sonde_classe_l_echec_de_yt_dlp() {
    let dir = fixtures();
    let ytdlp = sidecar(
        dir.path(),
        &format!(
            "ytdlp-x1-msg{}",
            hex("ERROR: [youtube] x: Sign in to confirm you're not a bot")
        ),
    );
    let e = probe::probe(&ytdlp, &url::parse(URL).unwrap(), &Access::default(), None)
        .expect_err("sonde en échec");
    assert_eq!(e.exit_code(), 12, "{e}");
}

// ------------------------------------------------------------------ cache ---

/// Le seul test qui consulte le cache : il le détourne vers un répertoire
/// temporaire.
#[test]
fn une_transcription_en_cache_ne_sollicite_ni_modele_ni_reseau() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY : aucun autre test de ce binaire ne lit cette variable.
    unsafe { std::env::set_var("SCRIPTA_CACHE_DIR", dir.path()) };

    // Modèle et sidecars absents : les atteindre ferait échouer le test.
    let mut req = requete("yt-dlp-absent".into());
    req.use_cache = true;

    let mut doc = scripta_core::Document::default();
    doc.source.title = "Déjà transcrite".into();
    scripta_core::cache::put(&pipeline::cache_key(&req).unwrap(), &doc).expect("mise en cache");

    let (issue, trace) = executer(&req, &CancelToken::new());
    let resultat = issue.expect("resservie par le cache");
    assert_eq!(resultat.origin, Origin::Cache);
    assert_eq!(resultat.document.source.title, "Déjà transcrite");
    assert_eq!(trace.noms(), ["CacheKey", "CacheHit"]);

    // Un paramètre qui change le résultat ne doit pas resservir l'entrée.
    req.lang = Some("fr".into());
    let (issue, _) = executer(&req, &CancelToken::new());
    assert_eq!(issue.expect_err("pas en cache").exit_code(), 30);

    // SAFETY : idem.
    unsafe { std::env::remove_var("SCRIPTA_CACHE_DIR") };
}
