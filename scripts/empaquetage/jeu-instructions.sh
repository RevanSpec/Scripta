#!/bin/sh
# Vérifie qu'un binaire x86-64 destiné à la distribution s'en tient à l'AVX2.
#
#   scripts/empaquetage/jeu-instructions.sh <binaire>
#
# POURQUOI. ggml se compile par défaut pour le processeur de la machine de
# build (GGML_NATIVE, soit -march=native, ou sa détection sous MSVC). Or les
# machines de GitHub Actions varient d'un job à l'autre, et certaines ont
# l'AVX-512 : le binaire hérite alors d'instructions que la plupart des
# processeurs grand public n'ont pas, et meurt sur « Illegal instruction » dès
# le chargement du modèle. Le défaut est invisible sur la machine de build,
# qui les exécute sans peine (risque R14 de la roadmap).
#
# Relevé le 2026-09-28 sur un i7-13700H, sans AVX-512 : le binaire Linux
# publié de la v0.1.1 en portait 6 217, l'application Windows d'une
# répétition générale 4 838 ; les binaires sains, 26. Ces 26 appartiennent à
# crc32fast, qui ne les emploie qu'après avoir interrogé le processeur à
# l'exécution. Le seuil laisse cette marge à un code qui choisit ainsi, et à
# lui seul.
#
# Le binaire doit en revanche porter de l'AVX2 : sans lui, une build réglée
# trop bas passerait le contrôle en perdant, en silence, l'essentiel du débit.
set -eu
binaire=${1:?binaire attendu}
[ -f "$binaire" ] || { echo "[ÉCHEC] $binaire introuvable" >&2; exit 1; }

# Au plus : le code choisi à l'exécution, crc32fast aujourd'hui (26).
ZMM_MAX=200
# Au moins : les noyaux AVX2 de ggml en portent plus de dix mille.
YMM_MIN=5000

desassembler() {
    if command -v llvm-objdump >/dev/null 2>&1; then
        llvm-objdump -d --no-show-raw-insn "$1"
    elif [ -x "/c/Program Files/LLVM/bin/llvm-objdump.exe" ]; then
        "/c/Program Files/LLVM/bin/llvm-objdump.exe" -d --no-show-raw-insn "$1"
    else
        objdump -d --no-show-raw-insn "$1"
    fi
}

compte=$(desassembler "$binaire" | awk '/zmm/ {z++} /ymm/ {y++} END {print z+0, y+0}')
zmm=${compte% *}
ymm=${compte#* }
echo "  $binaire : $zmm instructions AVX-512, $ymm AVX/AVX2"

if [ "$zmm" -gt "$ZMM_MAX" ]; then
    echo "[ÉCHEC] $zmm instructions AVX-512 (au plus $ZMM_MAX) : whisper.cpp a été compilé pour le processeur de la machine de build. GGML_NATIVE=OFF manque-t-il ?" >&2
    exit 1
fi
if [ "$ymm" -lt "$YMM_MIN" ]; then
    echo "[ÉCHEC] $ymm instructions AVX/AVX2 (au moins $YMM_MIN) : les noyaux de ggml ne sont plus vectorisés." >&2
    exit 1
fi
echo "  [OK] base AVX2, sans AVX-512"
