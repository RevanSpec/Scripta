//! État partagé entre les commandes IPC.
//!
//! Aucun verrou n'y est jamais tenu pendant un travail long : le bouton
//! « Annuler » doit pouvoir armer le jeton à tout instant, et une commande
//! qui attendrait la fin d'une inférence pour prendre un verrou figerait
//! l'interface — c'était le défaut du premier squelette.

use std::sync::{Arc, Mutex, PoisonError};

use scripta_core::pipeline::EngineSlot;
use scripta_core::{CancelToken, Document};

/// État de l'application, détenu par Tauri.
#[derive(Default)]
pub struct AppState {
    /// Moteur conservé entre deux transcriptions : son chargement coûte
    /// plusieurs secondes et jusqu'à un gigaoctet de mémoire.
    pub engines: EngineSlot,
    /// Tâche longue en cours — transcription, téléchargement de modèle, mise
    /// à jour de l'extracteur —, par son jeton d'annulation.
    tache: Arc<Mutex<Option<CancelToken>>>,
    /// Dernier document produit, pour réexporter sans réinférence.
    dernier: Mutex<Option<Document>>,
}

impl AppState {
    /// Réserve l'exécution d'une tâche longue.
    ///
    /// Une seule à la fois : deux inférences simultanées se disputeraient le
    /// processeur sans rien gagner, et un modèle ne doit pas disparaître sous
    /// une transcription qui l'emploie. `None` si une tâche est déjà en cours.
    pub fn commencer(&self) -> Option<Tache> {
        let mut emplacement = self.tache.lock().unwrap_or_else(PoisonError::into_inner);
        if emplacement.is_some() {
            return None;
        }
        let cancel = CancelToken::new();
        *emplacement = Some(cancel.clone());
        Some(Tache {
            emplacement: Arc::clone(&self.tache),
            cancel,
        })
    }

    /// Arme l'annulation de la tâche en cours. `false` si rien ne tournait.
    pub fn annuler(&self) -> bool {
        match &*self.tache.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(cancel) => {
                cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn occupe(&self) -> bool {
        self.tache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    pub fn memoriser(&self, doc: Document) {
        *self.dernier.lock().unwrap_or_else(PoisonError::into_inner) = Some(doc);
    }

    pub fn dernier(&self) -> Option<Document> {
        self.dernier
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Réservation d'une tâche longue, libérée à sa destruction — y compris sur
/// une erreur ou une panique : une tâche qui aurait échoué sans libérer sa
/// place bloquerait l'application jusqu'à son redémarrage.
pub struct Tache {
    emplacement: Arc<Mutex<Option<CancelToken>>>,
    pub cancel: CancelToken,
}

impl Drop for Tache {
    fn drop(&mut self) {
        *self
            .emplacement
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_seule_tache_a_la_fois() {
        let etat = AppState::default();
        let tache = etat.commencer().expect("aucune tâche en cours");
        assert!(etat.occupe());
        assert!(etat.commencer().is_none(), "seconde tâche acceptée");
        drop(tache);
        assert!(!etat.occupe());
        assert!(etat.commencer().is_some(), "place non libérée");
    }

    #[test]
    fn annuler_arme_le_jeton_de_la_tache_en_cours() {
        let etat = AppState::default();
        assert!(!etat.annuler(), "rien ne tournait");
        let tache = etat.commencer().unwrap();
        assert!(etat.annuler());
        assert!(tache.cancel.is_cancelled());
    }

    #[test]
    fn annuler_ne_bloque_pas_pendant_une_tache() {
        // La régression du premier squelette : `cancel` attendait un verrou
        // tenu pendant toute l'inférence. Ici, la tâche tourne sur un autre
        // thread et l'annulation doit aboutir sans elle.
        let etat = Arc::new(AppState::default());
        let tache = etat.commencer().unwrap();
        let travail = std::thread::spawn(move || {
            while !tache.cancel.is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        let debut = std::time::Instant::now();
        assert!(etat.annuler());
        travail.join().unwrap();
        assert!(debut.elapsed() < std::time::Duration::from_secs(1));
        assert!(!etat.occupe());
    }

    #[test]
    fn une_panique_libere_la_place() {
        let etat = Arc::new(AppState::default());
        let tache = etat.commencer().unwrap();
        let _ = std::thread::spawn(move || {
            let _garde = tache;
            panic!("échec simulé");
        })
        .join();
        assert!(!etat.occupe());
    }
}
