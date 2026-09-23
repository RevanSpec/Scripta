//! Jeton d'annulation partagé par toutes les opérations longues.
//!
//! Téléchargement d'un modèle, extraction audio, inférence : chacune peut
//! durer plusieurs minutes, et chacune doit pouvoir être interrompue — par
//! `Ctrl-C` en CLI (SPEC §4.1), par le bouton « Annuler » en GUI (§4.2).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Jeton d'annulation, armé depuis un autre thread (bouton « Annuler » de la
/// GUI, `SIGINT` en CLI). Chaque opération l'interroge à son rythme : entre
/// deux blocs lus pour un téléchargement, entre deux fenêtres de traitement
/// pour whisper.cpp.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(pub(crate) Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Vrai si un jeton est fourni **et** armé.
pub(crate) fn is_cancelled(token: Option<&CancelToken>) -> bool {
    token.is_some_and(CancelToken::is_cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_clone_partage_l_etat() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        // C'est ce qui permet d'armer l'annulation depuis un autre thread
        // pendant que l'opération bloque le sien.
        let clone = token.clone();
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn l_absence_de_jeton_n_annule_rien() {
        assert!(!is_cancelled(None));
        let token = CancelToken::new();
        assert!(!is_cancelled(Some(&token)));
        token.cancel();
        assert!(is_cancelled(Some(&token)));
    }
}
