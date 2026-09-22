//! Emplacements de stockage — SPEC SF-03, SF-08.

use std::path::PathBuf;

use crate::error::{Result, ScriptaError};

/// Racine des données de Scripta, propre à la plateforme.
///
/// Sous Windows les données d'application vont dans `LOCALAPPDATA` ; ailleurs
/// le cache XDG ou son équivalent macOS convient.
pub fn scripta_dir() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or_else(|| ScriptaError::ModelUnavailable {
        detail: "répertoire personnel introuvable".to_string(),
    })?;

    let racine = if cfg!(windows) {
        base.data_local_dir().to_path_buf()
    } else {
        base.cache_dir().to_path_buf()
    };

    Ok(racine.join("scripta"))
}

/// Répertoire d'un sous-ensemble, surchargeable par variable d'environnement.
///
/// La surcharge permet de partager un cache entre installations ou de le
/// placer sur un autre volume — un modèle `large-v3` pèse un gigaoctet.
pub fn sub_dir(nom: &str, variable: &str) -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(variable) {
        return Ok(PathBuf::from(dir));
    }
    Ok(scripta_dir()?.join(nom))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_racine_porte_le_nom_du_projet() {
        let d = scripta_dir().expect("racine résolue");
        assert!(d.ends_with("scripta"), "{}", d.display());
    }

    #[test]
    fn la_variable_d_environnement_prime() {
        // Lecture seule : écrire dans l'environnement rendrait les tests
        // dépendants de leur ordre d'exécution.
        let sans = sub_dir("models", "SCRIPTA_VARIABLE_ASSUREMENT_ABSENTE").unwrap();
        assert!(sans.ends_with("models"));
        assert!(sans.parent().unwrap().ends_with("scripta"));
    }
}
