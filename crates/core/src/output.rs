//! Écriture du fichier de sortie — SPEC §5.1.
//!
//! Un fichier existant n'est **jamais écrasé en silence** : il faut le demander
//! (`--force`). La vérification a lieu deux fois, et pour deux raisons
//! différentes :
//!
//! - [`check_target`] **avant** tout travail. Découvrir qu'on ne pourra pas
//!   écrire le résultat après vingt minutes d'inférence serait absurde ;
//! - [`write`] **au moment d'écrire**, de façon atomique. Le fichier a pu
//!   apparaître entre-temps, et seule l'ouverture exclusive (`create_new`)
//!   tranche sans course possible.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::error::{Result, ScriptaError};

fn existe_deja(path: &Path) -> ScriptaError {
    ScriptaError::OutputFailed {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            ErrorKind::AlreadyExists,
            "le fichier existe déjà (--force pour l'écraser)",
        ),
    }
}

fn echec(path: &Path, source: std::io::Error) -> ScriptaError {
    ScriptaError::OutputFailed {
        path: path.to_path_buf(),
        source,
    }
}

/// Vérifie qu'on pourra écrire `path`, sans rien créer.
///
/// Refuse un fichier existant sans `overwrite`, et un répertoire parent absent
/// — deux erreurs qu'il vaut mieux signaler avant l'inférence qu'après.
pub fn check_target(path: &Path, overwrite: bool) -> Result<()> {
    if path.is_dir() {
        return Err(echec(
            path,
            std::io::Error::new(ErrorKind::IsADirectory, "c'est un répertoire"),
        ));
    }
    if !overwrite && path.exists() {
        return Err(existe_deja(path));
    }
    // `Path::parent` rend `""` pour un nom nu : c'est le répertoire courant.
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && !parent.is_dir()
    {
        return Err(echec(
            path,
            std::io::Error::new(
                ErrorKind::NotFound,
                format!("répertoire « {} » introuvable", parent.display()),
            ),
        ));
    }
    Ok(())
}

/// Écrit `contenu` dans `path`.
///
/// - Sans `overwrite`, l'ouverture est exclusive : si le fichier existe, rien
///   n'est touché et l'écriture échoue.
/// - Avec `overwrite`, le contenu est d'abord écrit à côté, puis substitué
///   par renommage : une écriture interrompue ne remplace jamais un fichier
///   valide par un fichier tronqué.
///
/// Dans les deux cas, un échec en cours d'écriture ne laisse aucun fichier
/// partiel derrière lui.
pub fn write(path: &Path, contenu: &str, overwrite: bool) -> Result<()> {
    if overwrite {
        let temporaire = temporaire_de(path);
        if let Err(e) = ecrire_exclusif(&temporaire, contenu) {
            let _ = fs::remove_file(&temporaire);
            return Err(echec(path, e));
        }
        return fs::rename(&temporaire, path).map_err(|e| {
            let _ = fs::remove_file(&temporaire);
            echec(path, e)
        });
    }

    match ecrire_exclusif(path, contenu) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(existe_deja(path)),
        Err(e) => Err(echec(path, e)),
    }
}

/// Crée `path`, qui ne doit pas exister, et y écrit `contenu`. En cas d'échec
/// après la création, le fichier incomplet est supprimé.
fn ecrire_exclusif(path: &Path, contenu: &str) -> std::io::Result<()> {
    let mut fichier = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let resultat = fichier
        .write_all(contenu.as_bytes())
        .and_then(|()| fichier.sync_all());
    if resultat.is_err() {
        drop(fichier);
        let _ = fs::remove_file(path);
    }
    resultat
}

/// Fichier temporaire voisin : même répertoire, donc même volume, ce qui rend
/// le renommage atomique. L'identifiant de processus évite qu'une exécution
/// concurrente ne réutilise le même nom.
fn temporaire_de(path: &Path) -> PathBuf {
    let nom = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sortie".to_string());
    path.with_file_name(format!(".{nom}.{}.part", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cree_un_fichier_absent() {
        let dir = tempfile::tempdir().unwrap();
        let cible = dir.path().join("sortie.txt");
        check_target(&cible, false).expect("cible libre");
        write(&cible, "bonjour", false).expect("écriture");
        assert_eq!(fs::read_to_string(&cible).unwrap(), "bonjour");
    }

    #[test]
    fn refuse_d_ecraser_sans_force() {
        let dir = tempfile::tempdir().unwrap();
        let cible = dir.path().join("sortie.txt");
        fs::write(&cible, "précieux").unwrap();

        let e = check_target(&cible, false).expect_err("fichier existant");
        assert_eq!(e.exit_code(), 50);
        assert!(e.to_string().contains("--force"), "{e}");

        // Même en contournant la vérification préalable, l'écriture exclusive
        // protège le fichier.
        let e = write(&cible, "écrasé", false).expect_err("fichier existant");
        assert_eq!(e.exit_code(), 50);
        assert_eq!(fs::read_to_string(&cible).unwrap(), "précieux");
    }

    #[test]
    fn ecrase_avec_force_sans_laisser_de_temporaire() {
        let dir = tempfile::tempdir().unwrap();
        let cible = dir.path().join("sortie.txt");
        fs::write(&cible, "ancien contenu, plus long que le nouveau").unwrap();

        check_target(&cible, true).expect("écrasement autorisé");
        write(&cible, "nouveau", true).expect("écriture");
        assert_eq!(fs::read_to_string(&cible).unwrap(), "nouveau");

        let restes: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(restes.len(), 1, "fichiers résiduels : {restes:?}");
    }

    #[test]
    fn signale_un_repertoire_parent_absent_avant_tout_travail() {
        let dir = tempfile::tempdir().unwrap();
        let cible = dir.path().join("inexistant").join("sortie.txt");
        let e = check_target(&cible, true).expect_err("parent absent");
        assert_eq!(e.exit_code(), 50);
        assert!(e.to_string().contains("introuvable"), "{e}");
    }

    #[test]
    fn refuse_un_repertoire_comme_cible() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_target(dir.path(), true).is_err());
    }

    #[test]
    fn un_nom_nu_vise_le_repertoire_courant() {
        // `Path::parent` rend "" : ce n'est pas un répertoire manquant.
        assert!(check_target(Path::new("scripta-nom-nu-inexistant.txt"), false).is_ok());
    }
}
