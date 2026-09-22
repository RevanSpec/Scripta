# Scripta

Transcription de vidéos YouTube, **entièrement locale**. L'audio est extrait
sans télécharger la vidéo, l'inférence tourne sur votre machine via
[whisper.cpp](https://github.com/ggml-org/whisper.cpp), et rien de ce qui est
transcrit ne quitte votre poste.

```console
$ scripta "https://www.youtube.com/watch?v=jNQXAC9IVRw"
Alright so here we are one of the elephant's cool thing for these guys
it's that they have really really long and that's cool and that's pretty
much all it is to say
```

---

## État du projet

**En cours de développement.** Le cœur fonctionne de bout en bout ; l'outil
n'est pas encore empaqueté ni distribué.

| | État |
|---|---|
| Extraction audio sans fichier temporaire | ✅ |
| Transcription locale (Whisper) | ✅ |
| Formats `txt`, `srt`, `vtt`, `json` | ✅ |
| Horodatage au mot | ✅ |
| Interruption propre (`Ctrl-C`) | ✅ |
| Téléchargement des modèles, vérifié SHA-256 | ✅ |
| Accélération GPU | ⏳ compilable, non distribuée |
| Sous-titres YouTube officiels | ⏳ |
| Application de bureau | ⏳ |
| Binaires précompilés | ⏳ |

Suivi détaillé dans [`docs/ROADMAP.md`](docs/ROADMAP.md).

---

## Périmètre

**Scripta fait :** vidéos YouTube publiques et non listées, transcription
locale, traduction vers l'anglais, export `txt` / `srt` / `vtt` / `json`.

**Scripta ne fait pas**, et ce sont des décisions explicites
([SPEC §1.2](docs/SPEC.md#12-périmètre-et-non-objectifs)) :

- la **diarisation** — identifier qui parle demande un second modèle ;
- les **diffusions en direct** — un flux de durée non bornée est incompatible
  avec le modèle d'inférence retenu ; il est détecté et refusé proprement ;
- les **autres plateformes** — `yt-dlp` en gère des milliers, mais la validation
  d'URL est volontairement restrictive ;
- la **traduction vers une langue autre que l'anglais** — limite de Whisper.

---

## Installation

Aucun binaire précompilé pour l'instant : il faut compiler depuis les sources.

### Prérequis

`whisper.cpp` est compilé depuis ses sources et ses liaisons FFI sont générées à
la construction. Ces outils servent donc à **bâtir** Scripta, jamais à
l'exécuter ([SPEC Annexe E](docs/SPEC.md#annexe-e--prérequis-de-compilation)).

| Outil | Rôle |
|---|---|
| **CMake** ≥ 3.20 | build natif de whisper.cpp |
| **libclang** (LLVM) | génération des liaisons FFI |
| Toolchain C++ | MSVC 2022 · GCC/Clang · Xcode CLT |
| **yt-dlp**, **ffmpeg** | extraction audio, à l'exécution cette fois |

```powershell
# Windows
winget install Kitware.CMake LLVM.LLVM yt-dlp.yt-dlp Gyan.FFmpeg
```

```bash
# Debian / Ubuntu
sudo apt install cmake libclang-dev ffmpeg
pipx install yt-dlp
```

```bash
# macOS
brew install cmake yt-dlp ffmpeg
```

La version de Rust est imposée par [`rust-toolchain.toml`](rust-toolchain.toml)
— `cargo` l'installe seul.

### Compilation

```bash
cargo build --release
```

> **Windows/MSVC — deux variables à poser d'abord.** Sans elles, `--release`
> embarque un whisper.cpp non optimisé, quatre à six fois plus lent. La crate
> `cmake` écrase `CMAKE_CXX_FLAGS_<BUILD_TYPE>` alors que le générateur Visual
> Studio compile en `--config Release` (risque R9 de la
> [roadmap](docs/ROADMAP.md)).
>
> ```powershell
> $env:CMAKE_C_FLAGS_RELEASE   = "/MD /O2 /Ob2 /DNDEBUG"
> $env:CMAKE_CXX_FLAGS_RELEASE = "/MD /O2 /Ob2 /DNDEBUG"
> cargo build --release
> ```

Le binaire est dans `target/release/scripta`. Vérifiez l'installation :

```bash
scripta doctor
```

### Accélération matérielle

Les backends sont liés **à la compilation** : un artefact ne peut pas découvrir
un GPU à l'exécution
([ADR-001](docs/SPEC.md#adr-001--stratégie-daccélération-matérielle)).

```bash
cargo build --release --features vulkan   # NVIDIA, AMD, Intel
cargo build --release --features metal    # Apple Silicon
cargo build --release --features cuda     # NVIDIA uniquement
```

Sans feature, la build est CPU et démarre partout. `scripta doctor` indique le
backend compilé.

---

## Démarrage rapide

Le modèle est téléchargé au premier usage et vérifié par empreinte SHA-256.
Il n'y a rien à préparer.

```bash
scripta "https://www.youtube.com/watch?v=<ID>"
```

`--model auto`, le défaut, choisit `turbo` si un backend GPU est compilé et
`base` sinon. La gestion du cache passe par `scripta models` :

```bash
scripta models list          # catalogue et état local
scripta models pull small    # pré-télécharge
scripta models verify small  # recalcule l'empreinte
scripta models rm small
scripta models path
```

| Modèle | Taille | Remarque |
|---|---|---|
| `tiny` | 75 Mo | très rapide, qualité limitée |
| `base` | 142 Mo | bon point de départ sur CPU |
| `small` | 190 Mo | compromis recommandé |
| `large-v3` | 1,1 Go | qualité maximale ; seul modèle à savoir traduire |
| `large-v3-turbo` | 570 Mo | rapide et précis, **ne sait pas traduire** |

---

## Utilisation

```
scripta [OPTIONS] <URL>          # équivaut à `scripta run`
scripta <COMMANDE> [OPTIONS]
```

### Exemples

```bash
# Sous-titres, lignes de 32 caractères maximum
scripta -m small -f srt --max-line-width 32 -o cours.srt "<URL>"

# JSON enrichi, canalisé vers jq
scripta -m small -f json "<URL>" | jq '.segments[] | .text'

# Langue imposée et contexte pour les termes rares
scripta -m small -l fr --initial-prompt "Kubernetes, Prometheus, Grafana" "<URL>"

# Traduction vers l'anglais (pas avec turbo)
scripta -m large-v3 --translate "<URL>"
```

### Options principales

| Option | Effet |
|---|---|
| `-m, --model <NOM>` | `auto` (défaut), `tiny`, `base`, `small`, `medium`, `large-v3`, `turbo` |
| `--model-path <CHEMIN>` | chemin explicite, prioritaire |
| `-f, --format <FORMAT>` | `txt` (défaut), `srt`, `vtt`, `json` |
| `-o, --output <CHEMIN>` | fichier de sortie ; `stdout` par défaut |
| `-l, --lang <CODE>` | langue imposée ; détection automatique sinon |
| `--translate` | traduction vers l'anglais |
| `--initial-prompt <TEXTE>` | contexte pour les noms propres et le jargon |
| `--word-timestamps` | horodatage au mot (d'office avec `-f json`) |
| `--vad-model <CHEMIN>` | modèle VAD Silero ; réduit fortement les hallucinations sur les silences |
| `--max-line-width <N>` | largeur des lignes de sous-titres (défaut 42) |
| `--max-line-count <N>` | lignes par sous-titre (défaut 2) |
| `--max-duration <MIN>` | refus au-delà (défaut 240) |
| `-t, --threads <N>` | threads d'inférence |
| `-q, --quiet` | supprime la progression |

`scripta --help` donne la liste complète.

### Sortie et scripts

`stdout` ne porte **que** le résultat ; progression, avertissements et erreurs
vont sur `stderr`. `scripta <URL> -f json | jq` fonctionne donc sans `--quiet`.

Les codes de sortie sont contractuels
([SF-07](docs/SPEC.md#sf-07--taxonomie-derreurs-et-codes-de-sortie)) :

| Code | Signification |
|---:|---|
| `0` | succès |
| `2` | arguments invalides |
| `10` | URL non reconnue |
| `11` | vidéo privée, supprimée, géo-bloquée ou réservée aux membres |
| `12` | connexion requise (vérification anti-robot, limite d'âge) |
| `13` | diffusion en direct, non prise en charge |
| `14` | durée supérieure à `--max-duration` |
| `20` | échec de l'extraction audio |
| `21` | `yt-dlp` ou `ffmpeg` introuvable |
| `30` | modèle indisponible |
| `40` | échec de l'inférence |
| `50` | écriture impossible |
| `130` | interrompu par l'utilisateur |

### Interruption

Le premier `Ctrl-C` demande l'arrêt : whisper.cpp rend la main entre deux
fenêtres de traitement, les sous-processus sont tués, la sortie se fait en
`130`. Un second `Ctrl-C` force la terminaison immédiate.

---

## Confidentialité

L'inférence est **locale** : aucun segment transcrit, aucune URL, aucun
identifiant ne quitte la machine. Les seules destinations réseau sont
`youtube.com` (via `yt-dlp`) et `huggingface.co` (modèles).

Certaines vidéos exigent une session authentifiée. `--cookies-from-browser` le
permettra, **désactivé par défaut** et jamais activé automatiquement
([SF-09](docs/SPEC.md#sf-09--authentification-et-confidentialité)).

---

## Avertissement

Le téléchargement de contenu contrevient aux conditions d'utilisation de
YouTube. **Scripta est fourni pour un usage personnel et licite** — transcrire
vos propres contenus, exploiter des vidéos dont la licence l'autorise, produire
des sous-titres d'accessibilité. Il vous appartient de vérifier que votre usage
respecte les conditions de la plateforme et le droit applicable. Les auteurs
déclinent toute responsabilité à cet égard.

---

## Architecture

```
scripta (CLI)  ─┐
                ├─► crates/core ─┬─► yt-dlp → ffmpeg   (pipes, aucun fichier temporaire)
Scripta Desktop ┘                └─► whisper-rs        (inférence locale)
```

- [`crates/core`](crates/core) — validation d'URL, sonde de métadonnées,
  pipeline audio, inférence, formateurs, taxonomie d'erreurs. Aucune dépendance
  à une couche de présentation.
- [`crates/cli`](crates/cli) — le binaire `scripta`.

L'audio transite exclusivement par des **pipes anonymes** : rien n'est écrit sur
le disque, ce qui évite l'usure et l'attente d'E/S. Les deux sous-processus sont
câblés sans interpréteur de commandes, et l'URL est reconstruite à partir d'un
identifiant validé, jamais recopiée telle quelle
([SPEC §5.1](docs/SPEC.md#51-sécurité)).

---

## Documentation

| Document | Contenu |
|---|---|
| [`docs/SPEC.md`](docs/SPEC.md) | cahier des charges, décisions d'architecture |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | jalons, risques, traçabilité |
| [`docs/VERIFICATION.md`](docs/VERIFICATION.md) | procédure d'acceptation manuelle |

---

## Licence

[GPLv3](LICENSE).

Dépendances : [yt-dlp](https://github.com/yt-dlp/yt-dlp) (The Unlicense),
[whisper.cpp](https://github.com/ggml-org/whisper.cpp) et
[whisper-rs](https://github.com/tazz4843/whisper-rs) (MIT),
[FFmpeg](https://ffmpeg.org) (LGPL ou GPL selon la build) — toutes compatibles.

Scripta invoque `yt-dlp` et `ffmpeg` comme programmes externes sans les
redistribuer. Les obligations de licence attachées à leur distribution
s'appliqueront à partir de l'empaquetage des binaires, et un
`THIRD_PARTY_LICENSES.md` accompagnera alors les artefacts
([SPEC §1.3](docs/SPEC.md#13-licence-et-conformité)).
