//! Relais des événements vers l'interface — SPEC §4.2.
//!
//! Le pipeline émet depuis son thread : la progression bien plus souvent qu'à
//! chaque pourcent, les segments par rafales à chaque fenêtre de 30 s. Les
//! transmettre un à un inonderait l'interface — c'est le risque « débit
//! d'événements » du J3 —, et les transmettre depuis le thread d'inférence le
//! ralentirait. Un thread de relais les regroupe donc par lots, au plus dix
//! par seconde, et lui seul parle à l'interface.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use scripta_core::pipeline::{Event, Observer};

/// Fenêtre de regroupement : au plus dix envois par seconde.
pub const FENETRE: Duration = Duration::from_millis(100);

/// Message adressé à l'interface. Les types miroirs sont dans `ui/src/ipc.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Message {
    /// Nouvelle étape.
    Phase {
        phase: Phase,
    },
    /// Téléchargement d'un modèle ou de l'extracteur, en octets.
    Download {
        item: String,
        received: u64,
        total: u64,
    },
    /// Vidéo identifiée par la sonde.
    Video {
        title: String,
        channel: Option<String>,
        duration_s: Option<f64>,
    },
    /// Piste de sous-titres retenue.
    Subtitles {
        lang: String,
        auto: bool,
        translation: bool,
    },
    /// Information sans gravité : repli sur la transcription, cache non écrit.
    Notice {
        message: String,
    },
    /// Début de l'inférence.
    Transcribing {
        media_s: f64,
    },
    Progress {
        percent: i32,
    },
    Segments {
        segments: Vec<SegmentVue>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Transcription resservie par le cache.
    Cache,
    /// Chargement du modèle en mémoire.
    Loading,
    Probe,
    Extraction,
}

/// Segment tel que l'interface l'affiche.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SegmentVue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Traduit un événement du pipeline, ou l'écarte s'il n'intéresse pas
/// l'interface.
pub fn message_de(event: Event<'_>) -> Option<Message> {
    Some(match event {
        Event::CacheKey(_) | Event::Extracted { .. } | Event::Transcribed { .. } => return None,
        Event::CacheHit => Message::Phase {
            phase: Phase::Cache,
        },
        Event::Download {
            model,
            received,
            total,
        } => Message::Download {
            item: model.alias.to_string(),
            received,
            total,
        },
        Event::Loading { .. } => Message::Phase {
            phase: Phase::Loading,
        },
        Event::Probing => Message::Phase {
            phase: Phase::Probe,
        },
        Event::Probed(meta) => Message::Video {
            title: meta.title.clone(),
            channel: meta.channel.clone(),
            duration_s: meta.duration,
        },
        Event::Subtitles {
            track, translation, ..
        } => Message::Subtitles {
            lang: track.lang.clone(),
            auto: track.auto,
            translation,
        },
        Event::SubtitlesFailed(_) => Message::Notice {
            message: "Les sous-titres n'ont pas pu être téléchargés.".to_string(),
        },
        Event::SubtitlesFallback => Message::Notice {
            message: "Pas de sous-titres exploitables : la vidéo sera transcrite.".to_string(),
        },
        Event::Extracting => Message::Phase {
            phase: Phase::Extraction,
        },
        Event::Transcribing { media_s, .. } => Message::Transcribing { media_s },
        Event::Progress(percent) => Message::Progress { percent },
        Event::Segment(s) => Message::Segments {
            segments: vec![SegmentVue {
                start: s.start,
                end: s.end,
                text: s.text.trim().to_string(),
            }],
        },
        Event::CacheWriteFailed(_) => Message::Notice {
            message: "La transcription n'a pas pu être mise en cache.".to_string(),
        },
    })
}

/// Relais : reçoit les messages de n'importe quel thread, les livre par lots
/// depuis le sien.
pub struct Relais {
    tx: Option<mpsc::Sender<Message>>,
    fil: Option<thread::JoinHandle<()>>,
    /// Dernière progression transmise. whisper.cpp la rappelle des milliers
    /// de fois pour quelques pourcents : seule une hausse mérite un message.
    pourcent: AtomicI32,
}

impl Relais {
    /// Démarre le relais. `livrer` reçoit chaque lot, depuis le thread de
    /// relais, jamais depuis celui de l'appelant.
    pub fn new(livrer: impl FnMut(Vec<Message>) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let fil = thread::Builder::new()
            .name("scripta-relais".to_string())
            .spawn(move || relayer(rx, livrer))
            .expect("création du thread de relais");
        Self {
            tx: Some(tx),
            fil: Some(fil),
            pourcent: AtomicI32::new(-1),
        }
    }

    pub fn transmettre(&self, message: Message) {
        if let Some(tx) = &self.tx {
            // Une interface fermée entre-temps n'est pas une erreur pour qui
            // transcrit.
            let _ = tx.send(message);
        }
    }
}

impl Observer for Relais {
    fn on_event(&self, event: Event<'_>) {
        if let Event::Progress(p) = event
            && self.pourcent.fetch_max(p, Ordering::Relaxed) >= p
        {
            return;
        }
        if let Some(m) = message_de(event) {
            self.transmettre(m);
        }
    }
}

/// À la destruction, le dernier lot est livré avant de rendre la main : la
/// commande ne répond qu'une fois tous les segments transmis.
impl Drop for Relais {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(fil) = self.fil.take() {
            let _ = fil.join();
        }
    }
}

fn relayer(rx: mpsc::Receiver<Message>, mut livrer: impl FnMut(Vec<Message>)) {
    // Le premier message ouvre une fenêtre ; tout ce qui arrive avant sa fin
    // rejoint le même lot. L'échéance est vérifiée à chaque tour, et non
    // seulement quand la file est vide : sous un flux continu, `recv_timeout`
    // rend toujours un message, et un lot qui n'attendrait qu'une file vide
    // ne serait livré qu'à la fermeture — l'interface resterait figée
    // jusqu'à la fin de l'inférence.
    while let Ok(premier) = rx.recv() {
        let mut lot = vec![premier];
        let echeance = Instant::now() + FENETRE;
        let mut ferme = false;
        loop {
            let reste = echeance.saturating_duration_since(Instant::now());
            if reste.is_zero() {
                break;
            }
            match rx.recv_timeout(reste) {
                Ok(m) => fusionner(&mut lot, m),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    ferme = true;
                    break;
                }
            }
        }
        livrer(lot);
        if ferme {
            return;
        }
    }
}

/// Ajoute `m` au lot sans rien garder d'inutile : la dernière progression, le
/// dernier état d'un téléchargement, et les segments à la suite les uns des
/// autres.
fn fusionner(lot: &mut Vec<Message>, m: Message) {
    match (lot.last_mut(), m) {
        (Some(Message::Progress { percent }), Message::Progress { percent: p }) => *percent = p,
        (
            Some(Message::Download {
                item,
                received,
                total,
            }),
            Message::Download {
                item: i,
                received: r,
                total: t,
            },
        ) if *item == i => {
            *received = r;
            *total = t;
        }
        (Some(Message::Segments { segments }), Message::Segments { segments: s }) => {
            segments.extend(s)
        }
        (_, m) => lot.push(m),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Lots livrés, avec leur instant de livraison.
    type Livraisons = Arc<Mutex<Vec<(Instant, Vec<Message>)>>>;

    /// Relais dont les lots sont consignés.
    fn relais_espion() -> (Relais, Livraisons) {
        let lots = Arc::new(Mutex::new(Vec::new()));
        let consigne = Arc::clone(&lots);
        let relais = Relais::new(move |lot| consigne.lock().unwrap().push((Instant::now(), lot)));
        (relais, lots)
    }

    fn segment(i: usize) -> Message {
        Message::Segments {
            segments: vec![SegmentVue {
                start: i as f64,
                end: i as f64 + 1.0,
                text: format!("segment {i}"),
            }],
        }
    }

    #[test]
    fn une_rafale_tient_en_un_lot_sans_rien_perdre() {
        let (relais, lots) = relais_espion();
        for p in 0..1000 {
            relais.transmettre(Message::Progress { percent: p / 10 });
        }
        for i in 0..500 {
            relais.transmettre(segment(i));
        }
        drop(relais);

        let lots = lots.lock().unwrap();
        let messages: Vec<&Message> = lots.iter().flat_map(|(_, l)| l).collect();
        assert!(lots.len() <= 3, "{} lots pour une rafale", lots.len());
        // Toute la progression se résume à sa dernière valeur…
        assert!(messages.contains(&&Message::Progress { percent: 99 }));
        // …mais aucun segment ne manque, et leur ordre est préservé.
        let textes: Vec<&str> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Segments { segments } => Some(segments),
                _ => None,
            })
            .flatten()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(textes.len(), 500);
        assert_eq!(textes[0], "segment 0");
        assert_eq!(textes[499], "segment 499");
    }

    #[test]
    fn au_plus_dix_livraisons_par_seconde() {
        let (relais, lots) = relais_espion();
        let debut = Instant::now();
        let mut i = 0;
        while debut.elapsed() < Duration::from_secs(1) {
            relais.transmettre(segment(i));
            i += 1;
            thread::sleep(Duration::from_millis(1));
        }
        drop(relais);

        let lots = lots.lock().unwrap();
        // Dix fenêtres pleines, plus la dernière, livrée à la fermeture sans
        // attendre la fin de la sienne — d'où son exclusion ci-dessous.
        assert!(lots.len() <= 12, "{} lots en une seconde", lots.len());
        for paire in lots[..lots.len() - 1].windows(2) {
            let ecart = paire[1].0.duration_since(paire[0].0);
            assert!(
                ecart >= FENETRE - Duration::from_millis(5),
                "deux lots à {ecart:?} d'intervalle"
            );
        }
    }

    #[test]
    fn un_flux_continu_n_empeche_pas_la_livraison() {
        // La régression observée sur une vidéo d'une heure : whisper.cpp
        // rappelle la progression en continu, la file ne se vide jamais, et
        // aucun lot n'était livré avant la fin de l'inférence.
        let (relais, lots) = relais_espion();
        let relais = Arc::new(relais);
        let producteur = {
            let relais = Arc::clone(&relais);
            thread::spawn(move || {
                let debut = Instant::now();
                let mut i = 0;
                while debut.elapsed() < Duration::from_millis(600) {
                    relais.transmettre(segment(i));
                    i += 1;
                }
            })
        };
        thread::sleep(Duration::from_millis(350));
        let livres = lots.lock().unwrap().len();
        producteur.join().unwrap();
        drop(Arc::into_inner(relais));
        assert!(
            livres >= 2,
            "{livres} lot(s) livré(s) en 350 ms de flux continu"
        );
    }

    #[test]
    fn seule_une_progression_en_hausse_est_transmise() {
        let (relais, lots) = relais_espion();
        for p in [0, 0, 1, 1, 1, 0, 2, 2] {
            relais.on_event(Event::Progress(p));
        }
        drop(relais);
        let transmis: Vec<Message> = lots
            .lock()
            .unwrap()
            .iter()
            .flat_map(|(_, l)| l.clone())
            .collect();
        // Fusionnées dans le même lot, les hausses se réduisent à la dernière.
        assert_eq!(transmis, [Message::Progress { percent: 2 }]);
    }

    #[test]
    fn la_fermeture_livre_le_dernier_lot() {
        let (relais, lots) = relais_espion();
        relais.transmettre(Message::Notice {
            message: "fin".into(),
        });
        drop(relais);
        assert_eq!(lots.lock().unwrap().len(), 1);
    }

    #[test]
    fn les_telechargements_distincts_ne_se_confondent_pas() {
        let mut lot = Vec::new();
        for (item, received) in [("base", 10), ("base", 20), ("silero", 5)] {
            fusionner(
                &mut lot,
                Message::Download {
                    item: item.into(),
                    received,
                    total: 100,
                },
            );
        }
        assert_eq!(lot.len(), 2);
        assert_eq!(
            lot[0],
            Message::Download {
                item: "base".into(),
                received: 20,
                total: 100
            }
        );
    }

    #[test]
    fn le_texte_des_segments_est_nettoye() {
        // whisper.cpp fait précéder chaque segment d'une espace.
        let m = message_de(Event::Segment(&scripta_core::transcribe::NewSegment {
            id: 0,
            start: 1.0,
            end: 2.0,
            text: " Bonjour.".into(),
        }));
        assert_eq!(
            m,
            Some(Message::Segments {
                segments: vec![SegmentVue {
                    start: 1.0,
                    end: 2.0,
                    text: "Bonjour.".into()
                }]
            })
        );
    }

    #[test]
    fn la_forme_serialisee_est_celle_qu_attend_l_interface() {
        let json = serde_json::to_value(Message::Video {
            title: "Essai".into(),
            channel: None,
            duration_s: Some(19.0),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "video", "title": "Essai", "channel": null, "durationS": 19.0})
        );
    }
}
