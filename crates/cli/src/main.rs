//! Interface en ligne de commande de Scripta — SPEC §4.1.
//!
//! # État — Jalon 2
//!
//! Arborescence complète : `run` (implicite), `subs`, `models`, `cache`,
//! `update-extractor` et `doctor` — sous-commandes plutôt qu'options, correction
//! de la v1 qui plaçait `--update-extractor` en conflit avec un positionnel
//! requis. `run` couvre la chaîne URL → transcription, VAD compris.
//!
//! Les modèles, Whisper comme VAD, sont téléchargés au premier usage et
//! vérifiés par empreinte SHA-256 ; `--model-path` et `--vad-model` restent
//! disponibles pour des fichiers hors catalogue.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use clap::{ArgAction, Parser, Subcommand};
use scripta_core::audio::{self, Sidecars};
use scripta_core::format::{OutputFormat, SubtitleOptions};
use scripta_core::sidecar::UpdateNotice;
use scripta_core::transcribe::NewSegment;
use scripta_core::{
    CancelToken, Document, Engine, ModelSpec, Run, ScriptaError, Source, cache, diagnostic, models,
    output, probe, sidecar, subtitles, transcribe, url,
};

#[derive(Parser)]
#[command(
    name = "scripta",
    version,
    about = "Transcription locale de vidéos YouTube.",
    // `run` implicite : `scripta <URL>` reste la forme courante, sans que les
    // sous-commandes n'entrent en conflit avec le positionnel (SPEC §4.1).
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    /// URL YouTube (forme implicite de `run`).
    url: Option<String>,

    #[command(flatten)]
    run: RunArgs,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Transcrit une URL YouTube.
    Run {
        url: String,
        // Boxé : `RunArgs` pèse plusieurs centaines d'octets et `Doctor` est
        // vide ; sans indirection, chaque variante de l'énuméré porterait ce
        // poids.
        #[command(flatten)]
        args: Box<RunArgs>,
    },
    /// Récupère uniquement les sous-titres YouTube officiels.
    Subs {
        url: String,
        #[command(flatten)]
        args: Box<RunArgs>,
    },
    /// Gère le cache de modèles.
    Models {
        #[command(subcommand)]
        action: ModelsAction,
    },
    /// Gère le cache de transcriptions.
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Met à jour le binaire yt-dlp.
    ///
    /// L'installation se fait dans un répertoire utilisateur : remplacer un
    /// binaire dans un bundle signé en invaliderait la signature.
    UpdateExtractor,
    /// Diagnostic : sidecars, backend, modèles, répertoires, réseau.
    Doctor,
}

#[derive(Subcommand)]
enum ModelsAction {
    /// Liste les modèles et leur état local.
    List,
    /// Télécharge un modèle et vérifie son empreinte.
    ///
    /// Accepte aussi « silero », le modèle VAD.
    Pull { model: String },
    /// Supprime un modèle du cache.
    Rm { model: String },
    /// Affiche le répertoire de cache.
    Path,
    /// Recalcule l'empreinte d'un modèle installé.
    Verify { model: String },
}

#[derive(Subcommand)]
enum CacheAction {
    /// Liste les transcriptions en cache.
    List,
    /// Vide le cache.
    Clear,
    /// Affiche le répertoire de cache.
    Path,
}

#[derive(clap::Args, Clone)]
struct RunArgs {
    /// Fichier de sortie [défaut : stdout].
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Écrase le fichier de sortie s'il existe déjà.
    ///
    /// Sans elle, un fichier existant est protégé, et l'erreur tombe avant
    /// toute transcription plutôt qu'après.
    #[arg(long)]
    force: bool,

    /// Format de sortie : txt, srt, vtt ou json.
    ///
    /// Validé par clap : un format inconnu est une erreur d'usage (code 2),
    /// signalée avant tout travail réseau.
    #[arg(
        short,
        long,
        default_value = "txt",
        value_parser = parse_format,
        long_help = FORMAT_LONG_HELP
    )]
    format: OutputFormat,

    /// Modèle : auto, tiny, base, small, medium, large-v3, turbo.
    ///
    /// Téléchargé à la demande et vérifié par empreinte SHA-256.
    #[arg(short, long, default_value = "auto")]
    model: String,

    /// Chemin explicite du modèle, prioritaire sur --model.
    #[arg(long)]
    model_path: Option<PathBuf>,

    /// Active la détection d'activité vocale (défaut).
    #[arg(long, overrides_with = "no_vad")]
    vad: bool,

    /// Désactive la détection d'activité vocale.
    ///
    /// Le VAD écarte silences, génériques et bruits de fond, sur lesquels
    /// Whisper hallucine. À désactiver si un contenu chanté ou très musical
    /// perd des passages.
    #[arg(long, overrides_with = "vad")]
    no_vad: bool,

    /// Modèle VAD Silero explicite, à la place de celui du cache.
    #[arg(long, value_name = "CHEMIN", conflicts_with = "no_vad")]
    vad_model: Option<PathBuf>,

    /// Utilise les sous-titres YouTube officiels s'ils existent.
    ///
    /// Instantané, mais les pistes auto-générées sont dépourvues de
    /// ponctuation dans de nombreuses langues et restent en deçà de Whisper.
    /// En cas d'échec, repli silencieux sur la transcription.
    #[arg(long)]
    prefer_subs: bool,

    /// Lit les cookies du navigateur indiqué (firefox, chrome, edge…).
    ///
    /// Nécessaire pour les vidéos soumises à limite d'âge ou à vérification
    /// anti-robot. Les cookies ne servent qu'à youtube.com, ne sont jamais
    /// écrits sur disque ni journalisés.
    #[arg(long, value_name = "NAVIGATEUR")]
    cookies_from_browser: Option<String>,

    /// Langue forcée (fr, en, es…). Détection automatique par défaut.
    #[arg(short, long)]
    lang: Option<String>,

    /// Traduit vers l'anglais (incompatible avec les modèles « turbo »).
    #[arg(long)]
    translate: bool,

    /// Contexte guidant le modèle sur les noms propres et le jargon.
    #[arg(long)]
    initial_prompt: Option<String>,

    /// Seuil de probabilité d'absence de parole, entre 0 et 1.
    ///
    /// Un segment jugé muet au-delà de ce seuil est écarté. Défaut de
    /// whisper.cpp : 0.6.
    #[arg(long, value_name = "SEUIL", value_parser = parse_probabilite)]
    no_speech_thold: Option<f32>,

    /// Seuil d'entropie sous lequel un décodage, jugé répétitif, est repris.
    ///
    /// Défaut de whisper.cpp : 2.4.
    #[arg(long, value_name = "SEUIL", value_parser = parse_seuil_positif)]
    entropy_thold: Option<f32>,

    /// Nombre de threads d'inférence.
    #[arg(short, long)]
    threads: Option<usize>,

    /// Largeur maximale d'une ligne de sous-titre, en caractères.
    #[arg(long, default_value_t = 42)]
    max_line_width: usize,

    /// Nombre maximal de lignes par sous-titre.
    #[arg(long, default_value_t = 2)]
    max_line_count: usize,

    /// Horodatage au mot (requis pour un JSON enrichi).
    #[arg(long)]
    word_timestamps: bool,

    /// Refus au-delà de cette durée, en minutes.
    #[arg(long, default_value_t = 240)]
    max_duration: u64,

    /// Chemin explicite du binaire yt-dlp.
    ///
    /// Sans lui, la résolution suit ADR-004 : copie utilisateur mise à jour,
    /// puis copie embarquée, puis PATH.
    #[arg(long)]
    ytdlp_path: Option<PathBuf>,

    /// Chemin explicite du binaire ffmpeg.
    #[arg(long)]
    ffmpeg_path: Option<PathBuf>,

    /// Ignore le cache de transcriptions, en lecture comme en écriture.
    #[arg(long)]
    no_cache: bool,

    /// Supprime les messages de progression.
    #[arg(short, long)]
    quiet: bool,

    /// Détaille le déroulement : chemins résolus, clé de cache, journaux de
    /// whisper.cpp.
    #[arg(short, long, action = ArgAction::Count, conflicts_with = "quiet")]
    verbose: u8,
}

impl RunArgs {
    /// Le VAD est actif par défaut (SPEC SF-04) ; `--no-vad` le désactive, et
    /// le dernier des deux drapeaux l'emporte.
    fn vad_actif(&self) -> bool {
        self.vad || !self.no_vad
    }

    /// Contexte normalisé : vide ou blanc, il équivaut à son absence — et ne
    /// doit pas produire une clé de cache distincte.
    fn contexte(&self) -> Option<String> {
        self.initial_prompt
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
    }

    /// Le JSON porte toujours les mots (SF-05).
    fn mots_horodates(&self) -> bool {
        self.word_timestamps || self.format == OutputFormat::Json
    }

    fn affichage(&self) -> Affichage {
        Affichage::de(self.quiet)
    }

    /// Diagnostic de `--verbose`, sur `stderr` comme tout le reste.
    fn detail(&self, msg: &str) {
        if self.verbose > 0 {
            eprintln!("  · {msg}");
        }
    }
}

/// Manière de rendre compte d'une opération longue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Affichage {
    /// `--quiet`.
    Muet,
    /// `stderr` redirigé (CI, fichier) : des lignes, jamais de retour chariot
    /// qui transformerait un journal en bouillie (SPEC §4.1).
    Lignes,
    /// `stderr` est un terminal : barre de progression.
    Barre,
}

impl Affichage {
    fn de(quiet: bool) -> Self {
        if quiet {
            Self::Muet
        } else if std::io::stderr().is_terminal() {
            Self::Barre
        } else {
            Self::Lignes
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Vérification de mise à jour de yt-dlp — SPEC SF-06. Lancée d'emblée et en
    // arrière-plan, pour les seules commandes qui se servent de yt-dlp.
    let mise_a_jour = match (&cli.command, &cli.url) {
        (Some(Command::Run { args, .. } | Command::Subs { args, .. }), _) => {
            verification_mise_a_jour(args)
        }
        (None, Some(_)) => verification_mise_a_jour(&cli.run),
        _ => None,
    };

    let result = match (cli.command, cli.url) {
        (Some(Command::Doctor), _) => doctor(),
        (Some(Command::UpdateExtractor), _) => update_extractor(),
        (Some(Command::Models { action }), _) => models_cmd(&action),
        (Some(Command::Cache { action }), _) => cache_cmd(&action),
        (Some(Command::Subs { url, args }), _) => subs(&url, &args),
        (Some(Command::Run { url, args }), _) => run(&url, &args),
        (None, Some(url)) => run(&url, &cli.run),
        (None, None) => {
            eprintln!("Erreur : aucune URL fournie.\n\nEssayez « scripta --help ».");
            return ExitCode::from(2);
        }
    };

    let code = match &result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Tout diagnostic sur stderr : stdout ne transporte que le résultat,
            // afin que `scripta <URL> | …` reste utilisable (SPEC §4.1).
            eprintln!("Erreur : {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    };

    // Après l'erreur éventuelle, dont un extracteur périmé est souvent la cause.
    if let Some(avis) = mise_a_jour.and_then(|rx| rx.try_recv().ok()) {
        eprintln!(
            "\nyt-dlp {} est disponible (installé : {}). Pour le mettre à jour : \
             scripta update-extractor",
            avis.latest, avis.current
        );
    }

    code
}

/// Lance la vérification de mise à jour, sauf en `--quiet` : son seul effet
/// visible est un message, que `--quiet` supprimerait de toute façon.
fn verification_mise_a_jour(args: &RunArgs) -> Option<mpsc::Receiver<UpdateNotice>> {
    if args.quiet {
        return None;
    }
    sidecar::spawn_update_check(ytdlp_of(args))
}

/// Installe le gestionnaire d'interruption — SPEC §4.1, tâche 2.8.
///
/// Installé **dès le début** de la commande, pour que `Ctrl-C` sorte en 130
/// quelle que soit l'étape : téléchargement d'un modèle, sonde, extraction ou
/// inférence. Deux comportements selon le moment :
///
/// - **Premier `Ctrl-C`** : arme le jeton d'annulation. Chaque étape l'interroge
///   et rend la main proprement, ce qui laisse le programme nettoyer et sortir
///   en 130.
/// - **Second `Ctrl-C`** : terminaison immédiate. Si la première demande n'a pas
///   abouti — sidecar bloqué, fenêtre d'inférence anormalement longue —
///   l'utilisateur doit pouvoir reprendre son terminal sans attendre.
///
/// Un échec d'installation n'est pas fatal : le programme reste utilisable,
/// simplement moins docile à l'interruption.
fn install_interrupt_handler(cancel: CancelToken, quiet: bool) {
    let deja_demande = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let resultat = ctrlc::set_handler(move || {
        if deja_demande.swap(true, std::sync::atomic::Ordering::SeqCst) {
            if !quiet {
                eprintln!("\nInterruption forcée.");
            }
            // 128 + SIGINT, convention respectée jusque dans la sortie brutale.
            std::process::exit(130);
        }
        if !quiet {
            eprintln!("\nInterruption demandée… (Ctrl-C à nouveau pour forcer)");
        }
        cancel.cancel();
    });

    if resultat.is_err() && !quiet {
        eprintln!("Avertissement : gestionnaire d'interruption non installé.");
    }
}

/// Dans une console, `Ctrl-C` est délivré aux sidecars aussi : leur échec n'est
/// alors que la conséquence de l'interruption, et c'est elle qu'il faut
/// rapporter (code 130), pas une panne d'extraction.
fn verifier_interruption(cancel: &CancelToken) -> scripta_core::Result<()> {
    if cancel.is_cancelled() {
        Err(ScriptaError::Interrupted)
    } else {
        Ok(())
    }
}

/// Sous la minute, afficher « 0 min » est absurde.
fn format_duree(secondes: f64) -> String {
    if secondes < 60.0 {
        format!("{secondes:.0} s")
    } else {
        format!("{:.0} min", secondes / 60.0)
    }
}

/// Position dans une vidéo : `m:ss`, ou `h:mm:ss` au-delà de l'heure.
fn horodatage(secondes: f64) -> String {
    let s = secondes.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Un `value_parser` personnalisé prive clap de la liste des valeurs
/// possibles : il ne peut pas la déduire d'une fonction. Elle est donc
/// énumérée ici.
const FORMAT_LONG_HELP: &str = "Format de sortie.

  txt   texte continu, sans horodatage
  srt   sous-titres SubRip
  vtt   sous-titres WebVTT
  json  document enrichi (horodatage au mot inclus)";

fn parse_format(s: &str) -> Result<OutputFormat, String> {
    s.parse()
}

fn parse_probabilite(s: &str) -> Result<f32, String> {
    match s.parse::<f32>() {
        Ok(v) if (0.0..=1.0).contains(&v) => Ok(v),
        _ => Err(format!("« {s} » : attendu un nombre entre 0 et 1")),
    }
}

fn parse_seuil_positif(s: &str) -> Result<f32, String> {
    match s.parse::<f32>() {
        Ok(v) if v.is_finite() && v > 0.0 => Ok(v),
        _ => Err(format!("« {s} » : attendu un nombre positif")),
    }
}

/// Résout les deux sidecars selon ADR-004.
fn sidecars_of(args: &RunArgs) -> Sidecars {
    let yt = sidecar::resolve(sidecar::Kind::YtDlp, args.ytdlp_path.as_deref());
    let ff = sidecar::resolve(sidecar::Kind::Ffmpeg, args.ffmpeg_path.as_deref());
    args.detail(&format!(
        "yt-dlp : {} ({})",
        yt.path.display(),
        yt.origin.as_str()
    ));
    args.detail(&format!(
        "ffmpeg : {} ({})",
        ff.path.display(),
        ff.origin.as_str()
    ));
    Sidecars::new(yt.path, ff.path)
}

fn ytdlp_of(args: &RunArgs) -> PathBuf {
    sidecar::resolve(sidecar::Kind::YtDlp, args.ytdlp_path.as_deref()).path
}

fn access_of(args: &RunArgs) -> scripta_core::Access {
    scripta_core::Access {
        cookies_from_browser: args.cookies_from_browser.clone(),
    }
}

fn sous_titres_options(args: &RunArgs) -> SubtitleOptions {
    SubtitleOptions {
        max_line_width: args.max_line_width,
        max_line_count: args.max_line_count,
        ..Default::default()
    }
}

/// Identifiant du modèle **sans** le charger ni le télécharger.
///
/// Il entre dans la clé de cache, qu'il faut pouvoir calculer avant toute
/// opération coûteuse : sur un succès de cache, ni le modèle ni le réseau ne
/// sont nécessaires.
fn model_id_of(args: &RunArgs) -> scripta_core::Result<String> {
    let fichier = match &args.model_path {
        Some(p) => p.clone(),
        None => PathBuf::from(resolve_alias(&args.model)?.file),
    };
    Ok(fichier
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "inconnu".to_string()))
}

/// Modèle chargé, avec le modèle VAD prêt s'il est actif.
struct Moteur {
    engine: Engine,
    vad: Option<PathBuf>,
}

fn charger_moteur(args: &RunArgs, cancel: &CancelToken) -> scripta_core::Result<Moteur> {
    let chemin = resolve_model(args, cancel)?;
    let vad = resolve_vad(args, cancel)?;
    if !args.quiet {
        eprintln!("Chargement du modèle ({})…", chemin.display());
    }
    match &vad {
        Some(p) => args.detail(&format!("VAD : {}", p.display())),
        None => args.detail("VAD : désactivé"),
    }
    let engine = Engine::load(&chemin)?;
    Ok(Moteur { engine, vad })
}

fn document_de(
    url: &scripta_core::CanonicalUrl,
    meta: &probe::Metadata,
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

/// Tente la récupération des sous-titres officiels — SPEC SF-01.
///
/// Retourne `None` plutôt qu'une erreur en cas d'absence ou d'échec : la
/// spécification impose un **repli silencieux** sur la transcription. Les
/// endpoints de sous-titres de YouTube sont fréquemment limités en débit, et
/// un `--prefer-subs` ne doit jamais faire échouer une commande qui aurait
/// abouti sans lui.
fn recuperer_sous_titres(
    meta: &probe::Metadata,
    args: &RunArgs,
) -> Option<scripta_core::Transcript> {
    let piste = subtitles::best_track(meta, args.lang.as_deref())?;

    if !args.quiet {
        eprintln!(
            "Sous-titres {} trouvés ({}).",
            if piste.auto {
                "auto-générés"
            } else {
                "officiels"
            },
            piste.lang
        );
        if piste.auto {
            eprintln!(
                "  Attention : une piste auto-générée est souvent sans ponctuation\n\
                   et en deçà de Whisper. Retirez --prefer-subs pour transcrire."
            );
        }
        if subtitles::is_translation(&piste, meta) {
            eprintln!(
                "  Attention : traduction automatique, non la transcription d'origine\n\
                   ({} d'après YouTube). Deux passages machine se cumulent.",
                meta.language.as_deref().unwrap_or("?")
            );
        }
    }

    match subtitles::fetch(&piste) {
        Ok(t) => Some(t),
        Err(e) => {
            if !args.quiet {
                eprintln!("  Échec : {e}");
            }
            None
        }
    }
}

/// Sous-commande `subs` : sous-titres officiels **uniquement**.
///
/// À la différence de `--prefer-subs`, leur absence est ici une erreur : c'est
/// exactement ce que l'utilisateur a demandé, et un repli silencieux sur
/// trente minutes d'inférence serait une surprise désagréable.
fn subs(raw_url: &str, args: &RunArgs) -> scripta_core::Result<()> {
    let url = url::parse(raw_url)?;
    if let Some(p) = &args.output {
        output::check_target(p, args.force)?;
    }

    let cancel = CancelToken::new();
    install_interrupt_handler(cancel.clone(), args.quiet);
    let access = access_of(args);

    let meta = probe::probe(&ytdlp_of(args), &url, &access);
    verifier_interruption(&cancel)?;
    let meta = meta?;
    probe::check_admissible(&meta, args.max_duration)?;

    let piste = subtitles::best_track(&meta, args.lang.as_deref()).ok_or_else(|| {
        let dispo = meta.available_subtitle_langs().join(", ");
        ScriptaError::ExtractionFailed {
            detail: match (args.lang.as_deref(), dispo.is_empty()) {
                (_, true) => "cette vidéo n'a aucun sous-titre".to_string(),
                (Some(l), false) => format!("aucun sous-titre en « {l} » (disponibles : {dispo})"),
                (None, false) => format!("aucune piste exploitable (langues : {dispo})"),
            },
        }
    })?;

    let transcript = subtitles::fetch(&piste);
    verifier_interruption(&cancel)?;
    let doc = document_de(&url, &meta, transcript?, Run::default());
    ecrire(args, &doc)
}

fn run(raw_url: &str, args: &RunArgs) -> scripta_core::Result<()> {
    let url = url::parse(raw_url)?;
    // Avant tout travail : découvrir après vingt minutes d'inférence qu'on ne
    // peut pas écrire le résultat serait absurde (SPEC §5.1).
    if let Some(p) = &args.output {
        output::check_target(p, args.force)?;
    }
    if url.had_playlist() && !args.quiet {
        eprintln!("Avertissement : paramètre de playlist ignoré, seule la vidéo est traitée.");
    }
    if args.verbose > 0 {
        transcribe::enable_native_logs();
    }

    let cancel = CancelToken::new();
    install_interrupt_handler(cancel.clone(), args.quiet);

    let progress = |msg: &str| {
        if !args.quiet {
            eprintln!("{msg}");
        }
    };

    let access = access_of(args);
    let model_id = model_id_of(args)?;
    // Avant tout téléchargement : récupérer 570 Mo de `turbo` pour refuser
    // ensuite la traduction serait un gâchis (SPEC SF-04).
    transcribe::check_translate_supported(&model_id, args.translate)?;

    // La clé réunit tout ce qui influe sur le résultat. En omettre un seul
    // élément ferait resservir une transcription obtenue dans d'autres
    // conditions — un défaut silencieux, donc le pire (SPEC SF-08).
    let clef = cache::Key {
        video_id: url.video_id().to_string(),
        model: model_id,
        lang: args.lang.clone(),
        translate: args.translate,
        vad: args.vad_actif(),
        word_timestamps: args.mots_horodates(),
        initial_prompt: args.contexte(),
        no_speech_thold: args.no_speech_thold,
        entropy_thold: args.entropy_thold,
    };
    args.detail(&format!("clé de cache : {}", clef.digest()));

    // Consultation avant tout le reste : sur un succès, ni le modèle ni le
    // réseau ne sont sollicités.
    if !args.no_cache
        && let Some(doc) = cache::get(&clef)
    {
        progress("Transcription trouvée en cache.");
        return ecrire(args, &doc);
    }

    // Ordre dicté par `--prefer-subs`. Sans lui, le modèle est résolu en
    // premier pour qu'un modèle absent échoue immédiatement plutôt qu'après
    // plusieurs minutes de téléchargement. Avec lui, il se peut qu'aucun modèle
    // ne soit nécessaire : il serait absurde d'en télécharger un pour rien.
    let moteur = if args.prefer_subs {
        None
    } else {
        Some(charger_moteur(args, &cancel)?)
    };

    progress("Sonde des métadonnées…");
    let meta = probe::probe(&ytdlp_of(args), &url, &access);
    verifier_interruption(&cancel)?;
    let meta = meta?;
    probe::check_admissible(&meta, args.max_duration)?;

    if !args.quiet {
        let duree = meta
            .duration
            .map(format_duree)
            .unwrap_or_else(|| "durée inconnue".to_string());
        eprintln!("« {} » — {duree}", meta.title);
    }

    if args.prefer_subs {
        let sous_titres = recuperer_sous_titres(&meta, args);
        verifier_interruption(&cancel)?;
        match sous_titres {
            Some(transcript) => {
                let doc = document_de(&url, &meta, transcript, Run::default());
                return ecrire(args, &doc);
            }
            None => progress("Pas de sous-titres exploitables : repli sur la transcription."),
        }
    }

    let moteur = match moteur {
        Some(m) => m,
        None => charger_moteur(args, &cancel)?,
    };

    progress("Extraction audio…");
    let samples = audio::extract(
        &sidecars_of(args),
        &url,
        meta.duration,
        &access,
        Some(&cancel),
    )?;
    // Relevée avant l'inférence, qui consomme le tampon.
    let audio_s = transcribe::duration_of(&samples);
    progress(&format!(
        "{} échantillons extraits ({audio_s:.1} s à {} Hz).",
        samples.len(),
        audio::SAMPLE_RATE
    ));

    let options = transcribe::Options {
        language: args.lang.clone(),
        translate: args.translate,
        threads: args.threads.unwrap_or_else(transcribe::default_threads),
        initial_prompt: args.contexte(),
        word_timestamps: args.mots_horodates(),
        vad_model: moteur.vad.clone(),
        no_speech_thold: args.no_speech_thold,
        entropy_thold: args.entropy_thold,
    };
    args.detail(&format!("threads : {}", options.threads));

    let barre = match args.affichage() {
        Affichage::Barre => Some(BarreTranscription::new(meta.duration.unwrap_or(audio_s))),
        Affichage::Lignes => {
            progress("Transcription…");
            None
        }
        Affichage::Muet => None,
    };
    let hooks = transcribe::Hooks {
        on_progress: barre.as_ref().map(BarreTranscription::rappel_progression),
        on_segment: barre.as_ref().map(BarreTranscription::rappel_segment),
        cancel: Some(cancel.clone()),
    };

    let debut = Instant::now();
    let resultat = moteur.engine.transcribe(samples, &options, hooks);
    if let Some(b) = &barre {
        b.effacer();
    }
    let transcript = resultat?;
    let ecoule = debut.elapsed();
    let vitesse = if ecoule.as_secs_f64() > 0.0 {
        audio_s / ecoule.as_secs_f64()
    } else {
        0.0
    };

    if !args.quiet {
        eprintln!(
            "Transcription terminée en {:.1} s ({vitesse:.1}× temps réel, {} segments, langue : {}).",
            ecoule.as_secs_f64(),
            transcript.segments.len(),
            transcript.language.as_deref().unwrap_or("?")
        );
        if transcript.is_empty() {
            eprintln!("Avertissement : aucune parole détectée, la transcription est vide.");
        }
    }

    let doc = Document {
        source: Source {
            url: url.as_str(),
            video_id: url.video_id().to_string(),
            title: meta.title.clone(),
            channel: meta.channel.clone(),
            duration_s: meta.duration,
            upload_date: meta.upload_date.clone(),
        },
        run: Run {
            model: moteur.engine.model_id().to_string(),
            backend: scripta_core::Backend::compiled().as_str().to_string(),
            language: transcript.language.clone(),
            language_probability: transcript.language_probability,
            translated: options.translate,
            vad: options.vad_model.is_some(),
            duration_ms: ecoule.as_millis() as u64,
            speed_realtime: (vitesse * 10.0).round() / 10.0,
            ..Default::default()
        },
        transcript,
    };

    if !args.no_cache {
        // Un échec de mise en cache ne doit jamais faire échouer une
        // transcription réussie : le résultat est là, seule sa réutilisation
        // future serait perdue.
        match cache::put(&clef, &doc) {
            Ok(()) => {
                let _ = cache::evict(cache::DEFAULT_MAX_BYTES);
            }
            Err(e) if !args.quiet => eprintln!("Avertissement : mise en cache impossible ({e})."),
            Err(_) => {}
        }
    }

    ecrire(args, &doc)
}

/// Barre de progression de l'inférence — SPEC SF-04.
///
/// Combine le pourcentage de whisper.cpp et l'horodatage du dernier segment
/// émis : position dans la vidéo, et vitesse relative au temps réel. Les deux
/// rappels arrivent du thread d'inférence ; l'état est donc partagé.
#[derive(Clone)]
struct BarreTranscription(Arc<Mutex<EtatBarre>>);

struct EtatBarre {
    debut: Instant,
    /// Durée de la vidéo, en secondes.
    duree: f64,
    pourcent: i32,
    /// Fin du dernier segment émis, sur la chronologie de la vidéo.
    position: Option<f64>,
    /// Largeur de la ligne précédente, pour l'effacer sans séquence ANSI —
    /// que la console Windows classique n'interprète pas.
    largeur: usize,
}

impl BarreTranscription {
    fn new(duree: f64) -> Self {
        Self(Arc::new(Mutex::new(EtatBarre {
            debut: Instant::now(),
            duree,
            pourcent: -1,
            position: None,
            largeur: 0,
        })))
    }

    fn rappel_progression(&self) -> transcribe::ProgressHook {
        let barre = self.clone();
        Box::new(move |p: i32| {
            barre.maj(|e| {
                // whisper.cpp rappelle bien plus souvent qu'à chaque pourcent,
                // et chaque rappel coûterait une écriture terminal.
                let change = p > e.pourcent;
                e.pourcent = e.pourcent.max(p);
                change
            });
        })
    }

    fn rappel_segment(&self) -> transcribe::SegmentHook {
        let barre = self.clone();
        Box::new(move |s: &NewSegment| {
            barre.maj(|e| {
                e.position = Some(s.end);
                true
            });
        })
    }

    fn maj(&self, f: impl FnOnce(&mut EtatBarre) -> bool) {
        // Un verrou empoisonné ne doit pas interrompre une transcription pour
        // une question d'affichage.
        let Ok(mut etat) = self.0.lock() else { return };
        if f(&mut etat) {
            etat.afficher();
        }
    }

    fn effacer(&self) {
        if let Ok(etat) = self.0.lock()
            && etat.largeur > 0
        {
            eprint!("\r{:largeur$}\r", "", largeur = etat.largeur);
        }
    }
}

impl EtatBarre {
    /// Contenu de la barre, `ecoule` secondes après le début de l'inférence.
    fn ligne(&self, ecoule: f64) -> String {
        let mut ligne = format!("Transcription : {:>3} %", self.pourcent.max(0));
        if let Some(position) = self.position {
            ligne.push_str(&format!(
                "  ·  {} / {}",
                horodatage(position),
                horodatage(self.duree)
            ));
            // Sous la seconde, la vitesse n'a pas de sens : le premier segment
            // arrive d'un bloc.
            if ecoule >= 1.0 {
                ligne.push_str(&format!("  ·  {:.1}× temps réel", position / ecoule));
            }
        }
        ligne
    }

    fn afficher(&mut self) {
        let ligne = self.ligne(self.debut.elapsed().as_secs_f64());
        let largeur = ligne.chars().count();
        let bourrage = self.largeur.saturating_sub(largeur);
        eprint!("\r{ligne}{:bourrage$}", "");
        let _ = std::io::stderr().flush();
        self.largeur = largeur;
    }
}

/// Résout le modèle et garantit sa présence locale — SPEC SF-03.
///
/// `--model-path` court-circuite tout : il sert aux modèles absents du
/// catalogue, ou déposés à la main.
fn resolve_model(args: &RunArgs, cancel: &CancelToken) -> scripta_core::Result<PathBuf> {
    if let Some(p) = &args.model_path {
        return Ok(p.clone());
    }
    fetch_model(resolve_alias(&args.model)?, args.affichage(), Some(cancel))
}

/// Modèle VAD à utiliser — SPEC SF-03, SF-04.
///
/// Actif par défaut et téléchargé au premier usage, comme les modèles Whisper.
/// Un échec est une erreur (code 30), jamais une désactivation silencieuse :
/// le résultat, et sa clé de cache, en dépendent.
fn resolve_vad(args: &RunArgs, cancel: &CancelToken) -> scripta_core::Result<Option<PathBuf>> {
    if !args.vad_actif() {
        return Ok(None);
    }
    if let Some(p) = &args.vad_model {
        return Ok(Some(p.clone()));
    }
    fetch_model(&models::VAD_MODEL, args.affichage(), Some(cancel))
        .map(Some)
        .map_err(|e| match e {
            ScriptaError::ModelUnavailable { detail } => ScriptaError::ModelUnavailable {
                detail: format!("{detail} — relancez avec --no-vad pour vous en passer"),
            },
            autre => autre,
        })
}

/// Télécharge un modèle si besoin — SPEC SF-03.
///
/// Rien n'est imprimé quand le modèle est déjà en cache : le cas nominal doit
/// rester silencieux.
fn fetch_model(
    spec: &ModelSpec,
    affichage: Affichage,
    cancel: Option<&CancelToken>,
) -> scripta_core::Result<PathBuf> {
    let mut annonce = false;
    let mut dernier = u64::MAX;

    let mut progression = |recus: u64, total: u64| {
        if affichage == Affichage::Muet || total == 0 {
            return;
        }
        if !annonce {
            eprintln!(
                "Téléchargement de {} ({})…",
                spec.file,
                taille_lisible(spec.size)
            );
            annonce = true;
        }
        if affichage == Affichage::Barre {
            let pourcent = recus * 100 / total;
            if pourcent != dernier {
                dernier = pourcent;
                eprint!(
                    "\r  {pourcent:>3} %  ({} / {} Mo)",
                    recus / 1_048_576,
                    total / 1_048_576
                );
            }
        }
        // Le hachage d'un gros modèle prend quelques secondes : autant le dire.
        if recus >= total {
            if affichage == Affichage::Barre {
                eprintln!();
            }
            eprintln!("  Vérification de l'empreinte…");
        }
    };

    models::ensure(spec, &mut progression, cancel)
}

fn models_cmd(action: &ModelsAction) -> scripta_core::Result<()> {
    match action {
        ModelsAction::Path => {
            println!("{}", models::models_dir()?.display());
        }
        ModelsAction::List => {
            let dir = models::models_dir()?;
            println!("Cache : {}\n", dir.display());
            // Les en-têtes passent par des variables : clippy refuse les
            // littéraux en arguments de format, et le gabarit doit rester
            // identique à celui des lignes pour que les colonnes s'alignent.
            let (alias, taille, fichier, etat) = ("ALIAS", "TAILLE", "FICHIER", "ÉTAT");
            println!("  {alias:<10} {taille:>9}  {fichier:<30} {etat}");
            let ligne = |spec: &ModelSpec, present: bool| {
                println!(
                    "  {:<10} {:>9}  {:<30} {}",
                    spec.alias,
                    taille_lisible(spec.size),
                    spec.file,
                    if present { "installé" } else { "-" }
                );
            };
            for (spec, present) in models::installed()? {
                ligne(spec, present);
            }
            // Le modèle VAD à part : il ne se choisit pas avec --model.
            println!("\n  Détection d'activité vocale (VAD, active par défaut) :");
            ligne(
                &models::VAD_MODEL,
                models::is_installed(&models::VAD_MODEL)?,
            );
        }
        ModelsAction::Pull { model } => {
            let spec = resolve_any_alias(model)?;
            let chemin = fetch_model(spec, Affichage::de(false), None)?;
            println!("{}", chemin.display());
        }
        ModelsAction::Rm { model } => {
            let spec = resolve_any_alias(model)?;
            if models::remove(spec)? {
                eprintln!("Supprimé : {}", spec.file);
            } else {
                eprintln!("Absent du cache : {}", spec.file);
            }
        }
        ModelsAction::Verify { model } => {
            let spec = resolve_any_alias(model)?;
            if models::verify(spec)? {
                eprintln!("Empreinte conforme : {}", spec.file);
            } else {
                return Err(ScriptaError::ModelUnavailable {
                    detail: format!("{} absent ou corrompu", spec.file),
                });
            }
        }
    }
    Ok(())
}

fn taille_lisible(octets: u64) -> String {
    match octets {
        n if n >= 1_048_576 => format!("{:.1} Mo", n as f64 / 1_048_576.0),
        n if n >= 1024 => format!("{} Ko", n / 1024),
        n => format!("{n} o"),
    }
}

fn cache_cmd(action: &CacheAction) -> scripta_core::Result<()> {
    match action {
        CacheAction::Path => println!("{}", cache::cache_dir()?.display()),
        CacheAction::Clear => {
            let n = cache::clear()?;
            eprintln!("{n} transcription(s) supprimée(s).");
        }
        CacheAction::List => {
            let entrees = cache::list()?;
            println!("Cache : {}\n", cache::cache_dir()?.display());
            if entrees.is_empty() {
                println!("  (vide)");
                return Ok(());
            }
            for e in &entrees {
                println!("  {:>7}  {}", taille_lisible(e.bytes), e.title);
            }
            println!(
                "\n  {} entrée(s), {:.1} Mo",
                entrees.len(),
                cache::total_bytes()? as f64 / 1_048_576.0
            );
        }
    }
    Ok(())
}

/// Alias d'un modèle **de transcription** — `--model`.
fn resolve_alias(alias: &str) -> scripta_core::Result<&'static ModelSpec> {
    models::find(alias).ok_or_else(|| ScriptaError::ModelUnavailable {
        detail: format!(
            "modèle « {alias} » inconnu (disponibles : auto, {})",
            models::aliases().join(", ")
        ),
    })
}

/// Alias de tout le catalogue, modèle VAD compris — `scripta models`.
fn resolve_any_alias(alias: &str) -> scripta_core::Result<&'static ModelSpec> {
    models::find_any(alias).ok_or_else(|| ScriptaError::ModelUnavailable {
        detail: format!(
            "modèle « {alias} » inconnu (disponibles : {}, {})",
            models::aliases().join(", "),
            models::VAD_MODEL.alias
        ),
    })
}

/// Rend le document dans le format demandé et l'écrit — SPEC SF-05, §5.1.
fn ecrire(args: &RunArgs, doc: &Document) -> scripta_core::Result<()> {
    let contenu = args.format.render(doc, &sous_titres_options(args));
    match &args.output {
        Some(p) => output::write(p, &contenu, args.force),
        None => std::io::stdout()
            .write_all(contenu.as_bytes())
            .map_err(|source| ScriptaError::OutputFailed {
                path: PathBuf::from("<stdout>"),
                source,
            }),
    }
}

fn update_extractor() -> scripta_core::Result<()> {
    let affichage = Affichage::de(false);
    let avant = sidecar::resolve(sidecar::Kind::YtDlp, None);
    let version_avant = sidecar::version_of(&avant.path, sidecar::Kind::YtDlp);

    eprintln!(
        "Version actuelle : {} ({})",
        version_avant.as_deref().unwrap_or("inconnue"),
        avant.origin.as_str()
    );

    let mut annonce = false;
    let mut dernier = u64::MAX;
    let chemin = sidecar::update_ytdlp(&mut |recus: u64, total: u64| {
        if total == 0 {
            return;
        }
        if !annonce {
            eprintln!("Téléchargement de la dernière version…");
            annonce = true;
        }
        let pourcent = recus * 100 / total;
        if affichage == Affichage::Barre && pourcent != dernier {
            dernier = pourcent;
            eprint!("\r  {pourcent:>3} %");
        }
    })?;
    if affichage == Affichage::Barre && annonce {
        eprintln!();
    }
    eprintln!("  Empreinte vérifiée.");

    let apres = sidecar::version_of(&chemin, sidecar::Kind::YtDlp);
    eprintln!(
        "Installé : {} → {}",
        chemin.display(),
        apres.as_deref().unwrap_or("version inconnue")
    );

    if version_avant.is_some() && version_avant == apres {
        eprintln!("Déjà à jour.");
    }
    Ok(())
}

/// Diagnostic d'installation — SPEC SF-07.
///
/// Va toujours jusqu'au bout et rend 0 : c'est un rapport, pas une
/// vérification bloquante. Chaque problème est compté, et le bilan final dit
/// s'il y a lieu d'agir.
fn doctor() -> scripta_core::Result<()> {
    // Le réseau est interrogé d'emblée, en parallèle du reste : hors ligne, le
    // diagnostic ne dure qu'un délai d'attente.
    let reseau = std::thread::spawn(|| diagnostic::check_endpoints(Duration::from_secs(5)));
    let derniere = std::thread::spawn(|| sidecar::latest_ytdlp_version(Duration::from_secs(5)));

    let mut problemes = 0usize;
    println!("Scripta {} — diagnostic", env!("CARGO_PKG_VERSION"));

    println!("\nExtracteurs");
    let mut version_ytdlp = None;
    for kind in [sidecar::Kind::YtDlp, sidecar::Kind::Ffmpeg] {
        let r = sidecar::resolve(kind, None);
        match sidecar::version_of(&r.path, kind) {
            Some(v) => {
                println!("  {:<12} {v}  ({})", kind.name(), r.origin.as_str());
                if kind == sidecar::Kind::YtDlp {
                    version_ytdlp = Some(v);
                }
            }
            None => {
                problemes += 1;
                println!(
                    "  {:<12} ABSENT — voir README.md, section Installation",
                    kind.name()
                );
            }
        }
    }

    println!("\nInférence");
    let backend = scripta_core::Backend::compiled();
    println!(
        "  {:<12} whisper.cpp, backend « {} »",
        "moteur",
        backend.as_str()
    );
    if !backend.is_gpu() {
        // ADR-001 : le backend est lié à la compilation, aucun artefact ne
        // découvre un GPU à l'exécution. Le signaler évite de le chercher.
        println!(
            "  {:<12} build CPU ; pour un GPU, compiler avec --features vulkan (ou metal)",
            ""
        );
    }
    let installes: Vec<&str> = models::installed()?
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(m, _)| m.alias)
        .collect();
    println!(
        "  {:<12} {}",
        "modèles",
        if installes.is_empty() {
            "aucun — téléchargé au premier usage".to_string()
        } else {
            installes.join(", ")
        }
    );
    println!(
        "  {:<12} {}",
        "VAD",
        if models::is_installed(&models::VAD_MODEL)? {
            "silero installé"
        } else {
            "silero absent — téléchargé au premier usage"
        }
    );

    println!("\nRépertoires");
    for (nom, chemin) in [
        ("modèles", models::models_dir()),
        ("cache", cache::cache_dir()),
        ("binaires", sidecar::bin_dir()),
    ] {
        match chemin {
            Ok(p) => match diagnostic::check_writable(&p) {
                Ok(()) => println!("  {nom:<12} {}  (inscriptible)", p.display()),
                Err(e) => {
                    problemes += 1;
                    println!("  {nom:<12} {}  NON INSCRIPTIBLE : {e}", p.display());
                }
            },
            Err(e) => {
                problemes += 1;
                println!("  {nom:<12} indisponible ({e})");
            }
        }
    }
    match cache::list() {
        Ok(entrees) => println!("  {:<12} {} transcription(s) en cache", "", entrees.len()),
        Err(e) => println!("  {:<12} cache illisible ({e})", ""),
    }

    println!("\nRéseau");
    let resultats = reseau.join().unwrap_or_else(|_| Vec::new());
    for (destination, issue) in resultats {
        match issue {
            Ok(delai) => println!(
                "  {:<12} joignable ({} ms) — {}",
                destination.name,
                delai.as_millis(),
                destination.purpose
            ),
            Err(e) => {
                problemes += 1;
                println!(
                    "  {:<12} INJOIGNABLE — {} ({e})",
                    destination.name, destination.purpose
                );
            }
        }
    }
    match (derniere.join().ok().and_then(Result::ok), version_ytdlp) {
        (Some(derniere), Some(installee)) if sidecar::is_newer(&derniere, &installee) => {
            println!(
                "  {:<12} {derniere} disponible (installé : {installee}) — scripta update-extractor",
                "yt-dlp"
            );
        }
        (Some(derniere), Some(_)) => println!("  {:<12} à jour ({derniere})", "yt-dlp"),
        _ => {}
    }

    println!();
    match problemes {
        0 => println!("Aucun problème détecté."),
        1 => println!("1 problème détecté."),
        n => println!("{n} problèmes détectés."),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("scripta").chain(args.iter().copied()))
    }

    fn run_args(args: &[&str]) -> RunArgs {
        analyse(args).expect("arguments valides").run
    }

    const URL: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

    #[test]
    fn le_vad_est_actif_par_defaut() {
        assert!(run_args(&[URL]).vad_actif());
        assert!(!run_args(&["--no-vad", URL]).vad_actif());
        assert!(run_args(&["--vad", URL]).vad_actif());
    }

    #[test]
    fn le_dernier_drapeau_vad_l_emporte() {
        // Un alias de shell peut fixer --no-vad ; la ligne de commande doit
        // pouvoir le révoquer, et inversement.
        assert!(run_args(&["--no-vad", "--vad", URL]).vad_actif());
        assert!(!run_args(&["--vad", "--no-vad", URL]).vad_actif());
    }

    #[test]
    fn un_modele_vad_explicite_contredit_no_vad() {
        let e = analyse(&["--vad-model", "silero.bin", "--no-vad", URL])
            .err()
            .expect("combinaison refusée");
        // Erreur d'usage : code 2, avant tout travail.
        assert_eq!(e.exit_code(), 2);
    }

    #[test]
    fn quiet_et_verbose_s_excluent() {
        assert!(analyse(&["-q", "-v", URL]).is_err());
        assert_eq!(run_args(&["-vv", URL]).verbose, 2);
    }

    #[test]
    fn les_seuils_sont_valides_avant_tout_travail() {
        assert_eq!(
            run_args(&["--no-speech-thold", "0.4", URL]).no_speech_thold,
            Some(0.4)
        );
        assert!(analyse(&["--no-speech-thold", "1.5", URL]).is_err());
        assert!(analyse(&["--no-speech-thold", "abc", URL]).is_err());
        assert_eq!(
            run_args(&["--entropy-thold", "2.8", URL]).entropy_thold,
            Some(2.8)
        );
        assert!(analyse(&["--entropy-thold", "0", URL]).is_err());
        assert!(analyse(&["--entropy-thold", "-1", URL]).is_err());
        assert!(analyse(&["--entropy-thold", "inf", URL]).is_err());
    }

    #[test]
    fn un_contexte_vide_equivaut_a_son_absence() {
        // Sans quoi il produirait une clé de cache distincte pour un résultat
        // identique.
        assert_eq!(run_args(&[URL]).contexte(), None);
        assert_eq!(run_args(&["--initial-prompt", "   ", URL]).contexte(), None);
        assert_eq!(
            run_args(&["--initial-prompt", " Etienne Klein ", URL]).contexte(),
            Some("Etienne Klein".to_string())
        );
    }

    #[test]
    fn le_json_porte_toujours_les_mots() {
        assert!(!run_args(&[URL]).mots_horodates());
        assert!(run_args(&["-f", "json", URL]).mots_horodates());
        assert!(run_args(&["--word-timestamps", URL]).mots_horodates());
    }

    #[test]
    fn les_options_valent_aussi_pour_run_explicite() {
        let cli = analyse(&["run", "--no-vad", "--force", URL]).expect("valide");
        match cli.command {
            Some(Command::Run { args, .. }) => {
                assert!(!args.vad_actif());
                assert!(args.force);
            }
            _ => panic!("sous-commande run attendue"),
        }
    }

    #[test]
    fn la_barre_combine_pourcentage_position_et_vitesse() {
        let mut etat = EtatBarre {
            debut: Instant::now(),
            duree: 3657.0,
            pourcent: -1,
            position: None,
            largeur: 0,
        };
        // Avant tout rappel : 0 %, sans position inventée.
        assert_eq!(etat.ligne(0.0), "Transcription :   0 %");

        etat.pourcent = 42;
        etat.position = Some(1513.0);
        assert_eq!(
            etat.ligne(300.0),
            "Transcription :  42 %  ·  25:13 / 1:00:57  ·  5.0× temps réel"
        );
        // Sous la seconde, pas de vitesse : elle serait absurde.
        assert_eq!(etat.ligne(0.5), "Transcription :  42 %  ·  25:13 / 1:00:57");
    }

    #[test]
    fn horodatage_lisible() {
        assert_eq!(horodatage(0.0), "0:00");
        assert_eq!(horodatage(75.4), "1:15");
        assert_eq!(horodatage(3657.0), "1:00:57");
        assert_eq!(horodatage(-3.0), "0:00");
    }

    #[test]
    fn taille_lisible_selon_l_ordre_de_grandeur() {
        assert_eq!(taille_lisible(885_098), "864 Ko");
        assert_eq!(taille_lisible(147_951_465), "141.1 Mo");
        assert_eq!(taille_lisible(12), "12 o");
    }
}
