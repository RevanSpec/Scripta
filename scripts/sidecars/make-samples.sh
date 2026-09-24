#!/bin/sh
# Échantillons du test de fumée de ffmpeg (smoke-ffmpeg.sh), produits par un
# ffmpeg complet — celui du système.
#
#   scripts/sidecars/make-samples.sh <répertoire>
#
# Trois secondes de sinusoïde dans chaque forme que yt-dlp peut livrer à
# ffmpeg. Les MP4 sont fragmentés, comme les pistes DASH de YouTube : un MP4
# ordinaire range son index en fin de fichier, illisible depuis un tube.
set -eu
dest=${1:?répertoire de destination attendu}
mkdir -p "$dest"

son="-f lavfi -i sine=frequency=440:duration=3"
fragmente="-movflags +frag_keyframe+empty_moov"
faire() {
    # shellcheck disable=SC2086 # listes d'options, à découper
    ffmpeg -hide_banner -loglevel error -nostdin -y "$@"
}

faire $son -c:a libopus "$dest/opus.webm"
faire $son -c:a libvorbis "$dest/vorbis.webm"
faire $son -c:a aac $fragmente "$dest/aac.m4a"
faire $son -c:a libmp3lame "$dest/mp3.mp3"
# Repli « best » de yt-dlp : un MP4 avec image. `-vn` doit l'ignorer, sans
# décodeur vidéo.
faire -f lavfi -i testsrc=duration=3:size=64x64:rate=10 $son \
    -c:v libx264 -c:a aac -shortest $fragmente "$dest/video.mp4"

ls -l "$dest"
