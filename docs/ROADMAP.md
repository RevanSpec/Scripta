# Scripta — Roadmap d'intégration

**Version :** 1.4
**Référence :** [SPEC.md](SPEC.md) v2.4

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
>
> **Point au Jalon 2.** Trois critères sur quatre sont remplis. Reste
> l'**exécution** de l'inférence sous Unix : la CI compile whisper.cpp et
> exécute le pipeline sur les trois OS, mais ses tests d'inférence se sautent
> faute de modèle — la CI de PR n'a pas accès au réseau. La tâche planifiée
> quotidienne (4.7) la couvrira.

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

- [ ] Les spikes A et B compilent et s'exécutent sur **Windows et au moins une plateforme Unix**. *(Compilation : trois OS. Pipeline (B) : exécuté sur les trois OS en CI. Inférence (A) : exécutée sous Windows seulement.)*
- [x] La version de `whisper-rs` est **figée**, avec justification écrite au regard des quatre callbacks requis. *(`=0.16.0` depuis le J2 ; justification dans `crates/core/Cargo.toml` : `set_abort_callback_safe` contourné, VAD intégré inopérant par cette voie — voir R10.)*
- [x] L'[ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle) est confirmé, ou le repli « deux artefacts par plateforme » est acté et sa conséquence reportée sur le [Jalon 4](#jalon-4--packaging-et-cicd). *(Repli acté ; le J4 compte désormais les variantes Vulkan.)*
- [x] L'[Annexe D](SPEC.md#annexe-d--points-à-valider-en-implémentation) ne contient plus d'hypothèse non tranchée. *(Deux mesures restent à faire — performance Vulkan, taille d'une build FFmpeg minimale. Elles sont explicitement reportées au J4, chacune avec son repli.)*

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
> **CI confirmée sur les trois OS** le 2026-09-22
> ([run 35735596493](https://github.com/RevanSpec/Scripta/actions/runs/35735596493)) :
> le build natif de whisper.cpp passe sous Linux, macOS et Windows.

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
- [x] CI verte sur les trois OS, **sans accès réseau**. *(Confirmée le 2026-09-22 sur `main`.)*

### Risques

| Risque | Mitigation |
|---|---|
| Format audio inattendu sur certaines vidéos | Repli `/bestaudio/best` déjà spécifié ; constituer dès ce jalon un corpus de 10 vidéos hétérogènes (musique, live archivé, très longue, langue non latine, sous-titrée, restreinte) servant de banc de test manuel |
| Différences de sémantique des pipes sous Windows | Déjà levé en 0.2 |

---

## Jalon 2 — Robustesse CLI

> **Statut : clos** le 2026-09-23. Les seize tâches sont livrées.
>
> La clôture annoncée la veille était prématurée, sur quatre points depuis
> repris : le VAD n'était pas actif par défaut — et, par la voie de
> whisper-rs, ne s'appliquait même pas (voir 2.6 et R10) ; la progression
> ignorait l'absence de terminal ; `doctor` ne testait ni les droits d'écriture
> ni le réseau ; `--initial-prompt` manquait à la clé de cache, qui resservait
> alors en silence une transcription obtenue sans contexte.
>
> 176 tests, `fmt` et `clippy -D warnings` propres, CI verte sur les trois
> OS ; les tests d'inférence ont tourné sur de vrais modèles, VAD compris.
>
> La suite d'acceptation sur 61 min, VAD actif, a révélé deux défauts, tous
> deux corrigés : la détection VAD, lancée sur tous les threads, ajoutait
> cinq minutes par heure d'audio ; et le contexte glissant de Whisper a
> entretenu une boucle de répétition de 155 s (R11), que le mode VAD
> n'utilise plus. Rejouée sur le code final : aucune anomalie, pic mémoire
> de 863 Mo contre 1 040 sans VAD, 6,4 × temps réel, concordance de 75 % —
> celle de la mesure sans VAD —, et une seule répétition résiduelle, de
> 14 s, bornée à sa fenêtre.
>
> **La CLI est publiable.** C'est le jalon de valeur que le plan recommandait
> de publier avant d'entamer la GUI.

**Objectif :** une CLI **publiable**. C'est le jalon le plus dense et le plus créateur de valeur.

**Effort :** 8–12 jours. **Prérequis :** J1 clos.

### Lot de travail

| # | Tâche | Spécification |
|---|---|---|
| ~~**2.1**~~ | ~~`core::error`~~ ✅ énuméré `ScriptaError` exhaustif, classification large et tolérante du `stderr` de yt-dlp, codes de sortie figés | [SF-07](SPEC.md#sf-07--taxonomie-derreurs-et-codes-de-sortie) |
| ~~**2.2**~~ | ~~Gardes de recevabilité~~ ✅ directs, y compris programmés (`live_status`), `--max-duration`, limite d'âge (code 12), paramètre de playlist ignoré avec avertissement | [SF-01](SPEC.md#sf-01--validation-durl-et-sonde-de-métadonnées), [SF-07](SPEC.md#sf-07--taxonomie-derreurs-et-codes-de-sortie) |
| ~~**2.3**~~ | ~~`core::models`~~ ✅ téléchargement, SHA-256, `.part` + renommage atomique, reprise par `Range`, timeouts, progression, annulation (le `.part` est conservé pour la reprise) | [SF-03](SPEC.md#sf-03--gestion-et-cycle-de-vie-des-modèles-whisper) |
| ~~**2.4**~~ | ~~`--model auto`~~ ✅ résout vers `turbo` si un backend GPU est compilé, `base` sinon. `--backend` sans objet : le backend est lié à la compilation ([ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle) révisé) | [SF-03](SPEC.md#sf-03--gestion-et-cycle-de-vie-des-modèles-whisper) |
| ~~**2.5**~~ | ~~`core::format::{srt,vtt,json}`~~ ✅ contraintes de lisibilité, échappement XML pour WebVTT, schéma JSON versionné | [SF-05](SPEC.md#sf-05--formats-dexportation) |
| ~~**2.6**~~ | ~~VAD Silero (par défaut)~~ ✅ actif par défaut, modèle téléchargé au premier usage, `--no-vad`. **Orchestré par Scripta** (`core::vad`) : par whisper-rs, le VAD intégré de whisper.cpp n'est jamais appliqué, et l'autre voie laisserait les mots dans la chronologie compactée (R10). Sans contexte glissant en mode VAD (R11), sauf avec `--initial-prompt`. `--word-timestamps`, seuils `--no-speech-thold` et `--entropy-thold`, garde `turbo` + `--translate` — désormais avant tout téléchargement | [SF-04](SPEC.md#sf-04--moteur-de-transcription-locale) |
| ~~**2.7**~~ | ~~Progression~~ ✅ pourcentage, position dans la vidéo et vitesse, tirées du rappel de segments, sur **`stderr`** : barre vers un terminal, simples lignes sinon. `indicatif` écarté : une barre d'une ligne ne justifiait pas la dépendance. Le rappel de segments sert aussi l'affichage progressif de la GUI (3.5) | [SF-04](SPEC.md#sf-04--moteur-de-transcription-locale), [§4.1](SPEC.md#41-interface-en-ligne-de-commande) |
| ~~**2.8**~~ | ~~Interruption~~ ✅ premier `Ctrl-C` arme le jeton d'annulation, **à toute étape** — téléchargement, sonde, extraction (sidecars tués), inférence ; second force la sortie en 130 | [§4.1](SPEC.md#41-interface-en-ligne-de-commande) |
| ~~**2.9**~~ | ~~`core::sidecar`~~ ✅ résolution à quatre niveaux, détection de version, mise à jour depuis les releases GitHub vérifiée par SHA-256, installation hors bundle, vérification de disponibilité au plus quotidienne (`SCRIPTA_NO_UPDATE_CHECK`) | [ADR-004](SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour), [SF-06](SPEC.md#sf-06--maintenance-du-sidecar-yt-dlp) |
| ~~**2.10**~~ | ~~`core::cache`~~ ✅ clé couvrant tous les paramètres influents — contexte et seuils compris, entrées VAD antérieures au J2 écartées —, écriture atomique, éviction LRU, consultation avant chargement du modèle | [SF-08](SPEC.md#sf-08--cache-de-transcriptions) |
| ~~**2.11**~~ | ~~Sous-titres officiels~~ ✅ `--prefer-subs` avec repli silencieux, sous-commande `subs` où l'absence est une erreur, analyseur WebVTT déduplicant le défilement | [SF-01](SPEC.md#sf-01--validation-durl-et-sonde-de-métadonnées) |
| ~~**2.12**~~ | ~~`--cookies-from-browser`~~ ✅ transmis à la sonde et à l'extraction, désactivé par défaut | [SF-09](SPEC.md#sf-09--authentification-et-confidentialité) |
| ~~**2.13**~~ | ~~Arborescence CLI complète~~ ✅ `run` (implicite), `subs`, `models`, `cache`, `update-extractor`, `doctor` — droits d'écriture et joignabilité réseau compris ; `--force`, `-v` | [§4.1](SPEC.md#41-interface-en-ligne-de-commande) |
| ~~**2.14**~~ | ~~Suite de tests~~ ✅ 176 tests. Codes de sortie figés par un `match` exhaustif — ajouter une variante sans lui attribuer de code casse la compilation, vérifié par réintroduction. Le sidecar simulé est exclu de toute build ordinaire (feature `test-helpers`) | [§5.5](SPEC.md#55-stratégie-de-test) |
| ~~**2.15**~~ | ~~Banc de performance~~ ✅ `scripts/test-long-video.ps1`, mesure de référence à 4,8 × temps réel sur 61 min, sans VAD ; 6,4 × avec, sur une autre machine | [§5.2](SPEC.md#52-performance) |
| ~~**2.16**~~ | ~~Documentation~~ ✅ `README.md` avec avertissement CGU, `THIRD_PARTY_LICENSES.md`, `docs/VERIFICATION.md` | [§1.3](SPEC.md#13-licence-et-conformité) |

### Critères de sortie

- [x] **Chaque variante de `ScriptaError` est atteignable par un test** et produit le code de sortie contractuel.
- [x] Une vidéo privée, un live, une vidéo géo-bloquée et une vidéo exigeant une authentification produisent chacun un message **actionnable** — jamais une panique ni une trace brute de yt-dlp.
- [x] `scripta <URL> -f json | jq .` fonctionne **sans `--quiet`** (preuve de la séparation stdout/stderr).
- [x] `Ctrl-C` pendant l'inférence rend la main en **moins de 2 secondes**, sans processus orphelin ni fichier résiduel.
- [x] Les seuils du [§5.2](SPEC.md#52-performance) sont mesurés et inscrits dans la spécification (ajustés si nécessaire, avec justification).
- [x] `scripta doctor` diagnostique correctement une installation saine **et** une installation dégradée (sidecar absent, cache non inscriptible, pas de réseau). *(Coché à tort le 2026-09-22 : `doctor` ne testait alors ni l'écriture ni le réseau. Vérifié le 2026-09-23 sur les trois cas — `PATH` sans sidecars, `SCRIPTA_CACHE_DIR` sous un fichier, mandataire injoignable.)*
- [x] CI verte sur trois OS, **toujours sans réseau**. *(Confirmée le 2026-09-22 sur `main`, puis le 2026-09-23 sur la PR de clôture — [run 35840187124](https://github.com/RevanSpec/Scripta/actions/runs/35840187124), 175 tests par OS.)*

### Risques

| Risque | Mitigation |
|---|---|
| Les motifs de `stderr` de yt-dlp changent → mauvaise classification des erreurs | Classification par correspondance **large et tolérante**, avec repli sur `ExtractionFailed` (20) et affichage du `stderr` brut. Ne jamais faire dépendre une logique de contrôle d'une chaîne de caractères exacte |
| Les seuils de performance ne sont pas atteints | Ils sont mesurés **ici**, pas promis. Ajuster la spécification plutôt que le code |
| Le VAD dégrade certains contenus (chant, parole très continue) | `--no-vad`, documenté dans le README et dans l'aide de l'option |

---

## Jalon 3 — Interface de bureau (Tauri v2)

> **Statut : en cours — l'application fonctionne, en développement.** Les
> douze tâches sont livrées, la dernière — 3.12, `externalBin` — avec les
> sidecars embarqués du J4. Restent des critères de sortie, ci-dessous.
>
> - **3.1** Squelette Tauri v2 et Svelte 5. CSS natif plutôt que Tailwind :
>   deux vues, deux cents lignes de style, la dépendance ne se justifiait pas.
> - **3.2–3.3** Dix commandes IPC, et aucun travail long sur le thread
>   principal. Progression et segments arrivent par un canal propre à chaque
>   appel, regroupés à dix envois par seconde au plus.
> - **3.4–3.7** Écran principal ; segments au fil de l'eau, avec un suivi que
>   l'utilisateur interrompt en remontant ; Annuler ; options avancées (VAD,
>   traduction, horodatage au mot, contexte, cookies).
> - **3.8** Gestion des modèles : téléchargement avec progression et reprise,
>   suppression, espace occupé.
> - **3.9–3.11** Export par la boîte d'enregistrement native ; copie ; un
>   message propre à l'interface pour chaque variante de `ScriptaError` ;
>   mise à jour de yt-dlp.
>
> **Reprise du premier squelette.** Écrit avant la clôture du J2, il ne
> compilait plus contre le cœur, et portait trois défauts de fond :
> - `cancel` attendait un verrou tenu pendant toute l'inférence : la fenêtre
>   se figeait jusqu'à la fin, et l'annulation n'annulait rien ;
> - la sonde et le diagnostic s'exécutaient sur le thread principal ;
> - son propre enchaînement avait dérivé de celui de la CLI : VAD absent, clé
>   de cache incomplète.
>
> L'enchaînement vit désormais dans `core::pipeline`, commun aux deux
> interfaces. La CLI y est passée à sortie identique.
>
> **Acceptation sur 61 min**, build optimisée, sous Windows. La fenêtre est
> restée réactive de bout en bout : 14,0 × temps réel, pic mémoire de
> 882 Mo — 19 de plus que la CLI —, 930 segments, autant que la mesure de
> référence. Elle a révélé deux défauts, corrigés :
> - **R12.** Le nombre de threads par défaut prenait tous les cœurs logiques.
>   Qu'une application voisine occupe un seul cœur, et l'inférence tombait à
>   0,2 × le temps réel, CLI comprise.
> - **Fuite du rappel de progression.** Celui de whisper-rs fuyait sa
>   fermeture, et avec elle, à chaque transcription, le relais des messages
>   et son thread.

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

- [ ] `cargo tauri dev` fonctionne sur les trois plateformes. *(Windows : éprouvé, installeur compris. **Linux : éprouvé — l'AppImage démarre, les sidecars embarqués s'exécutent et l'interface s'affiche, après le correctif R13 et l'ajout de `libgles2` sur un hôte dépouillé ; voir [VERIFICATION](VERIFICATION.md).** macOS : le crate compile et ses tests passent en CI, mais l'application n'y a pas été lancée.)*
- [x] **La fenêtre reste réactive du début à la fin** d'une transcription d'une heure : aucun gel, l'annulation reste cliquable en permanence. *(61 min, build optimisée : onglets, options et défilement répondent pendant l'extraction comme pendant l'inférence.)*
- [x] L'annulation en cours d'inférence libère les ressources en moins de 2 s. *(Moins d'une demi-seconde, même en build de débogage. Sur la vidéo d'une heure, la mémoire retombe de 881 à 181 Mo dans la seconde ; seul le modèle reste chargé.)*
- [ ] L'accélération affichée correspond au matériel réel sur au moins deux configurations distinctes. *(Une seule configuration éprouvée, CPU : aucune machine GPU de test.)*
- [x] Aucune variante de `ScriptaError` ne produit un message technique brut dans l'interface. *(Correspondance exhaustive, qu'une variante nouvelle ne passe pas sans message ; un test vérifie chaque variante.)*

### Risques

| Risque | Mitigation |
|---|---|
| Sidecars non résolus en mode développement (chemins différents du mode bundle) | Abstraire la résolution dans `core::sidecar` dès le J2 (tâche 2.9), avec surcharge par variable d'environnement. **Traité :** en build optimisée, la GUI ne connaît que la copie mise à jour et la copie embarquée, jamais le `PATH` (§5.1) ; le `PATH` ne sert qu'en build de débogage |
| Débit d'événements trop élevé (un événement par segment sur une vidéo dense) | Regroupement temporel côté Rust (au plus ~10 émissions/s). **Traité :** relais à dix lots par seconde au plus, éprouvé sous flux continu ; la progression n'est transmise qu'en hausse |

---

## Jalon 4 — Packaging et CI/CD

> **Statut : entamé.** Un tag `v*` construit la CLI **et l'application de
> bureau** sur trois cibles — Windows x86-64, Linux x86-64, macOS Apple
> Silicon —, les contrôle, les empaquette avec leurs licences et leurs sommes
> de contrôle, puis prépare un **brouillon** de release : la publication
> reste un geste humain. Sans tag, le workflow sert de répétition générale.
>
> La v0.1.0, préparée ainsi, n'a pas été publiée : elle portait le défaut de
> threads révélé au J3 (R12). La première version publique est la v0.1.1.
>
> Livré :
>
> - **4.1** Sidecars acquis par triplet cible, et vérifiés : yt-dlp aux
>   versions et empreintes épinglées (`scripts/sidecars/versions.env`), ffmpeg
>   compilé depuis une archive source dont la signature a été vérifiée avant
>   d'en épingler l'empreinte.
> - **4.2** Build FFmpeg minimale, statique, sous LGPL : 3,2 Mo sous Linux,
>   1,9 Mo sous Windows et macOS. Sur sa propre plateforme, chaque binaire
>   décode cinq échantillons par la commande même du cœur.
> - **4.5** Installeurs non signés : NSIS (22 Mo), AppImage (125 Mo) et `.deb`
>   (47 Mo), `.dmg` (43 Mo). yt-dlp en fait l'essentiel — son exécutable
>   autonome embarque Python, 40 Mo sous Linux —, et l'AppImage, WebKitGTK.
> - **4.6** Sommes de contrôle, notes de version, versions des sidecars, et
>   l'archive source de FFmpeg, que la LGPL impose de distribuer.
>
> Restent :
>
> - le binaire universel de la CLI sous macOS (4.4) ;
> - les variantes Vulkan (4.3) ;
> - la tâche quotidienne (4.7) et le banc de performance en CI (4.8) ;
> - la glibc 2.31 visée par le §5.3 : la CLI exige la 2.34, relevée sur
>   le binaire construit sous Ubuntu 22.04.
>
> **Pas de signature** (décision du 2026-09-24) : ni Developer ID ni
> notarisation sous macOS, ni Authenticode sous Windows. Les installeurs le
> resteront ; SmartScreen et Gatekeeper avertissent au premier lancement, et
> le README donne la marche à suivre. Sans objet désormais : l'adhésion au
> Apple Developer Program, les certificats et leurs secrets en CI.

**Objectif :** un `git tag` produit des artefacts installables ~~et signés~~ pour les trois plateformes.

**Effort :** 5–8 jours. **Prérequis :** J2 pour la CLI ; J3 pour la GUI. *Peut démarrer partiellement en parallèle du J3.*

### Lot de travail

| # | Tâche | Spécification |
|---|---|---|
| **4.1** | Script d'acquisition des sidecars par triplet cible (yt-dlp + ffmpeg), avec **vérification d'intégrité** | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.2** | **Build FFmpeg minimale** (décodeurs et démultiplexeurs strictement nécessaires) ou sélection d'une source de builds réduites | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.3** | Matrice GitHub Actions : 3 OS × {CLI, GUI}, plus la variante Vulkan sous Windows et Linux ([ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle) révisé). Contournement R9 dans chaque job Windows | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.4** | ~~**Signature et notarisation macOS** de l'application **et de tous les sidecars**~~ — abandonnées (décision du 2026-09-24) —, binaire universel pour la CLI | [§5.4](SPEC.md#54-distribution-et-packaging), [ADR-004](SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour) |
| **4.5** | Installeurs : NSIS (Windows), AppImage + `.deb` (Linux), `.dmg` (macOS) | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.6** | Workflow de release : sommes de contrôle, notes de version, inventaire des versions de sidecars embarquées | [§5.4](SPEC.md#54-distribution-et-packaging) |
| **4.7** | **Tâche planifiée quotidienne** : transcription d'une vidéo de référence, alerte en cas de rupture d'extracteur | [§5.5](SPEC.md#55-stratégie-de-test) |
| **4.8** | Banc de performance en CI (non bloquant, suivi de tendance) | [§5.2](SPEC.md#52-performance) |

### Critères de sortie

- [ ] Un tag produit les artefacts de la matrice [ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle) révisée — CLI et GUI sur les trois OS, et leur variante Vulkan sous Windows et Linux —, publiés avec leurs sommes de contrôle.
- [ ] Le `.dmg` s'installe et se lance sur un **Mac vierge** (Apple Silicon), ~~sans avertissement Gatekeeper~~ par la procédure du README : sans signature, Gatekeeper avertit.
- [ ] `update-extractor` fonctionne sur l'application **installée** ~~et signée~~ des trois plateformes — c'est le test qui valide [ADR-004](SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour), et il **ne peut pas** être fait en mode développement. *(**Windows : fait** le 2026-09-25 sur l'installeur NSIS — le binaire mis à jour atterrit dans `bin/` et l'interface le marque « mis à jour » ; voir [VERIFICATION](VERIFICATION.md). Linux et macOS : restent.)*
- [ ] La CLI démarre sur une machine **sans GPU ni driver** (validation de [ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle)).
- [ ] La tâche quotidienne s'exécute et alerte correctement.

### Risques

| Risque | Probabilité | Mitigation |
|---|---|---|
| **Notarisation macOS** — sidecars non signés, droits manquants, chemins d'exécution | **Sans objet** | Pas de signature (décision du 2026-09-24) |
| Taille des artefacts GUI (FFmpeg) | Moyenne | Tâche 4.2 ; à défaut, accepter et documenter |
| Secrets de signature en CI (certificat Apple, mots de passe) | **Sans objet** | Pas de signature |

> ~~**Alerte de séquencement.** L'adhésion au Apple Developer Program et l'émission des certificats prennent des jours ouvrés. À initier au plus tard au début du Jalon 3.~~ Sans objet : pas de signature.

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
| R1 | Build `whisper-rs` + GPU sous Windows | Élevé | **Réduite** | J0 → J4 | Le build natif CPU passe sur les trois OS en CI. Le build **Vulkan** n'a jamais été tenté : à éprouver au plus tard au J4, où la variante Vulkan devient un artefact. Repli : CPU seul sous Windows |
| R2 | Notarisation macOS des sidecars | Élevé | **Sans objet** | J4 | Pas de signature (décision du 2026-09-24). Reste l'avertissement de Gatekeeper au premier lancement, que le README apprend à lever |
| R3 | **YouTube casse les extracteurs** | Élevé | **Certaine** *(question de quand, pas de si)* | Continu | [SF-06](SPEC.md#sf-06--maintenance-du-sidecar-yt-dlp) + tâche quotidienne 4.7 + [ADR-004](SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour). **C'est la raison d'être de ces trois éléments** |
| R4 | Vérification anti-robot bloquant les utilisateurs | Moyen | Élevée | J2 | Diagnostic explicite (code 12) + `--cookies-from-browser` documenté |
| R5 | Chargement dynamique des backends non supporté | Moyen | Moyenne | J0 | Repli deux artefacts (+2 j sur le J4) |
| R6 | Seuils de performance non atteints | Faible | **Écartée** | J2 | Mesuré à 4,8 × temps réel sur une machine au repos, contre un seuil de 3 ×. Les mesures antérieures à 2,1 × et 2,7 × étaient faussées par des compilations concurrentes : **un débit ne se mesure que sur une machine inoccupée**. Au J3, 15,8 × avec le VAD et le nombre de threads corrigé (R12), une application voisine occupant pourtant un cœur |
| R7 | Deadlock `stderr` découvert tardivement | Élevé | **Écartée par construction** | J1 | Test de non-régression dédié (1.8), exigé en critère de sortie |
| R8 | Dérive de périmètre vers la GUI avant stabilisation du cœur | Moyen | **Écartée** | J2/J3 | J2 clos et v0.1.0 CLI construite avant la reprise de la GUI, qui s'est branchée sur un cœur stable : son enchaînement est celui de la CLI, dans `core::pipeline` |
| R9 | **`--release` produit un whisper.cpp non optimisé sous MSVC** | Élevé | **Avérée — contournée** | J4 | Mesuré : sans correctif, release est 4 à 6× plus lent que debug. La crate `cmake` écrase `CMAKE_CXX_FLAGS_<BUILD_TYPE>` tandis que le générateur Visual Studio compile en `--config Release` : le profil release perd son `/O2`. **Contournement appliqué** — `CMAKE_{C,CXX}_FLAGS_RELEASE` forcés par l'environnement, que le `build.rs` de `whisper-rs-sys` réinjecte en define. Vérifié : 11,7–14,0× temps réel après correctif, contre 1,4–2,0× avant. Le contournement doit être porté dans la CI, le script de test et tout pipeline d'empaquetage : `.cargo/config.toml` ne permet pas d'`[env]` conditionné à la cible. Porté dans la CI et le script de test ; reste l'empaquetage (J4). ⚠ Depuis Git Bash, MSYS convertit ces valeurs — qui commencent par `/` — en chemins, et la compilation échoue : les poser depuis PowerShell ou cmd. Correctif amont souhaitable. |
| R10 | **VAD intégré de whisper.cpp inopérant via whisper-rs, et horodatages de mots faux par l'autre voie** | Élevé | **Avérée — contournée** | J2 | `WhisperState::full` appelle `whisper_full_with_state`, qui ignore `params.vad` : `--vad-model` n'a jamais eu d'effet. Et `whisper_full`, qui applique le VAD, ne replace pas les tokens sur la chronologie d'origine. **Contournement :** VAD orchestré par Scripta (`core::vad`) — détection Silero, compactage sur place, correspondance exacte des chronologies. Vérifié par réintroduction : avec le VAD intégré, une parole placée à 5 s ressort horodatée à 0 s, et `le_vad_conserve_la_chronologie_d_origine` échoue. À réexaminer à chaque montée de whisper-rs |
| R11 | **Boucle de répétition entretenue par le contexte glissant, révélée par le VAD** | Moyen | **Avérée — contournée** | J2 | Sur la vidéo de référence, VAD actif : 71 segments consécutifs « et qui est en train de se faire », de 2 240 à 2 395 s — 155 s de parole perdues, là où la mesure sans VAD n'en montrait aucune. Non reproduite sur un extrait de 700 s : elle dépend du contexte accumulé depuis le début. **Évaluée sur la vidéo entière :** sans contexte glissant (`n_max_text_ctx = 0`), la boucle disparaît — pire série, 7 segments bornés à leur fenêtre —, la concordance passe de 73 à 75 % et le débit de 6,9 à 8,0×. **Contournement :** en mode VAD, aucune fenêtre n'est conditionnée sur les précédentes. **Limite :** avec `--initial-prompt`, le contexte glissant est conservé, car whisper.cpp fait passer l'invite par le même canal et whisper-rs 0.16 n'expose pas `carry_initial_prompt` ; le risque demeure dans ce cas. Les entrées de cache VAD antérieures sont écartées |
| R12 | **Effondrement du débit quand un thread d'inférence est privé de processeur** | Élevé | **Avérée — corrigée** | J3 | ggml synchronise ses threads par attente active. Avec un thread par cœur logique — le défaut du J2 —, une application voisine occupant un seul cœur a fait tomber l'inférence à **0,2 ×** le temps réel sur un i7-13700H (14 cœurs, 20 threads logiques), contre 12 × avec 16 threads. Révélé par l'acceptation de la GUI ; la CLI y était tout aussi exposée. **Correctif :** par défaut, les cœurs physiques, en laissant au moins deux threads logiques libres (`num_cpus`), comme le prévoyait la SPEC. Sans charge voisine, le débit culmine justement aux cœurs physiques ; la vidéo de référence passe de 6,4 à 15,8 × en CLI, 14,0 × dans l'application de bureau |
| R13 | **L'AppImage embarque une `libwayland-client` qui empêche l'affichage sur un hôte récent** | Élevé | **Avérée — corrigée** | J4 | linuxdeploy embarque la bibliothèque de la machine de build (Ubuntu 22.04) ; l'AppRun la place devant celle du système, et `libEGL_mesa.so.0`, qui en dépend, ne s'initialise plus. WebKit abandonne son processus de rendu : **la fenêtre s'ouvre vide, sans message**. Isolée par bissection sur les 169 bibliothèques embarquées, le 2026-09-25 sous Ubuntu 26.04. **Correctif :** `scripts/empaquetage/appimage-sans-wayland-client.sh` la retire après l'empaquetage et contrôle le produit fini, en s'arrêtant si le bundler cesse de l'embarquer. Défaut invisible sur la machine de build, dont la bibliothèque est justement celle qui est embarquée : **seul un hôte différent le révèle** |

---

## Définition de terminé (transversale)

Aucun jalon n'est clos tant que, pour tout code produit :

- [ ] `cargo fmt --check` et `cargo clippy -- -D warnings` passent.
- [ ] Les tests associés existent et sont verts sur les trois OS, **sans réseau**.
- [ ] Les erreurs sont typées dans `ScriptaError` — **aucun `unwrap()` sur un chemin pouvant échouer à l'exécution**.
- [ ] Les écarts avec [SPEC.md](SPEC.md) sont, soit corrigés, soit répercutés dans la spécification avec justification.
- [ ] Les critères de sortie du jalon sont cochés et vérifiés, pas simplement supposés.
