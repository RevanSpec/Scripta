# Scripta — Cahier des charges technique et fonctionnel

**Version :** 2.4
**Statut :** Validé pour implémentation — ADR-001 révisé au Jalon 0, SF-04 précisé au Jalon 2
**Révision précédente :** 1.0 (voir [Annexe C — Journal des corrections](#annexe-c--journal-des-corrections))

---

## 1. Présentation générale

### 1.1 Vision

Scripta est un outil de transcription audio autonome, performant et respectueux de la vie privée. À partir d'une URL YouTube, il extrait le flux audio sans télécharger le flux vidéo, réalise l'inférence **localement** via Whisper, puis exporte le texte sous différents formats (brut, sous-titres, structuré).

L'outil se décline sous deux formes bâties sur le même moteur :

- **`scripta`** — un binaire CLI scriptable, intégrable dans des pipelines d'automatisation.
- **Scripta Desktop** — une application de bureau (GUI) réactive et multiplateforme.

### 1.2 Périmètre et non-objectifs

**Dans le périmètre :**

- Vidéos YouTube publiques et non-listées (VOD uniquement).
- Transcription locale, sans aucun envoi de contenu à un service tiers.
- Traduction vers l'anglais (fonction native de Whisper).
- Export `txt`, `srt`, `vtt`, `json`.

**Hors périmètre (v1) — décisions explicites, pas des oublis :**

| Non-objectif | Justification |
|---|---|
| Diarisation (identification des locuteurs) | Nécessite un second modèle (pyannote/sherpa) et une logique d'alignement. Candidat v2. |
| Transcription de lives en direct | Flux de durée non bornée, incompatible avec le modèle d'inférence retenu ([ADR-003](#adr-003--inférence-non-streamée)). Détecté et refusé proprement ([SF-07](#sf-07--taxonomie-derreurs-et-codes-de-sortie)). |
| Plateformes autres que YouTube | `yt-dlp` en supporte des milliers, mais la validation d'URL ([SF-01](#sf-01--validation-durl-et-sonde-de-métadonnées)) est volontairement restrictive. Élargissement trivial mais non testé en v1. |
| Édition du texte transcrit dans la GUI | La GUI affiche et exporte ; l'édition relève d'un éditeur de sous-titres. |
| Traduction vers une langue autre que l'anglais | Limite intrinsèque de Whisper. |

### 1.3 Licence et conformité

- **Licence du projet : GPLv3** — déjà actée (`LICENSE` à la racine du dépôt).
- **Compatibilité amont :** `yt-dlp` (The Unlicense) et `whisper.cpp` / `whisper-rs` (MIT) sont compatibles sans réserve.
- **FFmpeg :** distribué comme binaire séparé invoqué par sous-processus. Les builds GPL de FFmpeg sont « GPLv2 ou ultérieure », donc compatibles GPLv3. **Obligation :** embarquer les textes de licence de FFmpeg et de yt-dlp dans le bundle (`THIRD_PARTY_LICENSES.md`) et publier une offre de code source conforme. *(J4 : l'application de bureau embarque une build **LGPL** minimale de FFmpeg, sans composant GPL, compilée depuis les sources publiées. Chaque installeur joint dans `licences/` les textes de licence de FFmpeg et de yt-dlp, la notice des composants qu'embarquent les exécutables de yt-dlp, et la fiche de construction de FFmpeg ; l'archive source de FFmpeg accompagne chaque release.)*
- **⚠️ Incompatibilité connue :** la GPLv3 est incompatible avec les conditions de l'App Store d'Apple. La distribution macOS se fait exclusivement par `.dmg`, hors App Store — non signé, voir [§5.4](#54-distribution-et-packaging).
- **Conditions d'utilisation YouTube :** le téléchargement de contenu contrevient aux CGU de YouTube. Le `README.md` doit porter un avertissement explicite indiquant que l'outil est fourni à des fins d'usage personnel et licite, et que la responsabilité de l'usage incombe à l'utilisateur.

### 1.4 Nommage et conventions

| Élément | Valeur |
|---|---|
| Nom du projet / dépôt | **Scripta** |
| Binaire CLI | **`scripta`** |
| Application de bureau | **Scripta Desktop** |
| Identifiant bundle | `com.scripta.desktop` (à ajuster selon le domaine retenu) |
| Répertoire de cache | `scripta/` (voir [SF-03](#sf-03--gestion-et-cycle-de-vie-des-modèles-whisper)) |

> **Décision :** le nom `yt-transcribe` de la v1 du cahier des charges est abandonné au profit de `scripta`, par cohérence avec le dépôt. Un renommage ultérieur se paierait en dette documentaire pendant des mois.

---

## 2. Architecture technique

### 2.1 Vue d'ensemble

Le cœur applicatif et les interfaces sont développés en Rust. L'extraction réseau et le décodage du flux YouTube sont délégués à deux binaires sidecars autonomes (`yt-dlp`, `ffmpeg`).

```
 ┌─────────────────────────────────────────────────────────────┐
 │                    Couche Présentation                      │
 │   CLI (Rust / clap)      │     GUI (Tauri v2 + Svelte)      │
 └──────────────────────────┬──────────────────────────────────┘
                            │
                            ▼
 ┌─────────────────────────────────────────────────────────────┐
 │                     crates/core (Rust)                      │
 │  Orchestration · sonde métadonnées · cache modèles ·        │
 │  cache transcriptions · formateurs · taxonomie d'erreurs    │
 └───────────┬─────────────────────────────────┬───────────────┘
             │ Pipes anonymes (stdout)         │ Mémoire (Vec<f32>)
             ▼                                 ▼
 ┌───────────────────────────────┐  ┌──────────────────────────┐
 │        Sidecars externes      │  │  whisper-rs (ggml/C++)   │
 │  yt-dlp  ──►  ffmpeg          │  │  Inférence IA locale     │
 │  (WebM/Opus)  (PCM s16le      │  │  Vulkan / Metal / CPU    │
 │               16 kHz mono)    │  │  Backend lié à la        │
 │                               │  │  compilation (ADR-001)   │
 └───────────────────────────────┘  └──────────────────────────┘
```

**Flux nominal :**

1. Validation de l'URL (`core::url`).
2. Sonde métadonnées unique `yt-dlp -J` → titre, chaîne, ID, durée, `is_live`, sous-titres disponibles.
3. Contrôle de recevabilité (live ? durée excessive ? cache présent ?).
4. Résolution du modèle (cache local ou téléchargement vérifié).
5. Extraction audio streamée `yt-dlp | ffmpeg` → `Vec<f32>` en RAM.
6. Inférence `whisper_full` avec callbacks de progression et de segments.
7. Formatage et écriture.

> **Mis en œuvre au Jalon 3.** Cet enchaînement vit dans `core::pipeline`, et
> nulle part ailleurs : ce qui entre dans la clé de cache, l'ordre des étapes
> selon `--prefer-subs`, le repli silencieux, la primauté de l'annulation. La
> CLI et la GUI n'en reçoivent que les événements, qu'elles rendent chacune à
> leur manière. Le premier squelette de la GUI, qui tenait sa propre version
> de l'enchaînement, avait dérivé en quelques semaines : VAD absent, clé de
> cache incomplète, éviction oubliée.

### 2.2 Découpage des crates

```
Scripta/
├── Cargo.toml                  # workspace
├── LICENSE                     # GPLv3
├── THIRD_PARTY_LICENSES.md
├── docs/
│   ├── SPEC.md
│   └── ROADMAP.md
├── crates/
│   ├── core/                   # bibliothèque — aucune dépendance UI
│   │   ├── src/
│   │   │   ├── url.rs          # validation stricte
│   │   │   ├── probe.rs        # yt-dlp -J → Metadata
│   │   │   ├── audio.rs        # pipeline yt-dlp|ffmpeg → Vec<f32>
│   │   │   ├── models.rs       # cache, téléchargement, SHA-256
│   │   │   ├── transcribe.rs   # wrapper whisper-rs
│   │   │   ├── format/         # txt, srt, vtt, json
│   │   │   ├── sidecar.rs      # résolution de chemin, mise à jour
│   │   │   ├── cache.rs        # cache de transcriptions
│   │   │   ├── pipeline.rs     # orchestration, commune à la CLI et à la GUI
│   │   │   └── error.rs        # ScriptaError (taxonomie SF-07)
│   │   └── tests/
│   │       └── fixtures/       # WAV courts, faux sidecars, golden files
│   ├── cli/                    # binaire `scripta`
│   └── desktop/                # wrapper Tauri v2
│       ├── tauri.conf.json
│       ├── capabilities/       # permissions de la fenêtre : core:default seul
│       ├── src/                # backend Rust (commandes IPC)
│       ├── ui/                 # frontend TypeScript + Svelte
│       └── binaries/           # sidecars par triplet cible, acquis ou compilés
│                               # par scripts/sidecars/ — non versionnés
```

> **Correction v1 :** `src-tauri/` était placé à la racine, à côté de `crates/`. Il devient `crates/desktop/` pour homogénéiser le workspace.

### 2.3 Décisions d'architecture (ADR)

Ces quatre décisions sont structurantes : les inverser après le Jalon 2 coûte cher.

#### ADR-001 — Stratégie d'accélération matérielle

**Contexte.** La v1 du cahier des charges contenait une contradiction : le §5.2 décrivait une *compilation conditionnelle* des backends (`cuda`, `metal`, AVX2), tandis que la maquette GUI affichait « CUDA (NVIDIA RTX 4070) détectée », ce qui suppose une *détection à l'exécution*. Les deux sont incompatibles : un binaire lié statiquement à CUDA **ne démarre pas** sur une machine dépourvue de driver NVIDIA — l'éditeur de liens dynamique échoue avant `main()`.

> **⚠ Révisé au Jalon 0 — le chargement dynamique n'est pas disponible.** La
> décision initiale reposait sur `GGML_BACKEND_DL`. Vérification faite,
> `whisper-rs-sys` 0.15 (dépendance de `whisper-rs` 0.16) **ne l'expose pas** :
> son `build.rs` sélectionne les backends par `cfg!(feature = …)` et les lie
> statiquement. Un artefact ne peut donc pas découvrir un backend à l'exécution.
> Le repli prévu par l'[Annexe D](#annexe-d--points-à-valider-en-implémentation)
> — un artefact par backend — est appliqué ci-dessous.

**Décision.**

| Plateforme | Artefact par défaut | Artefact accéléré |
|---|---|---|
| macOS (Apple Silicon) | **Metal** (toujours présent sur la cible) | — |
| macOS (Intel) | CPU (AVX2) | — |
| Windows x86_64 | CPU (AVX2) | `scripta-vulkan` |
| Linux x86_64 | CPU (AVX2) | `scripta-vulkan` |

- **Vulkan plutôt que CUDA** : un seul backend couvre NVIDIA, AMD et Intel. CUDA n'apporte un gain significatif que sur les gros modèles et impose une matrice de build (toolkit CUDA + MSVC sur Windows) notoirement fragile, pour ne couvrir qu'un seul fabricant.
- **Sélection par feature Cargo** : `vulkan`, `cuda`, `metal` dans `crates/core`. L'absence de feature donne une build CPU, qui démarre partout.
- Une build CUDA (`--features cuda`) reste possible depuis les sources ; **elle n'est pas distribuée**.
- `Backend::compiled()` rapporte le backend de la compilation. **La GUI affiche donc ce avec quoi le binaire a été construit, pas le matériel découvert** — la maquette du [§4.2](#42-interface-de-bureau-tauri-v2) est à lire dans ce sens.

**Conséquences.**

- **Deux artefacts par plateforme** sous Windows et Linux, un seul sous macOS. Le [Jalon 4](ROADMAP.md#jalon-4--packaging-et-cicd) s'alourdit d'autant (estimation révisée : +2 j).
- Le téléchargement doit orienter l'utilisateur vers le bon artefact ; `scripta doctor` indique le backend compilé et signale qu'une variante accélérée existe.
- À réexaminer lorsque `whisper-rs-sys` exposera `GGML_BACKEND_DL` : la décision initiale redeviendrait alors applicable et supprimerait un artefact.

#### ADR-002 — Pipeline audio sans shell

**Contexte.** La v1 documentait le pipeline sous forme de commande shell (`yt-dlp … | ffmpeg …`) tout en interdisant le shell au §5.1 — contradiction directe.

**Décision.** Les deux processus sont câblés manuellement en Rust, sans interpréteur de commandes. Le `stdout` de `yt-dlp` est branché sur le `stdin` de `ffmpeg` via `Stdio::from()`. Voir [SF-02](#sf-02--pipeline-dextraction-audio--zero-disk-) pour l'implémentation de référence.

#### ADR-003 — Inférence non streamée

**Contexte.** La v1 laissait entendre que l'inférence consommait l'audio en flux. C'est faux : `whisper_full()` prend le buffer PCM **entier** en argument.

**Décision.** Le téléchargement est streamé (aucun fichier temporaire), l'inférence ne l'est pas. L'audio est accumulé intégralement en RAM, puis soumis en un appel.

**Justification.** Le téléchargement d'une heure d'audio Opus prend quelques dizaines de secondes, l'inférence plusieurs minutes : le recouvrement des deux n'apporterait qu'un gain marginal. À l'inverse, un découpage manuel en tranches dégrade la qualité aux jointures (perte de contexte, mots coupés, répétitions) et casse la cohérence des horodatages.

**Conséquences.**

- **Empreinte mémoire : ≈ 560 Mo par heure d'audio au pic, plus ~260 Mo fixes.** Mesuré sur une vidéo de 61 min avec le modèle `base`, sans VAD : **1 040 Mo**.

  | Poste | Échelle |
  |---|---|
  | PCM `f32` (notre tampon) | 223 Mo/h (16 000 × 4 octets) |
  | Tampons de calcul ggml | ≈ 337 Mo, mesurés, pour `base` |
  | **Copie padded de whisper.cpp** | 223 Mo/h + 1,8 Mo de padding fixe |
  | Spectrogramme mel | 112 Mo/h (80 bandes × 100 trames/s × 4 octets) |
  | Modèle | 75 Mo à 1,1 Go selon la variante |

  > **Corrigé au Jalon 2, en deux temps.** La v2.0 annonçait 230 Mo/h en ne
  > comptant que le PCM. Deux postes majeurs manquaient :
  >
  > - le **spectrogramme mel**, calculé intégralement en amont ;
  > - une **copie complète de l'audio** que `log_mel_spectrogram` alloue pour
  >   y appliquer 30 s de padding (`samples_padded`, whisper.cpp:3194). L'audio
  >   réside donc **deux fois** en mémoire pendant le calcul du mel.
  >
  > Cette copie est transitoire — libérée dès le mel calculé — mais elle
  > survient précisément au moment du pic. Elle explique à elle seule pourquoi
  > l'empreinte réelle vaut le double de l'estimation initiale.
  >
  > **Optimisation possible (v2) :** notre tampon reste vivant pendant toute
  > l'inférence alors que whisper n'en a plus besoin après le mel. L'API
  > `full(&[f32])` de `whisper-rs` interdit de le libérer plus tôt.
  >
  > **Réservé n'est pas résident.** Le doublement du `Vec` portait sur
  > l'espace d'adressage, pas sur la mémoire physique : la moitié haute de la
  > capacité n'était jamais écrite, donc jamais résidente. Deux mesures
  > successives, avant et après correctif, ont donné **1 039 Mo à l'octet
  > près** — par construction, et non par coïncidence. Corriger la
  > pré-allocation reste utile (une recopie de 224 Mo évitée, l'espace
  > d'adressage divisé par deux) mais ne réduit pas l'empreinte physique.
  > Le seul instrument fiable ici est l'allocation elle-même, vérifiée par
  > `le_tampon_audio_n_est_pas_realloue`.
  >
  > **Avec le VAD (Jalon 2).** L'audio est compacté **sur place**
  > (`core::vad`) : aucun tampon n'est ajouté, et la copie padded comme le
  > spectrogramme ne portent plus que sur la parole. Le VAD ne peut donc
  > qu'abaisser le pic. Le VAD intégré de whisper.cpp, lui, aurait ajouté une
  > copie de la parole au moment même du pic — l'une des raisons de l'écarter
  > (SF-04).

- Une garde `--max-duration` (défaut 240 min) protège contre les vidéos pathologiques. À 4 h, l'empreinte approcherait 1,7 Go.
- L'affichage progressif de la GUI est alimenté par le **callback de nouveaux segments** de whisper.cpp, pas par un découpage. whisper.cpp traite l'audio séquentiellement par fenêtres de 30 s et émet ses segments au fil de l'eau : le rendu est donc bien progressif, simplement il démarre une fois le téléchargement achevé.

#### ADR-004 — Emplacement des sidecars mis à jour

**Contexte.** La v1 prévoyait une mise à jour in-place du binaire `yt-dlp` via `yt-dlp -U`. **Sur macOS, remplacer un fichier à l'intérieur d'un `.app` signé invalide la signature de code ; sur Apple Silicon, l'application refuse alors de se lancer.** Le même mécanisme échoue sous Windows lorsque l'application est installée dans `Program Files` (écriture refusée sans élévation).

**Décision.** Deux emplacements, avec priorité à la copie utilisateur :

| | Chemin |
|---|---|
| Sidecar embarqué (lecture seule) | à l'intérieur du bundle applicatif |
| Sidecar mis à jour (inscriptible) | `<data_dir>/scripta/bin/` |

`core::sidecar::resolve()` retourne la copie utilisateur si elle existe **et** que sa version est supérieure, sinon la copie embarquée. La mise à jour ([SF-06](#sf-06--maintenance-du-sidecar-yt-dlp)) télécharge toujours vers l'emplacement inscriptible ; le bundle signé n'est jamais modifié.

> **Sans signature (décision du 2026-09-24).** Les bundles ne sont pas signés,
> mais la décision tient pour la seconde raison : l'emplacement d'installation
> n'est pas inscriptible sans élévation — `Program Files` pour une
> installation par machine, `/usr/bin` pour un `.deb`, `/Applications` pour un
> compte non administrateur.

---

## 3. Spécifications fonctionnelles

### SF-01 — Validation d'URL et sonde de métadonnées

**Validation stricte.** Avant tout appel réseau, l'URL est analysée par `url::Url` (jamais par expression régulière) :

- Schéma : `https` exclusivement.
- Hôte, en liste blanche exacte : `youtube.com`, `www.youtube.com`, `m.youtube.com`, `music.youtube.com`, `youtu.be`.
- Extraction de l'identifiant vidéo, validé contre `^[A-Za-z0-9_-]{11}$`.
- **L'URL n'est jamais transmise telle quelle au sidecar.** Une URL canonique est reconstruite à partir de l'identifiant validé : `https://www.youtube.com/watch?v=<ID>`. Cela neutralise par construction toute injection d'argument ou de paramètre de requête.
- Les URL de playlist (`list=`) sont détectées ; en v1 le paramètre est ignoré et un avertissement signale que seule la vidéo est traitée (`--no-playlist` est passé à `yt-dlp`).

**Sonde unique.** Une seule invocation `yt-dlp -J --no-playlist -- <URL>` (`--dump-single-json`) retourne l'ensemble des métadonnées nécessaires :

| Champ | Usage |
|---|---|
| `title`, `channel`, `id`, `upload_date` | export JSON, nom de fichier par défaut |
| `duration` | **calcul de la progression** (SF-04), garde `--max-duration` |
| `is_live`, `live_status` | refus des flux en direct (SF-07) |
| `age_limit` | message d'erreur actionnable |
| `subtitles`, `automatic_captions` | SF-01 bis, alimente `--prefer-subs` |

> **Correction v1 :** la v1 prévoyait un appel `--list-subs` distinct de la récupération des métadonnées. `-J` couvre les deux en un seul aller-retour réseau et fournit `duration`, sans lequel aucune progression fiable n'est calculable.

**Sous-titres officiels.** Si `--prefer-subs` est actif et que des sous-titres existent dans la langue demandée, ils sont récupérés directement et Whisper n'est pas exécuté.

- Priorité : sous-titres manuels > sous-titres auto-générés.
- **Comportement par défaut : désactivé.** Les sous-titres auto-générés de YouTube sont dépourvus de ponctuation dans de nombreuses langues et de qualité inférieure à Whisper `small`. L'utilisateur doit les demander explicitement.
- L'endpoint de sous-titres de YouTube est fréquemment limité en débit ou refusé (HTTP 429/403) : en cas d'échec, repli silencieux sur la transcription Whisper (sauf en sous-commande `subs`, où l'échec est remonté).

> **Complété au Jalon 3.** La sonde est annulable : un jeton armé tue
> `yt-dlp`. Dans une console, `Ctrl-C` atteignait déjà le sidecar ; le bouton
> « Annuler » de la GUI, lui, ne dispose d'aucun signal, et une sonde figée sur
> un réseau muet l'aurait rendu inopérant.

### SF-02 — Pipeline d'extraction audio « zero-disk »

**Aucun fichier temporaire.** L'audio transite exclusivement par des pipes anonymes et de la mémoire.

**Commandes de référence (arguments, jamais une ligne de shell) :**

```
yt-dlp -q --no-warnings --no-playlist
       -f "bestaudio[ext=webm]/bestaudio/best"
       -o - -- "https://www.youtube.com/watch?v=<ID>"

ffmpeg -hide_banner -loglevel error -nostdin
       -i pipe:0 -vn -ar 16000 -ac 1 -c:a pcm_s16le -f s16le pipe:1
```

> **Correction v1 — `-x` supprimé.** La v1 spécifiait `yt-dlp -x -o -`. L'option `-x` (`--extract-audio`) déclenche le post-processeur `FFmpegExtractAudio`, qui requiert un fichier seekable : vers `stdout` le comportement est au mieux dégradé, au pire cassé. Elle est de surcroît **redondante**, puisque le `ffmpeg` en aval effectue déjà le décodage et le rééchantillonnage.

> **Correction v1 — sélecteur de format explicite.** `[ext=webm]` privilégie Opus en conteneur WebM, lisible séquentiellement. Un `.m4a` non fragmenté dont l'atome `moov` se situe en fin de fichier fait échouer `ffmpeg` sur un pipe non-seekable. Le repli `/bestaudio/best` garantit qu'aucune vidéo n'est rejetée pour absence de piste WebM.

**Câblage en Rust :**

```rust
let mut dl = Command::new(ytdlp_path)
    .args(["-q", "--no-warnings", "--no-playlist",
           "-f", "bestaudio[ext=webm]/bestaudio/best", "-o", "-", "--"])
    .arg(&canonical_url)          // URL reconstruite, jamais l'entrée brute
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())       // ⚠ doit être drainé — voir ci-dessous
    .spawn()?;

let dl_out = dl.stdout.take().expect("stdout piped");

let mut ff = Command::new(ffmpeg_path)
    .args(["-hide_banner", "-loglevel", "error", "-nostdin",
           "-i", "pipe:0", "-vn", "-ar", "16000", "-ac", "1",
           "-c:a", "pcm_s16le", "-f", "s16le", "pipe:1"])
    .stdin(Stdio::from(dl_out))   // câblage direct, sans shell
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
```

**⚠ Règle impérative — drainage de `stderr`.** Un `stderr` configuré en `Stdio::piped()` mais jamais lu remplit le tampon du pipe (typiquement 64 Kio) et **bloque définitivement le processus enfant**. Le symptôme est pernicieux : tout fonctionne sur les vidéos courtes et se fige sur les longues. Chaque `stderr` doit être :

- soit consommé par un thread dédié (recommandé : les messages sont précieux pour la taxonomie d'erreurs SF-07 — les 4 Kio les plus récents sont conservés dans un tampon circulaire),
- soit explicitement mis à `Stdio::null()`.

La même règle s'applique à la lecture de `stdout` : elle doit se faire dans un thread distinct de l'attente de terminaison des processus.

**Conversion.** Les octets `s16le` sont lus par blocs depuis `stdout` et convertis à la volée en `f32` normalisés (`sample as f32 / 32768.0`), dans un `Vec<f32>` pré-alloué à partir de `duration × 16000`. Un octet impair résiduel en fin de flux est reporté sur le bloc suivant.

**Terminaison.** À la fin du flux, les deux processus sont attendus (`wait()`) et leurs codes de sortie vérifiés. En cas d'interruption ([SF-07](#sf-07--taxonomie-derreurs-et-codes-de-sortie)), les deux enfants sont tués explicitement — un `yt-dlp` orphelin continuerait à télécharger.

### SF-03 — Gestion et cycle de vie des modèles Whisper

**Modèles pris en charge.** Format GGML pour whisper.cpp, variantes quantifiées privilégiées :

| Alias | Fichier | Taille ≈ | Usage |
|---|---|---|---|
| `tiny` | `ggml-tiny.bin` | 75 Mo | tests, CI |
| `base` | `ggml-base.bin` | 142 Mo | défaut CPU |
| `small` | `ggml-small-q5_1.bin` | 190 Mo | bon compromis |
| `medium` | `ggml-medium-q5_0.bin` | 540 Mo | |
| `large-v3` | `ggml-large-v3-q5_0.bin` | 1,1 Go | qualité maximale, traduction |
| `turbo` | `ggml-large-v3-turbo-q5_0.bin` | 570 Mo | **défaut GPU** |

**Sélection automatique (`--model auto`, défaut).** Le modèle est choisi d'après les backends détectés : `turbo` si un backend GPU est disponible, `base` sinon. L'utilisateur garde évidemment la main.

**Téléchargement à la demande.** Source : dépôt HuggingFace **`ggerganov/whisper.cpp`**.

> **Corrigé au Jalon 1.** La v2.0 indiquait `ggml-org/whisper.cpp`. Vérification
> faite, cette adresse renvoie **HTTP 401** en accès anonyme, tandis que
> `ggerganov/whisper.cpp` sert les modèles sans authentification. L'erreur aurait
> bloqué la tâche 2.3 dès sa première exécution.

- **Référencement par révision épinglée**, jamais par `main` : une URL de branche n'est pas reproductible et invaliderait les empreintes.
- Vérification d'intégrité **SHA-256** obligatoire contre une table embarquée dans le binaire. Un fichier dont l'empreinte diffère est supprimé et l'opération échoue (code 30).
- Téléchargement vers un fichier temporaire `.part` dans le répertoire cible, puis renommage atomique — un `Ctrl-C` pendant le téléchargement ne laisse jamais un modèle tronqué qui serait ensuite considéré comme valide.
- Timeouts explicites : 30 s à la connexion, 60 s d'inactivité. Reprise sur `Range` si le serveur la supporte.
- Barre de progression sur `stderr`.

**Modèle VAD.** Le modèle Silero utilisé par [SF-04](#sf-04--moteur-de-transcription-locale) (`ggml-silero-v5.1.2.bin`, ≈ 2 Mo) suit le même cycle de vie et est téléchargé à la première utilisation.

**Répertoire de cache.** Résolu via la crate `directories` :

| Plateforme | Chemin |
|---|---|
| Linux | `~/.cache/scripta/models/` |
| macOS | `~/Library/Caches/scripta/models/` |
| Windows | `%LOCALAPPDATA%\scripta\models\` |

Surchargeable par `SCRIPTA_MODELS_DIR`. Géré par `scripta models {list,pull,rm,path,verify}` ; le modèle VAD s'y désigne par `silero`, mais ne s'accepte pas en `--model`, puisqu'il ne transcrit pas.

### SF-04 — Moteur de transcription locale

**Configuration d'inférence.**

- Langue source : détection automatique par défaut, ou code ISO 639-1 forcé (`fr`, `en`, …).
- `--translate` : traduction vers l'anglais.
  **⚠ Incompatibilité : `--translate` est refusé avec `--model turbo`.** `large-v3-turbo` a été entraîné pour la transcription seule ; sa sortie en mode traduction est inexploitable. La CLI rejette la combinaison avec un message explicite suggérant `large-v3`.
- `--threads` : défaut = nombre de cœurs **physiques** (`num_cpus::get_physical()`), en laissant au moins deux threads logiques libres. Sans effet notable lorsqu'un backend GPU est actif.
  > **Écart du J2 résorbé au J3.** Le J2 avait retenu tous les cœurs logiques, faute de mesure comparative. La mesure est venue de la GUI : sur un i7-13700H (14 cœurs, 20 threads logiques), une application voisine occupant un seul cœur suffit à faire tomber 20 threads à **0,2 ×** le temps réel, contre 12 × avec 16 threads. ggml synchronise ses threads par attente active, et un thread privé de processeur arrête tous les autres à chaque barrière. Sans charge voisine, sur 213 s d'audio, modèle `base` :
  >
  > | Threads | 6 | 8 | 10 | 12 | 14 | 16 | 18 | 19 |
  > |---|---|---|---|---|---|---|---|---|
  > | × temps réel | 17,6 | 17,7 | 17,7 | 17,6 | **18,1** | 17,0 | 13,6 | 10,3 |
  >
  > Le débit plafonne dès 6 threads et culmine aux cœurs physiques. Les mesures du [§5.2](#52-performance) antérieures au J3 ont été faites avec 20 threads.
- `--word-timestamps` : horodatage au mot (`token_timestamps`), nécessaire au JSON enrichi.

**VAD (détection d'activité vocale) — activé par défaut.**

> **Ajout v2.** Absent de la v1. C'est le levier le plus rentable sur la qualité perçue : Whisper hallucine en boucle sur les silences prolongés, les génériques musicaux et les bruits de fond — typiquement en répétant une phrase d'abonnement ou de remerciement. Le VAD Silero de whisper.cpp élimine l'essentiel de ces artefacts et réduit au passage le temps d'inférence sur les contenus peu denses.

Paramètres complémentaires exposés : `no_speech_thold`, `entropy_thold` (`--no-speech-thold`, `--entropy-thold`), désactivable par `--no-vad`. `--vad` le réactive — le dernier des deux l'emporte —, et `--vad-model <CHEMIN>` désigne un modèle hors cache.

> **Précisé au Jalon 2 — VAD orchestré par Scripta.** La v2.1 supposait que
> whisper.cpp appliquerait le VAD lui-même (`enable_vad`). Trois constats
> l'écartent :
>
> 1. **La voie est inopérante via whisper-rs.** whisper.cpp 1.8.3 n'applique
>    le VAD que dans `whisper_full`, sur l'état par défaut du contexte ;
>    `WhisperState::full` de whisper-rs 0.16 appelle `whisper_full_with_state`,
>    qui ignore `params.vad`. Les paramètres sont acceptés, sans effet.
> 2. **Elle fausserait les mots.** Même par `whisper_full`, seules les bornes
>    des segments sont replacées sur la chronologie d'origine ; celles des
>    tokens restent dans la chronologie compactée, et chaque silence retiré
>    décale d'autant tous les mots qui le suivent.
> 3. **Elle recopierait l'audio** au moment du pic mémoire (ADR-003).
>
> Scripta détecte donc la parole avec le module Silero de whisper.cpp
> (`WhisperVadContext`), compacte l'audio sur place en insérant 0,1 s de
> silence entre les plages — les réglages de whisper.cpp —, et replace segments
> **et** mots par une table de correspondance exacte. Le test
> `le_vad_conserve_la_chronologie_d_origine` échoue si l'on revient au VAD
> intégré : une parole placée à 5 s y ressort horodatée à 0 s.
>
> Sans parole détectée, la transcription est vide — ce n'est pas une erreur.
>
> La détection tourne sur **un seul thread**, quel que soit `--threads` :
> chaque fenêtre de 32 ms est un graphe ggml minuscule, que la synchronisation
> de plusieurs threads ralentit. Mesuré sur 10 min d'audio : 1,3 s avec un
> thread, 3,4 s avec 4, 52 s avec 20.
>
> **Sans contexte glissant (R11).** En mode VAD, aucune fenêtre n'est
> conditionnée sur le texte des précédentes (`n_max_text_ctx = 0`). Sur la
> vidéo de référence, ce contexte avait entretenu une boucle de répétition de
> 155 s. Sans lui, une boucle ne survit pas à sa fenêtre de 30 s ; la
> concordance avec les sous-titres passe de 73 à 75 %, et le débit de 6,9 à
> 8,0 × temps réel. Exception : avec `--initial-prompt`, le contexte est
> conservé. whisper.cpp fait passer l'invite par le même canal, et whisper-rs
> 0.16 n'expose pas `carry_initial_prompt`, qui permettrait de garder l'invite
> seule.

**`--initial-prompt`.**

> **Ajout v2.** Levier de qualité majeur et quasi gratuit : fournir un contexte (noms propres, jargon, acronymes du domaine) améliore sensiblement la transcription des termes rares. À exposer en CLI comme en GUI.

> **Limite constatée au Jalon 2.** whisper.cpp place l'invite en tête du
> contexte glissant, qu'il reconstruit après chaque fenêtre en ne gardant que
> les 223 derniers tokens : l'invite en sort au bout de quelques fenêtres, et
> n'influence donc que les premières minutes. `carry_initial_prompt` la
> maintiendrait sur toute la durée, mais whisper-rs 0.16 ne l'expose pas — à
> reprendre avec une version qui le fera.

**Progression et restitution.**

| Mécanisme | Usage |
|---|---|
| `progress_callback` | pourcentage global → barre de progression CLI / GUI |
| `new_segment_callback` | émission des segments au fil de l'eau → affichage progressif GUI |
| `abort_callback` | **annulation en cours d'inférence** |

> **Ajout v2 — annulation.** La v1 ne couvrait que `SIGINT` sur les sous-processus, ce qui ne résout rien pendant un `whisper_full` déjà lancé depuis plusieurs minutes. whisper.cpp expose un `abort_callback` interrogé entre les fenêtres de traitement. **Vérifier son exposition par la version de `whisper-rs` retenue — c'est un critère de sélection de la dépendance**, au même titre que le chargement dynamique des backends ([ADR-001](#adr-001--stratégie-daccélération-matérielle)).

La progression combine la durée connue (`duration`, issue de SF-01) et l'horodatage du dernier segment émis. Une vitesse relative au temps réel (× temps réel) est affichée.

### SF-05 — Formats d'exportation

| Format | Contenu |
|---|---|
| `txt` | Transcription continue, nettoyée, sans horodatage. |
| `srt` / `vtt` | Segments horodatés respectant les contraintes de lisibilité. |
| `json` | Document structuré complet. |

**Contraintes de lisibilité des sous-titres** (paramétrables) :

- `--max-line-width` (défaut **42** caractères) — segmentation sur les frontières de mots.
- `--max-line-count` (défaut **2** lignes par cue).
- Durée d'affichage : minimum 1,0 s, maximum 7,0 s ; les segments trop longs sont scindés sur les horodatages de mots lorsqu'ils sont disponibles.
- Échappement conforme : entités XML pour WebVTT, numérotation séquentielle et virgule décimale pour SRT (`00:00:12,500`), point pour VTT (`00:00:12.500`).

**Schéma JSON :**

```json
{
  "schema_version": 1,
  "source": {
    "url": "https://www.youtube.com/watch?v=...",
    "video_id": "...", "title": "...", "channel": "...",
    "duration_s": 1834.0, "upload_date": "2025-11-04"
  },
  "transcription": {
    "engine": "whisper.cpp", "model": "large-v3-turbo-q5_0",
    "backend": "vulkan", "language": "fr", "language_probability": 0.98,
    "translated": false, "vad": true,
    "duration_ms": 214300, "speed_realtime": 8.6
  },
  "segments": [
    {
      "id": 0, "start": 12.50, "end": 15.00,
      "text": "Bonjour à tous et bienvenue dans ce nouvel épisode.",
      "no_speech_prob": 0.01, "avg_logprob": -0.23,
      "words": [ { "word": "Bonjour", "start": 12.50, "end": 12.91, "probability": 0.99 } ]
    }
  ]
}
```

Le champ `words` n'est présent que si `--word-timestamps` est actif. `schema_version` permet l'évolution sans casser les consommateurs.

**Sortie standard.** Lorsque `--output` est omis, le résultat est écrit sur `stdout`. **Toute progression, tout log et toute barre d'avancement vont sur `stderr`** — `scripta <URL> -f json | jq` doit fonctionner sans `--quiet`.

### SF-06 — Maintenance du sidecar `yt-dlp`

YouTube modifie fréquemment ses mécanismes d'extraction : un `yt-dlp` embarqué est périmé quelques semaines après la publication d'une version de Scripta. La mise à jour n'est donc pas un confort mais une condition de fonctionnement.

- Commande CLI `scripta update-extractor`, bouton équivalent en GUI.
- **Conformément à [ADR-004](#adr-004--emplacement-des-sidecars-mis-à-jour), la mise à jour écrit exclusivement dans `<data_dir>/scripta/bin/` et ne touche jamais au bundle signé.** Le mécanisme interne `yt-dlp -U` n'est donc **pas** utilisé sur la copie embarquée ; la dernière version est téléchargée depuis les *releases* GitHub de yt-dlp, vérifiée, puis installée à l'emplacement inscriptible.
- Vérification d'intégrité contre le fichier `SHA2-256SUMS` publié avec chaque release.
- Sur macOS, le binaire téléchargé reçoit une signature ad-hoc (`codesign -s -`) et l'attribut de quarantaine est retiré, faute de quoi il ne s'exécutera pas sur Apple Silicon.
- Vérification de disponibilité au démarrage, au plus une fois par période de 24 h, sans blocage et sans télémétrie. Désactivable par `SCRIPTA_NO_UPDATE_CHECK=1`.

> **Mis en œuvre au Jalon 2.** La dernière version est lue dans la redirection
> de `github.com/yt-dlp/yt-dlp/releases/latest` : une requête `HEAD`, ni API ni
> quota, et aucune autre destination que `github.com`. La vérification tourne
> dans un thread détaché que la commande n'attend jamais. Une commande brève
> pouvant se terminer avant la réponse, l'avis trouvé est mémorisé
> (`<data_dir>/scripta/verification-yt-dlp`) et rappelé aux lancements suivants,
> jusqu'à ce que yt-dlp soit à jour.

### SF-07 — Taxonomie d'erreurs et codes de sortie

> **Ajout v2.** Entièrement absent de la v1. C'est pourtant le premier poste de contact avec la réalité d'exploitation : la majorité des échecs de ce type d'outil ne sont pas des bugs mais des conditions attendues de la plateforme, qui doivent produire un diagnostic actionnable plutôt qu'une trace de panique.

`core::error::ScriptaError` est un énuméré exhaustif. Chaque variante porte un message utilisateur explicite et un code de sortie stable, contractuel pour les scripts.

| Code | Variante | Message utilisateur (résumé) | Détection |
|---:|---|---|---|
| 0 | — | Succès | |
| 2 | `Usage` | Arguments invalides | clap |
| 10 | `InvalidUrl` | URL non reconnue ou domaine non supporté | validation locale |
| 11 | `Unavailable` | Vidéo privée, supprimée, géo-bloquée ou réservée aux membres | motif dans `stderr` de yt-dlp |
| 12 | `AuthRequired` | Connexion requise (vérification anti-robot ou limite d'âge) — voir `--cookies-from-browser` | idem |
| 13 | `LiveNotSupported` | Diffusion en direct non prise en charge | `is_live` de la sonde `-J` |
| 14 | `TooLong` | Durée supérieure à `--max-duration` | `duration` de la sonde |
| 20 | `ExtractionFailed` | Échec de l'extraction audio (yt-dlp/ffmpeg) | code de sortie ≠ 0 |
| 21 | `SidecarMissing` | Binaire `yt-dlp` ou `ffmpeg` introuvable — voir `scripta doctor` | résolution de chemin |
| 30 | `ModelUnavailable` | Modèle indisponible, téléchargement échoué ou empreinte invalide | SF-03 |
| 40 | `InferenceFailed` | Échec de l'inférence (mémoire insuffisante, backend défaillant) | whisper-rs |
| 50 | `OutputFailed` | Écriture du fichier de sortie impossible | E/S |
| 130 | `Interrupted` | Interrompu par l'utilisateur | `SIGINT` (convention 128 + 2) |

**Cas particuliers dignes d'attention :**

- **Code 12 — « Sign in to confirm you're not a bot ».** C'est aujourd'hui l'échec le plus fréquent en conditions réelles, en particulier depuis des adresses IP de centres de données. Le message doit orienter vers `--cookies-from-browser` ([SF-09](#sf-09--authentification-et-confidentialité)) plutôt que de laisser l'utilisateur face à une erreur opaque.
- **Code 13 — flux en direct.** Un live produit un flux de durée non bornée : sans ce garde-fou, le `Vec<f32>` croît jusqu'à épuisement de la mémoire. La détection se fait **avant** l'extraction, à partir de la sonde métadonnées.

**Diagnostic.** `scripta doctor` vérifie et affiche : sidecars résolus et leurs versions, backends ggml détectés, modèles en cache, chemins et droits d'écriture, connectivité vers HuggingFace et YouTube.

> **Mis en œuvre au Jalon 2.** Les droits d'écriture sont éprouvés sans rien
> créer — sur le plus proche ancêtre existant quand le répertoire n'existe pas
> encore. Les trois destinations de SF-09 sont interrogées en parallèle, cinq
> secondes au plus : hors ligne, le diagnostic dure deux secondes. Le backend
> affiché est celui de la compilation (ADR-001), avec la marche à suivre pour
> un GPU. `doctor` rend toujours 0 : c'est un rapport, dont la dernière ligne
> dénombre les problèmes.

### SF-08 — Cache de transcriptions

> **Ajout v2.** Retraiter une vidéo déjà transcrite est fréquent (changement de format d'export, réglage des sous-titres, erreur de manipulation) et coûte plusieurs minutes d'inférence pour rien.

- Clé : `sha256(video_id ‖ model_id ‖ lang ‖ translate ‖ vad ‖ word_timestamps ‖ initial_prompt ‖ no_speech_thold ‖ entropy_thold)`.

  > **Corrigé au Jalon 2.** La clé omettait `--initial-prompt` : relancer une
  > transcription avec un contexte resservait en silence celle obtenue sans.
  > Les champs facultatifs sont étiquetés, et omis quand ils sont absents :
  > l'empreinte des clés antérieures est inchangée, et le cache existant reste
  > valable. Seules les entrées marquées `vad` sont écartées, par une étiquette
  > dédiée : leur VAD était inopérant (Annexe D), ou gardait le contexte
  > glissant (SF-04).
- Contenu stocké : le JSON complet (SF-05), dont tous les autres formats se dérivent sans réinférence.
- Emplacement : `<cache_dir>/scripta/transcripts/`.
- Contournement par `--no-cache` ; administration par `scripta cache {list,clear,path}`.
- Éviction : LRU au-delà d'un plafond configurable (défaut 2 Go).

### SF-09 — Authentification et confidentialité

> **Ajout v2.** La v1 affirmait « aucune fuite de données » sans traiter le fait qu'une partie croissante des vidéos exige désormais une session authentifiée. Les deux exigences sont en tension et l'arbitrage doit être explicite.

- `--cookies-from-browser <navigateur>` transmet l'option homonyme à `yt-dlp`, qui lit les cookies du navigateur local.
- **Désactivé par défaut**, et jamais activé automatiquement.
- Les cookies ne transitent **que** vers `youtube.com`, uniquement dans le processus `yt-dlp`, ne sont jamais écrits sur disque par Scripta, jamais journalisés, jamais inclus dans un rapport d'erreur.
- La GUI affiche un avertissement explicite avant la première activation.
- **Invariant maintenu :** aucun segment transcrit, aucune URL, aucun identifiant utilisateur ne quitte la machine. Les seules destinations réseau autorisées sont `youtube.com` (via yt-dlp), `huggingface.co` (modèles) et `github.com` (mise à jour du sidecar).

---

## 4. Spécifications des interfaces

### 4.1 Interface en ligne de commande

> **Correction v1.** La v1 plaçait `--update-extractor` parmi les options d'une commande exigeant un `<URL>` positionnel obligatoire : la combinaison est impossible à exprimer en clap sans conflit. L'architecture passe en sous-commandes, avec `run` implicite pour préserver la concision d'usage.

```
USAGE:
    scripta [OPTIONS] <URL>          # équivaut à `scripta run`
    scripta <COMMANDE> [OPTIONS]

COMMANDES:
    run                  Transcrit une URL YouTube (commande par défaut)
    subs                 Récupère uniquement les sous-titres officiels
    models               Gère le cache de modèles (list | pull | rm | path | verify)
    cache                Gère le cache de transcriptions (list | clear | path)
    update-extractor     Met à jour le binaire yt-dlp
    doctor               Diagnostic système (backends, sidecars, modèles)
    help                 Affiche l'aide

OPTIONS DE `run` :
    -m, --model <NAME>          Modèle [défaut: auto]
                                [auto, tiny, base, small, medium, large-v3, turbo]
        --model-path <PATH>     Modèle hors catalogue, prioritaire sur --model
    -l, --lang <CODE>           Langue forcée (fr, en, es, …) [défaut: auto]
        --translate             Traduit vers l'anglais (incompatible avec --model turbo)
    -f, --format <FORMAT>       Format de sortie [défaut: txt] [txt, srt, vtt, json]
    -o, --output <PATH>         Fichier de sortie [défaut: stdout]
        --force                 Écrase le fichier de sortie s'il existe
    -t, --threads <NUM>         Threads CPU [défaut: cœurs physiques]
        --vad / --no-vad        Détection d'activité vocale [défaut: activée]
        --vad-model <PATH>      Modèle VAD hors cache
        --no-speech-thold <F>   Seuil d'absence de parole [défaut whisper.cpp: 0.6]
        --entropy-thold <F>     Seuil d'entropie [défaut whisper.cpp: 2.4]
        --initial-prompt <TXT>  Contexte (noms propres, jargon) pour guider le modèle
        --word-timestamps       Horodatage au mot (requis pour un JSON enrichi)
        --prefer-subs           Utilise les sous-titres officiels s'ils existent
        --max-line-width <N>    Largeur de ligne des sous-titres [défaut: 42]
        --max-line-count <N>    Lignes par cue [défaut: 2]
        --max-duration <MIN>    Refus au-delà de cette durée [défaut: 240]
        --cookies-from-browser <NAV>   Cookies pour les vidéos restreintes
        --no-cache              Ignore le cache de transcriptions
        --ytdlp-path <PATH>     Binaire yt-dlp explicite (ADR-004)
        --ffmpeg-path <PATH>    Binaire ffmpeg explicite
    -q, --quiet                 Supprime logs et progression
    -v, --verbose               Verbosité accrue (répétable)
    -h, --help                  Aide
    -V, --version               Version
```

> **Corrigé en v2.2 — `--backend` retiré.** Le backend est lié à la
> compilation ([ADR-001](#adr-001--stratégie-daccélération-matérielle) révisé) :
> une option d'exécution ne pourrait rien choisir.

**Contrats d'exécution :**

- `stdout` ne transporte **que** le résultat ; progression, logs et avertissements vont sur `stderr`.
- Les codes de sortie suivent la table [SF-07](#sf-07--taxonomie-derreurs-et-codes-de-sortie).
- `SIGINT` (et `Ctrl-Break` sous Windows) : les sous-processus sont tués, l'inférence en cours est interrompue via `abort_callback`, les fichiers partiels sont supprimés, sortie en 130 — **quelle que soit l'étape** : téléchargement d'un modèle (le `.part` est conservé pour la reprise), sonde, extraction, inférence. Un second `SIGINT` force une terminaison immédiate.
- La barre de progression est automatiquement désactivée si `stderr` n'est pas un terminal (CI, redirection). Les étapes restent annoncées, en lignes simples.
- Un fichier de sortie existant n'est jamais écrasé sans `--force`, et le refus tombe avant tout travail (§5.1).

### 4.2 Interface de bureau (Tauri v2)

```
┌────────────────────────────────────────────────────────────────────────┐
│  Scripta                                                 [-] [□] [×]   │
├────────────────────────────────────────────────────────────────────────┤
│  [ https://www.youtube.com/watch?v=dQw4w9WgXcQ               ] [Coller]│
│                                                                        │
│  Modèle : [ Turbo (recommandé)  ▼ ]   Langue : [ Détection auto  ▼ ]   │
│  Accélération : [● Vulkan (AMD Radeon RX 7800 XT) détectée        ▼ ]  │
│  ▸ Options avancées  (VAD · contexte · horodatage au mot · cookies)    │
│                                                                        │
│  [  Démarrer la transcription  ]                                       │
├────────────────────────────────────────────────────────────────────────┤
│  Téléchargement audio ✓   Inférence : [███████████░░░░░] 68% (8.6×)    │
│                                                        [  Annuler  ]   │
├────────────────────────────────────────────────────────────────────────┤
│  Sortie :                                                              │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │ [00:00:12.500 --> 00:00:15.000]                                  │  │
│  │ Bonjour à tous et bienvenue dans ce nouvel épisode...            │  │
│  │                                                                  │  │
│  │ [00:00:15.200 --> 00:00:18.400]                                  │  │
│  │ Aujourd'hui, nous allons étudier l'architecture système...       │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│  [Copier]                  Exporter : [ .TXT ] [ .SRT ] [ .VTT ] [JSON]│
└────────────────────────────────────────────────────────────────────────┘
```

**Exigences d'implémentation :**

- **Aucun travail bloquant sur le thread principal.** Extraction et inférence s'exécutent sur un thread dédié (`tauri::async_runtime::spawn_blocking`) ; la progression et les segments remontent par événements Tauri. Un `whisper_full` appelé directement dans une commande IPC figerait la fenêtre pendant plusieurs minutes.
- Le bouton **Annuler** arme le drapeau lu par l'`abort_callback` et tue les sidecars ; il doit rester réactif pendant l'inférence.
- Les sidecars sont déclarés en `externalBin` dans `tauri.conf.json`. **Rappel Tauri : les fichiers doivent porter le suffixe du triplet cible** (`yt-dlp-x86_64-pc-windows-msvc.exe`, `ffmpeg-aarch64-apple-darwin`, …), faute de quoi le bundle échoue silencieusement à embarquer le binaire. *(J4 : dans `tauri.bundle.conf.json`, que seul l'empaquetage passe à `cargo tauri build --config` ; `cargo tauri dev` et la CI n'ont ainsi pas à acquérir les sidecars. Noms préfixés, `scripta-yt-dlp-<triplet>` : un `.deb` installe les sidecars dans `/usr/bin`, où `/usr/bin/ffmpeg` appartient déjà au paquet de la distribution. Le cœur cherche la copie embarquée sous ce nom d'abord, puis sous le nom nu.)*
- L'accélération affichée est celle **de la compilation** (`Backend::compiled()`) : conformément à [ADR-001](#adr-001--stratégie-daccélération-matérielle) révisé, aucun artefact ne découvre un GPU à l'exécution. *(Corrigé en v2.2 : cette ligne annonçait encore l'énumération à l'exécution de la v2.0.)*
- Les segments s'affichent au fil de leur émission ; la zone de sortie suit automatiquement, sauf si l'utilisateur a fait défiler manuellement.

> **Mis en œuvre au Jalon 3.**
>
> - **Commandes :** `backend_info`, `list_models`, `download_model`,
>   `remove_model`, `probe_url`, `transcribe`, `cancel`, `export`,
>   `copy_text`, `update_extractor`. Toute commande qui touche au disque, au
>   réseau ou à un processus est asynchrone et travaille sur un thread
>   bloquant. Seule `cancel` est synchrone : elle arme un jeton, sans jamais
>   attendre un verrou tenu par la tâche. Le premier squelette tenait un
>   verrou pendant toute l'inférence, et `cancel` l'attendait : la fenêtre se
>   figeait jusqu'à la fin, et l'annulation n'annulait rien.
> - **Progression et segments** passent par un canal propre à chaque appel
>   (`tauri::ipc::Channel`) plutôt que par des événements globaux. Un thread
>   de relais les regroupe en lots, au plus dix par seconde, et ne transmet
>   qu'une progression en hausse : whisper.cpp la rappelle des milliers de
>   fois par pourcent.
> - **Une tâche longue à la fois** — transcription, téléchargement de modèle,
>   mise à jour de l'extracteur. La réservation est libérée à sa destruction,
>   erreur ou panique comprise.
> - **Export et presse-papiers** passent par des commandes, qui ouvrent la
>   boîte d'enregistrement native côté Rust. La fenêtre n'a que la
>   permission `core:default` : ni système de fichiers, ni presse-papiers, et
>   aucune commande n'écrit un contenu arbitraire à un chemin arbitraire.
> - **Erreurs :** chaque variante de `ScriptaError` a son message pour
>   l'interface, par une correspondance exhaustive ; les messages du cœur
>   citent des options de la ligne de commande. Le détail technique est
>   joint, replié.
> - **Sidecars :** résolus sans le `PATH` (§5.1). En build de débogage
>   seulement, le `PATH` sert de repli, tant que les sidecars ne sont pas
>   embarqués (J4).
> - Le bouton **Coller** de la maquette n'est pas repris : `Ctrl-V` suffit,
>   et lire le presse-papiers exigerait une permission de plus.

---

## 5. Exigences non fonctionnelles

### 5.1 Sécurité

- **Aucune concaténation shell.** Aucun `sh -c`, aucun `cmd.exe`. Tous les arguments sont passés sous forme de tableau, précédés du séparateur `--`.
- **URL canonique reconstruite** à partir de l'identifiant validé ([SF-01](#sf-01--validation-durl-et-sonde-de-métadonnées)) : l'entrée utilisateur brute n'atteint jamais un sidecar.
- **Chemins de sidecars résolus explicitement**, jamais recherchés via `PATH` dans le contexte GUI — cela éviterait un détournement par un binaire homonyme placé en amont du `PATH`. *(J3 : `sidecar::resolve_installed` ne connaît que la copie mise à jour et la copie embarquée ; la GUI ne se replie sur le `PATH` qu'en build de débogage.)*
- **Vérification d'intégrité SHA-256** sur tout artefact téléchargé (modèles, mises à jour de sidecars).
- **Timeouts réseau explicites** sur toutes les requêtes ; aucune redirection suivie vers un hôte hors liste blanche.
- **Aucune exécution de code téléchargé** autre que les sidecars vérifiés.
- Le chemin de sortie utilisateur est vérifié avant écriture (pas d'écrasement silencieux d'un fichier existant sans `--force`). *(J2 : vérification avant tout travail, puis ouverture exclusive — `create_new` — au moment d'écrire ; avec `--force`, écriture à côté puis renommage, pour ne jamais remplacer un fichier valide par un fichier tronqué.)*

### 5.2 Performance

> **Ajout v2.** La v1 comportait une section performance sans aucune valeur chiffrée, donc non testable. Les seuils ci-dessous sont des **minima d'acceptation** mesurés sur un échantillon de référence de 10 minutes d'audio parlé en français, à valider et ajuster au [Jalon 2](ROADMAP.md#jalon-2--robustesse-cli).

| Configuration | Modèle | Seuil |
|---|---|---|
| CPU x86_64, 12 cœurs, AVX2 | `base` | ≥ 3 × temps réel |
| Apple Silicon M1+, Metal | `turbo` | ≥ 8 × temps réel |
| GPU discret ≥ 6 Go VRAM, Vulkan | `turbo` | ≥ 8 × temps réel |

> **Mesuré au Jalon 2.** Sur 61 min de parole française, modèle `base`, build
> `--release` avec le contournement R9, 12 cœurs logiques, **machine au
> repos** : **4,8 × temps réel**.
>
> Le même code avait d'abord donné 2,1 × puis 2,7 ×. Les deux mesures étaient
> faussées par des compilations concurrentes sur la même machine. J'avais
> abaissé le seuil de 3 × à 2 × sur cette base : il est rétabli à 3 ×, la
> valeur d'origine, qui était correcte.
>
> **Leçon de méthode :** une mesure de débit n'a de sens que sur une machine
> au repos. Les trois mesures du même binaire s'échelonnent de 2,1 × à 4,8 ×
> selon la charge concurrente.
>
> Les seuils GPU restent **non mesurés** : aucune machine de test disponible.
>
> **Mesuré au Jalon 3.** Même vidéo de 61 min, modèle `base`, VAD actif,
> horodatage au mot, depuis l'application de bureau en build optimisée : **14,0 ×
> temps réel**, pic mémoire de 882 Mo. Le gain sur les 6,4 × du J2 tient au
> nombre de threads par défaut, ramené de 20 aux 14 cœurs physiques (SF-04),
> et ce alors même qu'une application voisine occupait un cœur : avec
> 20 threads, cette seule charge faisait tomber l'inférence à 0,2 ×. La suite
> d'acceptation de la CLI, rejouée dans les mêmes conditions, mesure
> **15,8 ×** — 266 s pour l'heure de vidéo, contre 598 s au J2 —, pic mémoire
> et concordance inchangés (863 Mo, 75 %).

**Autres seuils :**

| Métrique | Seuil |
|---|---|
| Empreinte RSS, 1 h d'audio, modèle `base` | < 1,1 Go (mesuré : 1 040 Mo sans VAD ; 863 Mo avec) |
| Empreinte totale au pic | ≈ 560 Mo / h + ~260 Mo fixes ([ADR-003](#adr-003--inférence-non-streamée)) |
| Démarrage CLI (`--version`, `--help`) | < 150 ms |
| Sonde de métadonnées (SF-01) | < 3 s en conditions nominales |
| Écritures disque hors sortie et caches | **0 octet** (invariant « zero-disk ») |

L'invariant « zero-disk » est vérifiable automatiquement : instrumenter le répertoire temporaire pendant un test d'intégration et vérifier qu'aucun fichier n'y apparaît.

### 5.3 Compatibilité

| Plateforme | Cible | Version minimale |
|---|---|---|
| Linux | `x86_64-unknown-linux-gnu` | glibc 2.31 (Ubuntu 20.04) |
| Windows | `x86_64-pc-windows-msvc` | Windows 10 1809 |
| macOS | `aarch64-apple-darwin`, `x86_64-apple-darwin` | macOS 11 |

> **Écart des v0.1.x.** La CLI est construite sur les runners standard :
> sous Linux, elle exige la glibc 2.34 — relevée sur le binaire construit sous
> Ubuntu 22.04 —, et non la 2.31 visée ; sous macOS, seul Apple Silicon est
> fourni, sans binaire universel ; sous Windows, l'exécutable est autonome,
> runtime C lié statiquement. Une build en conteneur et `lipo` les résorberont au
> [Jalon 4](ROADMAP.md#jalon-4--packaging-et-cicd).

### 5.4 Distribution et packaging

| Plateforme | CLI | GUI |
|---|---|---|
| Linux | tarball `.tar.gz` | AppImage + `.deb` |
| Windows | `.exe` autonome | installeur NSIS |
| macOS | binaire universel | `.dmg` non signé |

- Les sidecars `yt-dlp` et `ffmpeg` sont embarqués dans les bundles GUI. Pour la CLI, ils sont **recherchés sur le système puis téléchargés à la demande** dans `<data_dir>/scripta/bin/` — embarquer 70 Mo de FFmpeg dans un binaire CLI contredirait l'objectif de légèreté.
- **Build FFmpeg minimal** : seuls les décodeurs (`opus`, `vorbis`, `aac`, `mp3`), démultiplexeurs (`matroska`, `mov`, `mp3`) et le rééchantillonneur sont nécessaires. Une build ciblée descend autour de 10–15 Mo, contre 60–70 Mo pour une build complète. *(Mesuré au J4 : FFmpeg 9.0.2 statique, sous LGPL, sans bibliothèque externe — **3,2 Mo** sous Linux, **1,9 Mo** sous Windows et macOS. S'y ajoutent les démultiplexeurs `ogg`, `aac` et `wav`, par prudence. Sur sa propre plateforme, chaque binaire décode cinq échantillons — Opus, Vorbis, AAC, MP3, MP4 avec image — par la commande même du cœur. Construction : `scripts/sidecars/build-ffmpeg.sh` ; Windows se compile depuis Linux par mingw-w64.)*
- ~~Sur macOS, **tous** les binaires du bundle — application et sidecars — doivent être signés et notarisés ensemble, avec les droits d'exécution appropriés.~~ **Aucune signature** (décision du 2026-09-24) : ni Developer ID ni notarisation sous macOS, ni Authenticode sous Windows. Gatekeeper et SmartScreen avertissent au premier lancement ; le README donne la marche à suivre. Sur Apple Silicon, chaque binaire garde la signature *ad hoc* que lui appose l'éditeur de liens — sans elle, macOS refuserait de l'exécuter —, ce qui n'engage aucun compte ni certificat.
- Chaque release publie un fichier de sommes de contrôle et la liste des versions de sidecars embarquées. *(J4 : versions et empreintes dans `scripts/sidecars/versions.env`, qui refuse tout fichier ne portant pas la sienne ; fiche de construction de FFmpeg dans chaque installeur.)*
- *(v0.1.x.)* Les archives de la CLI n'embarquent aucun sidecar. Elles joignent `LICENSE`, `THIRD_PARTY_LICENSES.md` et l'inventaire des licences des bibliothèques compilées (`LICENCES-DEPENDANCES.md`, généré par `cargo about` ; `about.toml` fixe les licences acceptées). Un tag produit un **brouillon** de release, publié à la main après relecture.

### 5.5 Stratégie de test

> **Ajout v2.** Absente de la v1. La difficulté propre à ce projet est que le chemin nominal traverse le réseau, un service tiers instable et du calcul non déterministe : sans découplage explicite, aucun test n'est exécutable en CI.

| Niveau | Portée | Exécution en CI |
|---|---|---|
| **Unitaire** | Validation d'URL (corpus d'URL valides et malveillantes), conversion `s16le → f32`, découpage des cues, formatage des horodatages | ✅ |
| **Golden file** | Formateurs `txt`/`srt`/`vtt`/`json` à partir d'un jeu de segments figé | ✅ |
| **Sidecar simulé** | Faux `yt-dlp` et `ffmpeg` (scripts) émettant un PCM connu, ou un code d'erreur donné, ou **rien tout en écrivant massivement sur `stderr`** — c'est le test de non-régression du deadlock [SF-02](#sf-02--pipeline-dextraction-audio--zero-disk-) | ✅ |
| **Intégration locale** | WAV de référence (30 s) → modèle `tiny` → transcription ; assertion sur le WER et non sur une égalité de chaîne | ✅ (CPU) |
| **Taxonomie d'erreurs** | Chaque variante de `ScriptaError` atteignable via un sidecar simulé, vérification du code de sortie | ✅ |
| **Réseau** | Chemin réel sur une vidéo stable, marqué `#[ignore]` | ⚠️ tâche planifiée quotidienne, hors CI de PR |
| **Performance** | Seuils du [§5.2](#52-performance) sur un runner de référence | ⚠️ non bloquant, suivi de tendance |

**Principes :**

- **Aucun test de la CI de PR ne touche le réseau.** YouTube est instable et bloque les adresses de centres de données : une CI qui en dépend devient rouge sans lien avec le code.
- La vérification des extracteurs se fait par une **tâche planifiée quotidienne** qui transcrit une vidéo de référence ; son échec signale une rupture côté YouTube, pas une régression de Scripta.
- Les assertions d'inférence portent sur un taux d'erreur sur les mots (WER) sous un seuil, jamais sur une égalité exacte — la sortie varie entre backends et versions de ggml.

---

## Annexe A — Résumé des corrections apportées à la v1

| # | Objet | Nature |
|---|---|---|
| 1 | `yt-dlp -x -o -` | **Erreur** — `-x` invoque un post-processeur incompatible avec `stdout`, et redondant avec ffmpeg en aval |
| 2 | Sélecteur de format audio | **Robustesse** — `[ext=webm]` évite l'échec sur MP4 non seekable |
| 3 | Pipeline shell vs interdiction du shell | **Contradiction interne** — résolue par [ADR-002](#adr-002--pipeline-audio-sans-shell) |
| 4 | Drainage de `stderr` | **Deadlock** — non mentionné en v1, se manifeste uniquement sur les vidéos longues |
| 5 | Backend compilé vs détecté | **Contradiction interne** (§5.2 vs maquette GUI) — résolue par [ADR-001](#adr-001--stratégie-daccélération-matérielle) |
| 6 | `yt-dlp -U` in-place | **Erreur** — invalide la signature macOS, échoue dans `Program Files` — résolue par [ADR-004](#adr-004--emplacement-des-sidecars-mis-à-jour) |
| 7 | « Streaming » vers Whisper | **Imprécision** — `whisper_full` n'est pas incrémental — clarifiée par [ADR-003](#adr-003--inférence-non-streamée) |
| 8 | `turbo` + `--translate` | **Erreur fonctionnelle** — turbo ne traduit pas |
| 9 | `--update-extractor` + `<URL>` requis | **Erreur** — conflit clap — passage en sous-commandes |
| 10 | Deux sondes réseau | **Optimisation** — `-J` remplace `--list-subs` et fournit `duration` |
| 11 | `src-tauri/` à la racine | **Cohérence** — devient `crates/desktop/` |
| 12 | `yt-transcribe` vs `Scripta` | **Cohérence** — nom unifié |

## Annexe B — Ajouts v2

| # | Objet | Section |
|---|---|---|
| 1 | Taxonomie d'erreurs et codes de sortie | [SF-07](#sf-07--taxonomie-derreurs-et-codes-de-sortie) |
| 2 | Refus des diffusions en direct | [SF-07](#sf-07--taxonomie-derreurs-et-codes-de-sortie) |
| 3 | Vérification anti-robot / cookies | [SF-09](#sf-09--authentification-et-confidentialité) |
| 4 | VAD Silero activé par défaut | [SF-04](#sf-04--moteur-de-transcription-locale) |
| 5 | `--initial-prompt` | [SF-04](#sf-04--moteur-de-transcription-locale) |
| 6 | Annulation en cours d'inférence (`abort_callback`) | [SF-04](#sf-04--moteur-de-transcription-locale) |
| 7 | Cache de transcriptions | [SF-08](#sf-08--cache-de-transcriptions) |
| 8 | Stratégie de test | [§5.5](#55-stratégie-de-test) |
| 9 | Seuils de performance chiffrés | [§5.2](#52-performance) |
| 10 | Périmètre et non-objectifs explicites | [§1.2](#12-périmètre-et-non-objectifs) |
| 11 | Commande `doctor` | [SF-07](#sf-07--taxonomie-derreurs-et-codes-de-sortie) |
| 12 | Gestion des playlists | [SF-01](#sf-01--validation-durl-et-sonde-de-métadonnées) |
| 13 | Contraintes de lisibilité des sous-titres paramétrables | [SF-05](#sf-05--formats-dexportation) |
| 14 | Build FFmpeg minimal | [§5.4](#54-distribution-et-packaging) |
| 15 | Incompatibilité GPLv3 / App Store, avertissement CGU | [§1.3](#13-licence-et-conformité) |

## Annexe C — Journal des corrections

**v2.4** — Jalon 4, premier lot : sidecars embarqués dans l'application de bureau, build FFmpeg minimale mesurée (§5.4, Annexe D), déclaration `externalBin` propre au bundle et noms préfixés (§4.2), obligations de licence mises en œuvre (§1.3) ; hypothèse sur les rappels de whisper-rs précisée (Annexe D) ; signature abandonnée (§1.3, §5.4, ADR-004 — décision du 2026-09-24).

**v2.3** — Retours du Jalon 3 : orchestration déplacée dans `core::pipeline`, commune aux deux interfaces (§2.1) ; sonde annulable (SF-01) ; mise en œuvre de la GUI décrite (§4.2) : canal par appel et relais regroupant les messages, tâche unique, export et presse-papiers côté Rust, messages d'erreur propres à l'interface ; résolution des sidecars sans `PATH` mise en œuvre (§5.1).

**v2.2** — Retours du Jalon 2 : VAD orchestré par Scripta et motifs de l'écart, détection sur un thread, contexte glissant coupé en mode VAD, limite de `--initial-prompt` (SF-04, ADR-003, Annexe D) ; seuils `no_speech_thold` / `entropy_thold` exposés ; clé de cache complétée (SF-08) ; `--backend` retiré et §4.2 aligné sur ADR-001 révisé ; `--force`, `--model-path`, `--vad-model`, chemins de sidecars ajoutés au §4.1 ; mises en œuvre de SF-06 et du diagnostic SF-07 décrites ; écart assumé sur `--threads` ; mesure mémoire de référence corrigée (1 040 Mo).

**v2.1** — Retours du Jalon 0 : [ADR-001](#adr-001--stratégie-daccélération-matérielle) révisé (le chargement dynamique des backends ggml n'est pas exposé par `whisper-rs-sys` 0.15 ; passage à un artefact par backend), [Annexe D](#annexe-d--points-à-valider-en-implémentation) mise à jour avec l'état réel de chaque hypothèse, [Annexe E](#annexe-e--prérequis-de-compilation) ajoutée (prérequis de compilation), MSRV portée à 1.88.

**v2.0** — Révision complète. 12 corrections (Annexe A) et 15 ajouts (Annexe B). Introduction de quatre ADR pour figer les décisions structurantes. Nom du binaire unifié en `scripta`.

**v1.0** — Cahier des charges initial.

## Annexe D — Points à valider en implémentation

État au terme du [Jalon 0](ROADMAP.md#jalon-0--dérisquage), sur `whisper-rs` 0.16 / `whisper-rs-sys` 0.15, complété au Jalon 2.

| Hypothèse | État | Constat |
|---|---|---|
| `whisper-rs` expose `abort_callback` | ✅ **Confirmée — par l'API brute** (J2, J3) | Les trois rappels existent, mais leurs versions « sûres » ne le sont pas en 0.16.0 : `set_abort_callback_safe` confond les types, `set_progress_callback_safe` et `set_segment_callback_safe` fuient leur fermeture à chaque inférence. Scripta passe par des trampolines prêtés le temps de l'appel (`crates/core/src/transcribe.rs`) |
| Chargement dynamique des backends ggml | ❌ **Invalidée** | Non exposé : sélection par feature Cargo, liaison statique. Repli appliqué — voir [ADR-001](#adr-001--stratégie-daccélération-matérielle) |
| VAD Silero accessible depuis `whisper-rs` | ✅ **Confirmée — mais pas par la voie prévue** (J2) | La détection (`WhisperVadContext`) fonctionne. En revanche `enable_vad` est sans effet via whisper-rs : `WhisperState::full` appelle `whisper_full_with_state`, qui ignore `params.vad`. Et la voie `whisper_full` laisse les tokens dans la chronologie compactée. VAD orchestré par Scripta — voir [SF-04](#sf-04--moteur-de-transcription-locale) |
| Vulkan atteint les seuils du [§5.2](#52-performance) | ⏳ **Reportée au J4** | Aucune machine GPU disponible. À mesurer quand la variante Vulkan sera construite ([Jalon 4](ROADMAP.md#jalon-4--packaging-et-cicd)). Repli : les seuils GPU restent indicatifs, le CPU tient les siens |
| Build FFmpeg minimale ≤ 15 Mo | ✅ **Confirmée** (J4) | 3,2 Mo sous Linux, 1,9 Mo sous Windows et macOS, contre 60 à 145 Mo pour une build complète. Voir [§5.4](#54-distribution-et-packaging) |

## Annexe E — Prérequis de compilation

> **Ajout v2.1.** Découverts au Jalon 0. `whisper-rs-sys` compile whisper.cpp
> depuis les sources et génère ses liaisons FFI à la construction : ces outils
> sont requis pour **bâtir** Scripta, jamais pour l'exécuter.

| Outil | Rôle | Obtention |
|---|---|---|
| **CMake** ≥ 3.20 | Build natif de whisper.cpp | `winget install Kitware.CMake` · `apt install cmake` · `brew install cmake` |
| **libclang** (LLVM) | `bindgen`, génération des liaisons FFI | `winget install LLVM.LLVM` · `apt install libclang-dev` · fourni par Xcode |
| **Rust** ≥ 1.88 | MSRV imposée par `whisper-rs-sys` 0.15 | `rustup` |
| Toolchain C++ | MSVC 2022 · GCC/Clang · Xcode CLT | — |

**`WHISPER_DONT_GENERATE_BINDINGS`** court-circuite `bindgen` au profit des liaisons pré-générées du crate, ce qui lève la dépendance à libclang. **Ce repli ne fonctionne que sous Linux** : les liaisons embarquées décrivent des types glibc (`_IO_FILE`, `_G_fpos_t`) dont les assertions de taille échouent sous MSVC. Il n'est donc pas utilisable comme solution multiplateforme.
