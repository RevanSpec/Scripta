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

Le téléchargement automatique arrive avec la tâche 2.3. En attendant, poser le
fichier à la main.

```powershell
$env:SCRIPTA_MODELS_DIR = "$HOME\.cache\scripta\models"
New-Item -ItemType Directory -Force $env:SCRIPTA_MODELS_DIR | Out-Null
Invoke-WebRequest `
  -Uri "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin" `
  -OutFile "$env:SCRIPTA_MODELS_DIR\ggml-base.bin"
```

> Le dépôt est bien **`ggerganov/whisper.cpp`**. `ggml-org/whisper.cpp` renvoie
> HTTP 401 en accès anonyme ([SF-03](SPEC.md#sf-03--gestion-et-cycle-de-vie-des-modèles-whisper)).

| Modèle | Taille | Usage |
|---|---|---|
| `ggml-tiny.bin` | 75 Mo | tests rapides |
| `ggml-base.bin` | 142 Mo | défaut CPU |
| `ggml-small-q5_1.bin` | 190 Mo | bon compromis |

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

> **⚠ Sur Windows/MSVC, employer la build de débogage.**
>
> Contre-intuitif, mais mesuré : `--release` embarque un whisper.cpp **non
> optimisé** et se révèle quatre à six fois plus lente. Sur 11 s d'audio avec
> le modèle `tiny` : **1,8 s en debug contre 7,7 à 11,2 s en release**.
>
> La crate `cmake` écrase `CMAKE_CXX_FLAGS_<BUILD_TYPE>`, alors que le
> générateur Visual Studio compile toujours en `--config Release`. En profil
> debug elle écrase `RELWITHDEBINFO` — sans effet, la config `Release`
> conserve son `/O2 /Ob2` par défaut. En profil release elle écrase
> précisément la config utilisée, et `/O2` disparaît.
>
> Conséquence pour l'empaquetage : **en l'état, les artefacts du Jalon 4
> seraient distribués non optimisés.**

```powershell
cargo build --workspace
$S = ".\target\debug\scripta.exe"
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

## 6. Vidéo longue — mémoire et débit

**Dernier critère de sortie du Jalon 1 encore ouvert.** L'attendu est
d'environ **230 Mo par heure** d'audio
([ADR-003](SPEC.md#adr-003--inférence-non-streamée)), auxquels s'ajoute la
taille du modèle.

### Vidéo de référence

`cZwuhte5ZBI` — « Les photons ont-ils la notion du temps ? feat. Etienne
Klein », e-penser 2.0, **60 min 57 s**.

Retenue pour trois raisons : elle dépasse l'heure, elle est **en français**, ce
qui éprouve la détection de langue hors anglais, et son jargon scientifique
rend mesurable l'apport de `--initial-prompt`.

| Grandeur | Valeur |
|---|---|
| Durée | 3 657 s |
| PCM `f32` en mémoire | **223,2 Mo** (3657 × 16000 × 4) |
| Crête attendue, modèle `base` | ≈ 400 Mo (PCM + modèle + état) |

Le calcul recoupe l'estimation de 230 Mo/h de l'ADR-003 : c'est précisément
cette prédiction que la mesure doit confirmer ou démentir.

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
| Mémoire crête | ≈ 230 Mo/h d'audio + taille du modèle. Une croissance très supérieure signale une fuite. |
| Code de sortie | `0` |
| Segments | Couvrent toute la durée, sans trou ni répétition en boucle |
| Vitesse | À comparer aux seuils du [§5.2](SPEC.md#52-performance), en gardant à l'esprit le défaut d'optimisation ci-dessus |

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
