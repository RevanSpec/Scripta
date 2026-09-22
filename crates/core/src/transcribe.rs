//! Moteur de transcription locale — SPEC SF-04, ADR-003.
//!
//! L'inférence n'est **pas** incrémentale : `whisper_full` consomme le buffer
//! PCM entier en un appel. La restitution progressive passe donc par le rappel
//! de nouveaux segments, pas par un découpage manuel de l'audio — qui
//! dégraderait la qualité aux jointures.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, get_lang_str,
};

use crate::audio::SAMPLE_RATE;
use crate::error::{Result, ScriptaError};
use crate::transcript::{Segment, Transcript, Word};

/// Backend d'accélération effectivement compilé dans ce binaire.
///
/// **ADR-001 révisé (Jalon 0).** `whisper-rs-sys` 0.15 lie les backends à la
/// compilation et n'expose pas le chargement dynamique de ggml : la sélection
/// est faite par feature Cargo, et un artefact ne peut pas découvrir un backend
/// à l'exécution. La détection annoncée en GUI se limite donc à rapporter ce
/// avec quoi le binaire a été construit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Vulkan,
    Cuda,
    Metal,
}

impl Backend {
    /// Backend de cette compilation.
    pub const fn compiled() -> Self {
        #[cfg(feature = "cuda")]
        {
            Self::Cuda
        }
        #[cfg(all(feature = "vulkan", not(feature = "cuda")))]
        {
            Self::Vulkan
        }
        #[cfg(all(feature = "metal", not(feature = "cuda"), not(feature = "vulkan")))]
        {
            Self::Metal
        }
        #[cfg(not(any(feature = "cuda", feature = "vulkan", feature = "metal")))]
        {
            Self::Cpu
        }
    }

    pub const fn is_gpu(&self) -> bool {
        !matches!(self, Self::Cpu)
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Vulkan => "vulkan",
            Self::Cuda => "cuda",
            Self::Metal => "metal",
        }
    }
}

/// Paramètres d'inférence — SPEC SF-04.
#[derive(Debug, Clone)]
pub struct Options {
    /// Langue forcée (ISO 639-1), ou `None` pour la détection automatique.
    pub language: Option<String>,
    /// Traduction vers l'anglais.
    pub translate: bool,
    pub threads: usize,
    /// Contexte guidant le modèle sur les noms propres et le jargon.
    pub initial_prompt: Option<String>,
    /// Horodatage au mot, requis par le JSON enrichi.
    pub word_timestamps: bool,
    /// Modèle VAD Silero. Son absence désactive le VAD.
    pub vad_model: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            language: None,
            translate: false,
            threads: default_threads(),
            initial_prompt: None,
            word_timestamps: false,
            vad_model: None,
        }
    }
}

pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Jeton d'annulation, armé depuis un autre thread (bouton « Annuler » de la
/// GUI, `SIGINT` en CLI). Interrogé par whisper.cpp entre ses fenêtres de
/// traitement : sans lui, une inférence lancée depuis plusieurs minutes ne peut
/// pas être interrompue.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Trampoline du rappel d'abandon, appelé par ggml depuis le code natif.
///
/// # Sécurité
///
/// `user_data` doit être un pointeur obtenu par `Arc::into_raw` sur un
/// `Arc<AtomicBool>` encore vivant. [`Engine::transcribe`] en garantit la
/// validité sur toute la durée de l'appel à `full()` et le reprend ensuite.
unsafe extern "C" fn abort_trampoline(user_data: *mut std::ffi::c_void) -> bool {
    if user_data.is_null() {
        return false;
    }
    unsafe { (*(user_data as *const AtomicBool)).load(Ordering::SeqCst) }
}

/// Rappels de progression — SPEC SF-04.
#[derive(Default)]
pub struct Hooks {
    /// Progression globale, en pourcentage.
    pub on_progress: Option<Box<dyn FnMut(i32) + Send>>,
    pub cancel: Option<CancelToken>,
}

/// Modèle chargé. Le chargement est coûteux : réutiliser l'instance entre
/// plusieurs transcriptions.
pub struct Engine {
    ctx: WhisperContext,
    model_id: String,
}

/// whisper.cpp et ggml écrivent une vingtaine de lignes de diagnostic sur
/// `stderr` à chaque chargement de modèle, hors de tout contrôle de `--quiet`.
/// Les rediriger vers les hooks de `whisper-rs` — sans activer les backends
/// `log` ou `tracing` — les supprime purement et simplement, ce qui préserve le
/// contrat de sortie du [§4.1](SPEC) : `stderr` ne porte que nos diagnostics.
///
/// Les erreurs réelles ne sont pas perdues : elles remontent par `Result`.
/// TODO(J2) : réactiver ces logs derrière `--verbose` via la feature
/// `log_backend`.
fn silence_native_logs() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(whisper_rs::install_logging_hooks);
}

impl Engine {
    pub fn load(model_path: &Path) -> Result<Self> {
        silence_native_logs();

        if !model_path.is_file() {
            return Err(ScriptaError::ModelUnavailable {
                detail: format!("fichier introuvable : {}", model_path.display()),
            });
        }

        let mut params = WhisperContextParameters::new();
        params.use_gpu(Backend::compiled().is_gpu());

        let ctx = WhisperContext::new_with_params(model_path, params).map_err(|e| {
            ScriptaError::ModelUnavailable {
                detail: format!("chargement de {} : {e}", model_path.display()),
            }
        })?;

        Ok(Self {
            ctx,
            model_id: model_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "inconnu".to_string()),
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Transcrit un buffer PCM 16 kHz mono.
    pub fn transcribe(
        &self,
        samples: &[f32],
        options: &Options,
        hooks: Hooks,
    ) -> Result<Transcript> {
        validate(options)?;

        if samples.is_empty() {
            return Err(ScriptaError::InferenceFailed {
                detail: "aucun échantillon audio à transcrire".to_string(),
            });
        }

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| ScriptaError::InferenceFailed {
                detail: format!("création de l'état : {e}"),
            })?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
        params.set_n_threads(options.threads as i32);
        params.set_translate(options.translate);
        params.set_token_timestamps(options.word_timestamps);

        // whisper.cpp écrit sur stdout par défaut, ce qui corromprait une sortie
        // destinée à un tube (SPEC §4.1).
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        // `"auto"` suffit à déclencher la détection de langue (whisper.cpp,
        // `whisper_full_with_state`). Surtout, ne PAS appeler
        // `set_detect_language(true)` : ce drapeau fait sortir `whisper_full`
        // juste après la détection, sans jamais décoder — la transcription
        // revient vide et sans la moindre erreur.
        params.set_language(Some(options.language.as_deref().unwrap_or("auto")));

        if let Some(prompt) = &options.initial_prompt {
            params.set_initial_prompt(prompt);
        }

        // VAD — SPEC SF-04. `enable_vad` panique si le chemin n'est pas défini
        // au préalable : l'ordre des deux appels est contraint.
        if let Some(vad) = &options.vad_model {
            if vad.is_file() {
                params.set_vad_model_path(Some(&vad.to_string_lossy()));
                params.enable_vad(true);
            } else {
                return Err(ScriptaError::ModelUnavailable {
                    detail: format!("modèle VAD introuvable : {}", vad.display()),
                });
            }
        }

        let Hooks {
            on_progress,
            cancel,
        } = hooks;

        if let Some(mut cb) = on_progress {
            params.set_progress_callback_safe(move |p: i32| cb(p));
        }

        // ⚠ `set_abort_callback_safe` est INUTILISABLE en whisper-rs 0.16.0 :
        // il place dans `user_data` un `Box<dyn FnMut() -> bool>` — un pointeur
        // gras — mais instancie son trampoline avec le type concret `F` de la
        // fermeture. Le pointeur est donc réinterprété comme une autre
        // structure : confusion de types et comportement indéfini. En pratique
        // le rappel renvoie n'importe quoi, et une valeur vraie fait échouer
        // l'encodage avec le code -6 (`failed to encode`).
        //
        // On passe donc par l'API brute avec un trampoline correctement typé
        // sur un pointeur FIN vers l'`AtomicBool`, ce qui écarte le problème par
        // construction. À retirer si le correctif est accepté en amont.
        let cancel_raw: Option<*const AtomicBool> =
            cancel.as_ref().map(|t| Arc::into_raw(Arc::clone(&t.0)));

        if let Some(raw) = cancel_raw {
            // SÉCURITÉ : `raw` reste vivant jusqu'à sa reprise après `full()`,
            // et le trampoline ne fait qu'y lire un booléen atomique.
            unsafe {
                params.set_abort_callback(Some(abort_trampoline));
                params.set_abort_callback_user_data(raw as *mut std::ffi::c_void);
            }
        }

        let issue = state.full(params, samples);

        // Reprise du compteur de références cédé à `Arc::into_raw`. À faire
        // impérativement après `full()` : le code natif lit ce pointeur
        // pendant tout l'appel.
        if let Some(raw) = cancel_raw {
            // SÉCURITÉ : `raw` provient d'un `Arc::into_raw` de cette fonction
            // et n'est repris qu'une fois.
            drop(unsafe { Arc::from_raw(raw) });
        }

        // L'annulation prime sur l'erreur remontée, et doit être testée avant
        // elle : whisper.cpp signale l'abandon tantôt par un code d'erreur,
        // tantôt par un retour normal avec zéro segment. Classer le premier cas
        // en `InferenceFailed` ferait passer une annulation volontaire pour une
        // panne, et le second livrerait une transcription tronquée présentée
        // comme complète.
        if cancel.map(|t| t.is_cancelled()).unwrap_or(false) {
            return Err(ScriptaError::Interrupted);
        }

        issue.map_err(|e| ScriptaError::InferenceFailed {
            detail: format!("inférence : {e}"),
        })?;

        Ok(build_transcript(&self.ctx, &state, options))
    }
}

/// Contrôles de cohérence — SPEC SF-04.
fn validate(options: &Options) -> Result<()> {
    if options.threads == 0 {
        return Err(ScriptaError::InferenceFailed {
            detail: "le nombre de threads doit être supérieur à zéro".to_string(),
        });
    }
    Ok(())
}

/// `large-v3-turbo` n'a été entraîné que pour la transcription : sa sortie en
/// mode traduction est inexploitable. La combinaison est refusée en amont
/// plutôt que de livrer un résultat silencieusement faux (SPEC SF-04).
pub fn check_translate_supported(model_id: &str, translate: bool) -> Result<()> {
    if translate && model_id.to_lowercase().contains("turbo") {
        return Err(ScriptaError::InferenceFailed {
            detail: "le modèle « turbo » ne prend pas en charge la traduction ; \
                     utilisez --model large-v3"
                .to_string(),
        });
    }
    Ok(())
}

/// Reconstruit les mots à partir des tokens du segment.
///
/// Whisper ne produit pas des mots mais des tokens BPE : « bonjour » peut
/// arriver en « bon » + « jour ». La convention du modèle est qu'un token
/// **initial de mot commence par une espace** ; c'est elle qui sert de
/// frontière. Les bornes du mot sont celles de son premier et de son dernier
/// token, et sa probabilité la moyenne des leurs.
fn build_words(ctx: &WhisperContext, segment: &whisper_rs::WhisperSegment<'_>) -> Vec<Word> {
    let eot = ctx.token_eot();
    let mut mots: Vec<Word> = Vec::new();
    let mut probas: Vec<Vec<f32>> = Vec::new();

    for i in 0..segment.n_tokens() {
        let Some(token) = segment.get_token(i) else {
            continue;
        };
        // Au-delà de EOT se trouvent les tokens de service (horodatage, langue,
        // marqueurs de tâche) : ils n'ont pas de texte à restituer.
        if token.token_id() >= eot {
            continue;
        }
        let Ok(texte) = token.to_str_lossy() else {
            continue;
        };
        if texte.trim().is_empty() {
            continue;
        }

        let data = token.token_data();
        let debut = data.t0 as f64 / 100.0;
        let fin = data.t1 as f64 / 100.0;
        let nouveau_mot = texte.starts_with(' ') || mots.is_empty();

        if nouveau_mot {
            mots.push(Word {
                word: texte.trim_start().to_string(),
                start: debut,
                end: fin,
                probability: None,
            });
            probas.push(vec![token.token_probability()]);
        } else if let (Some(mot), Some(p)) = (mots.last_mut(), probas.last_mut()) {
            mot.word.push_str(&texte);
            mot.end = fin;
            p.push(token.token_probability());
        }
    }

    for (mot, p) in mots.iter_mut().zip(&probas) {
        if !p.is_empty() {
            mot.probability = Some(p.iter().sum::<f32>() / p.len() as f32);
        }
    }
    mots
}

fn build_transcript(
    ctx: &WhisperContext,
    state: &whisper_rs::WhisperState,
    options: &Options,
) -> Transcript {
    let n = state.full_n_segments();
    let mut segments = Vec::with_capacity(n.max(0) as usize);

    for i in 0..n {
        let Some(seg) = state.get_segment(i) else {
            continue;
        };
        // `to_str_lossy` plutôt que `to_str` : un token UTF-8 tronqué en fin de
        // segment ne doit pas faire perdre toute la transcription.
        let text = seg
            .to_str_lossy()
            .map(|c| c.into_owned())
            .unwrap_or_default();

        segments.push(Segment {
            id: i as usize,
            // Les horodatages whisper.cpp sont en centisecondes.
            start: seg.start_timestamp() as f64 / 100.0,
            end: seg.end_timestamp() as f64 / 100.0,
            text,
            no_speech_prob: Some(seg.no_speech_probability()),
            avg_logprob: None,
            words: if options.word_timestamps {
                build_words(ctx, &seg)
            } else {
                Vec::new()
            },
        });
    }

    let language = if options.language.is_some() {
        options.language.clone()
    } else {
        get_lang_str(state.full_lang_id_from_state()).map(|s| s.to_string())
    };

    Transcript {
        segments,
        language,
        language_probability: None,
        translated: options.translate,
    }
}

/// Durée couverte par un buffer PCM, en secondes.
pub fn duration_of(samples: &[f32]) -> f64 {
    samples.len() as f64 / SAMPLE_RATE as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuse_la_traduction_avec_turbo() {
        assert!(check_translate_supported("ggml-large-v3-turbo-q5_0", true).is_err());
        // Sans traduction, turbo reste parfaitement utilisable.
        assert!(check_translate_supported("ggml-large-v3-turbo-q5_0", false).is_ok());
        assert!(check_translate_supported("ggml-large-v3", true).is_ok());
    }

    #[test]
    fn jeton_d_annulation() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        // Un clone partage l'état : c'est ce qui permet d'armer l'annulation
        // depuis un autre thread pendant que full() bloque.
        let clone = token.clone();
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn modele_absent_est_signale() {
        // `unwrap_err` exigerait `Debug` sur Engine, qui encapsule un contexte
        // FFI non formatable.
        match Engine::load(Path::new("modele-qui-n-existe-pas.bin")) {
            Err(e) => assert_eq!(e.exit_code(), 30),
            Ok(_) => panic!("un modèle inexistant ne devrait pas se charger"),
        }
    }

    #[test]
    fn threads_par_defaut_non_nuls() {
        assert!(default_threads() >= 1);
        assert!(validate(&Options::default()).is_ok());
        assert!(
            validate(&Options {
                threads: 0,
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn duree_calculee_depuis_la_frequence() {
        assert_eq!(duration_of(&vec![0.0; 16_000]), 1.0);
        assert_eq!(duration_of(&[]), 0.0);
    }

    #[test]
    fn backend_compile_coherent() {
        let b = Backend::compiled();
        assert_eq!(b.is_gpu(), !matches!(b, Backend::Cpu));
        assert!(!b.as_str().is_empty());
    }
}
