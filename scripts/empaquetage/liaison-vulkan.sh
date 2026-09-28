#!/bin/sh
# Vérifie qu'un binaire de la variante Vulkan se lie bien au chargeur Vulkan.
#
#   scripts/empaquetage/liaison-vulkan.sh <binaire>
#
# Une feature mal relayée donnerait une seconde build CPU, publiée sous le nom
# de la variante accélérée et plusieurs fois plus lente qu'annoncé. La
# liaison au chargeur — vulkan-1.dll, libvulkan.so.1 — en est la preuve la
# plus simple, et ne demande aucun GPU.
set -eu
binaire=${1:?binaire attendu}

if [ -x "/c/Program Files/LLVM/bin/llvm-objdump.exe" ]; then
    deps=$("/c/Program Files/LLVM/bin/llvm-objdump.exe" -p "$binaire" | sed -n 's/.*DLL Name: //p')
else
    deps=$(objdump -p "$binaire" | awk '/NEEDED/ {print $2}')
fi

if echo "$deps" | grep -qiE '^(vulkan-1\.dll|libvulkan\.so\.1)$'; then
    echo "  [OK] $binaire se lie au chargeur Vulkan"
else
    echo "[ÉCHEC] $binaire ne se lie pas au chargeur Vulkan : feature non relayée ?" >&2
    echo "$deps" | sed 's/^/    /' >&2
    exit 1
fi
