//! Interface en ligne de commande de Scripta — SPEC §4.1.
//!
//! # État — Jalon 1
//!
//! Le squelette de sous-commandes est en place (SPEC §4.1, correction de la v1
//! qui plaçait `--update-extractor` en conflit avec un positionnel requis).
//! `run` aboutit à l'extraction audio ; l'inférence est bloquée en amont, voir
//! `docs/ROADMAP.md`, Jalon 0, tâche 0.1.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use scripta_core::audio::{self, Sidecars};
use scripta_core::{ScriptaError, probe, url};

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
        #[command(flatten)]
        args: RunArgs,
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

    // Jalon 0, tâche 0.1 : l'intégration de whisper-rs est bloquée sur
    // l'indisponibilité de CMake dans l'environnement de développement.
    // Tout le pipeline amont est complet et vérifié jusqu'à ce point.
    Err(ScriptaError::InferenceFailed {
        detail: "moteur Whisper non encore intégré (Jalon 1, tâche 1.5 — voir docs/ROADMAP.md)"
            .to_string(),
    })
}

/// Branché par la tâche 1.5, en même temps que l'inférence.
#[allow(dead_code)]
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
