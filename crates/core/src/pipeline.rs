//! Orchestration d'une transcription — SPEC §2.1, flux nominal.
//!
//! Cache, modèle, sonde, sous-titres, extraction audio, inférence, mise en
//! cache : l'enchaînement et ses règles vivent ici, et nulle part ailleurs. Ce
//! qui entre dans la clé de cache, l'ordre imposé par `prefer_subs`, le repli
//! silencieux sur la transcription… Deux interfaces qui en tiendraient chacune
//! leur version finiraient par diverger, et le premier écart probable — un
//! paramètre oublié dans la clé de cache — resservirait une transcription
//! obtenue dans d'autres conditions : un défaut silencieux, le pire qui soit
//! (SF-08).
//!
//! La CLI et l'application de bureau n'en sont donc que deux présentations
//! (SPEC §2.2). Chacune reçoit les [`Event`] du déroulement et les rend à sa
//! manière : lignes sur `stderr`, ou messages vers l'interface.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::{self, Sidecars};
use crate::cache;
use crate::cancel::CancelToken;
use crate::document::{Document, Run, Source};
use crate::error::{Result, ScriptaError};
use crate::models::{self, ModelSpec};
use crate::probe::{self, Access, Metadata};
use crate::subtitles::{self, Track};
use crate::transcribe::{self, Backend, Engine, Hooks, NewSegment, Options};
use crate::transcript::Transcript;
use crate::url::CanonicalUrl;

/// Durée maximale d'une vidéo, en minutes — `--max-duration`.
pub const DEFAULT_MAX_DURATION_MIN: u64 = 240;

/// Modèle de transcription demandé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelChoice {
    /// Alias du catalogue — `auto`, `base`, `turbo`… —, téléchargé à la
    /// demande et vérifié par empreinte.
    Alias(String),
    /// Fichier explicite, hors catalogue.
    File(PathBuf),
}

/// Détection d'activité vocale — SPEC SF-04.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VadChoice {
    Off,
    /// Modèle Silero du catalogue, téléchargé au premier usage. C'est le
    /// défaut.
    Catalogue,
    /// Modèle Silero explicite.
    File(PathBuf),
}

/// Tout ce qui définit une transcription.
#[derive(Debug, Clone)]
pub struct Request {
    pub url: CanonicalUrl,
    pub model: ModelChoice,
    pub vad: VadChoice,
    /// Langue forcée ; `None` pour la détection automatique.
    pub lang: Option<String>,
    pub translate: bool,
    /// Contexte guidant le modèle sur les noms propres et le jargon. Vide ou
    /// blanc, il équivaut à son absence : voir [`Request::prompt`].
    pub initial_prompt: Option<String>,
    pub word_timestamps: bool,
    pub no_speech_thold: Option<f32>,
    pub entropy_thold: Option<f32>,
    pub threads: usize,
    /// Sous-titres officiels s'ils existent, repli silencieux sur la
    /// transcription sinon (SF-01).
    pub prefer_subs: bool,
    /// Consulte et alimente le cache de transcriptions (SF-08).
    pub use_cache: bool,
    pub max_duration_min: u64,
    pub access: Access,
    pub sidecars: Sidecars,
}

impl Request {
    /// Requête aux valeurs par défaut : modèle `auto`, VAD actif, cache
    /// consulté, sans sous-titres ni cookies.
    pub fn new(url: CanonicalUrl, sidecars: Sidecars) -> Self {
        Self {
            url,
            model: ModelChoice::Alias("auto".to_string()),
            vad: VadChoice::Catalogue,
            lang: None,
            translate: false,
            initial_prompt: None,
            word_timestamps: false,
            no_speech_thold: None,
            entropy_thold: None,
            threads: transcribe::default_threads(),
            prefer_subs: false,
            use_cache: true,
            max_duration_min: DEFAULT_MAX_DURATION_MIN,
            access: Access::default(),
            sidecars,
        }
    }

    /// Contexte normalisé : vide ou blanc, il équivaut à son absence — et ne
    /// doit pas produire une clé de cache distincte.
    pub fn prompt(&self) -> Option<String> {
        self.initial_prompt
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
    }

    pub fn vad_enabled(&self) -> bool {
        self.vad != VadChoice::Off
    }
}

/// Étape du déroulement, adressée à l'interface.
///
/// Rien n'y est imposé : chaque interface n'en retient que ce qu'elle affiche.
#[derive(Debug)]
pub enum Event<'a> {
    /// Empreinte de la clé de cache, calculée avant tout travail.
    CacheKey(&'a str),
    /// Transcription trouvée en cache : aucun autre événement ne suivra.
    CacheHit,
    /// Téléchargement d'un modèle, Whisper ou VAD, en octets. Rien n'est émis
    /// pour un modèle déjà présent.
    Download {
        model: &'a ModelSpec,
        received: u64,
        total: u64,
    },
    /// Chargement du modèle en mémoire.
    Loading {
        model: &'a Path,
        vad: Option<&'a Path>,
    },
    Probing,
    /// Métadonnées obtenues, et vidéo recevable.
    Probed(&'a Metadata),
    /// Piste de sous-titres retenue, avant son téléchargement.
    Subtitles {
        track: &'a Track,
        /// Traduction automatique par YouTube, non la langue d'origine.
        translation: bool,
        /// Langue déclarée de la vidéo.
        video_lang: Option<&'a str>,
    },
    /// Échec du téléchargement des sous-titres. Pas une erreur : la
    /// transcription prend le relais.
    SubtitlesFailed(&'a ScriptaError),
    /// Aucun sous-titre exploitable : repli sur la transcription.
    SubtitlesFallback,
    Extracting,
    Extracted {
        samples: usize,
        /// Durée de l'audio extrait, en secondes.
        audio_s: f64,
    },
    /// Début de l'inférence.
    Transcribing {
        threads: usize,
        /// Durée du média, en secondes : la chronologie des segments.
        media_s: f64,
    },
    /// Progression de l'inférence, en pourcentage. Émise bien plus souvent
    /// qu'à chaque pourcent : à filtrer côté interface.
    Progress(i32),
    /// Segment émis en cours d'inférence.
    Segment(&'a NewSegment),
    /// Inférence terminée.
    Transcribed {
        transcript: &'a Transcript,
        elapsed: Duration,
        /// Rapport entre la durée de l'audio et celle de l'inférence.
        speed: f64,
    },
    /// Mise en cache impossible. Pas une erreur : le résultat est là, seule
    /// sa réutilisation future est perdue.
    CacheWriteFailed(&'a ScriptaError),
}

/// Destinataire des [`Event`].
///
/// Partagé avec le thread d'inférence, d'où arrivent progression et segments :
/// d'où `Send + Sync`.
pub trait Observer: Send + Sync {
    fn on_event(&self, event: Event<'_>);
}

impl<F> Observer for F
where
    F: Fn(Event<'_>) + Send + Sync,
{
    fn on_event(&self, event: Event<'_>) {
        self(event)
    }
}

/// Transcription obtenue, et par quel chemin.
#[derive(Debug)]
pub struct Outcome {
    pub document: Document,
    pub origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Resservie par le cache.
    Cache,
    /// Sous-titres YouTube ; `auto` pour une piste auto-générée.
    Subtitles { auto: bool },
    /// Transcrite par Whisper.
    Inference,
}

/// Moteur chargé, conservé d'une transcription à l'autre.
///
/// Un chargement coûte plusieurs secondes et jusqu'à un gigaoctet de mémoire :
/// l'application de bureau garde le moteur entre deux transcriptions. Un seul
/// à la fois : changer de modèle libère le précédent avant de charger le
/// suivant, pour ne jamais tenir les deux en mémoire.
#[derive(Default)]
pub struct EngineSlot(Mutex<Option<(PathBuf, Arc<Engine>)>>);

impl EngineSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moteur du modèle `path`, chargé s'il ne l'est pas déjà.
    pub fn get_or_load(&self, path: &Path) -> Result<Arc<Engine>> {
        let mut emplacement = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((chemin, moteur)) = emplacement.as_ref()
            && chemin == path
        {
            return Ok(Arc::clone(moteur));
        }
        *emplacement = None;
        let moteur = Arc::new(Engine::load(path)?);
        *emplacement = Some((path.to_path_buf(), Arc::clone(&moteur)));
        Ok(moteur)
    }

    /// Libère le moteur — dès qu'aucune transcription ne l'emploie plus.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Résout l'alias d'un modèle **de transcription**.
pub fn resolve_alias(alias: &str) -> Result<&'static ModelSpec> {
    models::find(alias).ok_or_else(|| ScriptaError::ModelUnavailable {
        detail: format!(
            "modèle « {alias} » inconnu (disponibles : auto, {})",
            models::aliases().join(", ")
        ),
    })
}

/// Identifiant du modèle, **sans** le charger ni le télécharger.
///
/// Il entre dans la clé de cache, qu'il faut pouvoir calculer avant toute
/// opération coûteuse : sur un succès de cache, ni le modèle ni le réseau ne
/// sont sollicités.
pub fn model_id(choice: &ModelChoice) -> Result<String> {
    let fichier = match choice {
        ModelChoice::File(p) => p.clone(),
        ModelChoice::Alias(alias) => PathBuf::from(resolve_alias(alias)?.file),
    };
    Ok(fichier
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "inconnu".to_string()))
}

/// Clé de cache de la requête — SPEC SF-08.
///
/// Elle réunit tout ce qui influe sur le résultat, et rien d'autre : ni les
/// sous-titres, ni le nombre de threads, ni la durée maximale n'y entrent.
pub fn cache_key(req: &Request) -> Result<cache::Key> {
    Ok(cache::Key {
        video_id: req.url.video_id().to_string(),
        model: model_id(&req.model)?,
        lang: req.lang.clone(),
        translate: req.translate,
        vad: req.vad_enabled(),
        word_timestamps: req.word_timestamps,
        initial_prompt: req.prompt(),
        no_speech_thold: req.no_speech_thold,
        entropy_thold: req.entropy_thold,
    })
}

/// Garantit la présence locale d'un modèle, en rapportant son téléchargement.
///
/// Rien n'est émis quand le modèle est déjà présent : le cas nominal reste
/// silencieux.
pub fn fetch_model(
    spec: &ModelSpec,
    observer: &dyn Observer,
    cancel: Option<&CancelToken>,
) -> Result<PathBuf> {
    models::ensure(
        spec,
        &mut |received, total| {
            observer.on_event(Event::Download {
                model: spec,
                received,
                total,
            })
        },
        cancel,
    )
}

/// Document de sortie : provenance, conditions d'exécution, transcription.
pub fn document(url: &CanonicalUrl, meta: &Metadata, transcript: Transcript, run: Run) -> Document {
    Document {
        source: Source {
            url: url.as_str(),
            video_id: url.video_id().to_string(),
            title: meta.title.clone(),
            channel: meta.channel.clone(),
            duration_s: meta.duration,
            upload_date: meta.upload_date.clone(),
        },
        run: Run {
            language: transcript.language.clone(),
            ..run
        },
        transcript,
    }
}

/// Exécute une transcription de bout en bout.
///
/// Le jeton est interrogé à chaque étape. Armé, il interrompt l'étape en
/// cours — téléchargement, sonde, extraction, inférence — et rend
/// [`ScriptaError::Interrupted`].
pub fn run(
    req: &Request,
    engines: &EngineSlot,
    observer: &Arc<dyn Observer>,
    cancel: &CancelToken,
) -> Result<Outcome> {
    let clef = cache_key(req)?;
    // Avant tout téléchargement : récupérer 570 Mo de `turbo` pour refuser
    // ensuite la traduction serait un gâchis (SPEC SF-04).
    transcribe::check_translate_supported(&clef.model, req.translate)?;
    observer.on_event(Event::CacheKey(&clef.digest()));

    // Consultation avant tout le reste : sur un succès, ni le modèle ni le
    // réseau ne sont sollicités.
    if req.use_cache
        && let Some(doc) = cache::get(&clef)
    {
        observer.on_event(Event::CacheHit);
        return Ok(Outcome {
            document: doc,
            origin: Origin::Cache,
        });
    }

    // Ordre dicté par `prefer_subs`. Sans lui, le modèle est résolu en premier
    // pour qu'un modèle absent échoue immédiatement plutôt qu'après plusieurs
    // minutes de téléchargement. Avec lui, il se peut qu'aucun modèle ne soit
    // nécessaire : il serait absurde d'en télécharger un pour rien.
    let moteur = if req.prefer_subs {
        None
    } else {
        Some(charger(req, engines, observer.as_ref(), cancel)?)
    };

    interrompu(cancel)?;
    observer.on_event(Event::Probing);
    let meta = probe::probe(&req.sidecars.ytdlp, &req.url, &req.access, Some(cancel));
    interrompu(cancel)?;
    let meta = meta?;
    probe::check_admissible(&meta, req.max_duration_min)?;
    observer.on_event(Event::Probed(&meta));

    if req.prefer_subs {
        let sous_titres = sous_titres(&meta, req, observer.as_ref());
        interrompu(cancel)?;
        match sous_titres {
            Some((transcript, auto)) => {
                return Ok(Outcome {
                    document: document(&req.url, &meta, transcript, Run::default()),
                    origin: Origin::Subtitles { auto },
                });
            }
            None => observer.on_event(Event::SubtitlesFallback),
        }
    }

    let moteur = match moteur {
        Some(m) => m,
        None => charger(req, engines, observer.as_ref(), cancel)?,
    };

    interrompu(cancel)?;
    observer.on_event(Event::Extracting);
    let samples = audio::extract(
        &req.sidecars,
        &req.url,
        meta.duration,
        &req.access,
        Some(cancel),
    )?;
    // Relevée avant l'inférence, qui consomme le tampon.
    let audio_s = transcribe::duration_of(&samples);
    observer.on_event(Event::Extracted {
        samples: samples.len(),
        audio_s,
    });

    let options = Options {
        language: req.lang.clone(),
        translate: req.translate,
        threads: req.threads,
        initial_prompt: req.prompt(),
        word_timestamps: req.word_timestamps,
        vad_model: moteur.vad.clone(),
        no_speech_thold: req.no_speech_thold,
        entropy_thold: req.entropy_thold,
    };
    observer.on_event(Event::Transcribing {
        threads: options.threads,
        media_s: meta.duration.unwrap_or(audio_s),
    });

    let hooks = Hooks {
        on_progress: Some(Box::new({
            let o = Arc::clone(observer);
            move |p| o.on_event(Event::Progress(p))
        })),
        on_segment: Some(Box::new({
            let o = Arc::clone(observer);
            move |s: &NewSegment| o.on_event(Event::Segment(s))
        })),
        cancel: Some(cancel.clone()),
    };

    let debut = Instant::now();
    let transcript = moteur.engine.transcribe(samples, &options, hooks)?;
    let ecoule = debut.elapsed();
    let vitesse = if ecoule.as_secs_f64() > 0.0 {
        audio_s / ecoule.as_secs_f64()
    } else {
        0.0
    };
    observer.on_event(Event::Transcribed {
        transcript: &transcript,
        elapsed: ecoule,
        speed: vitesse,
    });

    let language_probability = transcript.language_probability;
    let doc = document(
        &req.url,
        &meta,
        transcript,
        Run {
            model: moteur.engine.model_id().to_string(),
            backend: Backend::compiled().as_str().to_string(),
            language_probability,
            translated: options.translate,
            vad: options.vad_model.is_some(),
            duration_ms: ecoule.as_millis() as u64,
            speed_realtime: (vitesse * 10.0).round() / 10.0,
            ..Default::default()
        },
    );

    if req.use_cache {
        // Un échec de mise en cache ne fait jamais échouer une transcription
        // réussie : le résultat est là, seule sa réutilisation future serait
        // perdue.
        match cache::put(&clef, &doc) {
            Ok(()) => {
                let _ = cache::evict(cache::DEFAULT_MAX_BYTES);
            }
            Err(e) => observer.on_event(Event::CacheWriteFailed(&e)),
        }
    }

    Ok(Outcome {
        document: doc,
        origin: Origin::Inference,
    })
}

/// Modèle chargé, avec le modèle VAD prêt s'il est actif.
struct Moteur {
    engine: Arc<Engine>,
    vad: Option<PathBuf>,
}

/// Obtient les modèles — téléchargés au besoin — et charge le moteur.
fn charger(
    req: &Request,
    engines: &EngineSlot,
    observer: &dyn Observer,
    cancel: &CancelToken,
) -> Result<Moteur> {
    let chemin = match &req.model {
        ModelChoice::File(p) => p.clone(),
        ModelChoice::Alias(alias) => fetch_model(resolve_alias(alias)?, observer, Some(cancel))?,
    };
    let vad = resolve_vad(&req.vad, observer, cancel)?;
    observer.on_event(Event::Loading {
        model: &chemin,
        vad: vad.as_deref(),
    });
    let engine = engines.get_or_load(&chemin)?;
    Ok(Moteur { engine, vad })
}

/// Modèle VAD à utiliser — SPEC SF-03, SF-04.
///
/// Actif par défaut et téléchargé au premier usage, comme les modèles Whisper.
/// Un échec est une erreur (code 30), jamais une désactivation silencieuse :
/// le résultat, et sa clé de cache, en dépendent.
fn resolve_vad(
    choice: &VadChoice,
    observer: &dyn Observer,
    cancel: &CancelToken,
) -> Result<Option<PathBuf>> {
    match choice {
        VadChoice::Off => Ok(None),
        VadChoice::File(p) => Ok(Some(p.clone())),
        VadChoice::Catalogue => fetch_model(&models::VAD_MODEL, observer, Some(cancel))
            .map(Some)
            .map_err(|e| match e {
                ScriptaError::ModelUnavailable { detail } => ScriptaError::ModelUnavailable {
                    detail: format!("{detail} — relancez avec --no-vad pour vous en passer"),
                },
                autre => autre,
            }),
    }
}

/// Tente la récupération des sous-titres officiels — SPEC SF-01.
///
/// Retourne `None` plutôt qu'une erreur en cas d'absence ou d'échec : la
/// spécification impose un **repli silencieux** sur la transcription. Les
/// endpoints de sous-titres de YouTube sont fréquemment limités en débit, et
/// `prefer_subs` ne doit jamais faire échouer une transcription qui aurait
/// abouti sans lui.
fn sous_titres(
    meta: &Metadata,
    req: &Request,
    observer: &dyn Observer,
) -> Option<(Transcript, bool)> {
    let piste = subtitles::best_track(meta, req.lang.as_deref())?;
    observer.on_event(Event::Subtitles {
        track: &piste,
        translation: subtitles::is_translation(&piste, meta),
        video_lang: meta.language.as_deref(),
    });
    match subtitles::fetch(&piste) {
        Ok(t) => Some((t, piste.auto)),
        Err(e) => {
            observer.on_event(Event::SubtitlesFailed(&e));
            None
        }
    }
}

/// Un jeton armé prime sur l'issue de l'étape qui s'achève : dans une console,
/// `Ctrl-C` est délivré aux sidecars aussi, et leur échec n'est alors que la
/// conséquence de l'interruption. C'est elle qu'il faut rapporter.
fn interrompu(cancel: &CancelToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(ScriptaError::Interrupted)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requete() -> Request {
        Request::new(
            crate::url::parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap(),
            Sidecars::new("yt-dlp", "ffmpeg"),
        )
    }

    #[test]
    fn un_contexte_vide_equivaut_a_son_absence() {
        // Sans quoi il produirait une clé de cache distincte pour un résultat
        // identique.
        let mut r = requete();
        assert_eq!(r.prompt(), None);
        r.initial_prompt = Some("   ".into());
        assert_eq!(r.prompt(), None);
        assert_eq!(cache_key(&r).unwrap(), cache_key(&requete()).unwrap());
        r.initial_prompt = Some(" Etienne Klein ".into());
        assert_eq!(r.prompt().as_deref(), Some("Etienne Klein"));
    }

    #[test]
    fn la_cle_ignore_ce_qui_n_influe_pas_sur_le_resultat() {
        let base = cache_key(&requete()).unwrap();
        let mut r = requete();
        r.threads = 1;
        r.prefer_subs = true;
        r.max_duration_min = 5;
        r.use_cache = false;
        r.access.cookies_from_browser = Some("firefox".into());
        assert_eq!(cache_key(&r).unwrap(), base);
    }

    #[test]
    fn la_cle_distingue_tout_ce_qui_influe_sur_le_resultat() {
        let base = cache_key(&requete()).unwrap().digest();
        let variantes: [fn(&mut Request); 8] = [
            |r| r.model = ModelChoice::Alias("small".into()),
            |r| r.vad = VadChoice::Off,
            |r| r.lang = Some("fr".into()),
            |r| r.translate = true,
            |r| r.initial_prompt = Some("Kubernetes".into()),
            |r| r.word_timestamps = true,
            |r| r.no_speech_thold = Some(0.4),
            |r| r.entropy_thold = Some(2.8),
        ];
        for (i, modifier) in variantes.iter().enumerate() {
            let mut r = requete();
            modifier(&mut r);
            assert_ne!(cache_key(&r).unwrap().digest(), base, "variante {i}");
        }
    }

    #[test]
    fn un_modele_vad_explicite_vaut_le_vad_du_catalogue() {
        // Même effet, même clé : seul l'emplacement du fichier diffère.
        let mut r = requete();
        r.vad = VadChoice::File("silero.bin".into());
        assert_eq!(cache_key(&r).unwrap(), cache_key(&requete()).unwrap());
    }

    #[test]
    fn l_identifiant_de_modele_ne_telecharge_rien() {
        assert_eq!(
            model_id(&ModelChoice::Alias("base".into())).unwrap(),
            "ggml-base"
        );
        assert_eq!(
            model_id(&ModelChoice::File("/modeles/perso-v2.bin".into())).unwrap(),
            "perso-v2"
        );
        let e = model_id(&ModelChoice::Alias("gigantesque".into())).unwrap_err();
        assert_eq!(e.exit_code(), 30);
        assert!(e.to_string().contains("auto, tiny"), "{e}");
    }
}
