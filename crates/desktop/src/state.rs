//! État partagé entre les commandes IPC.

use std::sync::Mutex;

use scripta_core::transcribe::CancelToken;
use scripta_core::{Document, Engine};

/// État de l'application, détenu par Tauri.
///
/// Le moteur est conservé entre deux transcriptions : son chargement coûte
/// plusieurs secondes et jusqu'à un gigaoctet de mémoire, qu'il serait absurde
/// de payer à chaque lancement.
#[derive(Default)]
pub struct AppState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Moteur chargé, avec le modèle qui lui correspond.
    engine: Option<(String, Engine)>,
    /// Jeton de la transcription en cours, seul moyen d'interrompre une
    /// inférence déjà lancée (SPEC SF-04).
    cancel: Option<CancelToken>,
    /// Dernier document produit, pour réexporter sans réinférence.
    last: Option<Document>,
}

impl AppState {
    /// Arme l'annulation de la transcription en cours, s'il y en a une.
    pub fn cancel(&self) -> bool {
        let inner = self.inner.lock().expect("état non empoisonné");
        match &inner.cancel {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    pub fn set_cancel(&self, token: Option<CancelToken>) {
        self.inner.lock().expect("état non empoisonné").cancel = token;
    }

    pub fn set_last(&self, doc: Option<Document>) {
        self.inner.lock().expect("état non empoisonné").last = doc;
    }

    pub fn last(&self) -> Option<Document> {
        self.inner.lock().expect("état non empoisonné").last.clone()
    }

    /// Exécute `f` sur le moteur du modèle demandé, en le chargeant si besoin.
    ///
    /// Le verrou est tenu pendant toute l'inférence : deux transcriptions
    /// simultanées se disputeraient le CPU sans rien accélérer, et la seconde
    /// écraserait l'état d'annulation de la première.
    pub fn with_engine<T>(
        &self,
        model_id: &str,
        charger: impl FnOnce() -> scripta_core::Result<Engine>,
        f: impl FnOnce(&Engine) -> T,
    ) -> scripta_core::Result<T> {
        let mut inner = self.inner.lock().expect("état non empoisonné");

        let recharger = match &inner.engine {
            Some((id, _)) => id != model_id,
            None => true,
        };
        if recharger {
            // L'ancien moteur est libéré avant le chargement du nouveau :
            // conserver les deux doublerait l'empreinte mémoire.
            inner.engine = None;
            inner.engine = Some((model_id.to_string(), charger()?));
        }

        let (_, engine) = inner.engine.as_ref().expect("moteur chargé");
        Ok(f(engine))
    }
}
