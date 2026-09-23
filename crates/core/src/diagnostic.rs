//! Briques du diagnostic d'installation — SPEC SF-07 (`scripta doctor`).
//!
//! Chaque vérification rend un constat plutôt qu'une erreur : un diagnostic
//! doit aller jusqu'au bout et tout rapporter, pas s'arrêter au premier
//! problème rencontré.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::time::{Duration, Instant};

/// Destination réseau utilisée par Scripta — SPEC SF-09, qui en fixe la liste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    pub name: &'static str,
    /// À quoi sert cette destination, pour un diagnostic lisible.
    pub purpose: &'static str,
    pub url: &'static str,
}

/// Les trois seules destinations réseau autorisées (SF-09).
pub const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        name: "YouTube",
        purpose: "extraction",
        url: "https://www.youtube.com",
    },
    Endpoint {
        name: "HuggingFace",
        purpose: "modèles",
        url: "https://huggingface.co",
    },
    Endpoint {
        name: "GitHub",
        purpose: "mise à jour de yt-dlp",
        url: "https://github.com",
    },
];

/// Vérifie qu'on peut créer des fichiers dans `dir`, **sans créer `dir`**.
///
/// Si le répertoire n'existe pas encore — cas normal d'une installation
/// neuve —, c'est son plus proche ancêtre existant qui est éprouvé : c'est là
/// que la première écriture devra le créer. Un fichier sonde est créé puis
/// aussitôt supprimé.
pub fn check_writable(dir: &Path) -> std::io::Result<()> {
    let mut cible = dir;
    while !cible.exists() {
        cible = cible
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| {
                std::io::Error::new(ErrorKind::NotFound, "aucun répertoire parent existant")
            })?;
    }
    if !cible.is_dir() {
        return Err(std::io::Error::new(
            ErrorKind::NotADirectory,
            format!("« {} » n'est pas un répertoire", cible.display()),
        ));
    }

    let sonde = cible.join(format!(".scripta-sonde-{}", std::process::id()));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&sonde)?;
    fs::remove_file(&sonde)
}

/// Vérifie qu'un hôte répond, et rend le temps de réponse.
///
/// Toute réponse HTTP, même une erreur, prouve que l'hôte est joignable :
/// seul l'échec du transport compte ici. Les redirections ne sont pas suivies,
/// pour ne jamais sortir de la liste des destinations autorisées (§5.1).
pub fn check_reachable(url: &str, timeout: Duration) -> Result<Duration, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .new_agent();

    let debut = Instant::now();
    agent
        .head(url)
        .call()
        .map(|_| debut.elapsed())
        .map_err(|e| e.to_string())
}

/// Vérifie les destinations de [`ENDPOINTS`] en parallèle : hors ligne, le
/// diagnostic ne dure qu'un délai d'attente, pas trois.
pub fn check_endpoints(timeout: Duration) -> Vec<(Endpoint, Result<Duration, String>)> {
    let taches: Vec<_> = ENDPOINTS
        .iter()
        .map(|e| {
            let e = *e;
            (
                e,
                std::thread::spawn(move || check_reachable(e.url, timeout)),
            )
        })
        .collect();

    taches
        .into_iter()
        .map(|(e, t)| {
            let issue = t
                .join()
                .unwrap_or_else(|_| Err("vérification interrompue".to_string()));
            (e, issue)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn un_repertoire_temporaire_est_inscriptible() {
        let dir = tempfile::tempdir().unwrap();
        check_writable(dir.path()).expect("inscriptible");
        // La sonde ne laisse aucune trace.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn un_repertoire_a_creer_est_eprouve_par_son_ancetre() {
        let dir = tempfile::tempdir().unwrap();
        let futur = dir.path().join("scripta").join("models");
        check_writable(&futur).expect("ancêtre inscriptible");
        // Le diagnostic ne crée rien.
        assert!(!futur.exists());
    }

    #[test]
    fn un_fichier_a_la_place_d_un_repertoire_est_signale() {
        // Cas d'une variable SCRIPTA_*_DIR mal renseignée.
        let dir = tempfile::tempdir().unwrap();
        let fichier = dir.path().join("pas-un-repertoire");
        fs::write(&fichier, "x").unwrap();
        assert!(check_writable(&fichier.join("models")).is_err());
    }

    #[test]
    fn un_hote_local_qui_repond_est_joignable() {
        let ecoute = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = ecoute.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut flux, _)) = ecoute.accept() {
                let mut tampon = [0u8; 1024];
                let _ = flux.read(&mut tampon);
                // Une erreur HTTP prouve aussi que l'hôte est joignable.
                let _ = flux.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
            }
        });

        let issue = check_reachable(&format!("http://127.0.0.1:{port}/"), Duration::from_secs(5));
        assert!(issue.is_ok(), "{issue:?}");
    }

    #[test]
    fn un_port_ferme_est_injoignable() {
        // Port obtenu du système puis libéré : plus rien n'y écoute.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let issue = check_reachable(&format!("http://127.0.0.1:{port}/"), Duration::from_secs(5));
        assert!(issue.is_err());
    }

    #[test]
    fn la_liste_des_destinations_suit_sf09() {
        // SF-09 : youtube.com, huggingface.co et github.com, rien d'autre.
        let hotes: Vec<_> = ENDPOINTS.iter().map(|e| e.url).collect();
        assert_eq!(
            hotes,
            [
                "https://www.youtube.com",
                "https://huggingface.co",
                "https://github.com"
            ]
        );
    }
}
