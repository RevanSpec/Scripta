#!/bin/sh
# Retire de l'AppImage la copie embarquée de libwayland-client.
#
#   scripts/empaquetage/appimage-sans-wayland-client.sh <répertoire du bundle>
#
# POURQUOI. linuxdeploy embarque libwayland-client.so.0 telle qu'elle est sur
# la machine de build — Ubuntu 22.04, donc wayland 1.20. L'AppRun place
# $APPDIR/usr/lib en tête du chemin de recherche : sur un hôte plus récent,
# cette copie masque celle du système. Or libEGL_mesa.so.0 en dépend ; liée à
# la 1.20, son initialisation échoue, eglGetDisplay rend EGL_BAD_PARAMETER, et
# le processus de rendu de WebKit s'arrête aussitôt. La fenêtre s'ouvre malgré
# tout, vide, sans le moindre message : la panne la plus déroutante qui soit.
#
# Mesuré le 2026-09-25 sous Ubuntu 26.04 (Mesa 26.0.8, wayland 1.24), par
# bissection sur les 169 bibliothèques embarquées : retirer ce seul fichier
# rétablit EGL. Les trois autres libwayland restent dans le paquet — aucune
# n'est sur le chemin d'EGL, et libwayland-server manque sur un système sans
# compositeur, où la retirer empêche l'application de démarrer.
set -eu
bundle=${1:?répertoire du bundle attendu}

appimage=$(find "$bundle" -maxdepth 1 -type f -name '*.AppImage' | head -1)
[ -n "$appimage" ] || { echo "[ÉCHEC] aucune AppImage dans $bundle" >&2; exit 1; }

travail=$(mktemp -d)
trap 'rm -rf "$travail"' EXIT

chmod +x "$appimage"
decalage=$("$appimage" --appimage-offset)
# La compression est relue sur le paquet plutôt que fixée ici : la réassembler
# autrement qu'à l'identique donnerait une AppImage que le runtime livré avec
# elle ne saurait pas monter.
compression=$(unsquashfs -s -o "$decalage" "$appimage" | awk '/^Compression/ {print $2}')
echo "  AppImage    : $appimage ($(stat -c %s "$appimage") octets)"
echo "  runtime     : $decalage octets, charge en $compression"

unsquashfs -q -d "$travail/racine" -o "$decalage" "$appimage" >/dev/null

# Garde-fou. Si la bibliothèque n'est plus embarquée, c'est que le bundler a
# changé ; il vaut mieux casser la construction que retirer silencieusement
# rien du tout et croire le défaut corrigé.
if [ ! -e "$travail/racine/usr/lib/libwayland-client.so.0" ]; then
    echo "[ÉCHEC] libwayland-client.so.0 n'est plus embarquée : cette étape n'a plus lieu d'être." >&2
    exit 1
fi
rm -f "$travail/racine/usr/lib/libwayland-client.so.0"

head -c "$decalage" "$appimage" >"$travail/runtime.bin"
mksquashfs "$travail/racine" "$travail/charge.sqfs" \
    -root-owned -noappend -comp "$compression" -no-progress -quiet
cat "$travail/runtime.bin" "$travail/charge.sqfs" >"$travail/corrigee.AppImage"
chmod +x "$travail/corrigee.AppImage"

# Contrôle sur le produit fini, et non sur l'arborescence intermédiaire : c'est
# le fichier livré qui doit se monter et avoir perdu la bonne bibliothèque.
( cd "$travail" && ./corrigee.AppImage --appimage-extract 'usr/lib/libwayland-*' >/dev/null )
restantes=$(ls "$travail/squashfs-root/usr/lib" 2>/dev/null | tr '\n' ' ')
case "$restantes" in
    *libwayland-client.so.0*)
        echo "[ÉCHEC] libwayland-client.so.0 est encore dans le paquet réassemblé." >&2
        exit 1 ;;
esac
for attendue in libwayland-cursor.so.0 libwayland-egl.so.1 libwayland-server.so.0; do
    case "$restantes" in
        *"$attendue"*) ;;
        *) echo "[ÉCHEC] $attendue a disparu du paquet réassemblé." >&2; exit 1 ;;
    esac
done

mv "$travail/corrigee.AppImage" "$appimage"
echo "  réassemblée : $(stat -c %s "$appimage") octets"
echo "  conservées  : $restantes"
echo "[OK] libwayland-client.so.0 retirée."
