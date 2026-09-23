//! Sidecar simulé — fixture de test (SPEC §5.5).
//!
//! Se substitue à `yt-dlp` ou `ffmpeg` pour éprouver le pipeline SF-02 sans
//! réseau ni binaire externe.
//!
//! # Configuration par le nom de fichier
//!
//! Le comportement est encodé dans le **nom du binaire**, pas dans
//! l'environnement, pour deux raisons :
//!
//! - les deux enfants du pipeline héritent du même environnement, qui ne
//!   permettrait donc pas de les configurer séparément ;
//! - l'environnement d'un processus est global, ce qui rendrait les tests
//!   dépendants de leur ordre d'exécution sous `cargo test` (qui parallélise).
//!
//! Grammaire : `<rôle>[-sl<N>][-so<N>][-se<N>][-x<N>][-msg<HEX>]`
//!
//! | Jeton | Effet |
//! |---|---|
//! | `ytdlp` | rôle source : ignore stdin |
//! | `ffmpeg` | rôle filtre : lit stdin jusqu'à EOF, comme le ferait ffmpeg |
//! | `sl<N>` | attend `N` millisecondes avant toute chose : un sidecar figé |
//! | `so<N>` | émet `N` octets déterministes sur stdout |
//! | `se<N>` | déverse `N` octets sur stderr |
//! | `x<N>` | code de sortie `N` |
//! | `msg<HEX>` | message UTF-8 encodé en hexadécimal, écrit sur stderr |
//!
//! `se<N>` est l'outil du test de non-régression de l'interblocage : au-delà de
//! la taille du tampon de pipe (≈64 Kio), un `stderr` non drainé bloque
//! définitivement ce processus.

use std::io::{Read, Write};

struct Config {
    filter: bool,
    sleep_ms: u64,
    stdout_bytes: usize,
    stderr_bytes: usize,
    exit: i32,
    message: Option<String>,
}

fn parse_config() -> Config {
    let stem = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();

    let mut cfg = Config {
        filter: stem.starts_with("ffmpeg"),
        sleep_ms: 0,
        stdout_bytes: 0,
        stderr_bytes: 0,
        exit: 0,
        message: None,
    };

    for token in stem.split('-').skip(1) {
        if let Some(v) = token.strip_prefix("sl") {
            cfg.sleep_ms = v.parse().unwrap_or(0);
        } else if let Some(v) = token.strip_prefix("so") {
            cfg.stdout_bytes = v.parse().unwrap_or(0);
        } else if let Some(v) = token.strip_prefix("se") {
            cfg.stderr_bytes = v.parse().unwrap_or(0);
        } else if let Some(v) = token.strip_prefix("msg") {
            cfg.message = decode_hex(v);
        } else if let Some(v) = token.strip_prefix('x') {
            cfg.exit = v.parse().unwrap_or(0);
        }
    }
    cfg
}

fn decode_hex(s: &str) -> Option<String> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect();
    bytes.map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn main() {
    let cfg = parse_config();

    // Sidecar figé : réseau muet, serveur qui ne répond plus. C'est le cas où
    // seule l'annulation peut rendre la main.
    if cfg.sleep_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(cfg.sleep_ms));
    }

    // Rôle filtre : consomme intégralement stdin, comme ffmpeg. Sans cette
    // lecture, l'écrivain amont se bloquerait sur un pipe plein.
    if cfg.filter {
        let mut sink = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut sink);
    }

    if let Some(msg) = &cfg.message {
        let _ = writeln!(std::io::stderr(), "{msg}");
    }

    // stderr avant stdout : c'est l'ordre qui piège un appelant ne drainant pas
    // stderr, puisqu'il attendrait sur stdout des octets qui n'arriveront jamais.
    if cfg.stderr_bytes > 0 {
        let noise = vec![b'E'; 4096];
        let mut err = std::io::stderr();
        let mut written = 0;
        while written < cfg.stderr_bytes {
            let n = noise.len().min(cfg.stderr_bytes - written);
            if err.write_all(&noise[..n]).is_err() {
                break;
            }
            written += n;
        }
        let _ = err.flush();
    }

    if cfg.stdout_bytes > 0 {
        // Motif déterministe : compteur 16 bits little-endian, directement
        // vérifiable côté test après décodage PCM.
        let mut out = std::io::stdout();
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut counter: u16 = 0;
        let mut written = 0;
        while written < cfg.stdout_bytes {
            buf.clear();
            while buf.len() < 8192 {
                buf.extend_from_slice(&counter.to_le_bytes());
                counter = counter.wrapping_add(1);
            }
            let n = buf.len().min(cfg.stdout_bytes - written);
            if out.write_all(&buf[..n]).is_err() {
                break;
            }
            written += n;
        }
        let _ = out.flush();
    }

    std::process::exit(cfg.exit);
}
