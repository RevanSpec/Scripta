#!/bin/sh
# Test de fumée d'un ffmpeg minimal : chaque échantillon de make-samples.sh
# doit se décoder par la commande même de crates/core/src/audio.rs.
#
#   scripts/sidecars/smoke-ffmpeg.sh <ffmpeg> <échantillons>
set -eu
ffmpeg=${1:?binaire ffmpeg attendu}
echantillons=${2:?répertoire des échantillons attendu}

echecs=0
for f in "$echantillons"/*; do
    octets=$("$ffmpeg" -hide_banner -loglevel error -nostdin -i pipe:0 -vn \
        -ar 16000 -ac 1 -c:a pcm_s16le -f s16le pipe:1 <"$f" | wc -c | tr -d ' ')
    # Trois secondes à 16 kHz, 16 bits, mono : 96 000 octets. Chaque codec
    # ajoute ou retranche quelques millisecondes de bourrage.
    if [ "$octets" -ge 90000 ] && [ "$octets" -le 102000 ]; then
        echo "  [OK]    $(basename "$f") : $octets octets"
    else
        echo "  [ÉCHEC] $(basename "$f") : $octets octets, 96 000 attendus" >&2
        echecs=$((echecs + 1))
    fi
done
[ "$echecs" -eq 0 ]
