//! Banc de performance — SPEC §5.2, tâche 4.8 de la roadmap.
//!
//! Mesure le débit d'inférence sur un échantillon fixe, sans réseau : le
//! suivi de tendance ne doit dépendre ni de YouTube, ni du débit de la
//! machine de CI vers lui.
//!
//! ```text
//! cargo run --release -p scripta-core --example banc -- \
//!     <modèle.bin> <échantillon.wav> [répétitions] [vad.bin]
//! ```
//!
//! L'échantillon — WAV 16 kHz mono, 16 bits — est répété bout à bout : onze
//! secondes ne suffiraient pas à une mesure stable. Le résultat tient sur une
//! ligne JSON, sur la sortie standard.

use std::path::{Path, PathBuf};
use std::time::Instant;

use scripta_core::audio::PcmDecoder;
use scripta_core::transcribe::{self, Engine, Hooks, Options};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage : banc <modèle.bin> <échantillon.wav> [répétitions] [vad.bin]");
        std::process::exit(2);
    }
    let modele = PathBuf::from(&args[0]);
    let wav = PathBuf::from(&args[1]);
    let repetitions: usize = args.get(2).map_or(1, |r| r.parse().expect("répétitions"));
    let vad = args.get(3).map(PathBuf::from);

    let echantillon = lire_wav(&wav);
    let mut audio = Vec::with_capacity(echantillon.len() * repetitions);
    for _ in 0..repetitions {
        audio.extend_from_slice(&echantillon);
    }
    let duree_audio = transcribe::duration_of(&audio);

    let chargement = Instant::now();
    let engine = Engine::load(&modele).expect("chargement du modèle");
    let duree_chargement = chargement.elapsed().as_secs_f64();

    let options = Options {
        language: Some("en".to_string()),
        vad_model: vad.clone(),
        ..Options::default()
    };
    let threads = options.threads;

    let debut = Instant::now();
    let transcript = engine
        .transcribe(audio, &options, Hooks::default())
        .expect("inférence");
    let duree_inference = debut.elapsed().as_secs_f64();

    let resultat = serde_json::json!({
        "modele": modele.file_name().map(|n| n.to_string_lossy().into_owned()),
        "backend": transcribe::Backend::compiled().as_str(),
        "threads": threads,
        "vad": vad.is_some(),
        "audio_s": arrondi(duree_audio),
        "chargement_s": arrondi(duree_chargement),
        "inference_s": arrondi(duree_inference),
        "vitesse": arrondi(duree_audio / duree_inference),
        "segments": transcript.segments.len(),
    });
    println!("{resultat}");
}

fn arrondi(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Lecteur WAV minimal : PCM 16 bits mono à 16 kHz, comme le pipeline le
/// produit. Écrit à la main pour ne pas ajouter de dépendance.
fn lire_wav(chemin: &Path) -> Vec<f32> {
    let octets = std::fs::read(chemin).expect("lecture du WAV");
    assert!(
        octets.len() > 12 && &octets[0..4] == b"RIFF" && &octets[8..12] == b"WAVE",
        "fichier non RIFF/WAVE"
    );
    let mut pos = 12;
    while pos + 8 <= octets.len() {
        let id = &octets[pos..pos + 4];
        let taille = u32::from_le_bytes(octets[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let corps = pos + 8;
        if id == b"data" {
            let fin = (corps + taille).min(octets.len());
            let mut sortie = Vec::new();
            PcmDecoder::new().push(&octets[corps..fin], &mut sortie);
            return sortie;
        }
        // Les chunks sont alignés sur deux octets.
        pos = corps + taille + (taille % 2);
    }
    panic!("chunk « data » introuvable");
}
