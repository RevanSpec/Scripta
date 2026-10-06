#!/bin/sh
# Installe le SDK Vulkan épinglé, pour compiler la variante `vulkan`.
#
#   scripts/empaquetage/sdk-vulkan.sh <triplet> <répertoire>
#
# ADR-001, tâche 4.3 : la variante Vulkan est un artefact à part, sous Windows
# et Linux. Le SDK ne sert qu'à la compilation — glslc pour les shaders de
# ggml, la bibliothèque du chargeur pour l'édition de liens. Le fichier
# téléchargé doit porter l'empreinte épinglée dans vulkan.env, sans quoi il
# est supprimé.
#
# Sous GitHub Actions, VULKAN_SDK et le PATH sont transmis aux étapes
# suivantes ; ailleurs, le script affiche ce qu'il faut exporter.
set -eu
ici=$(cd "$(dirname "$0")" && pwd)
. "$ici/../sidecars/lib.sh"
. "$ici/vulkan.env"

triplet=${1:?triplet attendu}
dest=${2:?répertoire attendu}
mkdir -p "$dest"
dest=$(cd "$dest" && pwd)

case "$triplet" in
    x86_64-pc-windows-msvc)
        v=$VULKAN_SDK_VERSION_WINDOWS
        f=vulkansdk-windows-X64-$v.exe
        telecharger "https://sdk.lunarg.com/sdk/download/$v/windows/$f" "$dest/$f" "$VULKAN_SDK_SHA256_WINDOWS"
        # Installeur Qt, sans interface. Chemin Windows : c'est lui qui écrit.
        "$dest/$f" --root "$(cygpath -w "$dest/sdk")" --accept-licenses \
            --default-answer --confirm-command install
        rm -f "$dest/$f"
        # CMake et rustc lisent VULKAN_SDK : chemin Windows à barres obliques.
        sdk=$(cygpath -m "$dest/sdk")
        bin=$(cygpath -w "$dest/sdk/Bin")
        glslc="$dest/sdk/Bin/glslc.exe"
        ;;
    x86_64-unknown-linux-gnu)
        v=$VULKAN_SDK_VERSION_LINUX
        f=vulkansdk-linux-x86_64-$v.tar.xz
        telecharger "https://sdk.lunarg.com/sdk/download/$v/linux/$f" "$dest/$f" "$VULKAN_SDK_SHA256_LINUX"
        tar -xJf "$dest/$f" -C "$dest"
        rm -f "$dest/$f"
        sdk="$dest/$v/x86_64"
        bin="$sdk/bin"
        glslc="$bin/glslc"
        ;;
    *)
        echo "[ÉCHEC] pas de variante Vulkan pour $triplet (ADR-001)" >&2
        exit 1
        ;;
esac

[ -x "$glslc" ] || { echo "[ÉCHEC] glslc absent du SDK : $glslc" >&2; exit 1; }
echo "  SDK Vulkan $v : $sdk"
echo "  $("$glslc" --version | head -1)"

if [ -n "${GITHUB_ENV:-}" ]; then
    echo "VULKAN_SDK=$sdk" >> "$GITHUB_ENV"
    echo "$bin" >> "$GITHUB_PATH"
else
    echo "  export VULKAN_SDK=\"$sdk\" PATH=\"$bin:\$PATH\""
fi
