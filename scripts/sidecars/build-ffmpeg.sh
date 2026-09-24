#!/bin/sh
# Compile un ffmpeg minimal pour une cible — tâches 4.1 et 4.2 de la roadmap.
#
#   scripts/sidecars/build-ffmpeg.sh <triplet> <répertoire>
#
# Scripta ne demande à ffmpeg qu'une chose : décoder la piste audio que yt-dlp
# lui passe — Opus ou Vorbis en WebM, AAC en MP4, MP3 — et la rendre en PCM
# 16 bits, 16 kHz, mono (crates/core/src/audio.rs). Tout le reste est retiré :
# quelques mégaoctets au lieu de soixante-dix, aucune bibliothèque externe, et
# une build sous LGPL, sans composant GPL ni non libre.
#
# Linux et macOS se compilent sur leur propre système ; Windows depuis Linux,
# par mingw-w64. Les binaires sont statiques.
#
# SCRIPTA_FFMPEG_SOURCES, s'il est défini, reçoit une copie de l'archive
# vérifiée : la LGPL impose d'en distribuer les sources avec le binaire.
set -eu
ici=$(cd "$(dirname "$0")" && pwd)
. "$ici/versions.env"
. "$ici/lib.sh"

cible=${1:?triplet cible attendu}
mkdir -p "${2:?répertoire de destination attendu}"
dest=$(cd "$2" && pwd)

case "$cible" in
    x86_64-unknown-linux-gnu)
        plateforme="--extra-ldflags=-static"
        ;;
    x86_64-pc-windows-msvc)
        plateforme="--arch=x86_64 --target-os=mingw32 --cross-prefix=x86_64-w64-mingw32- --extra-ldflags=-static"
        ;;
    aarch64-apple-darwin)
        plateforme="--arch=arm64 --extra-cflags=-mmacosx-version-min=11.0 --extra-ldflags=-mmacosx-version-min=11.0"
        ;;
    *)
        echo "Cible non prise en charge : $cible" >&2
        exit 2
        ;;
esac

# Exactement ce qu'exige la commande de crates/core/src/audio.rs :
#   ffmpeg -nostdin -i pipe:0 -vn -ar 16000 -ac 1 -c:a pcm_s16le -f s16le pipe:1
# L'assembleur x86 est écarté : il exigerait nasm, pour un décodage audio qui
# prend quelques secondes par heure.
composants="--disable-everything --disable-autodetect --disable-network"
composants="$composants --disable-doc --disable-debug --enable-small --disable-x86asm"
composants="$composants --disable-ffplay --disable-ffprobe --disable-avdevice --disable-swscale"
composants="$composants --enable-protocol=pipe,file"
composants="$composants --enable-demuxer=matroska,mov,mp3,ogg,aac,wav"
composants="$composants --enable-parser=opus,vorbis,aac,mpegaudio"
composants="$composants --enable-decoder=opus,vorbis,aac,mp3float,mp3,pcm_s16le"
composants="$composants --enable-encoder=pcm_s16le --enable-muxer=pcm_s16le"
composants="$composants --enable-filter=aresample,aformat,anull --enable-swresample"

travail=$(mktemp -d)
trap 'rm -rf "$travail"' EXIT
archive="$travail/ffmpeg-$FFMPEG_VERSION.tar.xz"
telecharger "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz" "$archive" "$FFMPEG_SHA256"
if [ -n "${SCRIPTA_FFMPEG_SOURCES:-}" ]; then
    mkdir -p "$SCRIPTA_FFMPEG_SOURCES"
    cp "$archive" "$SCRIPTA_FFMPEG_SOURCES/"
fi
tar -xJf "$archive" -C "$travail"
cd "$travail/ffmpeg-$FFMPEG_VERSION"

# shellcheck disable=SC2086 # listes d'options, à découper
./configure $composants $plateforme
make -j"$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu)"

exe=$(suffixe_exe "$cible")
case "$cible" in
    *-windows-*) x86_64-w64-mingw32-strip "ffmpeg$exe" ;;
    *) strip "ffmpeg$exe" ;;
esac
cp "ffmpeg$exe" "$dest/scripta-ffmpeg-$cible$exe"

# Obligations de la LGPL : le texte de la licence, et de quoi reconstruire
# exactement ce binaire — version, empreinte de l'archive, options.
mkdir -p "$dest/licences/ffmpeg"
cp COPYING.LGPLv2.1 "$dest/licences/ffmpeg/"
cat >"$dest/licences/ffmpeg/CONSTRUCTION.txt" <<FIN
FFmpeg $FFMPEG_VERSION, sous licence LGPL 2.1 ou ultérieure.

Sources : https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz
SHA-256 : $FFMPEG_SHA256
L'archive est aussi jointe à chaque release de Scripta.

Configuration :
./configure $composants $plateforme

Script de construction : scripts/sidecars/build-ffmpeg.sh, dans le dépôt de
Scripta (https://github.com/RevanSpec/Scripta).
FIN

echo "ffmpeg $FFMPEG_VERSION : $dest/scripta-ffmpeg-$cible$exe ($(wc -c <"$dest/scripta-ffmpeg-$cible$exe" | tr -d ' ') octets)"
