#!/bin/sh
# Récupère yt-dlp pour une cible — tâche 4.1 de la roadmap.
#
#   scripts/sidecars/fetch-ytdlp.sh <triplet> <répertoire>
#
# Dépose `scripta-yt-dlp-<triplet>[.exe]` — le nom qu'attend `externalBin` —,
# et ses notices de licence sous `licences/yt-dlp/`. Chaque fichier est
# vérifié contre l'empreinte épinglée dans versions.env.
set -eu
ici=$(cd "$(dirname "$0")" && pwd)
. "$ici/versions.env"
. "$ici/lib.sh"

cible=${1:?triplet cible attendu}
dest=${2:?répertoire de destination attendu}

case "$cible" in
    x86_64-pc-windows-msvc) actif=yt-dlp.exe attendue=$YTDLP_SHA256_WINDOWS ;;
    x86_64-unknown-linux-gnu) actif=yt-dlp_linux attendue=$YTDLP_SHA256_LINUX ;;
    aarch64-apple-darwin) actif=yt-dlp_macos attendue=$YTDLP_SHA256_MACOS ;;
    *)
        echo "Cible non prise en charge : $cible" >&2
        exit 2
        ;;
esac

mkdir -p "$dest/licences/yt-dlp"
binaire="$dest/scripta-yt-dlp-$cible$(suffixe_exe "$cible")"
telecharger "https://github.com/yt-dlp/yt-dlp/releases/download/$YTDLP_VERSION/$actif" \
    "$binaire" "$attendue"
chmod +x "$binaire"

sources="https://raw.githubusercontent.com/yt-dlp/yt-dlp/$YTDLP_VERSION"
telecharger "$sources/LICENSE" "$dest/licences/yt-dlp/LICENSE" "$YTDLP_LICENSE_SHA256"
telecharger "$sources/THIRD_PARTY_LICENSES.txt" \
    "$dest/licences/yt-dlp/THIRD_PARTY_LICENSES.txt" "$YTDLP_THIRD_PARTY_SHA256"

echo "yt-dlp $YTDLP_VERSION : $binaire"
