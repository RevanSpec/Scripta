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
   accumulé depuis le début. Sur la vidéo entière, couper le contexte glissant
   la supprime ; c'est désormais le comportement du mode VAD (risque R11 de
   la [roadmap](ROADMAP.md)).

### Mesure avec VAD, code final — 2026-09-23

Même machine, VAD sur un thread, sans contexte glissant.

| Grandeur | Mesuré |
|---|---|
| Code de sortie | `0` |
| Durée totale | 598 s (10,0 min) |
| Vitesse | **6,4 × temps réel** |
| Crête mémoire | **863 Mo** |
| Langue détectée | `fr` |
| Segments | 930 |
| Couverture | 96,6 % |
| **Concordance des deux chemins** | **75 %** |
| Répétitions | 12 segments sur 14 s vers 1 142 s, « Je ne sais pas si c'est (pas) ça », là où l'orateur hésite |

La boucle de 155 s a disparu, et la concordance retrouve le niveau de la
mesure sans VAD. La répétition résiduelle est celle que ce réglage laisse
passer par construction : une hallucination du modèle `base` sur un passage
hésitant, qui ne survit pas à sa fenêtre de 30 s.

Le banc qui a comparé les deux réglages — mêmes données, lancés à la suite —
mesurait 6,9 × avec contexte glissant et 8,0 × sans. L'écart avec les 6,4 × de
la suite complète rappelle qu'un débit ne se compare qu'à conditions égales.

### Mesure du J3, nombre de threads corrigé — 2026-09-24

Même machine, code de la v0.1.1 : 14 threads — les cœurs physiques — au lieu
de 20. TextInputHost, le service de saisie de Windows, occupait un cœur
pendant toute la mesure.

| Grandeur | Mesuré |
|---|---|
| Code de sortie | `0` |
| Durée totale | **266 s** (4,4 min), contre 598 s |
| Vitesse | **15,8 × temps réel**, contre 6,4 × |
| Crête mémoire | 863 Mo |
| Segments | 930 |
| Couverture | 96,6 % |
| **Concordance des deux chemins** | **75 %** |
| Répétitions | les mêmes 12 segments hésitants, vers 1 142 s |

Même transcription, deux fois et demie plus vite : le débit perdu tenait au
seul nombre de threads (risque R12 de la [roadmap](ROADMAP.md)).

### Accélération matérielle — Vulkan sur RTX 3070, 2026-09-25

Première mesure GPU du projet. Windows 11, i7-13700H, **NVIDIA GeForce
RTX 3070**, SDK Vulkan 1.4.357.0. Vidéo de référence de 61 min, modèle `base`,
VAD, français imposé, `--no-cache`. Les deux variantes sont bâties depuis le
même commit et lancées dos à dos.

| Grandeur | CPU | **Vulkan** |
|---|---|---|
| Inférence, rapportée par Scripta | 301,4 s | **86,7 s** |
| Vitesse annoncée | 12,1 × | **42,2 ×** |
| Durée totale, extraction comprise | 318,7 s | 102,5 s |
| Crête mémoire | 867 Mo | **798 Mo** |
| Segments | 930 | 980 |
| Taille du binaire | 4,9 Mo | **60,7 Mo** |

**L'accélération annoncée correspond au matériel.** La vitesse que Scripta
affiche est bien celle de l'inférence mesurée — 3 657 s d'audio en 301,4 s
puis en 86,7 s —, et l'écart avec la durée totale n'est que le temps
d'extraction, identique pour les deux.

**Réserve sur le facteur.** La machine était à **36 % de charge** au départ,
ce qui pénalise le CPU et lui seul : le GPU y est peu sensible. Rapporté au
relevé du J3 sur machine au repos (15,8 ×), le facteur tomberait à **2,7 ×**.
Le chiffre honnête est donc *entre 2,7 et 3,5 ×*, et seule une reprise sur
machine inoccupée trancherait. C'est la leçon du risque R6, déjà payée une
fois : **un débit ne se mesure que sur une machine inoccupée.**

**Les deux backends ne rendent pas le même texte.** 88,0 % de concordance
lexicale entre eux, 1,2 % de mots en moins côté Vulkan, 50 segments de plus,
et la même couverture temporelle (96,6 %). L'arithmétique flottante diffère
d'un backend à l'autre ; l'écart reste bien en deçà de celui qui sépare
Scripta des sous-titres de YouTube (75 %). **Aucune régression de qualité,
mais pas non plus de résultat reproductible d'un backend à l'autre** — ce
qu'un test d'égalité stricte entre variantes ne devra jamais supposer.

**Conséquence pour l'empaquetage.** La variante Vulkan pèse **douze fois**
la variante CPU : les shaders de ggml ajoutent 56 Mo au binaire. À dire dans
les notes de version, sous peine de surprendre au téléchargement.

---


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

## 8. Application de bureau

L'installeur d'une release, de préférence : c'est lui que l'utilisateur
recevra, sidecars compris. À défaut, une build optimisée locale —
`cargo tauri dev` compile whisper.cpp sans optimisation, et une vidéo réelle y
prend cinquante fois plus de temps. Sidecars acquis par les scripts de la CI
(voir le README, « Installeur local ») ; sous Windows, un ffmpeg compilé depuis
Linux, ou celui du système pour la seule mécanique :

```powershell
$env:CMAKE_C_FLAGS_RELEASE   = "/MD /O2 /Ob2 /DNDEBUG"
$env:CMAKE_CXX_FLAGS_RELEASE = "/MD /O2 /Ob2 /DNDEBUG"
cd crates\desktop
cargo tauri build --config tauri.bundle.conf.json --bundles nsis
# Sidecars copiés à côté de l'exécutable, sous leurs noms préfixés :
& ..\..\target\release\scripta-desktop.exe
```

| Vérification | Attendu |
|---|---|
| Bandeau d'en-tête | le backend compilé et le nombre de threads ; aucun bandeau « introuvable » |
| Onglet « Modèles et outils » | yt-dlp et ffmpeg marqués « embarqué », jamais « système » |
| Répertoire d'installation | `scripta-yt-dlp`, `scripta-ffmpeg` à côté de l'application ; `licences/` avec les notices de yt-dlp, la LGPL et la fiche de construction de FFmpeg |
| Coller une URL | titre, chaîne, durée et sous-titres rédigés après une pause de saisie |
| URL invalide | « Cette adresse n'est pas celle d'une vidéo YouTube. », conseil, détail replié |
| Transcrire | étape en cours, progression, position, vitesse ; segments au fil de l'eau |
| Remonter dans la sortie | le suivi s'interrompt ; « ↓ Suivre la transcription » le rétablit |
| Onglets, options, pendant l'inférence | réponse immédiate |
| Annuler | « Transcription annulée. » en moins de 2 s ; la mémoire redescend au modèle seul |
| Exporter `.srt` | fichier identique octet pour octet à celui de la CLI |
| Télécharger un modèle, annuler, relancer | la reprise part du fichier partiel ; empreinte vérifiée |
| Supprimer un modèle | deux clics, le second confirme |

### Acceptation — 2026-09-24

Windows 11, i7-13700H (14 cœurs, 20 threads logiques), build optimisée, vidéo
de référence de 61 min, modèle `base`, VAD, horodatage au mot, français.

| Grandeur | Mesuré |
|---|---|
| Threads | 14, le nouveau défaut |
| Vitesse | **14,0 × temps réel** |
| Crête mémoire | **882 Mo**, contre 863 pour la CLI |
| Segments | 930, autant que la mesure de référence |
| Réactivité | onglets, options, défilement et Annuler répondent de bout en bout |
| Annulation, sur la même vidéo | immédiate ; mémoire de 881 à 181 Mo |
| Threads de l'application | 33 au repos, 30 après la transcription : aucun relais ne fuit |

Une première passe, avec 20 threads, n'a traité qu'une fenêtre de 30 s en
cinq minutes : TextInputHost, le service de saisie de Windows, occupait un
cœur, et ce seul thread privé de processeur arrêtait les 19 autres à chaque
barrière de ggml. D'où le nouveau défaut (risque R12 de la
[roadmap](ROADMAP.md)).
### Application installée sous Windows — 2026-09-25

Installeur NSIS de la v0.1.1, tel que la CI le produit. Installation par
profil, sans droits d'administrateur.

| Vérification | Résultat |
|---|---|
| Installation silencieuse | code 0, dans `%LOCALAPPDATA%\Scripta` |
| Contenu | `scripta-desktop.exe`, les deux sidecars, `uninstall.exe`, `licences/` avec la LGPL, la fiche de construction de FFmpeg et les notices de yt-dlp |
| Entrée de désinstallation | Scripta 0.1.1, sous `HKCU` |
| Raccourci | au menu Démarrer |
| Démarrage | la fenêtre s'ouvre, l'interface s'affiche |
| Onglet « Modèles et outils » | yt-dlp **embarqué**, ffmpeg **embarqué** ; espace occupé et chemin des modèles |
| **`update-extractor`** | `yt-dlp.exe` (17 840 399 o) déposé dans `%LOCALAPPDATA%\scripta\bin`, **sans toucher au binaire embarqué** ; l'interface annonce « yt-dlp 2026.08.19 installé et vérifié » et la ligne bascule de « embarqué » à « **mis à jour** » |

Ce dernier point valide [ADR-004](SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour)
sur une application installée, ce qu'aucun essai en mode développement ne peut
faire : `resolve_installed()` consulte `bin/` avant le répertoire du bundle, et
le marqueur d'origine affiché le confirme.

**Une collision à connaître.** L'installeur pose l'application dans
`%LOCALAPPDATA%\Scripta` et le cœur range ses données dans
`%LOCALAPPDATA%\scripta` : Windows ignorant la casse, **c'est le même
répertoire**. Les modèles téléchargés y cohabitent donc avec les fichiers
installés. Les sidecars mis à jour vont dans `bin/`, un sous-répertoire, de
sorte qu'ADR-004 tient ; mais ce qu'emporte une désinstallation n'a pas été
éprouvé — l'essai détruirait les modèles de la machine d'essai.

---


### Premier lancement sous Linux — 2026-09-25

WSL2, Ubuntu 26.04.1, WSLg. AppImage de la v0.1.1, telle que la CI la produit.
But : lancer l'application sous Linux, ce qui n'avait jamais été fait.

| Vérification | Résultat |
|---|---|
| Montage de l'AppImage par FUSE, puis démarrage | la fenêtre s'ouvre — 1036 × 857, 123 Mo |
| Dépendances dynamiques du binaire principal | aucune non résolue |
| yt-dlp embarqué, exécuté | `2026.08.19` |
| ffmpeg embarqué, exécuté | `9.0.2`, protocoles `file` et `pipe` seulement : la build minimale tient ses promesses |
| Dépendances déclarées du `.deb` | `libwebkit2gtk-4.1-0`, `libgtk-3-0` |
| **Affichage de la page** | **échec** — le processus de rendu de WebKit s'arrête au démarrage |

La fenêtre s'ouvre donc, mais reste vide, et rien ne le dit à l'utilisateur :
le journal ne porte qu'une ligne, `Could not create default EGL display:
EGL_BAD_PARAMETER. Aborting...`.

**Cause.** L'AppImage embarque 169 bibliothèques, dont `libwayland-client.so.0`
prise sur la machine de build. L'AppRun place `$APPDIR/usr/lib` en tête du
chemin de recherche, si bien que cette copie masque celle du système. Or
`libEGL_mesa.so.0` en dépend : liée à la version embarquée, son initialisation
échoue, et WebKit abandonne son processus de rendu.

**Comment la cause a été établie.** Une sonde EGL de vingt lignes — `ctypes`,
`eglGetDisplay`, `eglInitialize` — réussit avec le chemin du système et échoue,
sur la même erreur, avec celui de l'AppImage. Une bissection sur les 169
bibliothèques désigne `libwayland-client.so.0`, et elle seule : la retirer
rétablit EGL. Les autres `libwayland` restent — aucune n'est sur le chemin
d'EGL, et retirer `libwayland-server.so.0` empêche l'application de démarrer
sur un système sans compositeur.

**Correctif.** `scripts/empaquetage/appimage-sans-wayland-client.sh`, appelé
après l'empaquetage : il désassemble l'AppImage, retire ce seul fichier, la
réassemble, et vérifie le produit fini. Le script s'arrête de lui-même si le
bundler cesse un jour d'embarquer cette bibliothèque, plutôt que de laisser
croire le défaut corrigé.

**Taille du paquet réassemblé.** Réassembler recompresse, et le défaut de
`mksquashfs` varie d'une version à l'autre. Mesures sur la v0.1.1 :

| Paquet | Taille | Écart |
|---|---|---|
| Produit par `appimagetool`, coureur Ubuntu 22.04 | 125 102 584 o | — |
| Réassemblé au défaut de squashfs-tools 4.5 (le coureur) | 126 290 424 o | **+1,2 Mo** |
| Réassemblé au niveau 19, même coureur | 125 385 208 o | **+276 Kio** |
| Produit par `appimagetool`, paquet de la v0.1.1 | 125 073 912 o | — |
| Réassemblé au niveau 19, squashfs-tools 4.7 (machine d'essai) | 123 820 536 o | **−1,2 Mo** |

Fixer le niveau ramène donc l'écart de 1 % à 0,2 % sur le coureur, et le rend
négatif sur une version plus récente de l'outil. **Le reliquat de 276 Kio n'a
pas été expliqué** : le bloc du squashfs d'origine est le défaut (131 072), ce
n'est donc pas lui ; à cette échelle, la recherche n'en valait pas la peine.
L'écart s'écrit au journal de la construction, de sorte qu'une dérive se
verrait — c'est ainsi que le gonflement de 1,2 Mo a été repéré.

**L'interface, enfin.** Corrigée, l'AppImage butait encore sur
`libGLESv2.so.2` : absente de cette Ubuntu minimale, elle n'est pas non plus
embarquée — toute session de bureau la fournit, un environnement dépouillé
non (`sudo apt install libgles2`). Une fois cette bibliothèque posée,
**l'interface s'affiche** — onglets, champ d'URL, réglages, en-tête
« CPU · 6 threads », et aucun bandeau « introuvable » : les sidecars
embarqués sont résolus. Éprouvé sur l'AppImage corrigée à la main, puis sur
celle que la CI produit avec le correctif.

**Non vérifié sous Linux :** l'onglet « Modèles et outils », et une
transcription réelle de bout en bout. Seul le premier écran a été vu.

---


---

## Quoi rapporter

En cas d'anomalie, les éléments qui permettent de diagnostiquer sans refaire le
test :

1. La commande exacte et le `$LASTEXITCODE`.
2. `scripta doctor`.
3. La sortie `stderr` complète.
4. Pour un problème de qualité : le JSON, qui porte le modèle, le backend et la
   langue détectée.
