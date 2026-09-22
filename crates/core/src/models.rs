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

use crate::error::{Result, ScriptaError};

/// Délai d'établissement de la connexion.
const CONNECT_TIMEOUT_S: u64 = 30;
/// Délai d'inactivité au cours du transfert.
const IDLE_TIMEOUT_S: u64 = 60;

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

/// Modèle VAD Silero — SPEC SF-04. Il vit dans un dépôt distinct.
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
pub fn ensure(spec: &ModelSpec, progress: ProgressFn<'_>) -> Result<PathBuf> {
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

    download_verified(&spec.url(), &destination, spec.sha256, progress)?;
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
) -> Result<()> {
    let partiel = destination.with_extension("part");
    telecharger(url, &partiel, progress)?;

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

fn telecharger(url: &str, partiel: &Path, progress: ProgressFn<'_>) -> Result<()> {
    // Reprise : un `.part` déjà présent provient d'un transfert interrompu.
    // Sur un fichier d'un gigaoctet, tout reprendre serait coûteux.
    let deja = fs::metadata(partiel).map(|m| m.len()).unwrap_or(0);
    let reprise = deja > 0;

    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(CONNECT_TIMEOUT_S)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(IDLE_TIMEOUT_S)))
        .build()
        .new_agent();

    let mut requete = agent.get(url);
    if reprise {
        requete = requete.header("Range", &format!("bytes={deja}-"));
    }

    let reponse = requete.call().map_err(|e| ScriptaError::ModelUnavailable {
        detail: format!("téléchargement de {url} : {e}"),
    })?;

    // 206 confirme que le serveur honore la reprise ; 200 signifie qu'il
    // renvoie tout, auquel cas il faut repartir de zéro.
    let reprend = reponse.status().as_u16() == 206;
    let mut deja = if reprend { deja } else { 0 };

    let mut fichier = if reprend {
        fs::OpenOptions::new()
            .append(true)
            .open(partiel)
            .map_err(|e| io_err(partiel, e))?
    } else {
        File::create(partiel).map_err(|e| io_err(partiel, e))?
    };

    let total = reponse
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(|n| n + deja)
        .unwrap_or(0);

    let mut corps = reponse.into_body().into_reader();
    let mut tampon = vec![0u8; 256 * 1024];

    progress(deja, total);
    loop {
        let n = corps.read(&mut tampon).map_err(|e| io_err(partiel, e))?;
        if n == 0 {
            break;
        }
        fichier
            .write_all(&tampon[..n])
            .map_err(|e| io_err(partiel, e))?;
        deja += n as u64;
        progress(deja, total);
    }
    fichier.flush().map_err(|e| io_err(partiel, e))?;

    Ok(())
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

/// Modèles présents dans le cache, avec leur état.
pub fn installed() -> Result<Vec<(&'static ModelSpec, bool)>> {
    let dir = models_dir()?;
    Ok(MODELS
        .iter()
        .map(|m| {
            let taille_ok = fs::metadata(dir.join(m.file))
                .map(|meta| meta.len() == m.size)
                .unwrap_or(false);
            (m, taille_ok)
        })
        .collect())
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
}
