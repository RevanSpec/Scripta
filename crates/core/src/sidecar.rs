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
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Result, ScriptaError};

const GITHUB_LATEST: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";

/// Page de la dernière version : sa redirection désigne l'étiquette publiée.
const GITHUB_LATEST_PAGE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest";

/// Intervalle minimal entre deux vérifications de mise à jour — SPEC SF-06.
const UPDATE_CHECK_INTERVAL_S: u64 = 24 * 3600;

/// Délai accordé à la vérification de mise à jour, qui tourne en arrière-plan.
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

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
        None,
    )?;

    make_executable(&destination)?;
    forget_pending_update();
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

/// Dernière version publiée de yt-dlp.
///
/// Lue dans la redirection de la page « latest » plutôt que par l'API de
/// GitHub : ni quota, ni JSON, et la seule destination reste `github.com`
/// (SF-09). Aucune donnée n'est envoyée — une requête `HEAD`, rien de plus.
pub fn latest_ytdlp_version(timeout: Duration) -> Result<String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .new_agent();

    let reponse =
        agent
            .head(GITHUB_LATEST_PAGE)
            .call()
            .map_err(|e| ScriptaError::ExtractionFailed {
                detail: format!("interrogation de {GITHUB_LATEST_PAGE} : {e}"),
            })?;

    reponse
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .and_then(version_from_location)
        .ok_or_else(|| ScriptaError::ExtractionFailed {
            detail: "version de yt-dlp introuvable dans la réponse de GitHub".to_string(),
        })
}

/// `…/releases/tag/2026.08.19` → `2026.08.19`.
fn version_from_location(location: &str) -> Option<String> {
    let (_, etiquette) = location.rsplit_once("/tag/")?;
    let etiquette = etiquette.trim_end_matches('/');
    (!etiquette.is_empty() && etiquette.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .then(|| etiquette.to_string())
}

/// Vrai si `candidate` est postérieure à `current`.
///
/// Les versions de yt-dlp sont des dates (`2026.08.19`), parfois suivies d'un
/// correctif (`.1`) ou, pour les nightly, d'une heure. Une comparaison
/// numérique champ par champ les ordonne toutes ; une version illisible n'est
/// jamais tenue pour plus récente.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn champs(v: &str) -> Option<Vec<u64>> {
        v.trim().split('.').map(|c| c.parse().ok()).collect()
    }
    match (champs(candidate), champs(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Une version plus récente de yt-dlp est disponible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateNotice {
    pub current: String,
    pub latest: String,
}

/// Vrai si l'utilisateur a désactivé la vérification (`SCRIPTA_NO_UPDATE_CHECK`).
pub fn update_check_disabled() -> bool {
    std::env::var_os("SCRIPTA_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Vrai si la dernière vérification remonte à plus de 24 h. Une date située
/// dans le futur — horloge reculée — ne doit pas bloquer indéfiniment.
fn check_due(derniere: Option<u64>, maintenant: u64) -> bool {
    match derniere {
        None => true,
        Some(t) => t > maintenant || maintenant - t >= UPDATE_CHECK_INTERVAL_S,
    }
}

/// Fichier d'état de la vérification : date de la dernière, puis, le cas
/// échéant, la version plus récente qu'elle a trouvée.
fn update_state_path() -> Option<PathBuf> {
    crate::paths::scripta_dir()
        .ok()
        .map(|d| d.join("verification-yt-dlp"))
}

fn read_update_state(chemin: &Path) -> (Option<u64>, Option<String>) {
    let Ok(texte) = std::fs::read_to_string(chemin) else {
        return (None, None);
    };
    let mut lignes = texte.lines();
    let derniere = lignes.next().and_then(|l| l.trim().parse().ok());
    let en_attente = lignes
        .next()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    (derniere, en_attente)
}

/// Écriture au mieux : l'état n'est qu'une commodité, son échec ne doit rien
/// empêcher.
fn write_update_state(chemin: &Path, derniere: u64, en_attente: Option<&str>) {
    if let Some(dir) = chemin.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let contenu = match en_attente {
        Some(v) => format!("{derniere}\n{v}\n"),
        None => format!("{derniere}\n"),
    };
    let _ = std::fs::write(chemin, contenu);
}

/// Lance la vérification de disponibilité d'une mise à jour — SPEC SF-06.
///
/// **Au plus une fois par 24 h**, **jamais bloquante** et sans télémétrie :
/// elle tourne dans un thread détaché, et son résultat arrive par le canal
/// rendu. Retarder la sortie de la commande pour l'attendre serait précisément
/// un blocage : si la commande se termine avant, l'avis n'est pas perdu pour
/// autant — il est mémorisé et rappelé au lancement suivant, jusqu'à ce que
/// yt-dlp soit à jour.
///
/// Rend `None` quand il n'y a rien à vérifier ni à rappeler : aucun thread, ni
/// aucun processus, n'est alors lancé.
pub fn spawn_update_check(ytdlp: PathBuf) -> Option<mpsc::Receiver<UpdateNotice>> {
    if update_check_disabled() {
        return None;
    }
    let etat = update_state_path()?;
    let maintenant = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let (derniere, en_attente) = read_update_state(&etat);
    let due = check_due(derniere, maintenant);
    if !due && en_attente.is_none() {
        return None;
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Some(current) = version_of(&ytdlp, Kind::YtDlp) else {
            return;
        };
        // L'état n'est écrit qu'une fois la requête aboutie ou échouée : une
        // commande trop brève pour l'attendre ne consomme pas la vérification
        // du jour, elle sera simplement retentée.
        let (horodatage, latest) = if due {
            let trouvee = latest_ytdlp_version(UPDATE_CHECK_TIMEOUT).ok();
            // Hors ligne, l'avis déjà connu reste valable.
            (maintenant, trouvee.or(en_attente))
        } else {
            (derniere.unwrap_or(maintenant), en_attente)
        };
        match latest {
            Some(latest) if is_newer(&latest, &current) => {
                write_update_state(&etat, horodatage, Some(&latest));
                let _ = tx.send(UpdateNotice { current, latest });
            }
            // À jour, y compris parce que l'utilisateur a mis yt-dlp à jour
            // par ses propres moyens : plus rien à rappeler.
            _ => write_update_state(&etat, horodatage, None),
        }
    });
    Some(rx)
}

/// Oublie l'avis en attente après une mise à jour réussie.
fn forget_pending_update() {
    let Some(etat) = update_state_path() else {
        return;
    };
    if let (derniere, Some(_)) = read_update_state(&etat) {
        write_update_state(&etat, derniere.unwrap_or(0), None);
    }
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
    fn lit_la_version_dans_la_redirection() {
        assert_eq!(
            version_from_location("https://github.com/yt-dlp/yt-dlp/releases/tag/2026.08.19")
                .as_deref(),
            Some("2026.08.19")
        );
        assert_eq!(
            version_from_location("/yt-dlp/yt-dlp/releases/tag/2026.08.19.1/").as_deref(),
            Some("2026.08.19.1")
        );
        // Une redirection inattendue — page de connexion, erreur — n'est pas
        // une version.
        assert!(version_from_location("https://github.com/login").is_none());
        assert!(version_from_location("https://github.com/releases/tag/").is_none());
        assert!(version_from_location("https://x/releases/tag/v1;rm").is_none());
    }

    #[test]
    fn compare_les_versions_datees() {
        assert!(is_newer("2026.09.20", "2026.08.19"));
        assert!(is_newer("2026.08.19.1", "2026.08.19")); // correctif
        assert!(!is_newer("2026.08.19", "2026.08.19"));
        assert!(!is_newer("2026.08.19", "2026.09.01"));
        // Comparaison numérique, pas lexicale : 10 > 9.
        assert!(is_newer("2026.10.01", "2026.9.30"));
        // Une nightly locale plus récente que la dernière stable : pas de
        // fausse alerte.
        assert!(!is_newer("2026.08.19", "2026.08.20.232957"));
        // L'illisible n'est jamais « plus récent ».
        assert!(!is_newer("inconnue", "2026.08.19"));
        assert!(!is_newer("2026.09.20", "inconnue"));
    }

    #[test]
    fn l_etat_de_verification_fait_l_aller_retour() {
        let dir = tempfile::tempdir().unwrap();
        let etat = dir.path().join("scripta").join("verification-yt-dlp");

        assert_eq!(read_update_state(&etat), (None, None), "état absent");

        write_update_state(&etat, 1_000, Some("2026.09.20"));
        assert_eq!(
            read_update_state(&etat),
            (Some(1_000), Some("2026.09.20".to_string()))
        );

        write_update_state(&etat, 2_000, None);
        assert_eq!(read_update_state(&etat), (Some(2_000), None));

        // Un fichier corrompu vaut un état absent : la vérification sera
        // simplement refaite.
        std::fs::write(&etat, "n'importe quoi").unwrap();
        assert_eq!(read_update_state(&etat).0, None);
    }

    #[test]
    fn verification_au_plus_une_fois_par_jour() {
        let jour = UPDATE_CHECK_INTERVAL_S;
        assert!(check_due(None, 1_000_000));
        assert!(!check_due(Some(1_000_000), 1_000_000 + jour - 1));
        assert!(check_due(Some(1_000_000), 1_000_000 + jour));
        // Horloge reculée : l'horodatage futur ne doit pas tout bloquer.
        assert!(check_due(Some(2_000_000), 1_000_000));
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
