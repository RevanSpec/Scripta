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

Attendu : les deux extracteurs avec leur version, le backend compilé — `cpu`
en l'absence de feature, conformément à
[ADR-001](SPEC.md#adr-001--stratégie-daccélération-matérielle) —, trois
répertoires `inscriptible`, trois destinations `joignable`, et pour finir
**« Aucun problème détecté. »** Le tout en une à deux secondes.

**Installations dégradées** — chacune doit être nommée, et comptée dans le
bilan final ; `doctor` rend 0 dans tous les cas :

```powershell
# Sidecars absents
$p = $env:Path; $env:Path = "C:\Windows\System32"; scripta doctor; $env:Path = $p

# Cache non inscriptible : un fichier à la place d'un répertoire
Set-Content "$env:TEMP\pas-un-dossier" "x"
$env:SCRIPTA_CACHE_DIR = "$env:TEMP\pas-un-dossier\transcripts"; scripta doctor
Remove-Item Env:SCRIPTA_CACHE_DIR

# Pas de réseau : mandataire injoignable
$env:HTTPS_PROXY = "http://127.0.0.1:9"; scripta doctor; Remove-Item Env:HTTPS_PROXY
```

| Cas | Attendu |
|---|---|
| Sidecars absents | `yt-dlp` et `ffmpeg` : `ABSENT`, 2 problèmes |
| Cache non inscriptible | `cache` : `NON INSCRIPTIBLE`, avec la cause |
| Pas de réseau | trois destinations `INJOIGNABLE`, en deux secondes environ |

---

## 1. Modèle

Téléchargé au premier usage et vérifié par empreinte SHA-256 — il n'y a rien à
préparer. Pour anticiper :

```powershell
scripta models list
scripta models pull base
scripta models verify base
scripta models pull silero    # modèle VAD, actif par défaut
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
$m = scripta models path
$env:SCRIPTA_TEST_MODEL = "$m\ggml-base.bin"
$env:SCRIPTA_TEST_WAV   = "$HOME\jfk.wav"
$env:SCRIPTA_TEST_VAD   = "$m\ggml-silero-v5.1.2.bin"   # tests du VAD
cargo test --release -p scripta-core --test inference -- --test-threads=1
```

Sans accès à `jfk.wav`, la synthèse vocale de Windows produit un échantillon
équivalent — les assertions portent sur des mots, pas sur une voix :

```powershell
Add-Type -AssemblyName System.Speech
$v = New-Object System.Speech.Synthesis.SpeechSynthesizer
$v.SelectVoice("Microsoft Zira Desktop")   # voix anglaise
$f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, 16, 1)
$v.SetOutputToWaveFile("$HOME\jfk.wav", $f)
$v.Speak("And so, my fellow Americans: ask not what your country can do for you. Ask what you can do for your country.")
$v.Dispose()
```

`le_vad_conserve_la_chronologie_d_origine` est le test décisif du VAD : la
même phrase, séparée de longs silences, doit retrouver ses vraies positions,
mots compris. Il échoue si l'on revient au VAD intégré de whisper.cpp (risque
R10 de la [roadmap](ROADMAP.md)).

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
>
> Depuis PowerShell ou cmd, pas depuis Git Bash : MSYS y convertit ces
> valeurs, qui commencent par `/`, en chemins, et la compilation échoue.

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

**Progression hors terminal** — `stderr` redirigé ne doit contenir que des
lignes, sans retour chariot ; vers un terminal, une barre montre pourcentage,
position et vitesse :

```powershell
cmd /c "$S --no-cache $U 2> err.txt > nul"
[IO.File]::ReadAllText("$PWD\err.txt").Replace("`r`n", "").Contains("`r")   # False
```

**VAD** — actif par défaut. Dans le JSON, `transcription.vad` vaut `true`, et
chaque mot tombe dans les bornes de son segment :

```powershell
$d = & $S -f json $U 2>$null | ConvertFrom-Json
$d.transcription.vad
$d.segments | ForEach-Object { $seg = $_; @($_.words | Where-Object { $_.start -lt $seg.start - 0.05 -or $_.end -gt $seg.end + 0.05 }).Count }   # que des 0
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
| `50` — fichier existant, sans `--force` ; **immédiat**, avant toute transcription | `& $S -o sortie.txt $U` deux fois de suite |
| `50` — répertoire de sortie absent, immédiat | `& $S -o inexistant\x.txt $U` |
| `2` — `--vad-model` avec `--no-vad` | `& $S --vad-model x.bin --no-vad $U` |

---

## 5. Interruption

```powershell
& $S -m base "<URL d'une vidéo de 20 min ou plus>"
```

- **Premier `Ctrl-C`** pendant la transcription → message d'interruption
  demandée, puis sortie **en quelques secondes** avec le code `130`. Idem
  pendant le téléchargement d'un modèle ou l'extraction audio : toutes les
  étapes honorent l'interruption.
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


Attendu, sans VAD : **≈ 560 Mo par heure** d'audio — 223 de PCM, 225 de copie
padded par whisper.cpp, 112 de spectrogramme mel —, auxquels s'ajoutent le
modèle et ~340 Mo de tampons ggml
([ADR-003](SPEC.md#adr-003--inférence-non-streamée)). Avec le VAD, actif par
défaut, la copie padded et le mel ne portent que sur la parole : c'est un
majorant.

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
| Crête attendue, modèle `base` | ≈ 1 040 Mo sans VAD, moins avec |

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

### Mesure avec VAD — 2026-09-23

Même vidéo, même modèle, VAD actif — mais une autre machine : 20 cœurs
logiques, Windows 11.

| Grandeur | Mesuré |
|---|---|
| Code de sortie | `0` |
| Durée totale | 1 659 s — **faussée**, voir ci-dessous |
| Crête mémoire | **999 Mo**, contre 1 040 sans VAD |
| Langue détectée | `fr` |
| Segments | 1 626 |
| Couverture | 96,5 % — la musique de fin, écartée par le VAD, n'est plus transcrite |
| **Concordance des deux chemins** | **73 %** |
| Répétitions | **71 ×** « et qui est en train de se faire », de 2 240 à 2 395 s |

1. **Le débit a révélé un défaut, depuis corrigé.** La détection VAD suivait
   `--threads`, soit 20 threads ici ; sur des graphes aussi petits, la
   synchronisation l'emporte sur le calcul : 52 s pour 10 min d'audio, contre
   1,3 s sur un seul thread. Elle tourne désormais sur un thread. Le débit avec
   VAD reste à remesurer.
2. **La mémoire baisse**, comme prévu : l'audio compacté sur place réduit la
   copie padded et le mel sans ajouter de tampon.
3. **Une boucle de répétition est apparue** : 155 s de parole remplacées par
   la même phrase, là où la mesure sans VAD n'en montrait aucune. Un extrait de
   700 s autour d'elle ne la reproduit dans aucune configuration — sans VAD,
   avec VAD, avec VAD et sans contexte glissant : elle dépend du contexte
   accumulé depuis le début. Suivi en risque R11 de la [roadmap](ROADMAP.md).

### Ce qu'il faut regarder

| Point | Attendu |
|---|---|
| Mémoire crête | ≈ 560 Mo/h + modèle + ~340 Mo de tampons ggml sans VAD ; moins avec. Une croissance très supérieure signale une fuite. |
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
([SF-04](SPEC.md#sf-04--moteur-de-transcription-locale)), actif par défaut
depuis le Jalon 2 ; `--no-vad` permet la comparaison.

---

## Quoi rapporter

En cas d'anomalie, les éléments qui permettent de diagnostiquer sans refaire le
test :

1. La commande exacte et le `$LASTEXITCODE`.
2. `scripta doctor`.
3. La sortie `stderr` complète.
4. Pour un problème de qualité : le JSON, qui porte le modèle, le backend et la
   langue détectée.
