//! Interface en ligne de commande de Scripta — SPEC §4.1.
//!
//! # État — Jalon 1
//!
//! Le squelette de sous-commandes est en place (SPEC §4.1, correction de la v1
//! qui plaçait `--update-extractor` en conflit avec un positionnel requis).
//! `run` couvre la chaîne complète URL → transcription.
//!
//! Le modèle doit être mis en place manuellement (`SCRIPTA_MODELS_DIR` ou
//! `--model-path`) : son téléchargement relève de la tâche 2.3.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use scripta_core::audio::{self, Sidecars};
use scripta_core::format::{OutputFormat, SubtitleOptions};
use scripta_core::{
    CancelToken, Document, Engine, Run, ScriptaError, Source, probe, transcribe, url,
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
    /// Diagnostic : sidecars, backends, modèles, chemins.
    Doctor,
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

    /// Nom du modèle, résolu dans SCRIPTA_MODELS_DIR en `ggml-<nom>.bin`.
    #[arg(short, long, default_value = "base")]
    model: String,

    /// Chemin explicite du modèle, prioritaire sur --model.
    #[arg(long)]
    model_path: Option<PathBuf>,

    /// Modèle VAD Silero. Son absence désactive le VAD.
    #[arg(long)]
    vad_model: Option<PathBuf>,

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

    // Le modèle est résolu et chargé avant tout travail réseau : un modèle
    // absent doit échouer immédiatement, pas après plusieurs minutes de
    // téléchargement.
    let model_path = resolve_model(args)?;
    progress(&format!("Chargement du modèle ({})…", model_path.display()));
    let engine = Engine::load(&model_path)?;
    transcribe::check_translate_supported(engine.model_id(), args.translate)?;

    progress("Sonde des métadonnées…");
    let meta = probe::probe(&args.ytdlp_path, &url)?;
    probe::check_admissible(&meta, args.max_duration)?;

    if !args.quiet {
        let duree = meta
            .duration
            .map(format_duree)
            .unwrap_or_else(|| "durée inconnue".to_string());
        eprintln!("« {} » — {duree}", meta.title);
    }

    progress("Extraction audio…");
    let samples = audio::extract(
        &Sidecars::new(&args.ytdlp_path, &args.ffmpeg_path),
        &url,
        meta.duration,
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

/// Résout le modèle : `--model-path` s'il est fourni, sinon `--model` dans
/// `SCRIPTA_MODELS_DIR`.
///
/// Le téléchargement à la demande relève de la tâche 2.3 (SF-03) ; en attendant,
/// le modèle doit être mis en place manuellement.
fn resolve_model(args: &RunArgs) -> scripta_core::Result<PathBuf> {
    if let Some(p) = &args.model_path {
        return Ok(p.clone());
    }

    let dir =
        std::env::var_os("SCRIPTA_MODELS_DIR").ok_or_else(|| ScriptaError::ModelUnavailable {
            detail: "définissez SCRIPTA_MODELS_DIR, ou passez --model-path. \
                     Le téléchargement automatique arrive au Jalon 2 (SPEC SF-03)."
                .to_string(),
        })?;

    Ok(PathBuf::from(dir).join(format!("ggml-{}.bin", args.model)))
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
