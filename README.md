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

**La CLI est publiée** — v0.1.0, pour Windows, Linux et macOS Apple Silicon.
L'application de bureau est en cours de développement.

| | État |
|---|---|
| Extraction audio sans fichier temporaire | ✅ |
| Transcription locale (Whisper) | ✅ |
| Détection d'activité vocale (VAD) | ✅ active par défaut |
| Formats `txt`, `srt`, `vtt`, `json` | ✅ |
| Horodatage au mot | ✅ |
| Interruption propre (`Ctrl-C`) | ✅ |
| Téléchargement des modèles, vérifié SHA-256 | ✅ |
| Accélération GPU | ⏳ compilable, non distribuée |
| Sous-titres YouTube officiels | ✅ |
| Cache de transcriptions | ✅ |
| Mise à jour de l'extracteur | ✅ |
| Application de bureau | ⏳ |
| Binaires précompilés (CLI) | ✅ v0.1.0 |

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

### Binaires précompilés

Les archives de chaque version sont publiées dans les
[releases](https://github.com/RevanSpec/Scripta/releases), avec leurs sommes de
contrôle (`SHA256SUMS`) :

| Système | Archive |
|---|---|
| Windows 10 ou ultérieur, x86-64 | `scripta-<version>-x86_64-pc-windows-msvc.zip` |
| Linux x86-64 (Ubuntu 22.04, Debian 12 ou plus récent) | `scripta-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| macOS 11 ou ultérieur, Apple Silicon | `scripta-<version>-aarch64-apple-darwin.tar.gz` |

Scripta invoque **yt-dlp** et **ffmpeg** sans les embarquer : installez-les
séparément (commandes ci-dessous), placez `scripta` dans votre `PATH`, puis
vérifiez avec `scripta doctor`.

> **macOS** : le binaire n'est pas encore signé. Téléchargé par un navigateur,
> il est bloqué par Gatekeeper ; retirez l'attribut de quarantaine avec
> `xattr -d com.apple.quarantine scripta`.

Les binaires sont compilés pour le CPU. Pour un GPU, compilez depuis les
sources.

### Prérequis

`whisper.cpp` est compilé depuis ses sources et ses liaisons FFI sont générées à
la construction. Ces outils servent donc à **bâtir** Scripta, jamais à
l'exécuter ([SPEC Annexe E](docs/SPEC.md#annexe-e--prérequis-de-compilation)).

| Outil | Rôle |
|---|---|
| **CMake** ≥ 3.20 | build natif de whisper.cpp |
| **libclang** (LLVM) | génération des liaisons FFI |
| Toolchain C++ | MSVC 2022 · GCC/Clang · Xcode CLT |
| **yt-dlp**, **ffmpeg** | extraction audio, à l'exécution cette fois — requis aussi avec les binaires |

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
>
> Depuis PowerShell ou cmd, pas depuis Git Bash : MSYS y convertit ces
> valeurs, qui commencent par `/`, en chemins, et la compilation échoue.

Le binaire est dans `target/release/scripta`. Vérifiez l'installation :

```bash
scripta doctor
```

`doctor` passe en revue les extracteurs et leurs versions, le backend compilé,
les modèles présents, les droits d'écriture des répertoires et la joignabilité
de YouTube, HuggingFace et GitHub. Sa dernière ligne dénombre les problèmes.

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

> **Non éprouvé.** Seule la build CPU est construite et testée en CI. Les
> builds GPU compilent en principe, mais aucune n'a encore été construite ni
> mesurée (risque R1 de la [roadmap](docs/ROADMAP.md)).

---

## Démarrage rapide

Le modèle est téléchargé au premier usage et vérifié par empreinte SHA-256,
de même que le modèle de détection d'activité vocale (moins d'un mégaoctet).
Il n'y a rien à préparer.

```bash
scripta "https://www.youtube.com/watch?v=<ID>"
```

`--model auto`, le défaut, choisit `turbo` si un backend GPU est compilé et
`base` sinon. La gestion du cache passe par `scripta models` :

```bash
scripta models list          # catalogue et état local
scripta models pull small    # pré-télécharge
scripta models pull silero   # modèle VAD, pour un usage hors ligne
scripta models verify small  # recalcule l'empreinte
scripta models rm small
scripta models path
```

### Cache de transcriptions

Une vidéo déjà transcrite ressort **instantanément** — 62 ms contre plusieurs
minutes — et tous les formats s'en dérivent sans réinférence. La clé couvre
tout ce qui influe sur le résultat : vidéo, modèle, langue, traduction, VAD,
horodatage au mot, contexte (`--initial-prompt`) et seuils. Changer l'un d'eux
relance la transcription.

```bash
scripta cache list      # entrées et volume
scripta cache clear
scripta cache path
scripta --no-cache "<URL>"   # ignore le cache dans les deux sens
```

Éviction LRU au-delà de 2 Go.

### Mise à jour de l'extracteur

YouTube modifie fréquemment ses mécanismes d'extraction : un `yt-dlp` figé
devient inopérant en quelques semaines. Sa mise à jour n'est donc pas un
confort mais une condition de fonctionnement.

```bash
scripta update-extractor
```

Le binaire est téléchargé depuis les *releases* de yt-dlp, vérifié par
SHA-256, et installé dans un **répertoire utilisateur** — jamais dans le
bundle applicatif. Y écrire invaliderait sa signature, et sur Apple Silicon
l'application ne se lancerait plus (ADR-004). `scripta doctor` indique la
provenance du binaire retenu : `mis à jour`, `embarqué` ou `système`.

Scripta vérifie **au plus une fois par jour** qu'une version plus récente
existe, en arrière-plan et sans jamais retarder une commande, et le signale en
fin d'exécution. `SCRIPTA_NO_UPDATE_CHECK=1` désactive cette vérification.

| Modèle | Taille | Remarque |
|---|---|---|
| `tiny` | 75 Mo | très rapide, qualité limitée |
| `base` | 142 Mo | bon point de départ sur CPU |
| `small` | 190 Mo | compromis recommandé |
| `large-v3` | 1,1 Go | qualité maximale ; recommandé pour traduire |
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

# Sous-titres officiels : instantané, aucune inférence
scripta subs -l fr -f srt -o cours.srt "<URL>"

# Vidéo restreinte (limite d'âge, vérification anti-robot)
scripta --cookies-from-browser firefox "<URL>"
```

### Sous-titres officiels

`scripta subs` récupère les sous-titres de YouTube sans lancer Whisper — une
heure de vidéo en quelques secondes. `--prefer-subs` les tente d'abord et se
replie silencieusement sur la transcription en cas d'absence ou d'échec.

La langue retenue est celle demandée, à défaut celle déclarée par YouTube.
**Sans l'une ni l'autre, et face à plusieurs pistes, Scripta refuse de
deviner** : YouTube publie des traductions automatiques dans une centaine de
langues, et choisir la première reviendrait à tirer au sort.

Deux avertissements sont émis le cas échéant : piste auto-générée (souvent sans
ponctuation) et traduction automatique (deux passages machine cumulés).

### Options principales

| Option | Effet |
|---|---|
| `-m, --model <NOM>` | `auto` (défaut), `tiny`, `base`, `small`, `medium`, `large-v3`, `turbo` |
| `--model-path <CHEMIN>` | chemin explicite, prioritaire |
| `-f, --format <FORMAT>` | `txt` (défaut), `srt`, `vtt`, `json` |
| `-o, --output <CHEMIN>` | fichier de sortie ; `stdout` par défaut |
| `-l, --lang <CODE>` | langue imposée ; détection automatique sinon |
| `--translate` | traduction vers l'anglais |
| `--initial-prompt <TEXTE>` | contexte pour les noms propres et le jargon ; n'agit que sur les premières minutes ([SF-04](docs/SPEC.md#sf-04--moteur-de-transcription-locale)) |
| `--word-timestamps` | horodatage au mot (d'office avec `-f json`) |
| `--no-vad` | désactive la détection d'activité vocale, active par défaut |
| `--vad-model <CHEMIN>` | modèle VAD hors cache |
| `--no-speech-thold <S>` | seuil d'absence de parole, entre 0 et 1 (défaut whisper.cpp : 0.6) |
| `--entropy-thold <S>` | seuil d'entropie des décodages répétitifs (défaut : 2.4) |
| `--max-line-width <N>` | largeur des lignes de sous-titres (défaut 42) |
| `--max-line-count <N>` | lignes par sous-titre (défaut 2) |
| `--max-duration <MIN>` | refus au-delà (défaut 240) |
| `-t, --threads <N>` | threads d'inférence |
| `--force` | écrase le fichier de sortie s'il existe |
| `-q, --quiet` | supprime la progression |
| `-v, --verbose` | chemins résolus, clé de cache, journaux de whisper.cpp |

`scripta --help` donne la liste complète.

### Sortie et scripts

`stdout` ne porte **que** le résultat ; progression, avertissements et erreurs
vont sur `stderr`. `scripta <URL> -f json | jq` fonctionne donc sans `--quiet`.
Vers un terminal, la progression s'affiche en barre — pourcentage, position dans
la vidéo, vitesse ; redirigée vers un fichier ou une CI, en simples lignes.

`-o` n'écrase jamais un fichier existant sans `--force`, et le refus tombe
**avant** la transcription, pas après.

### Détection d'activité vocale

Whisper hallucine sur les silences prolongés et les passages musicaux —
typiquement en répétant une phrase de remerciement. Le VAD Silero écarte ces
passages avant l'inférence, qui s'en trouve aussi plus rapide sur les contenus
peu denses. Les horodatages restent ceux de la vidéo, au mot près.

Sur un contenu chanté ou très musical, le VAD peut écarter des passages utiles :
`--no-vad` le désactive.

En mode VAD, aucune fenêtre de 30 s n'est conditionnée sur le texte des
précédentes : une boucle de répétition, où Whisper redit la même phrase, ne peut
donc pas s'étendre au-delà d'une fenêtre (risque R11 de la
[roadmap](docs/ROADMAP.md)).

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

Le premier `Ctrl-C` demande l'arrêt, **quelle que soit l'étape** :
téléchargement d'un modèle (repris là où il s'est arrêté au lancement
suivant), sonde, extraction — les sous-processus sont tués —, ou inférence, où
whisper.cpp rend la main entre deux fenêtres de traitement. La sortie se fait
en `130`. Un second `Ctrl-C` force la terminaison immédiate.

---

## Confidentialité

L'inférence est **locale** : aucun segment transcrit, aucune URL, aucun
identifiant ne quitte la machine. Les seules destinations réseau sont
`youtube.com` (via `yt-dlp`), `huggingface.co` (modèles) et `github.com` (mise à
jour de `yt-dlp`). La vérification quotidienne de mise à jour se résume à une
requête `HEAD` vers `github.com` ; `SCRIPTA_NO_UPDATE_CHECK=1` la supprime.

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
redistribuer. Les bibliothèques compilées dans le binaire, elles, le sont :
chaque archive joint leurs licences (`LICENCES-DEPENDANCES.md`) et
[`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md)
([SPEC §1.3](docs/SPEC.md#13-licence-et-conformité)).
