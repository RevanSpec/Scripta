<#
.SYNOPSIS
    Suite d'acceptation sur vidéo longue.

.DESCRIPTION
    Éprouve la chaîne complète sur un contenu réel d'une heure, en deux phases.

    La phase rapide (~1 min) couvre les sous-titres officiels, les quatre
    formats de sortie et les codes de sortie contractuels. La phase longue
    (~25 min) lance la transcription Whisper en échantillonnant la mémoire.

    Les deux chemins étant indépendants — l'un ne passe pas par l'inférence —
    leur concordance lexicale constitue un signal de bout en bout : une
    transcription silencieusement corrompue s'en écarterait.

    Passer -Quick s'arrête après la phase rapide.

.PARAMETER Url
    URL de la vidéo. Par défaut la vidéo de référence (60 min 57 s, français).

.PARAMETER Model
    Modèle Whisper. `base` par défaut.

.PARAMETER Lang
    Langue des sous-titres. Déduite de la vidéo si omise.

.PARAMETER Quick
    S'arrête après la phase rapide, sans lancer la transcription.

.PARAMETER Profile
    `release` (défaut) ou `debug`. Le script applique le contournement du
    risque R9, sans lequel `--release` embarque sous MSVC un whisper.cpp non
    optimisé, quatre à six fois plus lent. Voir docs/ROADMAP.md.

.EXAMPLE
    .\scripts\test-long-video.ps1 -Quick

.EXAMPLE
    .\scripts\test-long-video.ps1
#>

[CmdletBinding()]
param(
    [string] $Url = "https://www.youtube.com/watch?v=cZwuhte5ZBI",
    [string] $Model = "base",
    [string] $Lang,
    [switch] $Quick,
    [ValidateSet("debug", "release")] [string] $Profile = "release",
    [string] $OutDir = (Join-Path $env:TEMP ("scripta-test-" + (Get-Date -Format "yyyyMMdd-HHmmss")))
)

# « Continue » et non « Stop » : PowerShell 5.1 convertit chaque ligne de
# stderr d'un exécutable natif en ErrorRecord, et `cargo` comme `scripta` y
# écrivent légitimement — la ligne « Finished » de cargo suffisait à faire
# avorter le script. Les échecs réels sont contrôlés explicitement par
# $LASTEXITCODE.
$ErrorActionPreference = "Continue"
$script:Echecs = 0

function Section($titre) {
    Write-Host ""
    Write-Host "=== $titre ===" -ForegroundColor Cyan
}

function Ok($msg)     { Write-Host "  [OK]     $msg" -ForegroundColor Green }
function Alerte($msg) { Write-Host "  [ALERTE] $msg" -ForegroundColor Red; $script:Echecs++ }
function Note($msg)   { Write-Host "  [note]   $msg" -ForegroundColor Yellow }

function Verifier($condition, $succes, $echec) {
    if ($condition) { Ok $succes } else { Alerte $echec }
}

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

# ---------------------------------------------------------------- prérequis --
Section "Prérequis"

# Une installation par winget met à jour le PATH persistant, mais jamais celui
# des sessions déjà ouvertes : un terminal lancé avant l'installation ne voit
# rien. Plutôt que d'exiger un redémarrage, on recharge le PATH depuis le
# registre, puis on sonde les emplacements connus.
function Resolve-Tool {
    param([Parameter(Mandatory)] [string] $Nom)

    $c = Get-Command $Nom -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }

    $persistant = @(
        [Environment]::GetEnvironmentVariable("Path", "Machine")
        [Environment]::GetEnvironmentVariable("Path", "User")
    ) -join ';'
    $env:PATH = "$env:PATH;$persistant"

    $c = Get-Command $Nom -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }

    foreach ($dossier in @(
        (Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Links")
        (Join-Path $env:USERPROFILE ".cargo\bin")
        "$env:ProgramFiles\ffmpeg\bin"
    )) {
        $p = Join-Path $dossier "$Nom.exe"
        if (Test-Path $p) {
            $env:PATH = "$dossier;$env:PATH"
            return $p
        }
    }
    return $null
}

foreach ($outil in @("cargo", "yt-dlp", "ffmpeg")) {
    $chemin = Resolve-Tool $outil
    if (-not $chemin) { throw "$outil est introuvable. Installation : voir README.md." }
    Write-Host "  $outil : $chemin"
}

# ------------------------------------------------------------------- build ---
Section "Binaire"

# Contournement du risque R9 — à poser AVANT tout appel à cargo. La crate
# `cmake` écrase CMAKE_CXX_FLAGS_<BUILD_TYPE>, or le générateur Visual Studio
# compile toujours en --config Release : sans ces variables, le profil release
# perd son /O2 et whisper.cpp tourne quatre à six fois plus lentement.
if ($env:OS -eq "Windows_NT") {
    $env:CMAKE_C_FLAGS_RELEASE   = "/MD /O2 /Ob2 /DNDEBUG"
    $env:CMAKE_CXX_FLAGS_RELEASE = "/MD /O2 /Ob2 /DNDEBUG"
}

# Toujours invoquer cargo, jamais se contenter de la présence du binaire :
# après un `git pull`, un exécutable déjà là est périmé et la mesure porterait
# sur l'ancien code. Cargo est incrémental.
Write-Host "  Compilation en $Profile (immédiate si rien n'a changé)..."
if ($Profile -eq "release") { cargo build --release --workspace } else { cargo build --workspace }
if ($LASTEXITCODE -ne 0) { throw "La compilation a échoué." }

$Exe = Join-Path $Root "target\$Profile\scripta.exe"
Write-Host "  $Exe"
Write-Host ("  Compilé le {0:yyyy-MM-dd HH:mm:ss}" -f (Get-Item $Exe).LastWriteTime)

New-Item -ItemType Directory -Force $OutDir | Out-Null
Write-Host "  Artefacts : $OutDir"

# ------------------------------------------------------------------ vidéo ----
Section "Vidéo"

$meta = (& yt-dlp -J --no-warnings --no-playlist -- $Url | ConvertFrom-Json)
$dureeAudio = [double] $meta.duration
$pcmMo = $dureeAudio * 16000 * 4 / 1MB
$melMo = 80 * $dureeAudio * 100 * 4 / 1MB

Write-Host ("  {0}" -f $meta.title)
Write-Host ("  Durée           : {0:N0} s = {1:N1} min" -f $dureeAudio, ($dureeAudio / 60))
Write-Host ("  Langue déclarée : {0}" -f $(if ($meta.language) { $meta.language } else { "(aucune)" }))

$langArgs = @()
if ($Lang) { $langArgs = @("-l", $Lang) }

# ------------------------------------------------- phase 1 : sous-titres -----
Section "Phase 1 - sous-titres officiels (aucune inference)"

$chrono = [Diagnostics.Stopwatch]::StartNew()
$subsTxt = Join-Path $OutDir "subs.txt"
& $Exe subs @langArgs -q -o $subsTxt -- $Url 2> $null
$codeSubs = $LASTEXITCODE
$chrono.Stop()

$texteSubs = $null
if ($codeSubs -ne 0) {
    Note "sous-titres indisponibles (code $codeSubs) : vidéo sans piste, ou limitation de débit (429)"
} else {
    $texteSubs = Get-Content $subsTxt -Raw -Encoding UTF8
    Ok ("récupérés en {0:N1} s, {1:N0} caractères" -f $chrono.Elapsed.TotalSeconds, $texteSubs.Length)

    foreach ($fmt in @("srt", "vtt", "json")) {
        $cible = Join-Path $OutDir "subs.$fmt"
        & $Exe subs @langArgs -q -f $fmt -o $cible -- $Url
        if ($LASTEXITCODE -ne 0) { Alerte "format $fmt : echec"; continue }
        $contenu = Get-Content $cible -Raw -Encoding UTF8

        switch ($fmt) {
            "srt" {
                # Numérotation depuis 1 et virgule décimale.
                Verifier ($contenu -match '(?m)^1\r?$' -and $contenu -match '\d{2}:\d{2}:\d{2},\d{3} --> ') `
                    "srt conforme" "srt : numerotation ou virgule decimale absente"
            }
            "vtt" {
                # En-tête obligatoire et point décimal.
                Verifier ($contenu.StartsWith("WEBVTT") -and $contenu -match '\d{2}:\d{2}:\d{2}\.\d{3} --> ') `
                    "vtt conforme" "vtt : en-tete WEBVTT ou point decimal absent"
            }
            "json" {
                try {
                    $d = $contenu | ConvertFrom-Json
                    Verifier ($d.schema_version -eq 1 -and @($d.segments).Count -gt 0) `
                        ("json conforme, {0} segments" -f @($d.segments).Count) "json : structure inattendue"
                } catch { Alerte "json : document invalide" }
            }
        }
    }

    # Les horodatages doivent couvrir la vidéo : un analyseur qui perd le
    # contenu des cues produirait des bornes aberrantes.
    $d = Get-Content (Join-Path $OutDir "subs.json") -Raw -Encoding UTF8 | ConvertFrom-Json
    $segs = @($d.segments)
    $couvert = [double] $segs[-1].end
    Verifier ($couvert -gt $dureeAudio * 0.9) `
        ("couverture {0:P0} ({1:N0} s sur {2:N0} s)" -f ($couvert / $dureeAudio), $couvert, $dureeAudio) `
        ("couverture insuffisante : {0:N0} s sur {1:N0} s" -f $couvert, $dureeAudio)

    # Un cue de 10 ms trahit l'analyseur qui perd le corps du bloc et laisse le
    # texte au cue de transition suivant.
    $courts = @($segs | Where-Object { ($_.end - $_.start) -lt 0.1 }).Count
    Verifier ($courts -eq 0) "aucun cue de duree aberrante" `
        "$courts cues durent moins de 100 ms : analyseur WebVTT suspect"
}

# ------------------------------------------------- codes de sortie -----------
Section "Phase 1 - codes de sortie contractuels"

$cas = @(
    @{ Code = 10; Desc = "URL refusee";       Args = @("https://youtube.com.evil.tld/watch?v=dQw4w9WgXcQ") }
    @{ Code = 10; Desc = "identifiant court"; Args = @("https://youtu.be/trop-court") }
    @{ Code = 2;  Desc = "format inconnu";    Args = @("-f", "xml", $Url) }
    @{ Code = 30; Desc = "modele inconnu";    Args = @("-m", "inexistant", $Url) }
    @{ Code = 14; Desc = "duree excessive";   Args = @("--max-duration", "1", $Url) }
)
foreach ($c in $cas) {
    & $Exe -q @($c.Args) > $null 2> $null
    Verifier ($LASTEXITCODE -eq $c.Code) `
        ("{0} -> {1}" -f $c.Desc, $c.Code) `
        ("{0} : attendu {1}, obtenu {2}" -f $c.Desc, $c.Code, $LASTEXITCODE)
}

if ($Quick) {
    Section "Bilan (phase rapide)"
    if ($script:Echecs -eq 0) { Ok "aucune anomalie" } else { Alerte "$script:Echecs anomalie(s)" }
    Write-Host "`n  Artefacts : $OutDir`n"
    exit $script:Echecs
}

# ------------------------------------------- phase 2 : transcription ---------
Section "Phase 2 - transcription Whisper"

# whisper.cpp recopie l'audio entier avec 30 s de padding avant de calculer le
# mel (log_mel_spectrogram) : il réside donc DEUX fois en mémoire. Omettre ce
# poste sous-estimait la prédiction de 224 Mo exactement.
#
# Avec le VAD, actif par défaut, l'audio est compacté sur place avant d'être
# confié à whisper.cpp : la copie padded et le mel ne portent plus que sur la
# parole. Calculée sur la durée totale, la prédiction devient un majorant.
$padMo = ($dureeAudio * 16000 + 480400) * 4 / 1MB
$overheadMo = 338   # tampons de calcul ggml, mesurés, indépendants de la durée
$modelMo = 141
$cretePrevueMo = $pcmMo + $padMo + $melMo + $modelMo + $overheadMo
Write-Host ("  PCM {0:N0} + copie whisper {1:N0} + mel {2:N0} + modele ~{3:N0} + ggml {4:N0} Mo" -f $pcmMo, $padMo, $melMo, $modelMo, $overheadMo)
Write-Host ("  Crete prevue : {0:N0} Mo (sans VAD ; majorant avec)" -f $cretePrevueMo)
Write-Host "  En cours... (Ctrl-C interrompt proprement, et c'est aussi un test)"

$Json = Join-Path $OutDir "run.json"
$ErrLog = Join-Path $OutDir "stderr.log"
$cliArgs = @("-m", $Model, "-f", "json", "-o", $Json) + $langArgs + @("--", $Url)

$p = Start-Process -FilePath $Exe -ArgumentList $cliArgs -PassThru -NoNewWindow `
                   -RedirectStandardError $ErrLog
# PowerShell libère le handle dès la terminaison, et ExitCode revient vide.
# Lire .Handle le met en cache.
$null = $p.Handle

$peak = 0L
$debut = Get-Date
while (-not $p.HasExited) {
    try {
        $p.Refresh()
        if ($p.PeakWorkingSet64 -gt $peak) { $peak = $p.PeakWorkingSet64 }
    } catch { }
    Start-Sleep -Milliseconds 500
}
$p.WaitForExit()
$ecoule = (Get-Date) - $debut

Section "Phase 2 - resultats"

Write-Host ("  Code de sortie : {0}" -f $p.ExitCode)
Write-Host ("  Duree totale   : {0:N1} s ({1:N1} min)" -f $ecoule.TotalSeconds, $ecoule.TotalMinutes)
Write-Host ("  Memoire crete  : {0:N0} Mo" -f ($peak / 1MB))

if ($p.ExitCode -ne 0) {
    Alerte "transcription en echec"
    Get-Content $ErrLog -Tail 15 | ForEach-Object { Write-Host "    $_" }
    exit 1
}

$r = Get-Content $Json -Raw -Encoding UTF8 | ConvertFrom-Json
$rsegs = @($r.segments)
Write-Host ("  Langue         : {0}" -f $r.transcription.language)
Write-Host ("  Vitesse        : {0}x temps reel" -f $r.transcription.speed_realtime)
Write-Host ("  Segments       : {0}" -f $rsegs.Count)

Section "Verdicts"

Verifier (($peak / 1MB) -lt ($cretePrevueMo * 1.5)) `
    ("memoire {0:N0} Mo, conforme a la prediction de {1:N0} Mo" -f ($peak / 1MB), $cretePrevueMo) `
    ("memoire {0:N0} Mo contre {1:N0} Mo prevus : fuite probable" -f ($peak / 1MB), $cretePrevueMo)

$couvertR = [double] $rsegs[-1].end
Verifier ($couvertR -gt $dureeAudio * 0.95) `
    ("couverture {0:P1}" -f ($couvertR / $dureeAudio)) `
    ("transcription tronquee : {0:N0} s sur {1:N0} s" -f $couvertR, $dureeAudio)

if ($meta.language) {
    Verifier ($r.transcription.language -eq $meta.language) `
        ("langue detectee « {0} », conforme a la declaration YouTube" -f $r.transcription.language) `
        ("langue detectee « {0} » alors que YouTube declare « {1} »" -f $r.transcription.language, $meta.language)
}

# Répétitions en boucle : whisper hallucine sur les silences et la musique.
$repets = $rsegs | Group-Object text | Where-Object { $_.Count -gt 3 } | Sort-Object Count -Descending
if ($repets) {
    Note "repetitions a examiner :"
    $repets | Select-Object -First 5 | ForEach-Object {
        $t = $_.Name; if ($t.Length -gt 55) { $t = $t.Substring(0, 55) + "..." }
        Write-Host ("             {0}x  {1}" -f $_.Count, $t)
    }
} else {
    Ok "aucune repetition en boucle"
}

# ------------------------------------------------- concordance ---------------
if ($texteSubs) {
    Section "Concordance sous-titres / transcription"

    # Deux chemins indépendants : l'un passe par YouTube, l'autre par Whisper.
    # Un fort recouvrement lexical rend improbable qu'ils soient tous deux
    # corrompus de la même manière. Les mots courts sont écartés : articles et
    # prépositions concordent toujours et diluent le signal.
    function Mots($texte) {
        $m = $texte.ToLower() -split '[^\p{L}\p{N}]+' | Where-Object { $_.Length -gt 3 }
        , [System.Collections.Generic.HashSet[string]]::new([string[]] $m)
    }

    $texteRun = ($rsegs | ForEach-Object { $_.text }) -join ' '
    $a = Mots $texteSubs
    $b = Mots $texteRun
    $commun = [System.Collections.Generic.HashSet[string]]::new($a)
    $commun.IntersectWith($b)
    $ratio = $commun.Count / [Math]::Max(1, [Math]::Min($a.Count, $b.Count))

    Write-Host ("  Vocabulaire : {0} mots cote sous-titres, {1} cote transcription" -f $a.Count, $b.Count)
    Verifier ($ratio -gt 0.55) `
        ("recouvrement {0:P0} : les deux chemins concordent" -f $ratio) `
        ("recouvrement {0:P0} seulement : un des deux chemins est suspect" -f $ratio)
}

# ---------------------------------------------------------------- bilan ------
Section "Bilan"

if ($script:Echecs -eq 0) {
    Ok "aucune anomalie sur l'ensemble de la suite"
} else {
    Alerte "$script:Echecs anomalie(s) - voir ci-dessus"
}
Write-Host ""
Write-Host "  Artefacts : $OutDir"
Write-Host "  A transmettre en cas d'anomalie : run.json, subs.json, stderr.log"
Write-Host ""
exit $script:Echecs
