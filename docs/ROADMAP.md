# Scripta — Roadmap d'intégration

**Version :** 1.1
**Référence :** [SPEC.md](SPEC.md) v2.1

---

## Principe directeur

Le découpage ci-dessous suit une règle unique : **les inconnues techniques sont levées avant que du code ne soit construit par-dessus.** La v1 du plan de livraison plaçait dans un même jalon l'intégration réseau, le pipeline de processus et l'inférence GPU — trois risques indépendants dont l'échec de l'un aurait invalidé le travail fait sur les deux autres. Le Jalon 0 les isole.

Les estimations sont indicatives, pour un développeur Rust expérimenté travaillant seul. Elles servent au séquencement, pas à l'engagement.

---

## Vue d'ensemble

```
 J0 ─────► J1 ─────► J2 ─────────────────► J4 ─────► v1.0
 Dérisquage  Cœur      Robustesse CLI    │  Packaging
  1–2 j      PoC CLI     8–12 j          │   5–8 j
              4–6 j                      │
                                         │
                            J3 ──────────┘
                          GUI Tauri
                            8–12 j
                                                  J5 (optionnel, post-v1.0)
                                                  Optimisations
```

**Chemin critique :** `J0 → J1 → J2 → J4`. Le Jalon 3 (GUI) démarre dès que le cœur est stabilisé (fin J2) et se déroule en parallèle de la préparation du packaging. Une **v0.1.0 CLI seule est publiable à l'issue du J2 + un sous-ensemble du J4** — c'est le jalon de valeur le plus précoce et il est recommandé de le publier.

---

## Jalon 0 — Dérisquage

> **Statut : partiellement clos.** Les spikes 0.1 et 0.3 ont été menés par
> inspection de l'API de `whisper-rs` 0.16 et tentative de compilation, et le
> spike 0.2 est **entièrement couvert** par les tests d'intégration permanents
> de `crates/core/tests/pipeline.rs` — y compris le test de non-régression de
> l'interblocage `stderr`, validé par réintroduction du défaut. Résultats dans
> l'[Annexe D](SPEC.md#annexe-d--points-à-valider-en-implémentation) ;
> [ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle) a été révisé en
> conséquence. Nouveau constat : les prérequis de compilation
> ([Annexe E](SPEC.md#annexe-e--prérequis-de-compilation)) sont plus lourds
> qu'anticipé — CMake **et** libclang.

**Objectif :** valider ou invalider les hypothèses de l'[Annexe D](SPEC.md#annexe-d--points-à-valider-en-implémentation) avant tout investissement structurel. Le code produit ici est **jetable** et ne sera pas repris.

**Effort :** 1–2 jours.

### Lot de travail

| # | Tâche | Valide |
|---|---|---|
| **0.1** | **Spike A — inférence isolée.** Un binaire minimal : WAV 16 kHz mono local → `whisper-rs` → texte sur stdout. Aucun réseau, aucun sidecar. | Build `whisper-rs` sur **Windows MSVC** (le plus pénible des trois) ; présence effective de `progress_callback`, `new_segment_callback`, `abort_callback` ; accès au VAD Silero ; **chargement dynamique des backends ggml** ([ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle)) |
| **0.2** | **Spike B — pipeline isolé.** Un binaire minimal : URL → `yt-dlp` → `ffmpeg` → `Vec<f32>` → somme de contrôle. Aucune IA. | Câblage des pipes sans shell **sous Windows** (sémantique différente de POSIX) ; drainage de `stderr` ; validité du PCM produit ; comportement de `-f bestaudio[ext=webm]` sur un échantillon de vidéos variées |
| **0.3** | **Spike C — backends.** Énumération des backends ggml disponibles à l'exécution, sur au moins deux machines de configurations différentes. | Faisabilité de l'affichage « accélération détectée » de la GUI avec un artefact unique |
| **0.4** | **Arbitrage.** Mise à jour de l'[Annexe D](SPEC.md#annexe-d--points-à-valider-en-implémentation) : chaque hypothèse est marquée confirmée, ou le repli correspondant est acté dans l'ADR concerné. | — |

### Critères de sortie

- [ ] Les spikes A et B compilent et s'exécutent sur **Windows et au moins une plateforme Unix**.
- [ ] La version de `whisper-rs` est **figée**, avec justification écrite au regard des quatre callbacks requis.
- [ ] L'[ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle) est confirmé, ou le repli « deux artefacts par plateforme » est acté et sa conséquence reportée sur le [Jalon 4](#jalon-4--packaging-et-cicd).
- [ ] L'[Annexe D](SPEC.md#annexe-d--points-à-valider-en-implémentation) ne contient plus d'hypothèse non tranchée.

### Risques

| Risque | Probabilité | Mitigation |
|---|---|---|
| Build `whisper-rs` + backend GPU sous Windows (MSVC, toolchain) | **Élevée** | C'est précisément l'objet de 0.1. Repli : CPU seul sous Windows pour la v1, GPU en v1.1 |
| `abort_callback` absent du binding Rust | Moyenne | Contribuer le binding en amont, ou vendorer le crate |
| Chargement dynamique des backends non supporté | Moyenne | Repli documenté : deux artefacts par plateforme (+2 j sur le J4) |

> **Ne pas sauter ce jalon.** Si 0.1 échoue, tout le J1 est à réécrire. Un jour investi ici en économise plusieurs plus tard.

---

## Jalon 1 — Cœur fonctionnel (PoC CLI)

> **Statut : livré.** La chaîne complète
> `URL → métadonnées → PCM → inférence → texte` a été exécutée sur une vraie
> vidéo YouTube (« Me at the zoo », 19 s) : 304 089 échantillons extraits, soit
> exactement 19,0 s à 16 kHz, transcrits en 2 segments avec détection de langue.
> 54 tests verts sur Windows, dont 6 d'inférence qui se sautent proprement sans
> les fixtures pour que la CI reste verte.
>
> **Tenue en mémoire confirmée** sur une vidéo de 61 min : 1 039 Mo, code 0,
> couverture 100,2 %, langue française correctement détectée sur 1 467
> segments. La mesure a révélé un doublement du tampon audio, depuis corrigé.
>
> **Reste à confirmer :** la CI sur les trois OS — le build natif de
> whisper.cpp n'a été éprouvé que sous Windows/MSVC.

**Objectif :** un chemin nominal de bout en bout, sans robustesse. `URL → texte sur stdout`.

**Effort :** 4–6 jours. **Prérequis :** J0 clos.

### Lot de travail

| # | Tâche | Spécification |
|---|---|---|
| **1.1** | Squelette du workspace Cargo (`crates/core`, `crates/cli`), CI de base (fmt, clippy, build sur 3 OS) | [§2.2](SPEC.md#22-découpage-des-crates) |
| **1.2** | `core::url` — validation stricte et **reconstruction de l'URL canonique** | [SF-01](SPEC.md#sf-01--validation-durl-et-sonde-de-métadonnées) |
| **1.3** | `core::probe` — sonde `yt-dlp -J`, désérialisation en `Metadata` | [SF-01](SPEC.md#sf-01--validation-durl-et-sonde-de-métadonnées) |
| **1.4** | `core::audio` — pipeline `yt-dlp \| ffmpeg` câblé en Rust, **drainage de `stderr` par threads dédiés**, conversion `s16le → f32` | [SF-02](SPEC.md#sf-02--pipeline-dextraction-audio--zero-disk-), [ADR-002](SPEC.md#adr-002--pipeline-audio-sans-shell) |
| **1.5** | `core::transcribe` — wrapper `whisper-rs` minimal (modèle fourni par `SCRIPTA_MODELS_DIR`, pas encore de téléchargement) | [SF-04](SPEC.md#sf-04--moteur-de-transcription-locale) |
| **1.6** | `core::format::txt` | [SF-05](SPEC.md#sf-05--formats-dexportation) |
| **1.7** | `crates/cli` — `scripta <URL>`, sans sous-commandes, options `-m -l -o` | [§4.1](SPEC.md#41-interface-en-ligne-de-commande) |
| **1.8** | Tests unitaires 1.2 et 1.4 (dont le **sidecar simulé saturant `stderr`**) | [§5.5](SPEC.md#55-stratégie-de-test) |

### Critères de sortie

- [x] `scripta "https://www.youtube.com/watch?v=<ID>" > out.txt` produit une transcription correcte. *(Vérifié sous Windows ; les deux autres plateformes dépendent de la CI.)*
- [x] **Invariant zero-disk vérifié** : aucun fichier créé dans le répertoire temporaire pendant l'exécution (test instrumenté).
- [x] Test de non-régression du deadlock `stderr` **en place et vert** — validé par réintroduction du défaut : sans drainage, `extract()` se fige et le test échoue au bout de 30 s.
- [x] Une vidéo de 1 h se transcrit sans dépassement de mémoire. *(Mesuré le 2026-09-22, machine au repos : 61 min, 1 040 Mo, 4,8 × temps réel, couverture 100,2 %, langue `fr` correctement détectée. Le modèle mémoire prédit désormais cette valeur à l'octet près.)*
- [ ] CI verte sur les trois OS, **sans accès réseau**. *(Vérifiée localement sous Windows ; les trois OS restent à confirmer sur la PR.)*

### Risques

| Risque | Mitigation |
|---|---|
| Format audio inattendu sur certaines vidéos | Repli `/bestaudio/best` déjà spécifié ; constituer dès ce jalon un corpus de 10 vidéos hétérogènes (musique, live archivé, très longue, langue non latine, sous-titrée, restreinte) servant de banc de test manuel |
| Différences de sémantique des pipes sous Windows | Déjà levé en 0.2 |

---

## Jalon 2 — Robustesse CLI

> **Statut : en cours.** Livrées — 2.1 (taxonomie), 2.2 (recevabilité), 2.3
> (téléchargement des modèles), 2.4 (`--model auto`), 2.5 (formateurs
> `srt`/`vtt`/`json`), 2.6 (VAD, `--initial-prompt`, horodatage au mot, garde
> `turbo`), 2.7 (progression), 2.8 (`SIGINT`), 2.11 (sous-titres officiels),
> 2.12 (cookies). Restent 2.9, 2.10, 2.13 à 2.16.

**Objectif :** une CLI **publiable**. C'est le jalon le plus dense et le plus créateur de valeur.

**Effort :** 8–12 jours. **Prérequis :** J1 clos.

### Lot de travail

| # | Tâche | Spécification |
|---|---|---|
| **2.1** | `core::error` — énuméré `ScriptaError` exhaustif, mise en correspondance des motifs de `stderr` de yt-dlp, codes de sortie | [SF-07](SPEC.md#sf-07--taxonomie-derreurs-et-codes-de-sortie) |
| **2.2** | Gardes de recevabilité : `is_live`, `--max-duration`, limite d'âge, playlists | [SF-01](SPEC.md#sf-01--validation-durl-et-sonde-de-métadonnées), [SF-07](SPEC.md#sf-07--taxonomie-derreurs-et-codes-de-sortie) |
| ~~**2.3**~~ | ~~`core::models`~~ ✅ téléchargement, SHA-256, `.part` + renommage atomique, reprise par `Range`, timeouts, progression | [SF-03](SPEC.md#sf-03--gestion-et-cycle-de-vie-des-modèles-whisper) |
| ~~**2.4**~~ | ~~`--model auto`~~ ✅ résout vers `turbo` si un backend GPU est compilé, `base` sinon. `--backend` sans objet : le backend est lié à la compilation ([ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle) révisé) | [SF-03](SPEC.md#sf-03--gestion-et-cycle-de-vie-des-modèles-whisper) |
| ~~**2.5**~~ | ~~`core::format::{srt,vtt,json}`~~ ✅ contraintes de lisibilité, échappement XML pour WebVTT, schéma JSON versionné | [SF-05](SPEC.md#sf-05--formats-dexportation) |
| **2.6** | VAD Silero (par défaut), `--initial-prompt`, `--word-timestamps`, **garde `turbo` + `--translate`** | [SF-04](SPEC.md#sf-04--moteur-de-transcription-locale) |
| **2.7** | Progression : `progress_callback` + `new_segment_callback` → `indicatif` sur **`stderr`**, désactivée hors TTY | [SF-04](SPEC.md#sf-04--moteur-de-transcription-locale), [§4.1](SPEC.md#41-interface-en-ligne-de-commande) |
| ~~**2.8**~~ | ~~Interruption~~ ✅ premier `Ctrl-C` arme le jeton d'annulation, second force la sortie en 130 | [§4.1](SPEC.md#41-interface-en-ligne-de-commande) |
| **2.9** | `core::sidecar` — résolution à deux emplacements, détection de version, **installation hors bundle** | [ADR-004](SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour), [SF-06](SPEC.md#sf-06--maintenance-du-sidecar-yt-dlp) |
| **2.10** | `core::cache` — cache de transcriptions, clé, éviction LRU | [SF-08](SPEC.md#sf-08--cache-de-transcriptions) |
| ~~**2.11**~~ | ~~Sous-titres officiels~~ ✅ `--prefer-subs` avec repli silencieux, sous-commande `subs` où l'absence est une erreur, analyseur WebVTT déduplicant le défilement | [SF-01](SPEC.md#sf-01--validation-durl-et-sonde-de-métadonnées) |
| ~~**2.12**~~ | ~~`--cookies-from-browser`~~ ✅ transmis à la sonde et à l'extraction, désactivé par défaut | [SF-09](SPEC.md#sf-09--authentification-et-confidentialité) |
| **2.13** | Arborescence CLI complète : `run`/`subs`/`models`/`cache`/`update-extractor`/`doctor`, `run` implicite | [§4.1](SPEC.md#41-interface-en-ligne-de-commande) |
| **2.14** | **Suite de tests complète** : golden files, sidecars simulés, couverture de toutes les variantes d'erreur, intégration `tiny` + assertion WER | [§5.5](SPEC.md#55-stratégie-de-test) |
| **2.15** | **Banc de performance** → mesure réelle et **ajustement des seuils du [§5.2](SPEC.md#52-performance)** | [§5.2](SPEC.md#52-performance) |
| **2.16** | Documentation utilisateur : `README.md`, avertissement CGU, `THIRD_PARTY_LICENSES.md` | [§1.3](SPEC.md#13-licence-et-conformité) |

### Critères de sortie

- [ ] **Chaque variante de `ScriptaError` est atteignable par un test** et produit le code de sortie contractuel.
- [ ] Une vidéo privée, un live, une vidéo géo-bloquée et une vidéo exigeant une authentification produisent chacun un message **actionnable** — jamais une panique ni une trace brute de yt-dlp.
- [ ] `scripta <URL> -f json | jq .` fonctionne **sans `--quiet`** (preuve de la séparation stdout/stderr).
- [ ] `Ctrl-C` pendant l'inférence rend la main en **moins de 2 secondes**, sans processus orphelin ni fichier résiduel.
- [ ] Les seuils du [§5.2](SPEC.md#52-performance) sont mesurés et inscrits dans la spécification (ajustés si nécessaire, avec justification).
- [ ] `scripta doctor` diagnostique correctement une installation saine **et** une installation dégradée (sidecar absent, cache non inscriptible, pas de réseau).
- [ ] CI verte sur trois OS, **toujours sans réseau**.

### Risques

| Risque | Mitigation |
|---|---|
| Les motifs de `stderr` de yt-dlp changent → mauvaise classification des erreurs | Classification par correspondance **large et tolérante**, avec repli sur `ExtractionFailed` (20) et affichage du `stderr` brut. Ne jamais faire dépendre une logique de contrôle d'une chaîne de caractères exacte |
| Les seuils de performance ne sont pas atteints | Ils sont mesurés **ici**, pas promis. Ajuster la spécification plutôt que le code |
| Le VAD dégrade certains contenus (chant, parole très continue) | `--no-vad` déjà prévu ; documenter le cas |

---

## Jalon 3 — Interface de bureau (Tauri v2)

**Objectif :** GUI fonctionnelle en mode développement. Le packaging relève du [Jalon 4](#jalon-4--packaging-et-cicd).

**Effort :** 8–12 jours. **Prérequis :** J2 clos (le cœur doit être stable avant d'y brancher une seconde interface).

### Lot de travail

| # | Tâche | Spécification |
|---|---|---|
| **3.1** | Squelette `crates/desktop`, Tauri v2, frontend Svelte + Tailwind | [§2.2](SPEC.md#22-découpage-des-crates) |
| **3.2** | Commandes IPC : `probe`, `transcribe`, `cancel`, `list_backends`, `list_models`, `export` | [§4.2](SPEC.md#42-interface-de-bureau-tauri-v2) |
| **3.3** | **Exécution hors thread principal** (`spawn_blocking`) + événements de progression et de segments | [§4.2](SPEC.md#42-interface-de-bureau-tauri-v2) |
| **3.4** | Écran principal : saisie d'URL, sélecteurs modèle/langue, affichage de l'accélération détectée | [§4.2](SPEC.md#42-interface-de-bureau-tauri-v2) |
| **3.5** | Affichage progressif des segments, défilement automatique interruptible | [§4.2](SPEC.md#42-interface-de-bureau-tauri-v2) |
| **3.6** | Bouton **Annuler** réactif pendant l'inférence | [SF-04](SPEC.md#sf-04--moteur-de-transcription-locale) |
| **3.7** | Panneau d'options avancées : VAD, contexte initial, horodatage au mot, cookies (avec avertissement) | [SF-04](SPEC.md#sf-04--moteur-de-transcription-locale), [SF-09](SPEC.md#sf-09--authentification-et-confidentialité) |
| **3.8** | Gestion des modèles : téléchargement avec progression, suppression, espace occupé | [SF-03](SPEC.md#sf-03--gestion-et-cycle-de-vie-des-modèles-whisper) |
| **3.9** | Export vers fichier + copie dans le presse-papiers | [SF-05](SPEC.md#sf-05--formats-dexportation) |
| **3.10** | Restitution des erreurs : chaque variante de `ScriptaError` rendue en message lisible et actionnable | [SF-07](SPEC.md#sf-07--taxonomie-derreurs-et-codes-de-sortie) |
| **3.11** | Bouton de mise à jour de l'extracteur | [SF-06](SPEC.md#sf-06--maintenance-du-sidecar-yt-dlp) |
| **3.12** | Déclaration `externalBin` avec **suffixes de triplet cible** | [§4.2](SPEC.md#42-interface-de-bureau-tauri-v2) |

### Critères de sortie

- [ ] `cargo tauri dev` fonctionne sur les trois plateformes.
- [ ] **La fenêtre reste réactive du début à la fin** d'une transcription d'une heure : aucun gel, l'annulation reste cliquable en permanence.
- [ ] L'annulation en cours d'inférence libère les ressources en moins de 2 s.
- [ ] L'accélération affichée correspond au matériel réel sur au moins deux configurations distinctes.
- [ ] Aucune variante de `ScriptaError` ne produit un message technique brut dans l'interface.

### Risques

| Risque | Mitigation |
|---|---|
| Sidecars non résolus en mode développement (chemins différents du mode bundle) | Abstraire la résolution dans `core::sidecar` dès le J2 (tâche 2.9), avec surcharge par variable d'environnement |
| Débit d'événements trop élevé (un événement par segment sur une vidéo dense) | Regroupement temporel côté Rust (au plus ~10 émissions/s) |

---

## Jalon 4 — Packaging et CI/CD

**Objectif :** un `git tag` produit des artefacts installables et signés pour les trois plateformes.

**Effort :** 5–8 jours. **Prérequis :** J2 pour la CLI ; J3 pour la GUI. *Peut démarrer partiellement en parallèle du J3.*

### Lot de travail

| # | Tâche | Spécification |
|---|---|---|
| **4.1** | Script d'acquisition des sidecars par triplet cible (yt-dlp + ffmpeg), avec **vérification d'intégrité** | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.2** | **Build FFmpeg minimale** (décodeurs et démultiplexeurs strictement nécessaires) ou sélection d'une source de builds réduites | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.3** | Matrice GitHub Actions : 3 OS × {CLI, GUI} | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.4** | **Signature et notarisation macOS** de l'application **et de tous les sidecars**, binaire universel pour la CLI | [§5.4](SPEC.md#54-distribution-et-packaging), [ADR-004](SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour) |
| **4.5** | Installeurs : NSIS (Windows), AppImage + `.deb` (Linux), `.dmg` (macOS) | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.6** | Workflow de release : sommes de contrôle, notes de version, inventaire des versions de sidecars embarquées | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.7** | **Tâche planifiée quotidienne** : transcription d'une vidéo de référence, alerte en cas de rupture d'extracteur | [§5.5](SPEC.md#55-stratégie-de-test) |
| **4.8** | Banc de performance en CI (non bloquant, suivi de tendance) | [§5.2](SPEC.md#52-performance) |

### Critères de sortie

- [ ] Un tag produit **six artefacts** (3 OS × CLI/GUI), publiés avec leurs sommes de contrôle.
- [ ] Le `.dmg` s'installe et se lance sur un **Mac vierge** (Apple Silicon), sans avertissement Gatekeeper.
- [ ] `update-extractor` fonctionne sur l'application **installée et signée** des trois plateformes — c'est le test qui valide [ADR-004](SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour), et il **ne peut pas** être fait en mode développement.
- [ ] La CLI démarre sur une machine **sans GPU ni driver** (validation de [ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle)).
- [ ] La tâche quotidienne s'exécute et alerte correctement.

### Risques

| Risque | Probabilité | Mitigation |
|---|---|---|
| **Notarisation macOS** — sidecars non signés, droits manquants, chemins d'exécution | **Élevée** | Commencer par un `.dmg` de test **dès le début du J4**, pas à la fin. C'est le poste qui déborde systématiquement |
| Taille des artefacts GUI (FFmpeg) | Moyenne | Tâche 4.2 ; à défaut, accepter et documenter |
| Secrets de signature en CI (certificat Apple, mots de passe) | Moyenne | Provisionner les comptes et certificats **pendant le J3**, le délai administratif Apple n'est pas compressible |

> **Alerte de séquencement.** L'adhésion au Apple Developer Program et l'émission des certificats prennent des jours ouvrés. À initier au plus tard au début du Jalon 3.

---

## Jalon 5 — Optimisations (post-v1.0, optionnel)

Aucune de ces tâches ne conditionne la v1.0.

| # | Tâche | Bénéfice | Réserve |
|---|---|---|---|
| **5.1** | Suppression de FFmpeg au profit d'un décodage Opus natif Rust + `rubato` | −50 Mo par bundle, un sidecar en moins, obligation LGPL levée | Le support Opus de `symphonia` est **à vérifier** ; un binding `libopus` reste l'option sûre |
| **5.2** | Build CUDA optionnelle distribuée | Gain sur les gros modèles avec GPU NVIDIA | Double la matrice de build |
| **5.3** | Traitement par lot et playlists | Cas d'usage réel et fréquent | Pose la question de la parallélisation et de la reprise |
| **5.4** | Diarisation | Demande utilisateur récurrente | Second modèle, alignement, complexité significative |
| **5.5** | Élargissement aux autres plateformes supportées par yt-dlp | Quasi gratuit techniquement | Surface de test multipliée |
| **5.6** | Mise à jour automatique de l'application (Tauri updater) | Confort | Infrastructure de signature supplémentaire |

---

## Traçabilité — couverture des spécifications

| Spécification | Jalon |
|---|---|
| [SF-01](SPEC.md#sf-01--validation-durl-et-sonde-de-métadonnées) Validation et sonde | J1 (1.2, 1.3) · J2 (2.2, 2.11) |
| [SF-02](SPEC.md#sf-02--pipeline-dextraction-audio--zero-disk-) Pipeline audio | J0 (0.2) · J1 (1.4) |
| [SF-03](SPEC.md#sf-03--gestion-et-cycle-de-vie-des-modèles-whisper) Modèles | J2 (2.3, 2.4) · J3 (3.8) |
| [SF-04](SPEC.md#sf-04--moteur-de-transcription-locale) Moteur de transcription | J0 (0.1) · J1 (1.5) · J2 (2.6, 2.7, 2.8) |
| [SF-05](SPEC.md#sf-05--formats-dexportation) Formats d'export | J1 (1.6) · J2 (2.5) · J3 (3.9) |
| [SF-06](SPEC.md#sf-06--maintenance-du-sidecar-yt-dlp) Maintenance sidecar | J2 (2.9) · J3 (3.11) · J4 (4.4) |
| [SF-07](SPEC.md#sf-07--taxonomie-derreurs-et-codes-de-sortie) Taxonomie d'erreurs | J2 (2.1, 2.2, 2.13) · J3 (3.10) |
| [SF-08](SPEC.md#sf-08--cache-de-transcriptions) Cache de transcriptions | J2 (2.10) |
| [SF-09](SPEC.md#sf-09--authentification-et-confidentialité) Authentification | J2 (2.12) · J3 (3.7) |
| [§5.1](SPEC.md#51-sécurité) Sécurité | J1 (1.2) · J2 (2.3) · transversal |
| [§5.2](SPEC.md#52-performance) Performance | J2 (2.15) · J4 (4.8) |
| [§5.4](SPEC.md#54-distribution-et-packaging) Packaging | J4 |
| [§5.5](SPEC.md#55-stratégie-de-test) Tests | J1 (1.8) · J2 (2.14) · J4 (4.7) |

---

## Registre des risques consolidé

| # | Risque | Impact | Prob. | Jalon | Mitigation |
|---|---|---|---|---|---|
| R1 | Build `whisper-rs` + GPU sous Windows | Élevé | Élevée | J0 | Spike 0.1 en tout premier ; repli CPU |
| R2 | Notarisation macOS des sidecars | Élevé | Élevée | J4 | `.dmg` de test en **début** de J4 ; certificats provisionnés dès le J3 |
| R3 | **YouTube casse les extracteurs** | Élevé | **Certaine** *(question de quand, pas de si)* | Continu | [SF-06](SPEC.md#sf-06--maintenance-du-sidecar-yt-dlp) + tâche quotidienne 4.7 + [ADR-004](SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour). **C'est la raison d'être de ces trois éléments** |
| R4 | Vérification anti-robot bloquant les utilisateurs | Moyen | Élevée | J2 | Diagnostic explicite (code 12) + `--cookies-from-browser` documenté |
| R5 | Chargement dynamique des backends non supporté | Moyen | Moyenne | J0 | Repli deux artefacts (+2 j sur le J4) |
| R6 | Seuils de performance non atteints | Faible | **Écartée** | J2 | Mesuré à 4,8 × temps réel sur une machine au repos, contre un seuil de 3 ×. Les mesures antérieures à 2,1 × et 2,7 × étaient faussées par des compilations concurrentes : **un débit ne se mesure que sur une machine inoccupée** |
| R7 | Deadlock `stderr` découvert tardivement | Élevé | **Écartée par construction** | J1 | Test de non-régression dédié (1.8), exigé en critère de sortie |
| R8 | Dérive de périmètre vers la GUI avant stabilisation du cœur | Moyen | Moyenne | J2/J3 | J3 bloqué tant que J2 n'est pas clos ; publier la **v0.1.0 CLI** pour matérialiser la valeur intermédiaire |
| R9 | **`--release` produit un whisper.cpp non optimisé sous MSVC** | Élevé | **Avérée — contournée** | J4 | Mesuré : sans correctif, release est 4 à 6× plus lent que debug. La crate `cmake` écrase `CMAKE_CXX_FLAGS_<BUILD_TYPE>` tandis que le générateur Visual Studio compile en `--config Release` : le profil release perd son `/O2`. **Contournement appliqué** — `CMAKE_{C,CXX}_FLAGS_RELEASE` forcés par l'environnement, que le `build.rs` de `whisper-rs-sys` réinjecte en define. Vérifié : 11,7–14,0× temps réel après correctif, contre 1,4–2,0× avant. Le contournement doit être porté dans la CI, le script de test et tout pipeline d'empaquetage : `.cargo/config.toml` ne permet pas d'`[env]` conditionné à la cible. Correctif amont souhaitable. |

---

## Définition de terminé (transversale)

Aucun jalon n'est clos tant que, pour tout code produit :

- [ ] `cargo fmt --check` et `cargo clippy -- -D warnings` passent.
- [ ] Les tests associés existent et sont verts sur les trois OS, **sans réseau**.
- [ ] Les erreurs sont typées dans `ScriptaError` — **aucun `unwrap()` sur un chemin pouvant échouer à l'exécution**.
- [ ] Les écarts avec [SPEC.md](SPEC.md) sont, soit corrigés, soit répercutés dans la spécification avec justification.
- [ ] Les critères de sortie du jalon sont cochés et vérifiés, pas simplement supposés.
