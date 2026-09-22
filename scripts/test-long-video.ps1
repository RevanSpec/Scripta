<#
.SYNOPSIS
    Test d'acceptation sur vidéo longue — mémoire, débit et hallucinations.

.DESCRIPTION
    Dernier critère de sortie du Jalon 1 encore ouvert : la tenue en mémoire sur
    une vidéo d'une heure. L'ADR-003 prédit environ 230 Mo de PCM par heure
    d'audio ; ce script mesure la crête réelle et confronte les deux.

    Il vérifie les prérequis, compile en --release si nécessaire, télécharge le
    modèle s'il manque, exécute la transcription en échantillonnant la mémoire,
    puis analyse le JSON produit.

.PARAMETER Url
    URL de la vidéo. Par défaut la vidéo de référence (60 min 57 s, français).

.PARAMETER Model
    Modèle à employer. `base` par défaut : bon compromis sur CPU.

.PARAMETER OutDir
    Répertoire des artefacts. Par défaut un sous-dossier horodaté de $env:TEMP.

.PARAMETER Profile
    `release` (défaut) ou `debug`.

    Le script applique le contournement du risque R9 : sans lui, la build
    `--release` embarque sous MSVC un whisper.cpp non optimisé, quatre à six
    fois plus lent que la build de débogage. Voir docs/ROADMAP.md.

.EXAMPLE
    .\scripts\test-long-video.ps1

.EXAMPLE
    .\scripts\test-long-video.ps1 -Model small -Url "https://www.youtube.com/watch?v=..."
#>

[CmdletBinding()]
param(
    [string] $Url = "https://www.youtube.com/watch?v=cZwuhte5ZBI",
    [string] $Model = "base",
    [ValidateSet("debug", "release")] [string] $Profile = "release",
    [string] $OutDir = (Join-Path $env:TEMP ("scripta-test-" + (Get-Date -Format "yyyyMMdd-HHmmss")))
)

$ErrorActionPreference = "Stop"

function Section($titre) {
    Write-Host ""
    Write-Host "=== $titre ===" -ForegroundColor Cyan
}

# Le script vit dans scripts/ ; la racine du dépôt est au-dessus.
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

# ---------------------------------------------------------------- prérequis --
Section "Prérequis"

foreach ($outil in @("cargo", "yt-dlp", "ffmpeg")) {
    $cmd = Get-Command $outil -ErrorAction SilentlyContinue
    if (-not $cmd) {
        Write-Host "  $outil : ABSENT" -ForegroundColor Red
        throw "$outil est requis. Voir README.md, section Installation."
    }
    Write-Host "  $outil : $($cmd.Source)"
}

# ------------------------------------------------------------------- build ---
Section "Binaire"

# Contournement du risque R9 — à poser AVANT tout appel à cargo. La crate
# `cmake` écrase CMAKE_CXX_FLAGS_<BUILD_TYPE>, or le générateur Visual Studio
# compile toujours en --config Release : sans ces variables, le profil release
# perd son /O2 et whisper.cpp tourne quatre à six fois plus lentement. Le
# build.rs de whisper-rs-sys réinjecte toute variable CMAKE_* en define, et
# ces defines-là gagnent.
if ($env:OS -eq "Windows_NT") {
    $env:CMAKE_C_FLAGS_RELEASE   = "/MD /O2 /Ob2 /DNDEBUG"
    $env:CMAKE_CXX_FLAGS_RELEASE = "/MD /O2 /Ob2 /DNDEBUG"
}

$Exe = Join-Path $Root "target\$Profile\scripta.exe"
if (-not (Test-Path $Exe)) {
    Write-Host "  Compilation en $Profile (plusieurs minutes, whisper.cpp est bati depuis ses sources)..."
    if ($Profile -eq "release") { cargo build --release --workspace } else { cargo build --workspace }
    if ($LASTEXITCODE -ne 0) { throw "La compilation a échoué." }
}
Write-Host "  $Exe"
if ($Profile -eq "debug") {
    Write-Host "  (profil debug : les mesures de debit ne sont pas comparables au SPEC 5.2)" -ForegroundColor Yellow
}


# ------------------------------------------------------------------ modèle ---
Section "Modèle"

if (-not $env:SCRIPTA_MODELS_DIR) {
    $env:SCRIPTA_MODELS_DIR = Join-Path $HOME ".cache\scripta\models"
}
New-Item -ItemType Directory -Force $env:SCRIPTA_MODELS_DIR | Out-Null

$ModelPath = Join-Path $env:SCRIPTA_MODELS_DIR "ggml-$Model.bin"
if (-not (Test-Path $ModelPath)) {
    # Le dépôt est bien ggerganov : ggml-org renvoie HTTP 401 en anonyme.
    $uri = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$Model.bin"
    Write-Host "  Téléchargement de ggml-$Model.bin…"
    $ancienne = $ProgressPreference
    $ProgressPreference = "SilentlyContinue"   # sinon Invoke-WebRequest rampe
    try { Invoke-WebRequest -Uri $uri -OutFile $ModelPath } finally { $ProgressPreference = $ancienne }
}
Write-Host ("  {0} ({1:N0} Mo)" -f $ModelPath, ((Get-Item $ModelPath).Length / 1MB))

# ------------------------------------------------------------------ sonde ----
Section "Vidéo"

$meta = (& yt-dlp -J --no-warnings --no-playlist -- $Url | ConvertFrom-Json)
$dureeAudio = [double] $meta.duration
$pcmMo = $dureeAudio * 16000 * 4 / 1MB

Write-Host ("  {0}" -f $meta.title)
Write-Host ("  Durée            : {0:N0} s = {1:N1} min" -f $dureeAudio, ($dureeAudio / 60))
Write-Host ("  PCM f32 attendu  : {0:N1} Mo" -f $pcmMo)
Write-Host ("  Crête prévue     : {0:N0} Mo (PCM + modèle + état)" -f ($pcmMo + ((Get-Item $ModelPath).Length / 1MB) + 100))

# ---------------------------------------------------------------- exécution --
Section "Transcription"

New-Item -ItemType Directory -Force $OutDir | Out-Null
$Json = Join-Path $OutDir "out.json"
$ErrLog = Join-Path $OutDir "stderr.log"

Write-Host "  Artefacts : $OutDir"
Write-Host "  En cours… (Ctrl-C interrompt proprement, et c'est aussi un test)"

$cliArgs = @("-m", $Model, "-f", "json", "-o", $Json, "--", $Url)
$p = Start-Process -FilePath $Exe -ArgumentList $cliArgs -PassThru -NoNewWindow `
                   -RedirectStandardError $ErrLog

$peak = 0L
$debut = Get-Date
while (-not $p.HasExited) {
    try {
        $p.Refresh()
        if ($p.PeakWorkingSet64 -gt $peak) { $peak = $p.PeakWorkingSet64 }
    } catch { }   # le processus peut disparaître entre Refresh et lecture
    Start-Sleep -Milliseconds 500
}
$ecoule = (Get-Date) - $debut
$code = $p.ExitCode

# ---------------------------------------------------------------- résultats --
Section "Résultats"

Write-Host ("  Code de sortie   : {0}" -f $code)
Write-Host ("  Durée totale     : {0:N1} s" -f $ecoule.TotalSeconds)
Write-Host ("  Mémoire crête    : {0:N0} Mo" -f ($peak / 1MB))

if ($code -ne 0) {
    Write-Host ""
    Write-Host "  ÉCHEC — stderr :" -ForegroundColor Red
    Get-Content $ErrLog -Tail 20 | ForEach-Object { Write-Host "    $_" }
    throw "Transcription en échec (code $code). Table des codes : README.md."
}

$d = Get-Content $Json -Raw -Encoding UTF8 | ConvertFrom-Json
# ConvertFrom-Json déballe un tableau d'un seul élément : @() force le tableau.
$segments = @($d.segments)

Write-Host ""
Write-Host ("  Langue détectée  : {0}" -f $d.transcription.language)
Write-Host ("  Modèle / backend : {0} / {1}" -f $d.transcription.model, $d.transcription.backend)
Write-Host ("  Inférence        : {0:N1} s" -f ($d.transcription.duration_ms / 1000))
Write-Host ("  Vitesse          : {0}x temps réel" -f $d.transcription.speed_realtime)
Write-Host ("  Segments         : {0}" -f $segments.Count)

# --------------------------------------------------------------- verdicts ----
Section "Verdicts"

# 1. Mémoire — c'est la prédiction de l'ADR-003 qui est jugée ici.
$moParHeure = ($peak / 1MB) / ($dureeAudio / 3600)
Write-Host ("  Mémoire par heure d'audio : {0:N0} Mo (PCM seul prédit : 230 Mo)" -f $moParHeure)
if ($moParHeure -lt 800) {
    Write-Host "  [OK]   Empreinte conforme à l'ordre de grandeur attendu." -ForegroundColor Green
} else {
    Write-Host "  [ALERTE] Empreinte très supérieure : fuite probable." -ForegroundColor Red
}

# 2. Couverture — des segments qui s'arrêtent à mi-parcours trahissent un flux
#    tronqué, que le code de sortie 0 ne révèle pas.
$couvert = if ($segments.Count -gt 0) { [double] $segments[-1].end } else { 0 }
$ratio = if ($dureeAudio) { $couvert / $dureeAudio } else { 0 }
Write-Host ("  Couverture                : {0:P1} ({1:N0} s sur {2:N0} s)" -f $ratio, $couvert, $dureeAudio)
if ($ratio -gt 0.95) {
    Write-Host "  [OK]   La transcription couvre toute la vidéo." -ForegroundColor Green
} else {
    Write-Host "  [ALERTE] Transcription tronquée." -ForegroundColor Red
}

# 3. Hallucinations — Whisper répète une phrase en boucle sur les silences et
#    la musique. C'est le défaut que le VAD Silero corrige (SF-04).
$repets = $segments | Group-Object text | Where-Object { $_.Count -gt 3 } | Sort-Object Count -Descending
if ($repets) {
    Write-Host "  [ATTENTION] Répétitions suspectes :" -ForegroundColor Yellow
    $repets | Select-Object -First 5 | ForEach-Object {
        $t = $_.Name; if ($t.Length -gt 60) { $t = $t.Substring(0, 60) + "…" }
        Write-Host ("    {0}x  {1}" -f $_.Count, $t)
    }
} else {
    Write-Host "  [OK]   Aucune répétition en boucle détectée." -ForegroundColor Green
}

Section "À transmettre en cas d'anomalie"
Write-Host "  $Json"
Write-Host "  $ErrLog"
Write-Host "  La sortie de : $Exe doctor"
Write-Host ""
