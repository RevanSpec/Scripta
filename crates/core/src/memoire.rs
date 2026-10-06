//! Mémoire d'une transcription — ADR-003, SPEC SF-07.
//!
//! L'audio d'une vidéo tient tout entier en mémoire (ADR-003) : sa durée fixe
//! donc la mémoire nécessaire, à raison d'un demi-gigaoctet par heure. Une
//! machine qui n'en a pas assez ne rend pas d'erreur : l'allocation échoue en
//! pleine transcription, et le processus s'arrête sans un mot. Ce module
//! chiffre le besoin, relève la mémoire dont l'ordinateur dispose et refuse
//! d'entrée la vidéo qui n'y tiendrait pas, en disant jusqu'où l'on peut aller.

use crate::audio::SAMPLE_RATE;
use crate::error::{Result, ScriptaError};

/// Variable d'environnement qui remplace la mesure, en Mio : pour imposer une
/// limite plus basse, ou lever celle d'une mesure que l'on juge trop prudente.
pub const VARIABLE: &str = "SCRIPTA_MEMORY_MB";

const OCTETS_F32: u64 = 4;
const MIO: u64 = 1 << 20;

/// Octets par seconde d'audio, au pic de l'inférence. Trois postes :
///
/// - le tampon PCM `f32` de Scripta ;
/// - la copie complète qu'en tire whisper.cpp pour y ajouter 30 s de
///   remplissage — transitoire, mais précisément au pic ;
/// - le spectrogramme mel : 100 trames par seconde, 80 bandes, en `f32`.
///
/// 12 h d'audio ont mesuré 6 680 Mio engagés au pic, pour 6 592 attendus.
const OCTETS_PAR_SECONDE: u64 = 2 * OCTETS_F32 * SAMPLE_RATE as u64 + OCTETS_F32 * 100 * 80;

/// Surcoût de l'horodatage au mot, par seconde d'audio : whisper.cpp garde
/// alors l'énergie du signal, un `f32` par échantillon.
const OCTETS_PAR_SECONDE_MOTS: u64 = OCTETS_F32 * SAMPLE_RATE as u64;

/// Part fixe : tampons de calcul et état de ggml, sorties. Elle s'ajoute aux
/// poids du modèle, déjà chargés quand la mémoire est relevée.
const MARGE_FIXE: u64 = 1 << 30;

/// Part de la mémoire disponible qu'une transcription peut occuper, en
/// pourcents : le reste revient au système et aux autres applications.
const PART_UTILISABLE: u64 = 85;

fn octets_par_seconde(mots: bool) -> u64 {
    OCTETS_PAR_SECONDE + if mots { OCTETS_PAR_SECONDE_MOTS } else { 0 }
}

/// Mémoire nécessaire, en octets, à la transcription de `duree_s` secondes
/// d'audio. Le pire cas : une parole continue, que le VAD ne réduit pas.
pub fn requis(duree_s: f64, mots: bool) -> u64 {
    let audio = (duree_s.max(0.0) * octets_par_seconde(mots) as f64).ceil() as u64;
    MARGE_FIXE.saturating_add(audio)
}

fn budget(disponible: u64) -> u64 {
    disponible / 100 * PART_UTILISABLE
}

/// Durée maximale, en minutes, que `disponible` octets permettent de
/// transcrire.
pub fn duree_max_min(disponible: u64, mots: bool) -> u64 {
    let utile = budget(disponible).saturating_sub(MARGE_FIXE);
    utile / octets_par_seconde(mots) / 60
}

/// Refuse la vidéo que la mémoire `disponible` ne permet pas de transcrire.
///
/// Sans mesure, rien n'est refusé : la durée seule borne alors la transcription.
pub fn verifier(duree_s: f64, mots: bool, disponible: Option<u64>) -> Result<()> {
    let Some(disponible) = disponible else {
        return Ok(());
    };
    if !duree_s.is_finite() || requis(duree_s, mots) <= budget(disponible) {
        return Ok(());
    }
    Err(ScriptaError::InsufficientMemory {
        actual_min: (duree_s / 60.0).ceil() as u64,
        limit_min: duree_max_min(disponible, mots),
        available_mb: disponible / MIO,
        limit_without_words_min: mots.then(|| duree_max_min(disponible, false)),
    })
}

/// Mémoire physique disponible, en octets, sans recourir au disque d'échange :
/// ce qui reste allouable sans l'écrire d'abord. [`VARIABLE`], quand elle est
/// renseignée, la remplace. `None` si le système ne la livre pas.
pub fn disponible() -> Option<u64> {
    depuis_la_variable(std::env::var(VARIABLE).ok().as_deref()).or_else(mesure)
}

/// Mio donnés par la variable d'environnement, en octets. Une valeur absente,
/// nulle ou illisible est ignorée : la mesure reprend la main.
fn depuis_la_variable(valeur: Option<&str>) -> Option<u64> {
    valeur?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|&mio| mio > 0)
        .and_then(|mio| mio.checked_mul(MIO))
}

#[cfg(windows)]
fn mesure() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    // SÉCURITÉ : la structure est entièrement initialisée à zéro, et sa taille
    // déclarée comme l'exige l'API avant l'appel.
    let mut etat: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    etat.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let rendu = unsafe { GlobalMemoryStatusEx(&mut etat) };
    (rendu != 0).then_some(etat.ullAvailPhys)
}

#[cfg(target_os = "linux")]
fn mesure() -> Option<u64> {
    meminfo_disponible(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(target_os = "macos")]
fn mesure() -> Option<u64> {
    let sortie = std::process::Command::new("/usr/bin/vm_stat")
        .output()
        .ok()
        .filter(|s| s.status.success())?;
    vm_stat_disponible(&String::from_utf8_lossy(&sortie.stdout))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn mesure() -> Option<u64> {
    None
}

/// `MemAvailable` de `/proc/meminfo`, en octets : l'estimation, par le noyau,
/// de ce qu'un programme peut allouer sans pousser le système à l'échange.
#[cfg(any(target_os = "linux", test))]
fn meminfo_disponible(texte: &str) -> Option<u64> {
    let ligne = texte.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kio: u64 = ligne.split_whitespace().nth(1)?.parse().ok()?;
    kio.checked_mul(1024)
}

/// Pages libres, inactives et spéculatives de `vm_stat`, en octets : ce que
/// macOS reprend sans toucher à la mémoire active.
#[cfg(any(target_os = "macos", test))]
fn vm_stat_disponible(texte: &str) -> Option<u64> {
    let page: u64 = texte
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |nom: &str| -> Option<u64> {
        let (_, valeur) = texte
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(cle, _)| cle.trim() == nom)?;
        valeur.trim().trim_end_matches('.').parse().ok()
    };
    let total = pages("Pages free")?
        .checked_add(pages("Pages inactive")?)?
        .checked_add(pages("Pages speculative")?)?;
    total.checked_mul(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIO: u64 = 1 << 30;

    #[test]
    fn le_besoin_couvre_le_pic_mesure_sur_douze_heures() {
        // VERIFICATION.md, « Très longues durées » : 12 h de parole continue
        // ont engagé 6 680 Mio au pic, modèle `tiny`.
        let mesure = 6_680 * MIO;
        let chiffre = requis(12.0 * 3600.0, false);
        assert!(chiffre >= mesure, "{chiffre} < {mesure}");
        // Sans prudence excessive : la marge fixe d'un gigaoctet, pas plus.
        assert!(chiffre < mesure + 2 * GIO, "{chiffre}");
    }

    #[test]
    fn le_besoin_croit_avec_la_duree_et_l_horodatage_au_mot() {
        assert!(requis(7200.0, false) > requis(3600.0, false));
        // Un `f32` de plus par échantillon : 230,4 Mo par heure.
        let ecart = requis(3600.0, true) - requis(3600.0, false);
        assert_eq!(ecart, 4 * 16_000 * 3600);
        assert_eq!(requis(-5.0, false), MARGE_FIXE);
    }

    #[test]
    fn la_duree_max_est_la_reciproque_du_besoin() {
        for &mots in &[false, true] {
            for dispo in [2 * GIO, 4 * GIO, 15 * GIO, 64 * GIO] {
                let max = duree_max_min(dispo, mots);
                assert!(
                    verifier(max as f64 * 60.0, mots, Some(dispo)).is_ok(),
                    "{max} min devraient tenir dans {dispo} octets (mots : {mots})"
                );
                assert!(
                    verifier((max + 1) as f64 * 60.0, mots, Some(dispo)).is_err(),
                    "{} min ne devraient pas tenir dans {dispo} octets (mots : {mots})",
                    max + 1
                );
            }
        }
    }

    #[test]
    fn douze_heures_exigent_une_dizaine_de_gigaoctets_libres() {
        let douze_heures = 12.0 * 3600.0;
        assert!(verifier(douze_heures, false, Some(10 * GIO)).is_ok());
        assert!(verifier(douze_heures, false, Some(8 * GIO)).is_err());
        // L'horodatage au mot alourdit : 12 h n'y tiennent plus dans 10 Gio.
        assert!(verifier(douze_heures, true, Some(10 * GIO)).is_err());
    }

    #[test]
    fn le_refus_dit_ce_que_la_memoire_permet() {
        let e = verifier(10.0 * 3600.0, true, Some(4 * GIO)).unwrap_err();
        let ScriptaError::InsufficientMemory {
            actual_min,
            limit_min,
            available_mb,
            limit_without_words_min,
        } = e
        else {
            panic!("attendu InsufficientMemory, obtenu {e:?}");
        };
        assert_eq!(actual_min, 600);
        assert_eq!(available_mb, 4096);
        assert!((1..600).contains(&limit_min), "{limit_min}");
        assert_eq!(limit_min, duree_max_min(4 * GIO, true));
        // Sans l'horodatage au mot, la mémoire permettrait davantage.
        let sans = limit_without_words_min.expect("l'horodatage au mot était demandé");
        assert_eq!(sans, duree_max_min(4 * GIO, false));
        assert!(sans > limit_min);

        // Pas d'horodatage au mot, pas de piste à proposer.
        let Err(ScriptaError::InsufficientMemory {
            limit_without_words_min: None,
            ..
        }) = verifier(10.0 * 3600.0, false, Some(4 * GIO))
        else {
            panic!("attendu un refus sans piste");
        };
    }

    #[test]
    fn sans_mesure_rien_n_est_refuse() {
        assert!(verifier(1e9, false, None).is_ok());
    }

    #[test]
    fn une_duree_inconnue_n_est_pas_refusee() {
        assert!(verifier(f64::NAN, false, Some(GIO)).is_ok());
        assert!(verifier(f64::INFINITY, false, Some(GIO)).is_ok());
    }

    #[test]
    fn la_variable_remplace_la_mesure() {
        assert_eq!(depuis_la_variable(Some("2048")), Some(2048 * MIO));
        assert_eq!(depuis_la_variable(Some(" 512 ")), Some(512 * MIO));
        for ignoree in [None, Some(""), Some("0"), Some("beaucoup"), Some("-3")] {
            assert_eq!(depuis_la_variable(ignoree), None, "{ignoree:?}");
        }
        assert_eq!(depuis_la_variable(Some("18446744073709551615")), None);
    }

    #[test]
    fn meminfo_rend_la_memoire_disponible() {
        let texte = "MemTotal:       32768000 kB\n\
                     MemFree:         1024000 kB\n\
                     MemAvailable:   16384000 kB\n\
                     Buffers:          204800 kB\n";
        assert_eq!(meminfo_disponible(texte), Some(16_384_000 * 1024));
        assert_eq!(meminfo_disponible("MemTotal: 1 kB\n"), None);
        assert_eq!(meminfo_disponible("MemAvailable: beaucoup kB\n"), None);
    }

    #[test]
    fn vm_stat_somme_les_pages_reprenables() {
        let texte = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                     Pages free:                               3918.\n\
                     Pages active:                           240563.\n\
                     Pages inactive:                         235233.\n\
                     Pages speculative:                        4586.\n\
                     Pages throttled:                             0.\n\
                     Pages wired down:                        89438.\n\
                     \"Translation faults\":                202078646.\n";
        assert_eq!(
            vm_stat_disponible(texte),
            Some((3918 + 235_233 + 4586) * 16_384)
        );
        // Pages de 4 Kio, comme sur un Mac à processeur Intel.
        let intel = texte.replace("16384", "4096");
        assert_eq!(
            vm_stat_disponible(&intel),
            Some((3918 + 235_233 + 4586) * 4096)
        );
        assert_eq!(vm_stat_disponible("sortie inattendue"), None);
        assert_eq!(
            vm_stat_disponible("Mach Virtual Memory Statistics: (page size of 16384 bytes)\n"),
            None
        );
    }

    #[test]
    fn la_mesure_du_systeme_est_plausible() {
        let Some(octets) = mesure() else {
            // macOS passe par `vm_stat` et d'autres systèmes n'ont rien : sans
            // mesure, la durée seule borne la transcription.
            #[cfg(any(windows, target_os = "linux"))]
            panic!("la mesure devrait exister sous Windows et sous Linux");
            #[cfg(not(any(windows, target_os = "linux")))]
            return;
        };
        assert!(octets > 16 * MIO, "{octets} octets disponibles ?");
        assert!(octets < (1 << 50), "{octets} octets disponibles ?");
    }
}
