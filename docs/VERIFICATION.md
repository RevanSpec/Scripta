# Scripta — Vérification manuelle

Procédure d'acceptation à exécuter localement. Elle complète la suite
automatisée ([SPEC §5.5](SPEC.md#55-stratégie-de-test)), qui ne couvre
délibérément ni le réseau ni le matériel.

Les commandes sont données pour **PowerShell** ; les équivalents `bash` figurent
en note quand ils diffèrent.

---

## 0. Prérequis

Nécessaires pour **bâtir** le projet, jamais pour l'exécuter
([SPEC Annexe E](SPEC.md#annexe-e--prérequis-de-compilation)).

```powershell
winget install Kitware.CMake LLVM.LLVM yt-dlp.yt-dlp Gyan.FFmpeg
```

La version de Rust est imposée par `rust-toolchain.toml` : `cargo` l'installe
seul au premier appel, il n'y a rien à faire.

```powershell
scripta doctor   # ou : cargo run -- doctor
```

Les quatre lignes doivent être renseignées. `inférence` indique le backend
compilé — `cpu` en l'absence de feature, conformément à
[ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle).

---

## 1. Modèle

Téléchargé au premier usage et vérifié par empreinte SHA-256 — il n'y a rien à
préparer. Pour anticiper :

```powershell
scripta models list
scripta models pull base
scripta models verify base
```

Le cache va dans `%LOCALAPPDATA%\scripta\models` sous Windows, `~/.cache/scripta/models`
ailleurs. `SCRIPTA_MODELS_DIR` prime sur ce choix.

**Test de corruption** — un modèle altéré doit être détecté, pas transmis à
whisper :

```powershell
Add-Content "$(scripta models path)\ggml-base.bin" "corrompu"
scripta models verify base      # doit échouer avec le code 30
scripta models pull base        # doit le retélécharger
```

---

## 2. Suite automatisée

```powershell
cargo test --workspace
```

Les tests d'inférence se **sautent** sans fixtures. Pour les activer :

```powershell
Invoke-WebRequest -Uri "https://github.com/ggml-org/whisper.cpp/raw/master/samples/jfk.wav" -OutFile "$HOME\jfk.wav"
$env:SCRIPTA_TEST_MODEL = "$env:SCRIPTA_MODELS_DIR\ggml-tiny.bin"
$env:SCRIPTA_TEST_WAV   = "$HOME\jfk.wav"
cargo test -p scripta-core --test inference -- --test-threads=1
```

`--test-threads=1` n'est pas cosmétique : chaque test charge le modèle et lance
une inférence multithread ; en parallèle ils se disputent le CPU et la durée
totale est multipliée par quarante.

---

## 3. Formats de sortie

> **⚠ Sur Windows/MSVC, poser ces deux variables avant de compiler.**
>
> Sans elles, `--release` embarque un whisper.cpp **non optimisé**, quatre à
> six fois plus lent que la build de débogage. La crate `cmake` écrase
> `CMAKE_CXX_FLAGS_<BUILD_TYPE>` alors que le générateur Visual Studio compile
> toujours en `--config Release` : le profil release y perd son `/O2`.
> Le `build.rs` de `whisper-rs-sys` réinjecte toute variable `CMAKE_*` en
> define, et ces defines-là l'emportent.
>
> Mesuré sur la vidéo de 19 s, modèle `tiny` : **1,4–1,6 s avec le
> contournement, contre 9,3–13,6 s sans**.
>
> Suivi en risque R9 dans [`ROADMAP.md`](ROADMAP.md). Déjà appliqué par la CI
> et par `scripts/test-long-video.ps1`.

```powershell
$env:CMAKE_C_FLAGS_RELEASE   = "/MD /O2 /Ob2 /DNDEBUG"
$env:CMAKE_CXX_FLAGS_RELEASE = "/MD /O2 /Ob2 /DNDEBUG"
```

```powershell
cargo build --release --workspace
$S = ".\target\release\scripta.exe"
$U = "https://youtu.be/jNQXAC9IVRw"   # « Me at the zoo », 19 s
```

| Attendu | Commande |
|---|---|
| Texte continu | `& $S -m base $U` |
| Sous-titres numérotés, virgule décimale | `& $S -m base -f srt $U` |
| En-tête `WEBVTT`, point décimal | `& $S -m base -f vtt $U` |
| JSON valide | `& $S -m base -f json $U \| ConvertFrom-Json` |
| Découpage à la largeur imposée | `& $S -m base -f srt --max-line-width 30 $U` |

**Sortie pipeable** — `stdout` ne doit porter que le résultat, sans `--quiet` :

```powershell
& $S -m base -f json $U 2>$null | ConvertFrom-Json | Select-Object -ExpandProperty transcription
```

---

## 4. Codes de sortie

Contractuels : des scripts en dépendent
([SF-07](SPEC.md#sf-07--taxonomie-derreurs-et-codes-de-sortie)). Après chaque
commande, `$LASTEXITCODE`.

| Attendu | Commande |
|---|---|
| `10` — URL refusée | `& $S "https://youtube.com.evil.tld/watch?v=dQw4w9WgXcQ"` |
| `10` — identifiant malformé | `& $S "https://youtu.be/trop-court"` |
| `2` — format inconnu | `& $S -f xml $U` |
| `30` — modèle absent | `& $S --model-path "C:\absent.bin" $U` |
| `13` — diffusion en direct | `& $S -m base "<URL d'un live en cours>"` |
| `14` — durée excessive | `& $S -m base --max-duration 1 $U` |

---

## 5. Interruption

```powershell
& $S -m base "<URL d'une vidéo de 20 min ou plus>"
```

- **Premier `Ctrl-C`** pendant la transcription → message d'interruption
  demandée, puis sortie **en quelques secondes** avec le code `130`.
- **Second `Ctrl-C`** → sortie immédiate, également en `130`.

Vérifier qu'aucun processus ne survit :

```powershell
Get-Process yt-dlp, ffmpeg -ErrorAction SilentlyContinue
```

La commande ne doit rien renvoyer.

---

## 6. Suite d'acceptation sur vidéo longue

Un seul script couvre l'ensemble. Depuis la racine du dépôt :

```powershell
.\scripts	est-long-video.ps1 -Quick   # ~1 min, sans inférence
.\scripts	est-long-video.ps1          # complet, ~25 min
```

Il vérifie les prérequis, compile, puis enchaîne deux phases.

**Phase 1 — rapide.** Sous-titres officiels dans les quatre formats, avec
contrôle de conformité de chacun (numérotation et virgule décimale du SRT,
en-tête `WEBVTT` et point décimal, schéma JSON), couverture des horodatages,
absence de cue de durée aberrante, et les cinq codes de sortie contractuels.

**Phase 2 — transcription.** Mesure de la mémoire crête et du débit, contrôle
de couverture, cohérence de la langue détectée avec celle déclarée par YouTube,
détection des répétitions en boucle.

**Concordance.** Les deux phases empruntent des chemins **indépendants** : l'une
passe par YouTube, l'autre par Whisper. Le script mesure leur recouvrement
lexical. Un score élevé rend improbable que les deux soient corrompus de la même
manière — c'est le seul contrôle de la suite qui valide la transcription sur le
fond plutôt que sur la forme.

> **Note PowerShell 5.1.** Le script emploie délibérément
> `ErrorActionPreference = "Continue"`. En « Stop », chaque ligne de `stderr`
> d'un exécutable natif devient un `ErrorRecord` fatal — la ligne « Finished »
> de `cargo` suffisait à faire avorter la suite. Les échecs réels sont
> contrôlés par `$LASTEXITCODE`.

---

## 7. Mémoire et débit — détail


Attendu : **≈ 350 Mo par heure** d'audio — 223 de PCM et 112 de spectrogramme
mel — auxquels s'ajoutent le modèle et ~340 Mo d'état
([ADR-003](SPEC.md#adr-003--inférence-non-streamée)).

### Vidéo de référence

`cZwuhte5ZBI` — « Les photons ont-ils la notion du temps ? feat. Etienne
Klein », e-penser 2.0, **60 min 57 s**.

Retenue pour trois raisons : elle dépasse l'heure, elle est **en français**, ce
qui éprouve la détection de langue hors anglais, et son jargon scientifique
rend mesurable l'apport de `--initial-prompt`.

| Grandeur | Valeur |
|---|---|
| Durée annoncée | 3 657 s |
| PCM `f32` | 223 Mo (3657 × 16000 × 4) |
| Spectrogramme mel | 112 Mo (80 × 365 700 × 4) |
| Crête attendue, modèle `base` | ≈ 820 Mo |

### Mesure de référence — 2026-09-22

Windows 11, 12 cœurs logiques, CPU seul, modèle `base`, build `--release` avec
le contournement R9, **machine au repos**.

| Grandeur | Mesuré |
|---|---|
| Code de sortie | `0` |
| Durée totale | 774 s (12,9 min) |
| Vitesse | **4,8 × temps réel** |
| Crête mémoire | **1 040 Mo** |
| Langue détectée | `fr`, conforme à la déclaration YouTube |
| Segments | 1 467 |
| Couverture | 100,2 % (3 664 s décodées pour 3 657 s annoncées) |
| Sous-titres officiels | 1 866 segments en 3,0 s |
| **Concordance des deux chemins** | **75 % de recouvrement lexical** |

**Trois enseignements.**

1. **Le modèle mémoire est exact.** 224 (notre tampon) + 225 (copie padded de
   whisper) + 112 (mel) + 141 (modèle) + 338 (tampons ggml) = **1 040 Mo**,
   soit la valeur mesurée. L'audio réside deux fois en mémoire pendant le
   calcul du mel, et c'est le poste le plus lourd après les tampons de calcul.

2. **Une mesure de débit exige une machine au repos.** Le même binaire a donné
   2,1 ×, puis 2,7 ×, puis 4,8 × — l'écart tient entièrement aux compilations
   concurrentes. Le seuil du [§5.2](SPEC.md#52-performance), que j'avais
   abaissé de 3 × à 2 × sur la foi d'une mesure polluée, est rétabli à 3 ×.

3. **Les répétitions ne sont pas des hallucinations.** 5 × `[Musique]` et
   4 × `C'est ça.` sur 1 467 segments, soit 0,6 %. `[Musique]` est un
   étiquetage correct des passages musicaux. Le VAD n'est pas le levier que ce
   contenu réclame.

**La concordance à 75 %** est le résultat le plus solide de la série : deux
chemins totalement indépendants — l'un par YouTube, l'autre par Whisper —
s'accordent sur trois mots de vocabulaire sur quatre. Le quart restant
s'explique par la ponctuation, la casse et les noms propres, que les pistes
auto-générées rendent mal. Aucun contrôle structurel n'apporte cette garantie.

```powershell
$U = "https://www.youtube.com/watch?v=cZwuhte5ZBI"
$p = Start-Process -FilePath $S -ArgumentList "-m","base","-f","json","-o","out.json",$U `
                   -PassThru -NoNewWindow
$peak = 0
while (-not $p.HasExited) {
  $p.Refresh()
  $peak = [Math]::Max($peak, $p.PeakWorkingSet64)
  Start-Sleep -Milliseconds 500
}
"Code de sortie  : $($p.ExitCode)"
"Mémoire crête   : {0:N0} Mo" -f ($peak / 1MB)
"Durée totale    : {0:N1} s" -f ($p.ExitTime - $p.StartTime).TotalSeconds
```

Puis, depuis le JSON produit :

```powershell
$d = Get-Content out.json -Raw | ConvertFrom-Json
"Audio      : {0:N1} min" -f ($d.source.duration_s / 60)
"Inférence  : {0:N1} s"   -f ($d.transcription.duration_ms / 1000)
"Vitesse    : {0}x"       -f $d.transcription.speed_realtime
"Segments   : {0}"        -f $d.segments.Count
```

### Ce qu'il faut regarder

| Point | Attendu |
|---|---|
| Mémoire crête | ≈ 350 Mo/h + modèle + ~340 Mo d'état. Une croissance très supérieure signale une fuite. |
| Code de sortie | `0` |
| Segments | Couvrent toute la durée, sans trou ni répétition en boucle |
| Vitesse | À comparer aux seuils du [§5.2](SPEC.md#52-performance) — **build `--release` avec le contournement R9 uniquement** |

### Hallucinations

Whisper répète parfois une phrase en boucle sur les silences et les passages
musicaux. Dans le JSON, un même `text` sur plusieurs segments consécutifs en est
le signe :

```powershell
$d.segments | Group-Object text | Where-Object Count -gt 3 | Select-Object Count, Name
```

C'est le défaut que le VAD Silero corrige
([SF-04](SPEC.md#sf-04--moteur-de-transcription-locale)) ; il n'est pas encore
activé par défaut, le modèle VAD n'étant pas téléchargé automatiquement.

---

## Quoi rapporter

En cas d'anomalie, les éléments qui permettent de diagnostiquer sans refaire le
test :

1. La commande exacte et le `$LASTEXITCODE`.
2. `scripta doctor`.
3. La sortie `stderr` complète.
4. Pour un problème de qualité : le JSON, qui porte le modèle, le backend et la
   langue détectée.
