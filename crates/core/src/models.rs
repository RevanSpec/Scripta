//! Gestion et cycle de vie des modèles Whisper — SPEC SF-03.
//!
//! Les modèles sont téléchargés à la demande depuis HuggingFace, vérifiés par
//! empreinte SHA-256, et conservés dans un cache propre à la plateforme.
//!
//! # Épinglage
//!
//! Chaque modèle est référencé par une **révision de dépôt figée**, jamais par
//! `main` : une branche n'est pas reproductible, et l'empreinte embarquée ici
//! cesserait d'être valable au premier commit amont.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::cancel::{self, CancelToken};
use crate::error::{Result, ScriptaError};

/// Délai d'établissement de la connexion.
const CONNECT_TIMEOUT_S: u64 = 30;
/// Délai de réception d'une tranche.
///
/// ureq borne la réception d'un corps **entier** (`timeout_recv_body`), pas
/// un silence du serveur. Demandé d'une traite, un modèle échouait donc dès
/// qu'il fallait plus d'une minute pour le recevoir : 547 Mo pour `turbo`,
/// soit toute connexion sous 9 Mo/s. La reprise sauvait les octets, pas
/// l'utilisateur, qui voyait l'erreur à chaque minute. D'où les tranches.
const TRANCHE_TIMEOUT_S: u64 = 60;
/// Taille d'une tranche : 16 Mio par minute, soit un débit plancher de
/// 280 Ko/s environ, sous lequel une tranche échoue — et la suivante tentative
/// reprend là où elle s'est arrêtée.
const TRANCHE: u64 = 16 * 1024 * 1024;

const HF: &str = "https://huggingface.co";

/// Un modèle distribuable, avec de quoi l'obtenir et le vérifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    /// Nom court accepté par `--model`.
    pub alias: &'static str,
    pub file: &'static str,
    pub repo: &'static str,
    /// Révision figée du dépôt.
    pub revision: &'static str,
    /// SHA-256 du contenu, relevé sur l'en-tête `X-Linked-ETag` de HuggingFace
    /// et confronté au fichier téléchargé.
    pub sha256: &'static str,
    pub size: u64,
}

impl ModelSpec {
    pub fn url(&self) -> String {
        format!("{HF}/{}/resolve/{}/{}", self.repo, self.revision, self.file)
    }

    /// Taille en mégaoctets, pour l'affichage.
    pub fn size_mb(&self) -> u64 {
        self.size / 1_048_576
    }

    /// Vrai si le modèle ne sait pas traduire — voir
    /// [`crate::transcribe::check_translate_supported`].
    pub fn is_turbo(&self) -> bool {
        self.alias.contains("turbo")
    }
}

const WHISPER_REPO: &str = "ggerganov/whisper.cpp";
const WHISPER_REV: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

/// Catalogue des modèles — SPEC SF-03.
///
/// Les variantes quantifiées sont privilégiées au-delà de `base` : à qualité
/// comparable elles divisent la taille par deux ou trois.
pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        alias: "tiny",
        file: "ggml-tiny.bin",
        repo: WHISPER_REPO,
        revision: WHISPER_REV,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        size: 77_691_713,
    },
    ModelSpec {
        alias: "base",
        file: "ggml-base.bin",
        repo: WHISPER_REPO,
        revision: WHISPER_REV,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        size: 147_951_465,
    },
    ModelSpec {
        alias: "small",
        file: "ggml-small-q5_1.bin",
        repo: WHISPER_REPO,
        revision: WHISPER_REV,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
        size: 190_085_487,
    },
    ModelSpec {
        alias: "medium",
        file: "ggml-medium-q5_0.bin",
        repo: WHISPER_REPO,
        revision: WHISPER_REV,
        sha256: "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
        size: 539_212_467,
    },
    ModelSpec {
        alias: "large-v3",
        file: "ggml-large-v3-q5_0.bin",
        repo: WHISPER_REPO,
        revision: WHISPER_REV,
        sha256: "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
        size: 1_081_140_203,
    },
    ModelSpec {
        alias: "turbo",
        file: "ggml-large-v3-turbo-q5_0.bin",
        repo: WHISPER_REPO,
        revision: WHISPER_REV,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        size: 574_041_195,
    },
];

/// Modèle VAD Silero — SPEC SF-03, SF-04. Il vit dans un dépôt distinct, et
/// suit le même cycle de vie que les modèles Whisper : téléchargé à la première
/// utilisation, vérifié par empreinte.
pub const VAD_MODEL: ModelSpec = ModelSpec {
    alias: "silero",
    file: "ggml-silero-v5.1.2.bin",
    repo: "ggml-org/whisper-vad",
    revision: "9ffd54a1e1ee413ddf265af9913beaf518d1639b",
    sha256: "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf",
    size: 885_098,
};

/// Résout un alias. `auto` choisit selon le backend compilé (SF-03).
pub fn find(alias: &str) -> Option<&'static ModelSpec> {
    if alias == "auto" {
        // Un GPU absorbe le surcoût de `turbo` ; sur CPU il serait pénalisant.
        let vise = if crate::transcribe::Backend::compiled().is_gpu() {
            "turbo"
        } else {
            "base"
        };
        return MODELS.iter().find(|m| m.alias == vise);
    }
    MODELS.iter().find(|m| m.alias == alias)
}

/// Résout un alias de **tout** le catalogue, modèle VAD compris.
///
/// Réservé à la gestion du cache (`scripta models pull|rm|verify`) : `--model`
/// passe par [`find`], puisque le modèle VAD ne sait pas transcrire.
pub fn find_any(alias: &str) -> Option<&'static ModelSpec> {
    find(alias).or_else(|| (alias == VAD_MODEL.alias || alias == "vad").then_some(&VAD_MODEL))
}

pub fn aliases() -> Vec<&'static str> {
    MODELS.iter().map(|m| m.alias).collect()
}

/// Répertoire de cache des modèles — SPEC SF-03.
///
/// `SCRIPTA_MODELS_DIR` prime sur tout, ce qui permet de partager un cache
/// entre plusieurs installations ou de le placer sur un autre volume.
pub fn models_dir() -> Result<PathBuf> {
    crate::paths::sub_dir("models", "SCRIPTA_MODELS_DIR")
}

pub fn path_of(spec: &ModelSpec) -> Result<PathBuf> {
    Ok(models_dir()?.join(spec.file))
}

/// Progression d'un téléchargement : octets reçus, total attendu.
pub type ProgressFn<'a> = &'a mut dyn FnMut(u64, u64);

/// Garantit la présence locale du modèle et retourne son chemin.
///
/// Un fichier déjà présent n'est **pas** rehaché : sur un modèle d'un
/// gigaoctet, la vérification coûterait plusieurs secondes à chaque lancement.
/// Sa taille est en revanche contrôlée, ce qui est gratuit et détecte la
/// troncature — de loin le mode de corruption le plus courant. La vérification
/// complète est disponible à la demande via [`verify`].
///
/// Un jeton armé interrompt le téléchargement en conservant le fichier `.part`,
/// que la tentative suivante reprendra là où celle-ci s'est arrêtée.
pub fn ensure(
    spec: &ModelSpec,
    progress: ProgressFn<'_>,
    cancel: Option<&CancelToken>,
) -> Result<PathBuf> {
    let destination = path_of(spec)?;

    if let Ok(meta) = fs::metadata(&destination) {
        if meta.len() == spec.size {
            return Ok(destination);
        }
        // Taille inattendue : téléchargement précédent interrompu, ou fichier
        // substitué. On le remplace plutôt que de laisser whisper échouer avec
        // un message incompréhensible.
        let _ = fs::remove_file(&destination);
    }

    let dossier = destination
        .parent()
        .ok_or_else(|| ScriptaError::ModelUnavailable {
            detail: format!("chemin de modèle invalide : {}", destination.display()),
        })?;
    fs::create_dir_all(dossier).map_err(|e| ScriptaError::ModelUnavailable {
        detail: format!("création de {} : {e}", dossier.display()),
    })?;

    download_verified(&spec.url(), &destination, spec.sha256, progress, cancel)?;
    Ok(destination)
}

/// Télécharge un fichier, vérifie son empreinte, puis l'installe atomiquement.
///
/// Mutualisé avec la mise à jour des sidecars (SF-06) : les deux ont les mêmes
/// exigences — reprise, vérification, et surtout l'impossibilité de laisser en
/// place un fichier tronqué qui serait ensuite tenu pour valide.
pub fn download_verified(
    url: &str,
    destination: &Path,
    sha256: &str,
    progress: ProgressFn<'_>,
    cancel: Option<&CancelToken>,
) -> Result<()> {
    let partiel = destination.with_extension("part");
    telecharger(url, &partiel, progress, cancel)?;

    let empreinte = hash_file(&partiel)?;
    if empreinte != sha256 {
        let _ = fs::remove_file(&partiel);
        return Err(ScriptaError::ModelUnavailable {
            detail: format!(
                "empreinte invalide pour {} : attendu {sha256}, obtenu {empreinte}",
                destination.display()
            ),
        });
    }

    // Renommage atomique : un Ctrl-C pendant le transfert ne laisse jamais un
    // fichier tronqué qui serait ensuite tenu pour valide.
    fs::rename(&partiel, destination).map_err(|e| ScriptaError::ModelUnavailable {
        detail: format!("installation de {} : {e}", destination.display()),
    })?;

    Ok(())
}

fn telecharger(
    url: &str,
    partiel: &Path,
    progress: ProgressFn<'_>,
    cancel: Option<&CancelToken>,
) -> Result<()> {
    telecharger_par_tranches(url, partiel, TRANCHE, progress, cancel)
}

/// Télécharge `url` dans `partiel`, par requêtes `Range` successives.
///
/// Chaque tranche a son propre délai ([`TRANCHE_TIMEOUT_S`]). Un `.part` déjà
/// présent provient d'un transfert interrompu : il est repris là où il s'est
/// arrêté — sur un fichier d'un gigaoctet, tout reprendre serait coûteux. Un
/// serveur qui ignore `Range` renvoie tout, qu'il faut prendre depuis zéro.
fn telecharger_par_tranches(
    url: &str,
    partiel: &Path,
    tranche: u64,
    progress: ProgressFn<'_>,
    cancel: Option<&CancelToken>,
) -> Result<()> {
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(CONNECT_TIMEOUT_S)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(TRANCHE_TIMEOUT_S)))
        .build()
        .new_agent();
    let echec = |detail: String| ScriptaError::ModelUnavailable {
        detail: format!("téléchargement de {url} : {detail}"),
    };

    let mut deja = fs::metadata(partiel).map(|m| m.len()).unwrap_or(0);
    let mut fichier = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(partiel)
        .map_err(|e| io_err(partiel, e))?;
    let mut total: Option<u64> = None;
    let mut tampon = vec![0u8; 256 * 1024];

    while total.is_none_or(|t| deja < t) {
        let reponse = match agent
            .get(url)
            .header("Range", &format!("bytes={deja}-{}", deja + tranche - 1))
            .call()
        {
            Ok(r) => r,
            // Rien au-delà de ce qui est déjà là : le `.part` est complet.
            // L'empreinte, vérifiée ensuite, en jugera.
            Err(ureq::Error::StatusCode(416)) => break,
            Err(e) => return Err(echec(e.to_string())),
        };

        // 206 : le serveur honore la tranche. Sinon il renvoie tout.
        let entier = reponse.status().as_u16() != 206;
        if entier {
            // Recréé plutôt que tronqué : sous Windows, un fichier ouvert en
            // ajout refuse la troncature.
            fichier = File::create(partiel).map_err(|e| io_err(partiel, e))?;
            deja = 0;
            total = en_tete(reponse.headers(), "content-length").and_then(|v| v.parse().ok());
        } else {
            let (debut, annonce) = en_tete(reponse.headers(), "content-range")
                .and_then(plage)
                .ok_or_else(|| echec("Content-Range illisible".to_string()))?;
            if debut != deja {
                return Err(echec(format!(
                    "tranche décalée : {debut} au lieu de {deja}"
                )));
            }
            total = annonce;
        }

        let avant = deja;
        let mut corps = reponse.into_body().into_reader();
        progress(deja, total.unwrap_or(0));
        loop {
            if cancel::is_cancelled(cancel) {
                // Le `.part` reste en place, complet jusqu'au dernier bloc
                // écrit : la prochaine tentative le reprendra.
                fichier.flush().map_err(|e| io_err(partiel, e))?;
                return Err(ScriptaError::Interrupted);
            }
            let n = corps.read(&mut tampon).map_err(|e| io_err(partiel, e))?;
            if n == 0 {
                break;
            }
            fichier
                .write_all(&tampon[..n])
                .map_err(|e| io_err(partiel, e))?;
            deja += n as u64;
            progress(deja, total.unwrap_or(0));
        }

        // Un corps entier est fini. Sans total annoncé, une tranche plus
        // courte que demandé marque la fin ; une tranche vide aussi, faute de
        // quoi la boucle ne s'arrêterait pas.
        let recu = deja - avant;
        if entier || recu == 0 || (total.is_none() && recu < tranche) {
            break;
        }
    }
    fichier.flush().map_err(|e| io_err(partiel, e))?;

    Ok(())
}

fn en_tete<'a>(entetes: &'a ureq::http::HeaderMap, nom: &str) -> Option<&'a str> {
    entetes.get(nom)?.to_str().ok()
}

/// `bytes 0-16777215/574041195` → début de la tranche et taille totale, si
/// le serveur l'annonce.
fn plage(valeur: &str) -> Option<(u64, Option<u64>)> {
    let (unite, reste) = valeur.split_once(' ')?;
    if unite != "bytes" {
        return None;
    }
    let (intervalle, total) = reste.split_once('/')?;
    let debut = intervalle.split_once('-')?.0.parse().ok()?;
    Some((debut, total.parse().ok()))
}

fn io_err(chemin: &Path, e: std::io::Error) -> ScriptaError {
    ScriptaError::ModelUnavailable {
        detail: format!("{} : {e}", chemin.display()),
    }
}

/// Vérifie l'empreinte d'un modèle déjà installé.
pub fn verify(spec: &ModelSpec) -> Result<bool> {
    let chemin = path_of(spec)?;
    if !chemin.is_file() {
        return Ok(false);
    }
    Ok(hash_file(&chemin)? == spec.sha256)
}

fn hash_file(chemin: &Path) -> Result<String> {
    let mut fichier = File::open(chemin).map_err(|e| io_err(chemin, e))?;
    let mut hacheur = Sha256::new();
    let mut tampon = vec![0u8; 256 * 1024];
    loop {
        let n = fichier.read(&mut tampon).map_err(|e| io_err(chemin, e))?;
        if n == 0 {
            break;
        }
        hacheur.update(&tampon[..n]);
    }
    Ok(hacheur
        .finalize()
        .iter()
        .map(|o| format!("{o:02x}"))
        .collect())
}

/// Vrai si le modèle est présent avec la taille attendue.
///
/// Même contrôle que [`ensure`] : la taille, gratuite, plutôt que l'empreinte,
/// qui coûterait plusieurs secondes sur un gros modèle.
pub fn is_installed(spec: &ModelSpec) -> Result<bool> {
    Ok(fs::metadata(path_of(spec)?)
        .map(|meta| meta.len() == spec.size)
        .unwrap_or(false))
}

/// Modèles Whisper présents dans le cache, avec leur état.
///
/// Le modèle VAD n'y figure pas : il ne se choisit pas comme modèle de
/// transcription. Voir [`VAD_MODEL`] et [`is_installed`].
pub fn installed() -> Result<Vec<(&'static ModelSpec, bool)>> {
    MODELS.iter().map(|m| Ok((m, is_installed(m)?))).collect()
}

pub fn remove(spec: &ModelSpec) -> Result<bool> {
    let chemin = path_of(spec)?;
    if !chemin.exists() {
        return Ok(false);
    }
    fs::remove_file(&chemin).map_err(|e| io_err(&chemin, e))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_coherent() {
        for m in MODELS.iter().chain(std::iter::once(&VAD_MODEL)) {
            assert_eq!(m.sha256.len(), 64, "empreinte mal formée : {}", m.alias);
            assert!(
                m.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "empreinte non hexadécimale : {}",
                m.alias
            );
            assert!(m.size > 0, "taille nulle : {}", m.alias);
            assert!(m.file.ends_with(".bin"), "fichier inattendu : {}", m.file);
        }
    }

    #[test]
    fn revisions_epinglees_jamais_une_branche() {
        // Une URL de branche n'est pas reproductible : l'empreinte embarquée
        // cesserait d'être valable au premier commit amont.
        for m in MODELS.iter().chain(std::iter::once(&VAD_MODEL)) {
            assert_eq!(m.revision.len(), 40, "révision non figée : {}", m.alias);
            assert!(m.revision.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(
                !m.url().contains("/main/"),
                "branche dans l'URL : {}",
                m.alias
            );
        }
    }

    #[test]
    fn aliases_uniques() {
        let mut vus = std::collections::HashSet::new();
        for m in MODELS {
            assert!(vus.insert(m.alias), "alias en double : {}", m.alias);
        }
    }

    #[test]
    fn resolution_des_alias() {
        assert_eq!(find("base").map(|m| m.file), Some("ggml-base.bin"));
        assert_eq!(find("turbo").map(|m| m.alias), Some("turbo"));
        assert!(find("inconnu").is_none());
        // `auto` doit toujours résoudre, quel que soit le backend compilé.
        assert!(find("auto").is_some());
    }

    #[test]
    fn le_modele_vad_ne_se_choisit_pas_pour_transcrire() {
        // `--model silero` chargerait un modèle incapable de transcrire.
        assert!(find("silero").is_none());
        assert!(find("vad").is_none());
        // Mais il se gère comme les autres dans le cache.
        assert_eq!(find_any("silero"), Some(&VAD_MODEL));
        assert_eq!(find_any("vad"), Some(&VAD_MODEL));
        assert_eq!(find_any("base").map(|m| m.alias), Some("base"));
        assert!(find_any("inconnu").is_none());
    }

    #[test]
    fn le_modele_vad_n_est_pas_liste_parmi_les_modeles_de_transcription() {
        let liste = installed().expect("liste des modèles");
        assert_eq!(liste.len(), MODELS.len());
        assert!(liste.iter().all(|(m, _)| m.alias != VAD_MODEL.alias));
    }

    #[test]
    fn auto_suit_le_backend_compile() {
        let choisi = find("auto").unwrap();
        if crate::transcribe::Backend::compiled().is_gpu() {
            assert_eq!(choisi.alias, "turbo");
        } else {
            // Sur CPU, turbo serait pénalisant.
            assert_eq!(choisi.alias, "base");
        }
    }

    #[test]
    fn url_construite_sur_la_revision() {
        let m = find("tiny").unwrap();
        assert_eq!(
            m.url(),
            format!("{HF}/{WHISPER_REPO}/resolve/{WHISPER_REV}/ggml-tiny.bin")
        );
    }

    #[test]
    fn seul_turbo_est_signale_comme_tel() {
        assert!(find("turbo").unwrap().is_turbo());
        assert!(!find("large-v3").unwrap().is_turbo());
        assert!(!find("base").unwrap().is_turbo());
    }

    #[test]
    fn la_variable_d_environnement_prime() {
        // Lecture seule : on n'écrit pas dans l'environnement, ce qui rendrait
        // les tests dépendants de leur ordre d'exécution.
        let dir = models_dir().expect("répertoire résolu");
        assert!(dir.ends_with("models") || std::env::var_os("SCRIPTA_MODELS_DIR").is_some());
    }

    #[test]
    fn empreinte_d_un_contenu_connu() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"abc").unwrap();
        // Vecteur de test standard de SHA-256.
        assert_eq!(
            hash_file(f.path()).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn lit_la_plage_d_une_tranche() {
        assert_eq!(
            plage("bytes 0-16777215/574041195"),
            Some((0, Some(574_041_195)))
        );
        // Taille totale inconnue du serveur.
        assert_eq!(plage("bytes 5-9/*"), Some((5, None)));
        assert_eq!(plage("octets 0-1/2"), None);
        assert_eq!(plage("bytes */574041195"), None);
    }

    /// Serveur HTTP minimal : sert `donnees`, honore `Range` — sauf si
    /// `ignore_range` —, et consigne les plages demandées.
    fn serveur(
        donnees: Vec<u8>,
        ignore_range: bool,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader};

        let ecoute = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/modele.bin", ecoute.local_addr().unwrap());
        let plages = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let journal = std::sync::Arc::clone(&plages);
        std::thread::spawn(move || {
            for flux in ecoute.incoming() {
                let Ok(mut flux) = flux else { continue };
                let mut lecteur = BufReader::new(flux.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut ligne = String::new();
                    if lecteur.read_line(&mut ligne).unwrap_or(0) == 0 || ligne == "\r\n" {
                        break;
                    }
                    if let Some(v) = ligne.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = Some(v.trim().to_string());
                    }
                }
                journal
                    .lock()
                    .unwrap()
                    .push(range.clone().unwrap_or_default());
                let total = donnees.len();
                let (tete, corps): (String, &[u8]) = match range.filter(|_| !ignore_range) {
                    None => (
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n"),
                        &donnees,
                    ),
                    Some(r) => {
                        let (a, b) = r.split_once('-').unwrap();
                        let a: usize = a.parse().unwrap();
                        if a >= total {
                            (
                                format!(
                                    "HTTP/1.1 416 Range Not Satisfiable\r\n\
                                     Content-Range: bytes */{total}\r\nContent-Length: 0\r\n"
                                ),
                                &[],
                            )
                        } else {
                            let b = b.parse::<usize>().map_or(total - 1, |b| b.min(total - 1));
                            (
                                format!(
                                    "HTTP/1.1 206 Partial Content\r\n\
                                     Content-Range: bytes {a}-{b}/{total}\r\nContent-Length: {}\r\n",
                                    b + 1 - a
                                ),
                                &donnees[a..=b],
                            )
                        }
                    }
                };
                let _ = flux.write_all(format!("{tete}Connection: close\r\n\r\n").as_bytes());
                let _ = flux.write_all(corps);
            }
        });
        (url, plages)
    }

    fn donnees_de_test() -> Vec<u8> {
        (0..10_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn telecharge_par_tranches() {
        let donnees = donnees_de_test();
        let (url, plages) = serveur(donnees.clone(), false);
        let dir = tempfile::tempdir().unwrap();
        let partiel = dir.path().join("modele.part");

        let mut vus = Vec::new();
        telecharger_par_tranches(&url, &partiel, 3_000, &mut |r, t| vus.push((r, t)), None)
            .unwrap();

        assert_eq!(fs::read(&partiel).unwrap(), donnees);
        assert_eq!(
            *plages.lock().unwrap(),
            ["0-2999", "3000-5999", "6000-8999", "9000-11999"]
        );
        // La progression connaît le total dès la première tranche.
        assert_eq!(vus.last(), Some(&(10_000, 10_000)));
    }

    #[test]
    fn reprend_un_telechargement_interrompu() {
        let donnees = donnees_de_test();
        let (url, plages) = serveur(donnees.clone(), false);
        let dir = tempfile::tempdir().unwrap();
        let partiel = dir.path().join("modele.part");
        fs::write(&partiel, &donnees[..4_000]).unwrap();

        telecharger_par_tranches(&url, &partiel, 3_000, &mut |_, _| {}, None).unwrap();

        assert_eq!(fs::read(&partiel).unwrap(), donnees);
        assert_eq!(*plages.lock().unwrap(), ["4000-6999", "7000-9999"]);
    }

    #[test]
    fn un_partiel_complet_ne_retelecharge_rien() {
        let donnees = donnees_de_test();
        let (url, plages) = serveur(donnees.clone(), false);
        let dir = tempfile::tempdir().unwrap();
        let partiel = dir.path().join("modele.part");
        fs::write(&partiel, &donnees).unwrap();

        telecharger_par_tranches(&url, &partiel, 3_000, &mut |_, _| {}, None).unwrap();

        assert_eq!(fs::read(&partiel).unwrap(), donnees);
        assert_eq!(*plages.lock().unwrap(), ["10000-12999"]);
    }

    #[test]
    fn un_serveur_sans_range_renvoie_tout() {
        let donnees = donnees_de_test();
        let (url, plages) = serveur(donnees.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let partiel = dir.path().join("modele.part");
        // Un partiel qui ne correspond à rien : il doit être écrasé, pas
        // prolongé.
        fs::write(&partiel, vec![0xAA; 4_000]).unwrap();

        telecharger_par_tranches(&url, &partiel, 3_000, &mut |_, _| {}, None).unwrap();

        assert_eq!(fs::read(&partiel).unwrap(), donnees);
        assert_eq!(plages.lock().unwrap().len(), 1);
    }
}
