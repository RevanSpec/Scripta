// Mise en forme des valeurs affichées.

/** Position dans une vidéo : `m:ss`, ou `h:mm:ss` au-delà de l'heure. */
export function horodatage(secondes: number): string {
  const s = Math.max(0, Math.floor(secondes));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${r}` : `${m}:${r}`;
}

/** Durée d'une vidéo : sous la minute, « 0 min » serait absurde. */
export function duree(secondes: number | null): string {
  if (secondes === null) return "durée inconnue";
  return secondes < 60 ? `${Math.round(secondes)} s` : `${Math.round(secondes / 60)} min`;
}

export function taille(octets: number): string {
  if (octets >= 1024 ** 3) return `${(octets / 1024 ** 3).toFixed(1).replace(".", ",")} Go`;
  if (octets >= 1024 ** 2) return `${Math.round(octets / 1024 ** 2)} Mo`;
  return `${Math.round(octets / 1024)} Ko`;
}

const noms = (() => {
  try {
    return new Intl.DisplayNames(["fr"], { type: "language" });
  } catch {
    return null;
  }
})();

const capitale = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/**
 * Nom d'une langue en français. À défaut — les données d'Intl embarquées par
 * le webview ne couvrent pas toutes les langues de Whisper —, le nom anglais
 * s'il est connu, sinon le code.
 */
export function langue(code: string, nomAnglais?: string): string {
  try {
    const nom = noms?.of(code);
    if (nom && nom !== code) return capitale(nom);
  } catch {
    // Code inconnu d'Intl : repli ci-dessous.
  }
  return nomAnglais ? capitale(nomAnglais) : code;
}
