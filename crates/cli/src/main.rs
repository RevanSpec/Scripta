//! Interface en ligne de commande de Scripta — SPEC §4.1.
//!
//! # État — Jalon 2
//!
//! Le squelette de sous-commandes est en place (SPEC §4.1, correction de la v1
//! qui plaçait `--update-extractor` en conflit avec un positionnel requis).
//! `run` couvre la chaîne complète URL → transcription.
//!
//! Le modèle est téléchargé au premier usage et vérifié par empreinte
//! SHA-256 ; `--model-path` reste disponible pour un fichier hors catalogue.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use scripta_core::audio::{self, Sidecars};
use scripta_core::format::{OutputFormat, SubtitleOptions};
use scripta_core::{
    CancelToken, Document, Engine, ModelSpec, Run, ScriptaError, Source, models, probe, subtitles,
    transcribe, url,
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
        // Boxé : `RunArgs` pèse ~288 octets et `Doctor` est vide ; sans
        // indirection, chaque variante de l'énuméré porterait ce poids.
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
    /// Diagnostic : sidecars, backends, modèles, chemins.
    Doctor,
}

#[derive(Subcommand)]
enum ModelsAction {
    /// Liste les modèles et leur état local.
    List,
    /// Télécharge un modèle et vérifie son empreinte.
    Pull { model: String },
    /// Supprime un modèle du cache.
    Rm { model: String },
    /// Affiche le répertoire de cache.
    Path,
    /// Recalcule l'empreinte d'un modèle installé.
    Verify { model: String },
}

#[derive(clap::Args, Clone)]
struct RunArgs {
    /// Fichier de sortie [défaut : stdout].
    #[arg(short, long)]
    output: Option<PathBuf>,

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

    /// Modèle VAD Silero. Son absence désactive le VAD.
    #[arg(long)]
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

    /// Chemin du binaire yt-dlp.
    #[arg(long, default_value = "yt-dlp")]
    ytdlp_path: PathBuf,

    /// Chemin du binaire ffmpeg.
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg_path: PathBuf,

    /// Supprime les messages de progression.
    #[arg(short, long)]
    quiet: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match (cli.command, cli.url) {
        (Some(Command::Doctor), _) => doctor(),
        (Some(Command::Models { action }), _) => models_cmd(&action),
        (Some(Command::Subs { url, args }), _) => subs(&url, &args),
        (Some(Command::Run { url, args }), _) => run(&url, &args),
        (None, Some(url)) => run(&url, &cli.run),
        (None, None) => {
            eprintln!("Erreur : aucune URL fournie.\n\nEssayez « scripta --help ».");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Tout diagnostic sur stderr : stdout ne transporte que le résultat,
            // afin que `scripta <URL> | …` reste utilisable (SPEC §4.1).
            eprintln!("Erreur : {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

/// Installe le gestionnaire d'interruption — SPEC §4.1, tâche 2.8.
///
/// Deux comportements distincts selon le moment :
///
/// - **Premier `Ctrl-C`** : arme le jeton d'annulation. whisper.cpp l'interroge
///   entre ses fenêtres de traitement et rend la main proprement, ce qui laisse
///   le programme nettoyer et sortir en 130.
/// - **Second `Ctrl-C`** : terminaison immédiate. Si la première demande n'a pas
///   abouti — sidecar bloqué, fenêtre d'inférence anormalement longue —
///   l'utilisateur doit pouvoir reprendre son terminal sans attendre.
///
/// Un échec d'installation n'est pas fatal : le programme reste utilisable,
/// simplement moins docile à l'interruption.
fn install_interrupt_handler(cancel: CancelToken, quiet: bool) {
    let deja_demande = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

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

/// Sous la minute, afficher « 0 min » est absurde.
fn format_duree(secondes: f64) -> String {
    if secondes < 60.0 {
        format!("{secondes:.0} s")
    } else {
        format!("{:.0} min", secondes / 60.0)
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

fn charger_moteur(args: &RunArgs, progress: &impl Fn(&str)) -> scripta_core::Result<Engine> {
    let model_path = resolve_model(args, args.quiet)?;
    progress(&format!("Chargement du modèle ({})…", model_path.display()));
    let engine = Engine::load(&model_path)?;
    transcribe::check_translate_supported(engine.model_id(), args.translate)?;
    Ok(engine)
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
    let access = access_of(args);

    let meta = probe::probe(&args.ytdlp_path, &url, &access)?;
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

    let transcript = subtitles::fetch(&piste)?;
    let doc = document_de(&url, &meta, transcript, Run::default());
    write_output(
        args.output.as_deref(),
        &args.format.render(&doc, &sous_titres_options(args)),
    )
}

fn run(raw_url: &str, args: &RunArgs) -> scripta_core::Result<()> {
    let url = url::parse(raw_url)?;
    if url.had_playlist() && !args.quiet {
        eprintln!("Avertissement : paramètre de playlist ignoré, seule la vidéo est traitée.");
    }

    let progress = |msg: &str| {
        if !args.quiet {
            eprintln!("{msg}");
        }
    };

    let access = access_of(args);

    // Ordre dicté par `--prefer-subs`. Sans lui, le modèle est résolu en
    // premier pour qu'un modèle absent échoue immédiatement plutôt qu'après
    // plusieurs minutes de téléchargement. Avec lui, il se peut qu'aucun modèle
    // ne soit nécessaire : il serait absurde d'en télécharger un pour rien.
    let engine = if args.prefer_subs {
        None
    } else {
        Some(charger_moteur(args, &progress)?)
    };

    progress("Sonde des métadonnées…");
    let meta = probe::probe(&args.ytdlp_path, &url, &access)?;
    probe::check_admissible(&meta, args.max_duration)?;

    if !args.quiet {
        let duree = meta
            .duration
            .map(format_duree)
            .unwrap_or_else(|| "durée inconnue".to_string());
        eprintln!("« {} » — {duree}", meta.title);
    }

    if args.prefer_subs {
        match recuperer_sous_titres(&meta, args) {
            Some(transcript) => {
                let doc = document_de(&url, &meta, transcript, Run::default());
                return write_output(
                    args.output.as_deref(),
                    &args.format.render(&doc, &sous_titres_options(args)),
                );
            }
            None => progress("Pas de sous-titres exploitables : repli sur la transcription."),
        }
    }

    let engine = match engine {
        Some(e) => e,
        None => charger_moteur(args, &progress)?,
    };

    progress("Extraction audio…");
    let samples = audio::extract(
        &Sidecars::new(&args.ytdlp_path, &args.ffmpeg_path),
        &url,
        meta.duration,
        &access,
    )?;
    progress(&format!(
        "{} échantillons extraits ({:.1} s à {} Hz).",
        samples.len(),
        samples.len() as f64 / audio::SAMPLE_RATE as f64,
        audio::SAMPLE_RATE
    ));

    let options = transcribe::Options {
        language: args.lang.clone(),
        translate: args.translate,
        threads: args.threads.unwrap_or_else(transcribe::default_threads),
        initial_prompt: args.initial_prompt.clone(),
        word_timestamps: args.word_timestamps || args.format == OutputFormat::Json,
        vad_model: args.vad_model.clone(),
    };

    // Le jeton est conservé par l'appelant : c'est ce qui permettra au
    // gestionnaire de SIGINT d'interrompre une inférence en cours (SPEC §4.1).
    let cancel = CancelToken::new();
    install_interrupt_handler(cancel.clone(), args.quiet);
    let quiet = args.quiet;
    let hooks = transcribe::Hooks {
        on_progress: (!quiet).then(|| {
            let mut dernier = -1i32;
            Box::new(move |p: i32| {
                // Un rappel par pourcent suffit : whisper.cpp en émet
                // beaucoup plus, et chacun coûterait une écriture terminal.
                if p / 5 > dernier {
                    dernier = p / 5;
                    eprint!("\rTranscription : {p:>3} %");
                }
            }) as Box<dyn FnMut(i32) + Send>
        }),
        cancel: Some(cancel),
    };

    let debut = std::time::Instant::now();
    let transcript = engine.transcribe(&samples, &options, hooks)?;
    let ecoule = debut.elapsed();
    let audio_s = transcribe::duration_of(&samples);
    let vitesse = if ecoule.as_secs_f64() > 0.0 {
        audio_s / ecoule.as_secs_f64()
    } else {
        0.0
    };

    if !quiet {
        eprintln!(
            "\rTranscription terminée en {:.1} s ({vitesse:.1}× temps réel, {} segments, langue : {}).",
            ecoule.as_secs_f64(),
            transcript.segments.len(),
            transcript.language.as_deref().unwrap_or("?")
        );
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
            model: engine.model_id().to_string(),
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

    let sous_titres = SubtitleOptions {
        max_line_width: args.max_line_width,
        max_line_count: args.max_line_count,
        ..Default::default()
    };

    write_output(
        args.output.as_deref(),
        &args.format.render(&doc, &sous_titres),
    )
}

/// Résout le modèle et garantit sa présence locale — SPEC SF-03.
///
/// `--model-path` court-circuite tout : il sert aux modèles absents du
/// catalogue, ou déposés à la main.
fn resolve_model(args: &RunArgs, quiet: bool) -> scripta_core::Result<PathBuf> {
    if let Some(p) = &args.model_path {
        return Ok(p.clone());
    }

    let spec = models::find(&args.model).ok_or_else(|| ScriptaError::ModelUnavailable {
        detail: format!(
            "modèle « {} » inconnu (disponibles : auto, {})",
            args.model,
            models::aliases().join(", ")
        ),
    })?;

    fetch_model(spec, quiet)
}

/// Télécharge un modèle si besoin, en affichant une progression sur `stderr`.
///
/// Rien n'est imprimé quand le modèle est déjà en cache : le cas nominal doit
/// rester silencieux.
fn fetch_model(spec: &ModelSpec, quiet: bool) -> scripta_core::Result<PathBuf> {
    let mut annonce = false;
    let mut dernier = u64::MAX;

    let mut progression = |recus: u64, total: u64| {
        if quiet || total == 0 {
            return;
        }
        if !annonce {
            eprintln!("Téléchargement de {} ({} Mo)…", spec.file, spec.size_mb());
            annonce = true;
        }
        let pourcent = recus * 100 / total;
        if pourcent != dernier {
            dernier = pourcent;
            eprint!(
                "\r  {pourcent:>3} %  ({} / {} Mo)",
                recus / 1_048_576,
                total / 1_048_576
            );
            if recus >= total {
                eprintln!("\n  Vérification de l'empreinte…");
            }
        }
    };

    models::ensure(spec, &mut progression)
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
            println!("  {alias:<10} {taille:>6}     {fichier:<30} {etat}");
            for (spec, present) in models::installed()? {
                println!(
                    "  {:<10} {:>6} Mo  {:<30} {}",
                    spec.alias,
                    spec.size_mb(),
                    spec.file,
                    if present { "installé" } else { "-" }
                );
            }
        }
        ModelsAction::Pull { model } => {
            let spec = resolve_alias(model)?;
            let chemin = fetch_model(spec, false)?;
            println!("{}", chemin.display());
        }
        ModelsAction::Rm { model } => {
            let spec = resolve_alias(model)?;
            if models::remove(spec)? {
                eprintln!("Supprimé : {}", spec.file);
            } else {
                eprintln!("Absent du cache : {}", spec.file);
            }
        }
        ModelsAction::Verify { model } => {
            let spec = resolve_alias(model)?;
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

fn resolve_alias(alias: &str) -> scripta_core::Result<&'static ModelSpec> {
    models::find(alias).ok_or_else(|| ScriptaError::ModelUnavailable {
        detail: format!(
            "modèle « {alias} » inconnu (disponibles : auto, {})",
            models::aliases().join(", ")
        ),
    })
}

fn write_output(path: Option<&std::path::Path>, content: &str) -> scripta_core::Result<()> {
    match path {
        Some(p) => std::fs::write(p, content).map_err(|source| ScriptaError::OutputFailed {
            path: p.to_path_buf(),
            source,
        }),
        None => std::io::stdout()
            .write_all(content.as_bytes())
            .map_err(|source| ScriptaError::OutputFailed {
                path: PathBuf::from("<stdout>"),
                source,
            }),
    }
}

fn doctor() -> scripta_core::Result<()> {
    println!("Scripta {} — diagnostic\n", env!("CARGO_PKG_VERSION"));

    for (nom, defaut) in [("yt-dlp", "yt-dlp"), ("ffmpeg", "ffmpeg")] {
        let trouve = std::process::Command::new(defaut)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok();
        println!(
            "  {:<10} {}",
            nom,
            if trouve {
                "présent"
            } else {
                "ABSENT (--ytdlp-path / --ffmpeg-path pour un chemin explicite)"
            }
        );
    }

    println!(
        "  {:<10} whisper.cpp, backend « {} »",
        "inférence",
        scripta_core::Backend::compiled().as_str()
    );

    match std::env::var_os("SCRIPTA_MODELS_DIR") {
        Some(d) => println!("  {:<10} {}", "modèles", PathBuf::from(d).display()),
        None => println!(
            "  {:<10} SCRIPTA_MODELS_DIR non défini (téléchargement automatique : Jalon 2)",
            "modèles"
        ),
    }
    Ok(())
}
