//! Commandes IPC — SPEC §4.2.
//!
//! **Aucun travail bloquant sur le thread principal.** Une inférence appelée
//! directement depuis une commande figerait la fenêtre plusieurs minutes ;
//! elle s'exécute donc sur un thread dédié, la progression et les segments
//! remontant par événements.

use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use scripta_core::format::{OutputFormat, SubtitleOptions};
use scripta_core::transcribe::{CancelToken, Hooks, Options};
use scripta_core::{Document, Engine, Run, ScriptaError, Source};

use crate::state::AppState;

/// Erreur transmise au frontend.
///
/// Le code de sortie accompagne le message : il permet à l'interface de
/// distinguer une cause actionnable — connexion requise, direct non pris en
/// charge — d'une panne, sans analyser du texte (SPEC SF-07).
#[derive(Debug, Serialize)]
pub struct IpcError {
    message: String,
    code: i32,
}

impl From<ScriptaError> for IpcError {
    fn from(e: ScriptaError) -> Self {
        Self {
            code: e.exit_code(),
            message: e.to_string(),
        }
    }
}

type IpcResult<T> = std::result::Result<T, IpcError>;

// ------------------------------------------------------------ diagnostic ---

#[derive(Serialize)]
pub struct BackendInfo {
    backend: String,
    gpu: bool,
    threads: usize,
    ytdlp: Option<String>,
    ffmpeg: Option<String>,
}

/// Renseigne l'interface sur l'environnement d'exécution.
///
/// Le backend rapporté est celui **compilé**, non un matériel découvert :
/// `whisper-rs-sys` lie les backends à la compilation ([ADR-001]).
///
/// [ADR-001]: ../../docs/SPEC.md
#[tauri::command]
pub fn backend_info() -> BackendInfo {
    use scripta_core::sidecar::{Kind, resolve, version_of};

    let yt = resolve(Kind::YtDlp, None);
    let ff = resolve(Kind::Ffmpeg, None);

    BackendInfo {
        backend: scripta_core::Backend::compiled().as_str().to_string(),
        gpu: scripta_core::Backend::compiled().is_gpu(),
        threads: scripta_core::transcribe::default_threads(),
        ytdlp: version_of(&yt.path, Kind::YtDlp),
        ffmpeg: version_of(&ff.path, Kind::Ffmpeg),
    }
}

#[derive(Serialize)]
pub struct ModelEntry {
    alias: String,
    size_mb: u64,
    installed: bool,
}

#[tauri::command]
pub fn list_models() -> IpcResult<Vec<ModelEntry>> {
    Ok(scripta_core::models::installed()?
        .into_iter()
        .map(|(spec, installed)| ModelEntry {
            alias: spec.alias.to_string(),
            size_mb: spec.size_mb(),
            installed,
        })
        .collect())
}

// ----------------------------------------------------------------- sonde ---

#[derive(Serialize)]
pub struct VideoInfo {
    video_id: String,
    title: String,
    channel: Option<String>,
    duration_s: Option<f64>,
    language: Option<String>,
    subtitle_langs: Vec<String>,
}

/// Valide l'URL et rapporte les métadonnées, sans rien télécharger.
///
/// Permet à l'interface d'afficher le titre et la durée avant de lancer une
/// opération de plusieurs minutes.
#[tauri::command]
pub fn probe_url(url: String) -> IpcResult<VideoInfo> {
    let canonique = scripta_core::url::parse(&url)?;
    let ytdlp = scripta_core::sidecar::resolve(scripta_core::sidecar::Kind::YtDlp, None).path;
    let meta = scripta_core::probe::probe(&ytdlp, &canonique, &Default::default())?;

    Ok(VideoInfo {
        video_id: canonique.video_id().to_string(),
        title: meta.title.clone(),
        channel: meta.channel.clone(),
        duration_s: meta.duration,
        language: meta.language.clone(),
        subtitle_langs: meta
            .available_subtitle_langs()
            .into_iter()
            .map(str::to_string)
            .collect(),
    })
}

// --------------------------------------------------------- transcription ---

#[derive(Debug, Deserialize)]
pub struct TranscribeArgs {
    url: String,
    model: String,
    lang: Option<String>,
    translate: bool,
    initial_prompt: Option<String>,
    word_timestamps: bool,
    prefer_subs: bool,
    cookies_from_browser: Option<String>,
}

#[derive(Clone, Serialize)]
struct Progress {
    phase: &'static str,
    percent: Option<i32>,
    detail: Option<String>,
}

fn emit(app: &tauri::AppHandle, phase: &'static str, percent: Option<i32>, detail: Option<String>) {
    let _ = app.emit(
        "scripta://progress",
        Progress {
            phase,
            percent,
            detail,
        },
    );
}

/// Transcrit une URL. **Appelée depuis un thread dédié par Tauri**, grâce à
/// `async` : une commande synchrone bloquerait la boucle d'événements et
/// figerait la fenêtre.
#[tauri::command]
pub async fn transcribe(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    args: TranscribeArgs,
) -> IpcResult<String> {
    let url = scripta_core::url::parse(&args.url)?;
    let access = scripta_core::Access {
        cookies_from_browser: args.cookies_from_browser.clone(),
    };

    let spec =
        scripta_core::models::find(&args.model).ok_or_else(|| ScriptaError::ModelUnavailable {
            detail: format!("modèle « {} » inconnu", args.model),
        })?;
    let model_id = std::path::Path::new(spec.file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let clef = scripta_core::cache::Key {
        video_id: url.video_id().to_string(),
        model: model_id.clone(),
        lang: args.lang.clone(),
        translate: args.translate,
        vad: false,
        word_timestamps: args.word_timestamps,
    };

    // Consultation du cache avant tout : sur un succès, ni modèle ni réseau.
    if let Some(doc) = scripta_core::cache::get(&clef) {
        emit(&app, "cache", Some(100), None);
        let texte = OutputFormat::Txt.render(&doc, &SubtitleOptions::default());
        state.set_last(Some(doc));
        return Ok(texte);
    }

    emit(&app, "probe", None, None);
    let ytdlp = scripta_core::sidecar::resolve(scripta_core::sidecar::Kind::YtDlp, None).path;
    let meta = scripta_core::probe::probe(&ytdlp, &url, &access)?;
    scripta_core::probe::check_admissible(&meta, 240)?;
    emit(&app, "probe", None, Some(meta.title.clone()));

    // Sous-titres officiels : instantané quand ils existent, avec repli
    // silencieux sur la transcription (SPEC SF-01).
    if args.prefer_subs
        && let Some(piste) = scripta_core::subtitles::best_track(&meta, args.lang.as_deref())
        && let Ok(transcript) = scripta_core::subtitles::fetch(&piste)
    {
        emit(&app, "subtitles", Some(100), Some(piste.lang.clone()));
        let doc = document_de(&url, &meta, transcript, Run::default());
        let texte = OutputFormat::Txt.render(&doc, &SubtitleOptions::default());
        state.set_last(Some(doc));
        return Ok(texte);
    }

    emit(&app, "model", None, Some(model_id.clone()));
    let spec_copie = *spec;
    state.with_engine(
        &model_id,
        || {
            let chemin = scripta_core::models::ensure(&spec_copie, &mut |_, _| {})?;
            Engine::load(&chemin)
        },
        |_| (),
    )?;
    scripta_core::transcribe::check_translate_supported(&model_id, args.translate)?;

    emit(&app, "download", None, None);
    let sidecars = scripta_core::audio::Sidecars::new(
        ytdlp,
        scripta_core::sidecar::resolve(scripta_core::sidecar::Kind::Ffmpeg, None).path,
    );
    let samples = scripta_core::audio::extract(&sidecars, &url, meta.duration, &access)?;

    let cancel = CancelToken::new();
    state.set_cancel(Some(cancel.clone()));

    let app_progress = app.clone();
    let options = Options {
        language: args.lang.clone(),
        translate: args.translate,
        threads: scripta_core::transcribe::default_threads(),
        initial_prompt: args.initial_prompt.clone(),
        word_timestamps: args.word_timestamps,
        vad_model: None,
    };

    let debut = std::time::Instant::now();
    let resultat = state.with_engine(
        &model_id,
        || unreachable!("le moteur vient d'être chargé"),
        |engine| {
            engine.transcribe(
                &samples,
                &options,
                Hooks {
                    on_progress: Some(Box::new(move |p| {
                        emit(&app_progress, "transcribe", Some(p), None);
                    })),
                    cancel: Some(cancel.clone()),
                },
            )
        },
    )?;
    state.set_cancel(None);

    let transcript = resultat?;
    let ecoule = debut.elapsed();
    let audio_s = scripta_core::transcribe::duration_of(&samples);
    let vitesse = if ecoule.as_secs_f64() > 0.0 {
        audio_s / ecoule.as_secs_f64()
    } else {
        0.0
    };

    let doc = document_de(
        &url,
        &meta,
        transcript,
        Run {
            model: model_id,
            backend: scripta_core::Backend::compiled().as_str().to_string(),
            translated: args.translate,
            duration_ms: ecoule.as_millis() as u64,
            speed_realtime: (vitesse * 10.0).round() / 10.0,
            ..Default::default()
        },
    );

    // Un échec de mise en cache n'invalide pas une transcription réussie.
    let _ = scripta_core::cache::put(&clef, &doc);

    let texte = OutputFormat::Txt.render(&doc, &SubtitleOptions::default());
    state.set_last(Some(doc));
    Ok(texte)
}

/// Interrompt la transcription en cours.
///
/// Retourne `false` si rien ne tournait : l'interface peut alors ignorer le
/// clic plutôt que d'afficher une confirmation trompeuse.
#[tauri::command]
pub fn cancel(state: State<'_, AppState>) -> bool {
    state.cancel()
}

// ------------------------------------------------------------- export ------

/// Rend le dernier document dans le format demandé, sans réinférence.
#[tauri::command]
pub fn render_as(
    state: State<'_, AppState>,
    format: String,
    max_line_width: Option<usize>,
    max_line_count: Option<usize>,
) -> IpcResult<String> {
    let doc = state.last().ok_or_else(|| ScriptaError::InferenceFailed {
        detail: "aucune transcription à exporter".to_string(),
    })?;

    let fmt: OutputFormat = format
        .parse()
        .map_err(|m: String| ScriptaError::InvalidUrl(m))?;
    let opts = SubtitleOptions {
        max_line_width: max_line_width.unwrap_or(42),
        max_line_count: max_line_count.unwrap_or(2),
        ..Default::default()
    };
    Ok(fmt.render(&doc, &opts))
}

#[tauri::command]
pub fn save_output(path: String, contents: String) -> IpcResult<()> {
    std::fs::write(&path, contents).map_err(|source| ScriptaError::OutputFailed {
        path: path.into(),
        source,
    })?;
    Ok(())
}

#[tauri::command]
pub async fn update_extractor(app: tauri::AppHandle) -> IpcResult<String> {
    let chemin = scripta_core::sidecar::update_ytdlp(&mut |recus, total| {
        if let Some(pourcent) = (recus * 100).checked_div(total) {
            emit(&app, "extractor", Some(pourcent as i32), None);
        }
    })?;

    Ok(
        scripta_core::sidecar::version_of(&chemin, scripta_core::sidecar::Kind::YtDlp)
            .unwrap_or_else(|| "version inconnue".to_string()),
    )
}

fn document_de(
    url: &scripta_core::CanonicalUrl,
    meta: &scripta_core::probe::Metadata,
    transcript: scripta_core::Transcript,
    run: Run,
) -> Document {
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
