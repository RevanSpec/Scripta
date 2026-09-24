# Fonctions communes aux scripts des sidecars. À sourcer, pas à exécuter.

# Empreinte SHA-256 d'un fichier, par l'outil que la plateforme fournit.
empreinte() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# Télécharge $1 dans $2 et exige l'empreinte $3. Un fichier qui ne la porte
# pas est supprimé : il ne doit jamais atteindre un bundle.
telecharger() {
    url=$1
    fichier=$2
    attendue=$3
    curl --fail --silent --show-error --location --retry 3 --output "$fichier.part" "$url"
    obtenue=$(empreinte "$fichier.part")
    if [ "$obtenue" != "$attendue" ]; then
        rm -f "$fichier.part"
        echo "Empreinte inattendue pour $url" >&2
        echo "  attendue : $attendue" >&2
        echo "  obtenue  : $obtenue" >&2
        exit 1
    fi
    mv "$fichier.part" "$fichier"
}

# Suffixe des exécutables d'une cible.
suffixe_exe() {
    case "$1" in
        *-windows-*) printf '.exe' ;;
        *) printf '' ;;
    esac
}
