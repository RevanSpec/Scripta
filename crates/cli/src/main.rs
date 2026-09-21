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
use scripta_core::{CancelToken, Engine, ScriptaError, probe, transcribe, url};

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

    /// Format de sortie.
    ///
    /// Validé par clap : un format inconnu ou non encore livré est une erreur
    /// d'usage (code 2), signalée avant tout travail réseau.
    #[arg(short, long, default_value = "txt", value_parser = parse_format)]
    format: scripta_core::format::OutputFormat,

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

fn parse_format(s: &str) -> Result<scripta_core::format::OutputFormat, String> {
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
            .map(|d| format!("{:.0} min", d / 60.0))
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
        word_timestamps: false,
        vad_model: args.vad_model.clone(),
    };

    // Le jeton est conservé par l'appelant : c'est ce qui permettra au
    // gestionnaire de SIGINT (tâche 2.8) d'interrompre une inférence en cours.
    let cancel = CancelToken::new();
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
    if !quiet {
        let ecoule = debut.elapsed().as_secs_f64();
        let audio_s = transcribe::duration_of(&samples);
        eprintln!(
            "\rTranscription terminée en {ecoule:.1} s ({:.1}× temps réel, {} segments, langue : {}).",
            if ecoule > 0.0 { audio_s / ecoule } else { 0.0 },
            transcript.segments.len(),
            transcript.language.as_deref().unwrap_or("?")
        );
    }

    write_output(args.output.as_deref(), &args.format.render(&transcript))
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

    println!("  {:<10} non intégré (Jalon 1, tâche 1.5)", "whisper");
    Ok(())
}
