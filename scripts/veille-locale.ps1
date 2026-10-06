<#
.SYNOPSIS
    Veille de l'extraction YouTube, depuis ce poste — tâche 4.7.

.DESCRIPTION
    YouTube refuse les machines de GitHub Actions par une vérification
    anti-robot (code 12) : la veille du workflow `veille.yml` y reste non
    concluante. Depuis une connexion résidentielle, elle conclut.

    Ce script transcrit la vidéo de référence avec la CLI installée, contrôle
    le texte obtenu, puis, par `gh`, tient l'issue « veille » du dépôt : il
    l'ouvre ou la complète en cas d'échec, la referme en cas de succès. Un
    refus anti-robot reste non concluant ici aussi.

    Prérequis : `scripta` et `gh`, ce dernier authentifié (`gh auth login`).
    Le modèle `tiny` est téléchargé au premier lancement.

.PARAMETER Scripta
    Chemin de la CLI. Par défaut, celle du PATH.

.PARAMETER Depot
    Dépôt GitHub où tenir l'issue.

.PARAMETER SansIssue
    Contrôle seul, sans toucher aux issues.

.EXAMPLE
    .\scripts\veille-locale.ps1 -SansIssue

.EXAMPLE
    # Chaque jour à 9 h, par le Planificateur de tâches :
    schtasks /create /tn "Scripta - veille" /sc daily /st 09:00 /tr "powershell -NoProfile -ExecutionPolicy Bypass -File C:\chemin\vers\Scripta\scripts\veille-locale.ps1"
#>

[CmdletBinding()]
param(
    [string] $Scripta = "scripta",
    [string] $Depot = "RevanSpec/Scripta",
    [switch] $SansIssue
)

# « Continue » : PowerShell 5.1 convertit chaque ligne de stderr d'un
# exécutable natif en erreur, et scripta y écrit sa progression.
$ErrorActionPreference = "Continue"
$video = "https://www.youtube.com/watch?v=jNQXAC9IVRw"
$sortie = Join-Path $env:TEMP "scripta-veille.json"
$journal = Join-Path $env:TEMP "scripta-veille.log"
$env:SCRIPTA_NO_UPDATE_CHECK = "1"

& $Scripta --no-cache -m tiny -l en -f json -o $sortie --force $video 2> $journal
$code = $LASTEXITCODE
$ytdlp = (& $Scripta doctor 2>$null | Select-String "yt-dlp\s+\S+" | Select-Object -First 1)
$ytdlp = "$ytdlp".Trim()

$etat = "echec"
if ($code -eq 12) {
    $etat = "non-concluant"
} elseif ($code -eq 0) {
    $texte = ((Get-Content $sortie -Raw -Encoding UTF8 | ConvertFrom-Json).segments | ForEach-Object { $_.text }) -join " "
    if ($texte.ToLower().Contains("elephant")) { $etat = "succes" }
}
Write-Host "Veille : $etat (code $code) — $ytdlp"
if ($etat -ne "succes") { Get-Content $journal | Select-Object -Last 15 | Write-Host }

if ($SansIssue -or $etat -eq "non-concluant") { exit $(if ($etat -eq "echec") { 1 } else { 0 }) }

$ouverte = gh issue list -R $Depot --state open --label veille --json number --jq '.[0].number // empty'
$date = Get-Date -Format "yyyy-MM-dd"
if ($etat -eq "echec") {
    gh label create veille -R $Depot --color B60205 --description "Alertes de la veille quotidienne" --force | Out-Null
    $corps = "La veille locale du $date a échoué, code $code ($ytdlp).`n`n" + ((Get-Content $journal | Select-Object -Last 15) -join "`n")
    if ($ouverte) {
        gh issue comment $ouverte -R $Depot --body $corps
    } else {
        gh issue create -R $Depot --title "Veille : l'extraction YouTube échoue" --label veille --body $corps
    }
    exit 1
} elseif ($ouverte) {
    gh issue close $ouverte -R $Depot --comment "La veille locale du $date passe de nouveau ($ytdlp)."
}
exit 0
