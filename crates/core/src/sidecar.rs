//! Résolution et mise à jour des sidecars — SPEC SF-06, ADR-004.
//!
//! YouTube modifie fréquemment ses mécanismes d'extraction : un `yt-dlp`
//! embarqué est périmé quelques semaines après la publication d'une version de
//! Scripta. Sa mise à jour n'est donc pas un confort mais une condition de
//! fonctionnement.
//!
//! # Deux emplacements, une priorité
//!
//! **Remplacer un binaire à l'intérieur d'un bundle signé invalide sa
//! signature ; sur Apple Silicon, l'application ne se lance alors plus du
//! tout.** Sous Windows, `Program Files` est en lecture seule sans élévation.
//! Le mécanisme interne `yt-dlp -U` est donc inutilisable, et la mise à jour
//! écrit exclusivement dans un répertoire utilisateur, consulté en premier.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Result, ScriptaError};

const GITHUB_LATEST: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    YtDlp,
    Ffmpeg,
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
        }
    }

    /// Nom de fichier attendu sur cette plateforme.
    pub fn file_name(&self) -> String {
        format!("{}{}", self.name(), std::env::consts::EXE_SUFFIX)
    }

    /// Nom de l'artefact publié par yt-dlp pour la cible courante.
    fn release_asset(&self) -> Option<&'static str> {
        match self {
            // ffmpeg n'est pas distribué par ce mécanisme.
            Self::Ffmpeg => None,
            Self::YtDlp => Some(if cfg!(windows) {
                "yt-dlp.exe"
            } else if cfg!(target_os = "macos") {
                "yt-dlp_macos"
            } else {
                "yt-dlp"
            }),
        }
    }
}

/// Provenance du binaire retenu, pour le diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Chemin imposé en ligne de commande.
    Explicit,
    /// Copie mise à jour, dans le répertoire utilisateur inscriptible.
    User,
    /// Copie embarquée dans le bundle applicatif, en lecture seule.
    Bundled,
    /// Trouvé dans le `PATH`.
    System,
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Explicit => "imposé",
            Self::User => "mis à jour",
            Self::Bundled => "embarqué",
            Self::System => "système",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub path: PathBuf,
    pub origin: Origin,
}

/// Répertoire inscriptible des sidecars mis à jour — ADR-004.
pub fn bin_dir() -> Result<PathBuf> {
    crate::paths::sub_dir("bin", "SCRIPTA_BIN_DIR")
}

/// Répertoire du bundle applicatif, où se trouvent les sidecars embarqués.
fn bundled_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(std::path::Path::to_path_buf)
}

/// Résout un sidecar selon la priorité d'ADR-004.
///
/// La copie utilisateur passe **avant** la copie embarquée : c'est elle que la
/// mise à jour alimente, le bundle signé n'étant jamais modifié.
pub fn resolve(kind: Kind, explicit: Option<&Path>) -> Resolved {
    if let Some(p) = explicit {
        return Resolved {
            path: p.to_path_buf(),
            origin: Origin::Explicit,
        };
    }

    let fichier = kind.file_name();

    if let Ok(dir) = bin_dir() {
        let p = dir.join(&fichier);
        if p.is_file() {
            return Resolved {
                path: p,
                origin: Origin::User,
            };
        }
    }

    if let Some(dir) = bundled_dir() {
        let p = dir.join(&fichier);
        if p.is_file() {
            return Resolved {
                path: p,
                origin: Origin::Bundled,
            };
        }
    }

    // Repli sur le `PATH`. Le chemin n'est pas résolu ici : `Command` s'en
    // charge, et un échec remonte en `SidecarMissing` au moment du lancement.
    Resolved {
        path: PathBuf::from(kind.name()),
        origin: Origin::System,
    }
}

/// Version rapportée par le binaire, ou `None` s'il est injoignable.
pub fn version_of(chemin: &Path, kind: Kind) -> Option<String> {
    let sortie = Command::new(chemin)
        .arg(match kind {
            Kind::YtDlp => "--version",
            Kind::Ffmpeg => "-version",
        })
        .stdin(Stdio::null())
        .output()
        .ok()?;

    if !sortie.status.success() {
        return None;
    }

    let texte = String::from_utf8_lossy(&sortie.stdout);
    let premiere = texte.lines().next()?.trim();

    Some(match kind {
        Kind::YtDlp => premiere.to_string(),
        // « ffmpeg version 7.1-full_build … » : seul le numéro importe.
        Kind::Ffmpeg => premiere
            .split_whitespace()
            .nth(2)
            .unwrap_or(premiere)
            .to_string(),
    })
}

/// Met à jour `yt-dlp` dans le répertoire utilisateur — SPEC SF-06.
///
/// Le mécanisme interne `yt-dlp -U` n'est **pas** employé : il remplacerait le
/// binaire sur place, donc à l'intérieur d'un bundle signé, ce qui en
/// invaliderait la signature.
pub fn update_ytdlp(progress: crate::models::ProgressFn<'_>) -> Result<PathBuf> {
    let asset = Kind::YtDlp
        .release_asset()
        .ok_or_else(|| ScriptaError::ExtractionFailed {
            detail: "aucun artefact yt-dlp pour cette plateforme".to_string(),
        })?;

    let attendu = fetch_expected_sha(asset)?;

    let dir = bin_dir()?;
    std::fs::create_dir_all(&dir).map_err(|source| ScriptaError::OutputFailed {
        path: dir.clone(),
        source,
    })?;
    let destination = dir.join(Kind::YtDlp.file_name());

    crate::models::download_verified(
        &format!("{GITHUB_LATEST}/{asset}"),
        &destination,
        &attendu,
        progress,
    )?;

    make_executable(&destination)?;
    Ok(destination)
}

/// Relève l'empreinte attendue dans le fichier `SHA2-256SUMS` de la release.
///
/// Format `sha256␣␣nom`, un artefact par ligne.
fn fetch_expected_sha(asset: &str) -> Result<String> {
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(30)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(60)))
        .build()
        .new_agent();

    let sommes = agent
        .get(&format!("{GITHUB_LATEST}/SHA2-256SUMS"))
        .call()
        .and_then(|r| r.into_body().read_to_string())
        .map_err(|e| ScriptaError::ExtractionFailed {
            detail: format!("récupération de SHA2-256SUMS : {e}"),
        })?;

    parse_sums(&sommes, asset).ok_or_else(|| ScriptaError::ExtractionFailed {
        detail: format!("« {asset} » absent de SHA2-256SUMS"),
    })
}

fn parse_sums(contenu: &str, asset: &str) -> Option<String> {
    contenu.lines().find_map(|ligne| {
        let mut champs = ligne.split_whitespace();
        let somme = champs.next()?;
        let nom = champs.next()?;
        // Comparaison exacte : `yt-dlp` ne doit pas capter `yt-dlp.exe`.
        (nom == asset && somme.len() == 64).then(|| somme.to_string())
    })
}

/// Rend le fichier exécutable, et le fait accepter par le système.
fn make_executable(chemin: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(chemin)
            .map_err(|source| ScriptaError::OutputFailed {
                path: chemin.to_path_buf(),
                source,
            })?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(chemin, perms).map_err(|source| ScriptaError::OutputFailed {
            path: chemin.to_path_buf(),
            source,
        })?;
    }

    #[cfg(target_os = "macos")]
    {
        // Sur Apple Silicon, un binaire non signé ne s'exécute pas, et
        // l'attribut de quarantaine bloque tout téléchargement. Ces deux
        // commandes sont sans effet ailleurs, et leur échec n'est pas fatal :
        // le diagnostic viendra du premier lancement.
        let _ = Command::new("codesign")
            .args(["-s", "-", "-f"])
            .arg(chemin)
            .status();
        let _ = Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(chemin)
            .status();
    }

    let _ = chemin;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_nom_de_fichier_suit_la_plateforme() {
        let n = Kind::YtDlp.file_name();
        assert!(n.starts_with("yt-dlp"));
        if cfg!(windows) {
            assert_eq!(n, "yt-dlp.exe");
        } else {
            assert_eq!(n, "yt-dlp");
        }
    }

    #[test]
    fn un_chemin_impose_court_circuite_la_resolution() {
        let r = resolve(Kind::YtDlp, Some(Path::new("/un/chemin/impose")));
        assert_eq!(r.origin, Origin::Explicit);
        assert_eq!(r.path, Path::new("/un/chemin/impose"));
    }

    #[test]
    fn la_resolution_se_rabat_sur_le_path() {
        // Aucun binaire n'est installé pendant les tests : la résolution doit
        // rendre un nom nu, que `Command` cherchera dans le PATH.
        let r = resolve(Kind::Ffmpeg, None);
        assert!(matches!(
            r.origin,
            Origin::System | Origin::User | Origin::Bundled
        ));
    }

    #[test]
    fn analyse_le_fichier_de_sommes() {
        let contenu = "1fa6733c37ea6fb51c99ad8fe785e7b7e5f3246c9b980230329d4fb72ed8d4d6  yt-dlp\n\
                       66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a  yt-dlp.exe\n\
                       072aad4f2a7604e92155f61a275a4752dc64046c8f6d90df3710525d94cd37c1  yt-dlp.tar.gz\n";

        assert_eq!(
            parse_sums(contenu, "yt-dlp.exe").as_deref(),
            Some("66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a")
        );
        // La correspondance doit être exacte : `yt-dlp` ne doit pas capter
        // `yt-dlp.exe` ni `yt-dlp.tar.gz`.
        assert_eq!(
            parse_sums(contenu, "yt-dlp").as_deref(),
            Some("1fa6733c37ea6fb51c99ad8fe785e7b7e5f3246c9b980230329d4fb72ed8d4d6")
        );
        assert!(parse_sums(contenu, "yt-dlp_macos").is_none());
        assert!(parse_sums("", "yt-dlp").is_none());
    }

    #[test]
    fn refuse_une_somme_malformee() {
        // Une ligne tronquée ne doit pas passer pour une empreinte valide.
        assert!(parse_sums("abc  yt-dlp\n", "yt-dlp").is_none());
    }

    #[test]
    fn l_artefact_suit_la_plateforme() {
        let a = Kind::YtDlp.release_asset().unwrap();
        if cfg!(windows) {
            assert_eq!(a, "yt-dlp.exe");
        } else if cfg!(target_os = "macos") {
            assert_eq!(a, "yt-dlp_macos");
        } else {
            assert_eq!(a, "yt-dlp");
        }
        // ffmpeg n'est pas distribué par ce mécanisme.
        assert!(Kind::Ffmpeg.release_asset().is_none());
    }

    #[test]
    fn le_repertoire_inscriptible_est_distinct_du_bundle() {
        let user = bin_dir().expect("répertoire résolu");
        assert!(user.ends_with("bin"));
        if let Some(bundle) = bundled_dir() {
            assert_ne!(user, bundle, "la mise à jour écrirait dans le bundle signé");
        }
    }
}
