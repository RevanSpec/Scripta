//! Pipeline d'extraction audio « zero-disk » — SPEC SF-02, ADR-002.
//!
//! `yt-dlp` écrit le flux audio sur sa sortie standard, branchée directement sur
//! l'entrée standard de `ffmpeg`, qui produit du PCM `s16le` 16 kHz mono. Aucun
//! fichier temporaire, aucun interpréteur de commandes : les deux processus sont
//! câblés à la main et tous les arguments passés sous forme de tableau.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;

use crate::error::{Result, ScriptaError, classify_sidecar_stderr};
use crate::url::CanonicalUrl;

/// Fréquence d'échantillonnage attendue par Whisper.
pub const SAMPLE_RATE: u32 = 16_000;

/// Taille des blocs lus sur la sortie de ffmpeg.
const READ_CHUNK: usize = 64 * 1024;

/// Volume de `stderr` conservé par sidecar, pour la classification d'erreurs.
const STDERR_TAIL_BYTES: usize = 4 * 1024;

/// Décodeur incrémental `s16le` → `f32` normalisé dans `[-1.0, 1.0)`.
///
/// Les blocs lus sur un pipe ne sont pas alignés sur les échantillons : un bloc
/// peut se terminer au milieu d'un `i16`. L'octet orphelin est reporté sur le
/// bloc suivant.
#[derive(Debug, Default)]
pub struct PcmDecoder {
    /// Octet de poids faible d'un échantillon incomplet.
    carry: Option<u8>,
}

impl PcmDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Décode un bloc et pousse les échantillons dans `out`.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<f32>) {
        let mut rest = chunk;

        if let Some(low) = self.carry.take() {
            match rest.split_first() {
                Some((high, tail)) => {
                    out.push(to_f32(i16::from_le_bytes([low, *high])));
                    rest = tail;
                }
                None => {
                    // Bloc vide : l'octet reste en attente.
                    self.carry = Some(low);
                    return;
                }
            }
        }

        // `as_chunks::<2>()` plutôt que `chunks_exact(2)` : la taille est
        // connue à la compilation, ce qui donne des tableaux `[u8; 2]`
        // directement consommables par `from_le_bytes`, sans indexation.
        let (pairs, remainder) = rest.as_chunks::<2>();
        for pair in pairs {
            out.push(to_f32(i16::from_le_bytes(*pair)));
        }
        if let [orphan] = remainder {
            self.carry = Some(*orphan);
        }
    }

    /// Vrai si un octet orphelin subsiste — flux tronqué.
    pub fn has_pending_byte(&self) -> bool {
        self.carry.is_some()
    }
}

/// -32768 → -1.0, 32767 → ~0.99997 (convention whisper.cpp).
#[inline]
fn to_f32(sample: i16) -> f32 {
    sample as f32 / 32768.0
}

/// Chemins des sidecars, résolus en amont (jamais recherchés dans `PATH`
/// depuis un contexte GUI — voir SPEC §5.1).
#[derive(Debug, Clone)]
pub struct Sidecars {
    pub ytdlp: PathBuf,
    pub ffmpeg: PathBuf,
}

impl Sidecars {
    pub fn new(ytdlp: impl Into<PathBuf>, ffmpeg: impl Into<PathBuf>) -> Self {
        Self {
            ytdlp: ytdlp.into(),
            ffmpeg: ffmpeg.into(),
        }
    }
}

/// Extrait l'audio d'une URL canonique sous forme de `Vec<f32>` 16 kHz mono.
///
/// `expected_duration_s`, issu de la sonde de métadonnées (SF-01), ne sert qu'à
/// pré-allouer le tampon.
pub fn extract(
    sidecars: &Sidecars,
    url: &CanonicalUrl,
    expected_duration_s: Option<f64>,
) -> Result<Vec<f32>> {
    let mut dl = spawn_downloader(&sidecars.ytdlp, url)?;

    // Le stdout de yt-dlp devient le stdin de ffmpeg : câblage direct par
    // descripteur, sans processus shell intermédiaire (ADR-002).
    let dl_stdout = dl
        .stdout
        .take()
        .expect("stdout de yt-dlp configuré en Stdio::piped");

    let mut ff = match spawn_decoder(&sidecars.ffmpeg, dl_stdout) {
        Ok(child) => child,
        Err(e) => {
            let _ = dl.kill();
            let _ = dl.wait();
            return Err(e);
        }
    };

    // ⚠ SF-02 — règle impérative : un `stderr` en Stdio::piped() jamais lu
    // sature le tampon du pipe (≈64 Kio) et bloque définitivement l'enfant. Le
    // symptôme n'apparaît que sur les flux volumineux. Chaque `stderr` est donc
    // drainé par un thread dédié, qui en conserve la fin pour la classification
    // d'erreurs (SF-07).
    let dl_err = drain_stderr(&mut dl, "yt-dlp");
    let ff_err = drain_stderr(&mut ff, "ffmpeg");

    let ff_stdout = ff
        .stdout
        .take()
        .expect("stdout de ffmpeg configuré en Stdio::piped");

    // La lecture doit précéder tout `wait()` : attendre un enfant dont la sortie
    // n'est pas consommée est l'autre moitié du même interblocage.
    let samples = read_samples(ff_stdout, expected_duration_s);

    let dl_status = dl.wait();
    let ff_status = ff.wait();
    let dl_stderr = dl_err.collect();
    let ff_stderr = ff_err.collect();

    let samples = match samples {
        Ok(s) => s,
        Err(io) => {
            return Err(ScriptaError::ExtractionFailed {
                detail: format!("lecture du flux décodé : {io}\n{ff_stderr}"),
            });
        }
    };

    // yt-dlp d'abord : c'est lui qui porte la cause réelle. Un ffmpeg en échec
    // n'est le plus souvent que la conséquence d'un flux amont vide.
    if !dl_status.as_ref().map(|s| s.success()).unwrap_or(false) {
        return Err(classify_sidecar_stderr(&dl_stderr).unwrap_or(
            ScriptaError::ExtractionFailed {
                detail: format!("yt-dlp a échoué : {dl_stderr}"),
            },
        ));
    }

    if !ff_status.as_ref().map(|s| s.success()).unwrap_or(false) {
        return Err(ScriptaError::ExtractionFailed {
            detail: format!("ffmpeg a échoué : {ff_stderr}"),
        });
    }

    if samples.is_empty() {
        return Err(ScriptaError::ExtractionFailed {
            detail: format!("aucun échantillon audio produit.\n{ff_stderr}"),
        });
    }

    Ok(samples)
}

fn spawn_downloader(ytdlp: &Path, url: &CanonicalUrl) -> Result<Child> {
    Command::new(ytdlp)
        .args([
            "-q",
            "--no-warnings",
            "--no-playlist",
            "-f",
            "bestaudio[ext=webm]/bestaudio/best",
            "-o",
            "-",
            // `--` clôt les options : aucun argument suivant ne peut être
            // réinterprété comme un drapeau.
            "--",
        ])
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| sidecar_spawn_error(ytdlp, "yt-dlp", e))
}

fn spawn_decoder(ffmpeg: &Path, input: std::process::ChildStdout) -> Result<Child> {
    Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-i",
            "pipe:0",
            "-vn",
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "pipe:1",
        ])
        .stdin(Stdio::from(input))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| sidecar_spawn_error(ffmpeg, "ffmpeg", e))
}

fn sidecar_spawn_error(path: &Path, name: &str, e: std::io::Error) -> ScriptaError {
    if e.kind() == std::io::ErrorKind::NotFound {
        ScriptaError::SidecarMissing {
            name: name.to_string(),
        }
    } else {
        ScriptaError::ExtractionFailed {
            detail: format!("lancement de {name} ({}) : {e}", path.display()),
        }
    }
}

/// Marge sur la pré-allocation du tampon audio.
///
/// La durée annoncée par `yt-dlp` est arrondie et **sous-estime
/// systématiquement** le flux réellement décodé : sur une vidéo de 19 s,
/// 304 089 échantillons arrivent pour 304 000 attendus. Sans marge, la
/// capacité est donc toujours dépassée — d'un cheveu, mais `Vec` double
/// quand même.
///
/// Sur une heure d'audio, ce doublement fait passer le tampon de 223 à
/// 446 Mo, et l'ancien coexiste avec le nouveau le temps de la copie : un pic
/// transitoire de 670 Mo. La marge de 2 % coûte 4,5 Mo et l'évite.
const PREALLOC_MARGIN: f64 = 1.02;

/// Nombre d'échantillons à pré-allouer pour une durée annoncée.
pub fn preallocation_len(expected_duration_s: Option<f64>) -> usize {
    expected_duration_s
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| (d * PREALLOC_MARGIN * SAMPLE_RATE as f64) as usize)
        .unwrap_or(SAMPLE_RATE as usize * 60)
}

fn read_samples(
    mut stdout: std::process::ChildStdout,
    expected_duration_s: Option<f64>,
) -> std::io::Result<Vec<f32>> {
    let mut out = Vec::with_capacity(preallocation_len(expected_duration_s));
    let mut decoder = PcmDecoder::new();
    let mut buf = vec![0u8; READ_CHUNK];

    loop {
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => decoder.push(&buf[..n], &mut out),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// Thread de drainage de `stderr`, conservant les derniers [`STDERR_TAIL_BYTES`].
struct StderrDrain {
    rx: Option<mpsc::Receiver<String>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl StderrDrain {
    fn collect(mut self) -> String {
        let text = self
            .rx
            .take()
            .and_then(|rx| rx.recv().ok())
            .unwrap_or_default();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        text
    }
}

fn drain_stderr(child: &mut Child, _name: &str) -> StderrDrain {
    let Some(mut stderr) = child.stderr.take() else {
        return StderrDrain {
            rx: None,
            handle: None,
        };
    };
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::with_capacity(STDERR_TAIL_BYTES);
        let mut buf = [0u8; 8192];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > STDERR_TAIL_BYTES {
                        let excess = tail.len() - STDERR_TAIL_BYTES;
                        tail.drain(..excess);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&tail).into_owned());
    });
    StderrDrain {
        rx: Some(rx),
        handle: Some(handle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(chunks: &[&[u8]]) -> Vec<f32> {
        let mut d = PcmDecoder::new();
        let mut out = Vec::new();
        for c in chunks {
            d.push(c, &mut out);
        }
        out
    }

    #[test]
    fn convertit_les_bornes() {
        let out = decode_all(&[&[0x00, 0x80, 0xFF, 0x7F, 0x00, 0x00]]);
        assert_eq!(out[0], -1.0); // i16::MIN
        assert!((out[1] - 0.999_97).abs() < 1e-4); // i16::MAX
        assert_eq!(out[2], 0.0);
    }

    #[test]
    fn reporte_l_octet_orphelin_entre_blocs() {
        // Le cœur du décodeur : un bloc coupé au milieu d'un échantillon.
        let decoupe = decode_all(&[&[0x00], &[0x80, 0xFF], &[0x7F]]);
        let entier = decode_all(&[&[0x00, 0x80, 0xFF, 0x7F]]);
        assert_eq!(decoupe, entier);
        assert_eq!(decoupe.len(), 2);
    }

    #[test]
    fn tolere_les_blocs_vides() {
        let mut d = PcmDecoder::new();
        let mut out = Vec::new();
        d.push(&[0x34], &mut out);
        d.push(&[], &mut out); // ne doit pas consommer l'octet en attente
        assert!(d.has_pending_byte());
        d.push(&[0x12], &mut out);
        assert!(!d.has_pending_byte());
        assert_eq!(out, vec![to_f32(0x1234)]);
    }

    #[test]
    fn signale_un_flux_tronque() {
        let mut d = PcmDecoder::new();
        let mut out = Vec::new();
        d.push(&[0x00, 0x80, 0x42], &mut out);
        assert_eq!(out.len(), 1);
        assert!(d.has_pending_byte());
    }

    #[test]
    fn la_preallocation_absorbe_le_depassement_reel() {
        // Cas mesuré : 19,0 s annoncées, 304 089 échantillons décodés.
        let capacite = preallocation_len(Some(19.0));
        assert!(
            capacite >= 304_089,
            "capacité {capacite} insuffisante : le Vec doublerait"
        );

        // Sur une heure, le doublement coûterait un pic transitoire de 670 Mo.
        let heure = preallocation_len(Some(3657.0));
        assert!(heure >= 3657 * SAMPLE_RATE as usize);
        // La marge reste modeste : quelques mégaoctets, pas un doublement.
        assert!(heure < (3657.0 * 1.05 * SAMPLE_RATE as f64) as usize);
    }

    #[test]
    fn preallocation_robuste_aux_durees_aberrantes() {
        let defaut = SAMPLE_RATE as usize * 60;
        for aberrante in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
        ] {
            assert_eq!(preallocation_len(aberrante), defaut, "{aberrante:?}");
        }
    }

    #[test]
    fn decoupage_arbitraire_equivalent_au_flux_entier() {
        let source: Vec<u8> = (0..1000u16).flat_map(|i| i.to_le_bytes()).collect();
        let reference = decode_all(&[&source]);
        for taille in [1usize, 2, 3, 7, 64, 999] {
            let blocs: Vec<&[u8]> = source.chunks(taille).collect();
            assert_eq!(decode_all(&blocs), reference, "découpage par {taille}");
        }
    }
}
