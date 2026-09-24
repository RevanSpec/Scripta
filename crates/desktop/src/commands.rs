//! Commandes IPC — SPEC §4.2.
//!
//! **Rien de long sur le thread principal.** Tauri exécute les commandes
//! synchrones sur le thread de la fenêtre : une sonde, un téléchargement ou
//! une inférence appelés directement la figeraient. Toute commande qui touche
//! au disque, au réseau ou à un processus est donc asynchrone, et son travail
//! part sur un thread bloquant. La progression remonte par un canal propre à
//! chaque appel, via le [`Relais`].
//!
//! Aucune logique métier ici : l'enchaînement d'une transcription appartient
//! à `scripta_core::pipeline`, que la CLI emploie aussi.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;

use scripta_core::audio::Sidecars;
use scripta_core::format::{OutputFormat, SubtitleOptions};
use scripta_core::pipeline::{self, ModelChoice, Observer, Origin, Request, VadChoice};
use scripta_core::sidecar::{self, Kind, Resolved};
use scripta_core::{Access, Backend, Document, ModelSpec, ScriptaError, models, output, probe};

use crate::erreur::{IpcError, IpcResult};
use crate::relais::{Message, Relais, SegmentVue};
use crate::state::AppState;

/// Exécute `f` sur un thread bloquant, hors de la boucle d'événements.
async fn en_tache<T: Send + 'static>(
    f: impl FnOnce() -> IpcResult<T> + Send + 'static,
) -> IpcResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(IpcError::interne)?
}

/// Relais vers le canal d'un appel.
fn relais_vers(canal: Channel<Vec<Message>>) -> Relais {
    Relais::new(move |lot| {
        let _ = canal.send(lot);
    })
}

/// Résout un sidecar pour la GUI — SPEC §5.1 : jamais par le `PATH`.
///
/// Exception en développement : tant que les sidecars ne sont pas embarqués
/// (tâche 3.12, qui relève de l'empaquetage), `cargo tauri dev` s'appuie sur
/// ceux du système. Une build de release n'y a jamais recours.
fn sidecar_gui(kind: Kind) -> scripta_core::Result<Resolved> {
    if let Some(r) = sidecar::resolve_installed(kind) {
        return Ok(r);
    }
    if cfg!(debug_assertions) {
        return Ok(sidecar::resolve(kind, None));
    }
    Err(ScriptaError::SidecarMissing {
        name: kind.name().to_string(),
    })
}

// ------------------------------------------------------------ diagnostic ---

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarInfo {
    /// `None` : introuvable ou injoignable.
    version: Option<String>,
    origin: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendInfo {
    version: &'static str,
    /// Backend **compilé** : `whisper-rs-sys` lie les backends à la
    /// compilation, aucun GPU n'est découvert à l'exécution (ADR-001).
    backend: &'static str,
    gpu: bool,
    threads: usize,
    ytdlp: SidecarInfo,
    ffmpeg: SidecarInfo,
    languages: Vec<Langue>,
}

/// Langue reconnue par Whisper. Le nom anglais sert de repli à l'interface,
/// qui nomme les langues en français quand elle le sait.
#[derive(Debug, Serialize)]
pub struct Langue {
    code: &'static str,
    name: &'static str,
}

fn sidecar_info(kind: Kind) -> SidecarInfo {
    match sidecar_gui(kind) {
        Ok(r) => SidecarInfo {
            version: sidecar::version_of(&r.path, kind),
            origin: Some(r.origin.as_str()),
        },
        Err(_) => SidecarInfo {
            version: None,
            origin: None,
        },
    }
}

/// Renseigne l'interface sur l'environnement d'exécution.
#[tauri::command]
pub async fn backend_info() -> IpcResult<BackendInfo> {
    // Interroger les sidecars lance deux processus : yt-dlp, exécutable
    // autoextractible, met plus d'une seconde à répondre sous Windows.
    en_tache(|| {
        Ok(BackendInfo {
            version: env!("CARGO_PKG_VERSION"),
            backend: Backend::compiled().as_str(),
            gpu: Backend::compiled().is_gpu(),
            threads: scripta_core::transcribe::default_threads(),
            ytdlp: sidecar_info(Kind::YtDlp),
            ffmpeg: sidecar_info(Kind::Ffmpeg),
            languages: scripta_core::transcribe::languages()
                .into_iter()
                .map(|(code, name)| Langue { code, name })
                .collect(),
        })
    })
    .await
}

// --------------------------------------------------------------- modèles ---

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    alias: &'static str,
    file: &'static str,
    size: u64,
    installed: bool,
    /// Faux pour les modèles « turbo », entraînés pour la seule transcription.
    translates: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsInfo {
    /// Modèle que `auto` désigne pour ce build.
    auto: Option<&'static str>,
    whisper: Vec<ModelEntry>,
    vad: ModelEntry,
    /// Espace occupé par les modèles installés, en octets.
    used_bytes: u64,
    dir: String,
}

fn entree(spec: &'static ModelSpec) -> IpcResult<ModelEntry> {
    Ok(ModelEntry {
        alias: spec.alias,
        file: spec.file,
        size: spec.size,
        installed: models::is_installed(spec)?,
        translates: !spec.is_turbo(),
    })
}

fn etat_des_modeles() -> IpcResult<ModelsInfo> {
    let whisper = models::MODELS
        .iter()
        .map(entree)
        .collect::<IpcResult<Vec<_>>>()?;
    let vad = entree(&models::VAD_MODEL)?;
    let used_bytes = whisper
        .iter()
        .chain(std::iter::once(&vad))
        .filter(|m| m.installed)
        .map(|m| m.size)
        .sum();
    Ok(ModelsInfo {
        auto: models::find("auto").map(|m| m.alias),
        whisper,
        vad,
        used_bytes,
        dir: models::models_dir()?.display().to_string(),
    })
}

fn modele(alias: &str) -> IpcResult<&'static ModelSpec> {
    models::find_any(alias).ok_or_else(|| {
        IpcError::from(ScriptaError::ModelUnavailable {
            detail: format!("modèle « {alias} » inconnu"),
        })
    })
}

#[tauri::command]
pub async fn list_models() -> IpcResult<ModelsInfo> {
    en_tache(etat_des_modeles).await
}

/// Télécharge un modèle et vérifie son empreinte — SF-03, tâche 3.8.
#[tauri::command]
pub async fn download_model(
    state: State<'_, AppState>,
    alias: String,
    canal: Channel<Vec<Message>>,
) -> IpcResult<ModelsInfo> {
    let spec = modele(&alias)?;
    let tache = state.commencer().ok_or_else(IpcError::occupe)?;
    en_tache(move || {
        let relais = relais_vers(canal);
        pipeline::fetch_model(spec, &relais, Some(&tache.cancel))?;
        drop(relais);
        etat_des_modeles()
    })
    .await
}

/// Supprime un modèle du cache — tâche 3.8.
#[tauri::command]
pub async fn remove_model(
    app: AppHandle,
    state: State<'_, AppState>,
    alias: String,
) -> IpcResult<ModelsInfo> {
    let spec = modele(&alias)?;
    // Réservée comme une tâche : un modèle ne doit pas disparaître sous une
    // transcription qui l'emploie.
    let tache = state.commencer().ok_or_else(IpcError::occupe)?;
    en_tache(move || {
        // Le moteur en mémoire est libéré avec son fichier : le garder
        // occuperait jusqu'à un gigaoctet pour un modèle « supprimé ».
        app.state::<AppState>().engines.clear();
        models::remove(spec)?;
        drop(tache);
        etat_des_modeles()
    })
    .await
}

// ----------------------------------------------------------------- sonde ---

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    video_id: String,
    title: String,
    channel: Option<String>,
    duration_s: Option<f64>,
    language: Option<String>,
    /// Langues des sous-titres rédigés.
    subtitles: Vec<String>,
    /// Langue de la piste auto-générée d'origine, s'il y en a une. Les
    /// traductions automatiques — une centaine de langues — ne sont pas
    /// listées : elles noieraient l'information utile.
    auto_captions: Option<String>,
}

/// Valide l'URL et rapporte les métadonnées, sans rien télécharger : le
/// titre et la durée s'affichent avant de lancer une opération de plusieurs
/// minutes.
#[tauri::command]
pub async fn probe_url(url: String, cookies_from_browser: Option<String>) -> IpcResult<VideoInfo> {
    en_tache(move || {
        let canonique = scripta_core::url::parse(&url)?;
        let ytdlp = sidecar_gui(Kind::YtDlp)?;
        let meta = probe::probe(&ytdlp.path, &canonique, &acces(cookies_from_browser), None)?;
        Ok(VideoInfo {
            video_id: canonique.video_id().to_string(),
            subtitles: meta.subtitles.keys().cloned().collect(),
            auto_captions: meta
                .language
                .clone()
                .filter(|l| meta.automatic_captions.contains_key(l)),
            title: meta.title,
            channel: meta.channel,
            duration_s: meta.duration,
            language: meta.language,
        })
    })
    .await
}

/// Cookies d'un navigateur, jamais activés d'office (SF-09).
fn acces(cookies_from_browser: Option<String>) -> Access {
    Access {
        cookies_from_browser: cookies_from_browser
            .map(|n| n.trim().to_lowercase())
            .filter(|n| !n.is_empty()),
    }
}

// --------------------------------------------------------- transcription ---

/// Paramètres saisis dans l'interface.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscribeArgs {
    url: String,
    model: String,
    lang: Option<String>,
    translate: bool,
    initial_prompt: Option<String>,
    word_timestamps: bool,
    vad: bool,
    prefer_subs: bool,
    cookies_from_browser: Option<String>,
}

impl TranscribeArgs {
    fn requete(self) -> scripta_core::Result<Request> {
        let url = scripta_core::url::parse(&self.url)?;
        let sidecars = Sidecars::new(
            sidecar_gui(Kind::YtDlp)?.path,
            sidecar_gui(Kind::Ffmpeg)?.path,
        );
        let mut r = Request::new(url, sidecars);
        r.model = ModelChoice::Alias(self.model);
        r.vad = if self.vad {
            VadChoice::Catalogue
        } else {
            VadChoice::Off
        };
        r.lang = self
            .lang
            .map(|l| l.trim().to_lowercase())
            .filter(|l| !l.is_empty() && l != "auto");
        r.translate = self.translate;
        r.initial_prompt = self.initial_prompt;
        r.word_timestamps = self.word_timestamps;
        r.prefer_subs = self.prefer_subs;
        r.access = acces(self.cookies_from_browser);
        Ok(r)
    }
}

/// Transcription achevée, telle que l'interface l'affiche.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionVue {
    /// `cache`, `subtitles`, `autoSubtitles` ou `inference`.
    origin: &'static str,
    title: String,
    channel: Option<String>,
    duration_s: Option<f64>,
    language: Option<String>,
    /// Vitesse relative au temps réel, pour une inférence.
    speed: Option<f64>,
    /// Vrai si les mots sont horodatés : le JSON exporté les portera.
    words: bool,
    segments: Vec<SegmentVue>,
}

impl TranscriptionVue {
    fn de(doc: &Document, origin: Origin) -> Self {
        Self {
            origin: match origin {
                Origin::Cache => "cache",
                Origin::Subtitles { auto: false } => "subtitles",
                Origin::Subtitles { auto: true } => "autoSubtitles",
                Origin::Inference => "inference",
            },
            title: doc.source.title.clone(),
            channel: doc.source.channel.clone(),
            duration_s: doc.source.duration_s,
            language: doc.transcript.language.clone(),
            speed: (origin == Origin::Inference).then_some(doc.run.speed_realtime),
            words: doc.transcript.segments.iter().any(|s| !s.words.is_empty()),
            segments: doc
                .transcript
                .segments
                .iter()
                .map(|s| SegmentVue {
                    start: s.start,
                    end: s.end,
                    text: s.text.trim().to_string(),
                })
                .collect(),
        }
    }
}

/// Transcrit une URL — SPEC §4.2.
///
/// La progression et les segments arrivent par `canal` au fil de l'eau ; le
/// résultat, qui fait foi, à la fin.
#[tauri::command]
pub async fn transcribe(
    app: AppHandle,
    state: State<'_, AppState>,
    args: TranscribeArgs,
    canal: Channel<Vec<Message>>,
) -> IpcResult<TranscriptionVue> {
    let tache = state.commencer().ok_or_else(IpcError::occupe)?;
    en_tache(move || {
        let requete = args.requete()?;
        let etat = app.state::<AppState>();

        let relais = Arc::new(relais_vers(canal));
        let observateur: Arc<dyn Observer> = relais.clone();
        let issue = pipeline::run(&requete, &etat.engines, &observateur, &tache.cancel);
        // Dernier lot livré avant la réponse : aucun segment ne doit arriver
        // après le résultat final.
        drop(observateur);
        drop(relais);

        let resultat = issue?;
        let vue = TranscriptionVue::de(&resultat.document, resultat.origin);
        etat.memoriser(resultat.document);
        Ok(vue)
    })
    .await
}

/// Interrompt la tâche en cours, quelle qu'elle soit.
///
/// Synchrone, donc exécutée sur le thread principal : elle ne fait qu'armer
/// un jeton, sans jamais attendre un verrou tenu par la tâche. `false` si
/// rien ne tournait.
#[tauri::command]
pub fn cancel(state: State<'_, AppState>) -> bool {
    state.annuler()
}

// ---------------------------------------------------------------- export ---

fn format_de(format: &str) -> IpcResult<OutputFormat> {
    format.parse().map_err(IpcError::interne)
}

fn dernier(state: &AppState) -> IpcResult<Document> {
    state
        .dernier()
        .ok_or_else(|| IpcError::interne("aucune transcription à exporter"))
}

/// Nom de fichier proposé : le titre de la vidéo, débarrassé de ce qu'un
/// système de fichiers refuserait.
fn nom_propose(doc: &Document, format: OutputFormat) -> String {
    let titre: String = doc
        .source
        .title
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let titre: String = titre.chars().take(80).collect();
    let titre = titre.trim_end_matches(['.', ' ']);
    let base = if titre.is_empty() {
        "transcription"
    } else {
        titre
    };
    format!("{base}.{}", format.extension())
}

/// Exporte la dernière transcription — SPEC SF-05, tâche 3.9.
///
/// La boîte d'enregistrement est ouverte côté Rust : l'interface n'a ainsi
/// aucun accès au système de fichiers, et aucune commande n'écrit un contenu
/// arbitraire à un chemin arbitraire. Rend le chemin écrit, ou `None` si
/// l'utilisateur a renoncé.
#[tauri::command]
pub async fn export(
    app: AppHandle,
    state: State<'_, AppState>,
    format: String,
) -> IpcResult<Option<String>> {
    let format = format_de(&format)?;
    let doc = dernier(&state)?;
    en_tache(move || {
        let Some(choix) = app
            .dialog()
            .file()
            .set_title("Exporter la transcription")
            .set_file_name(nom_propose(&doc, format))
            .add_filter(format.extension().to_uppercase(), &[format.extension()])
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let chemin = choix.into_path().map_err(IpcError::interne)?;
        let contenu = format.render(&doc, &SubtitleOptions::default());
        // Écrasement accepté : la boîte de dialogue l'a déjà fait confirmer.
        output::write(Path::new(&chemin), &contenu, true)?;
        Ok(Some(chemin.display().to_string()))
    })
    .await
}

/// Copie la dernière transcription, en texte continu, dans le presse-papiers.
#[tauri::command]
pub async fn copy_text(app: AppHandle, state: State<'_, AppState>) -> IpcResult<()> {
    let doc = dernier(&state)?;
    let texte = OutputFormat::Txt.render(&doc, &SubtitleOptions::default());
    app.clipboard().write_text(texte).map_err(IpcError::interne)
}

// ------------------------------------------------------------ extracteur ---

/// Met yt-dlp à jour dans le répertoire utilisateur — SF-06, tâche 3.11.
#[tauri::command]
pub async fn update_extractor(
    state: State<'_, AppState>,
    canal: Channel<Vec<Message>>,
) -> IpcResult<SidecarInfo> {
    let tache = state.commencer().ok_or_else(IpcError::occupe)?;
    en_tache(move || {
        let relais = relais_vers(canal);
        sidecar::update_ytdlp(&mut |received, total| {
            relais.transmettre(Message::Download {
                item: "yt-dlp".to_string(),
                received,
                total,
            })
        })?;
        drop(relais);
        drop(tache);
        Ok(sidecar_info(Kind::YtDlp))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(json: serde_json::Value) -> TranscribeArgs {
        serde_json::from_value(json).expect("arguments valides")
    }

    fn defaut() -> serde_json::Value {
        serde_json::json!({
            "url": "https://youtu.be/jNQXAC9IVRw",
            "model": "auto",
            "lang": null,
            "translate": false,
            "initialPrompt": null,
            "wordTimestamps": false,
            "vad": true,
            "preferSubs": false,
            "cookiesFromBrowser": null
        })
    }

    #[test]
    fn l_interface_et_la_cli_partagent_les_memes_defauts() {
        // Mêmes réglages, même clé de cache : une vidéo transcrite dans l'une
        // des deux interfaces est resservie par l'autre.
        let gui = args(defaut()).requete().unwrap();
        let cli = Request::new(gui.url.clone(), gui.sidecars.clone());
        assert_eq!(
            pipeline::cache_key(&gui).unwrap(),
            pipeline::cache_key(&cli).unwrap()
        );
        assert_eq!(gui.vad, VadChoice::Catalogue);
        assert!(gui.use_cache);
    }

    #[test]
    fn les_champs_vides_valent_leur_absence() {
        let mut json = defaut();
        json["lang"] = "auto".into();
        json["cookiesFromBrowser"] = "  ".into();
        let r = args(json).requete().unwrap();
        assert_eq!(r.lang, None);
        assert_eq!(r.access.cookies_from_browser, None);

        let mut json = defaut();
        json["lang"] = " FR ".into();
        json["cookiesFromBrowser"] = "Firefox".into();
        json["vad"] = false.into();
        let r = args(json).requete().unwrap();
        assert_eq!(r.lang.as_deref(), Some("fr"));
        assert_eq!(r.access.cookies_from_browser.as_deref(), Some("firefox"));
        assert_eq!(r.vad, VadChoice::Off);
    }

    #[test]
    fn une_url_invalide_est_refusee_avant_tout_travail() {
        let mut json = defaut();
        json["url"] = "https://exemple.org/video".into();
        let e = args(json).requete().unwrap_err();
        assert_eq!(e.exit_code(), 10);
    }

    #[test]
    fn le_nom_propose_est_un_nom_de_fichier_valable() {
        let mut doc = Document::default();
        doc.source.title = "Cours n°3 : « Rust » / partie 1/2 ?".into();
        assert_eq!(
            nom_propose(&doc, OutputFormat::Srt),
            "Cours n°3 « Rust » partie 1 2.srt"
        );
        doc.source.title = "…".repeat(200);
        assert!(nom_propose(&doc, OutputFormat::Txt).chars().count() <= 84);
        doc.source.title = " ?? ".into();
        assert_eq!(nom_propose(&doc, OutputFormat::Json), "transcription.json");
    }

    #[test]
    fn l_etat_des_modeles_couvre_le_catalogue() {
        let etat = etat_des_modeles().unwrap();
        assert_eq!(etat.whisper.len(), models::MODELS.len());
        assert_eq!(etat.vad.alias, models::VAD_MODEL.alias);
        assert!(etat.auto.is_some());
        let turbo = etat.whisper.iter().find(|m| m.alias == "turbo").unwrap();
        assert!(!turbo.translates);
    }
}
