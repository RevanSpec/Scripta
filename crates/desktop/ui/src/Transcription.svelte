<script lang="ts">
  import { tick } from "svelte";
  import Erreur from "./Erreur.svelte";
  import {
    cancel, copyText, enErreur, exportAs, probeUrl, transcribe, CODE_ANNULE,
    type BackendInfo, type IpcError, type Message, type ModelsInfo, type Segment,
    type Transcription, type VideoInfo,
  } from "./ipc";
  import { duree, horodatage, langue, taille } from "./format";

  interface Props {
    info: BackendInfo | null;
    modeles: ModelsInfo | null;
    occupe: boolean;
  }
  let { info, modeles, occupe = $bindable() }: Props = $props();

  // ---------------------------------------------------------------- saisie --
  let url = $state("");
  let modele = $state("auto");
  let lang = $state("");
  let sousTitres = $state(false);
  let avancees = $state(false);
  let vad = $state(true);
  let traduire = $state(false);
  let mots = $state(false);
  let contexte = $state("");
  let navigateur = $state("");

  const NAVIGATEURS = ["firefox", "chrome", "edge", "brave", "opera", "vivaldi", "chromium", "safari"];

  const langues = $derived(
    (info?.languages ?? [])
      .map((l) => ({ code: l.code, nom: langue(l.code, l.name) }))
      .sort((a, b) => a.nom.localeCompare(b.nom, "fr")),
  );

  /** Modèle effectivement employé : `auto` désigne l'un des autres. */
  const modeleEffectif = $derived(
    modeles?.whisper.find((m) => m.alias === (modele === "auto" ? modeles?.auto : modele)) ?? null,
  );
  const traductionPossible = $derived(modeleEffectif?.translates ?? true);
  $effect(() => {
    if (!traductionPossible) traduire = false;
  });

  // ----------------------------------------------------------------- sonde --
  let video = $state<VideoInfo | null>(null);
  let erreurSonde = $state<IpcError | null>(null);
  let sondage = $state(false);
  let minuterie: ReturnType<typeof setTimeout> | undefined;
  let numeroSonde = 0;

  /** Sonde l'URL après une courte pause de saisie ; seule la dernière compte. */
  function planifierSonde() {
    clearTimeout(minuterie);
    video = null;
    erreurSonde = null;
    if (!url.trim()) return;
    minuterie = setTimeout(sonder, 500);
  }

  async function sonder() {
    const numero = ++numeroSonde;
    sondage = true;
    try {
      const v = await probeUrl(url.trim(), navigateur || null);
      if (numero === numeroSonde) video = v;
    } catch (e) {
      if (numero === numeroSonde) erreurSonde = enErreur(e);
    } finally {
      if (numero === numeroSonde) sondage = false;
    }
  }

  // ---------------------------------------------------------- déroulement --
  let etape = $state<string | null>(null);
  let pourcent = $state<number | null>(null);
  let telechargement = $state<{ item: string; received: number; total: number } | null>(null);
  let dureeMedia = $state<number | null>(null);
  let debutInference = $state<number | null>(null);
  let maintenant = $state(Date.now());
  let annulation = $state(false);
  let notices = $state<string[]>([]);
  let segments = $state<Segment[]>([]);
  let resultat = $state<Transcription | null>(null);
  let erreur = $state<IpcError | null>(null);
  /** Information passagère : annulation, fichier écrit, texte copié. */
  let annonce = $state<string | null>(null);

  const position = $derived(segments.length ? segments[segments.length - 1].end : null);
  const vitesse = $derived(
    position !== null && debutInference !== null && maintenant - debutInference >= 1000
      ? position / ((maintenant - debutInference) / 1000)
      : null,
  );

  const ETAPES: Record<string, string> = {
    cache: "Trouvée en cache",
    loading: "Chargement du modèle",
    probe: "Lecture des métadonnées",
    extraction: "Extraction de l'audio",
  };

  function recevoir(m: Message) {
    switch (m.type) {
      case "phase":
        etape = ETAPES[m.phase];
        telechargement = null;
        break;
      case "download":
        etape = `Téléchargement du modèle ${m.item}`;
        telechargement = { item: m.item, received: m.received, total: m.total };
        break;
      case "video":
        if (!video) {
          video = {
            videoId: "", title: m.title, channel: m.channel, durationS: m.durationS,
            language: null, subtitles: [], autoCaptions: null,
          };
        }
        break;
      case "subtitles":
        etape = `Sous-titres ${m.auto ? "auto-générés" : "officiels"} (${langue(m.lang)})`;
        if (m.auto) notices.push("Piste auto-générée : souvent sans ponctuation, et en deçà de Whisper.");
        if (m.translation) notices.push("Traduction automatique de YouTube, non la langue d'origine.");
        break;
      case "notice":
        notices.push(m.message);
        break;
      case "transcribing":
        etape = "Transcription";
        telechargement = null;
        dureeMedia = m.mediaS;
        debutInference = Date.now();
        pourcent = 0;
        break;
      case "progress":
        pourcent = Math.max(pourcent ?? 0, m.percent);
        break;
      case "segments":
        segments.push(...m.segments);
        suivreSiBesoin();
        break;
    }
  }

  async function lancer() {
    if (!url.trim() || occupe) return;
    occupe = true;
    annulation = false;
    erreur = null;
    annonce = null;
    resultat = null;
    segments = [];
    notices = [];
    etape = "Préparation";
    pourcent = null;
    telechargement = null;
    dureeMedia = null;
    debutInference = null;
    suivre = true;
    const horloge = setInterval(() => (maintenant = Date.now()), 500);
    try {
      resultat = await transcribe(
        {
          url: url.trim(),
          model: modele,
          lang: lang || null,
          translate: traduire,
          initialPrompt: contexte.trim() || null,
          wordTimestamps: mots,
          vad,
          preferSubs: sousTitres,
          cookiesFromBrowser: navigateur || null,
        },
        recevoir,
      );
      // Le résultat fait foi : les segments reçus en cours de route n'étaient
      // qu'un aperçu.
      segments = resultat.segments;
      suivreSiBesoin();
    } catch (e) {
      const err = enErreur(e);
      if (err.code === CODE_ANNULE) {
        annonce = "Transcription annulée.";
      } else {
        erreur = err;
      }
    } finally {
      clearInterval(horloge);
      occupe = false;
      etape = null;
    }
  }

  async function annuler() {
    annulation = true;
    await cancel();
  }

  // ------------------------------------------------------------- défilement --
  // La zone suit les segments, sauf si l'utilisateur est remonté les lire
  // (SPEC §4.2) ; revenir en bas rétablit le suivi.
  let zone = $state<HTMLElement | null>(null);
  let suivre = $state(true);

  function surDefilement() {
    if (!zone) return;
    suivre = zone.scrollTop + zone.clientHeight >= zone.scrollHeight - 24;
  }

  async function suivreSiBesoin() {
    if (!suivre) return;
    await tick();
    if (zone) zone.scrollTop = zone.scrollHeight;
  }

  function reprendreSuivi() {
    suivre = true;
    suivreSiBesoin();
  }

  // ----------------------------------------------------------------- export --
  async function exporter(format: string) {
    annonce = null;
    try {
      const chemin = await exportAs(format);
      if (chemin) annonce = `Enregistré : ${chemin}`;
    } catch (e) {
      erreur = enErreur(e);
    }
  }

  async function copier() {
    try {
      await copyText();
      annonce = "Texte copié dans le presse-papiers.";
    } catch (e) {
      erreur = enErreur(e);
    }
  }

  const resume = $derived.by(() => {
    if (!resultat) return "";
    const morceaux: string[] = [];
    switch (resultat.origin) {
      case "cache": morceaux.push("Trouvée en cache"); break;
      case "subtitles": morceaux.push("Sous-titres officiels"); break;
      case "autoSubtitles": morceaux.push("Sous-titres auto-générés"); break;
      case "inference": morceaux.push("Transcrite"); break;
    }
    if (resultat.language) morceaux.push(langue(resultat.language));
    if (resultat.speed) morceaux.push(`${resultat.speed.toFixed(1).replace(".", ",")}× temps réel`);
    morceaux.push(`${resultat.segments.length} segment${resultat.segments.length > 1 ? "s" : ""}`);
    return morceaux.join(" · ");
  });
</script>

<section class="saisie">
  <input
    type="url"
    bind:value={url}
    oninput={planifierSonde}
    onkeydown={(e) => e.key === "Enter" && lancer()}
    placeholder="https://www.youtube.com/watch?v=…"
    disabled={occupe}
    aria-label="Adresse de la vidéo"
  />
  <button onclick={lancer} disabled={occupe || !url.trim()}>Transcrire</button>
</section>

<p class="video" aria-live="polite">
  {#if sondage}
    Lecture de la vidéo…
  {:else if video}
    <strong>{video.title}</strong>
    {#if video.channel}— {video.channel}{/if}
    · {duree(video.durationS)}
    {#if video.subtitles.length}
      · sous-titres : {video.subtitles.slice(0, 5).map((l) => langue(l)).join(", ")}{video.subtitles.length > 5 ? "…" : ""}
    {:else if video.autoCaptions}
      · sous-titres auto-générés ({langue(video.autoCaptions)})
    {/if}
  {:else if erreurSonde}
    <span class="erreur-texte">{erreurSonde.message}</span>
  {/if}
</p>

<section class="reglages">
  <label>
    Modèle
    <select bind:value={modele} disabled={occupe}>
      <option value="auto">Automatique{modeles?.auto ? ` (${modeles.auto})` : ""}</option>
      {#each modeles?.whisper ?? [] as m (m.alias)}
        <option value={m.alias}>{m.alias} — {taille(m.size)}{m.installed ? "" : " · à télécharger"}</option>
      {/each}
    </select>
  </label>
  <label>
    Langue
    <select bind:value={lang} disabled={occupe}>
      <option value="">Détection automatique</option>
      {#each langues as l (l.code)}
        <option value={l.code}>{l.nom}</option>
      {/each}
    </select>
  </label>
  <label class="case">
    <input type="checkbox" bind:checked={sousTitres} disabled={occupe} />
    Sous-titres YouTube s'ils existent
  </label>
  <button class="lien" onclick={() => (avancees = !avancees)} aria-expanded={avancees}>
    {avancees ? "▾" : "▸"} Options avancées
  </button>
</section>

{#if avancees}
  <section class="avancees">
    <label class="case">
      <input type="checkbox" bind:checked={vad} disabled={occupe} />
      Détection d'activité vocale
      <span class="note">écarte silences et musique, où Whisper invente du texte ; à désactiver pour un contenu chanté</span>
    </label>
    <label class="case">
      <input type="checkbox" bind:checked={traduire} disabled={occupe || !traductionPossible} />
      Traduire vers l'anglais
      {#if !traductionPossible}<span class="note">impossible avec {modeleEffectif?.alias}</span>{/if}
    </label>
    <label class="case">
      <input type="checkbox" bind:checked={mots} disabled={occupe} />
      Horodatage au mot
      <span class="note">porté par l'export JSON</span>
    </label>
    <label class="large">
      Contexte
      <span class="note">noms propres et jargon : améliore les termes rares, en début de vidéo surtout</span>
      <input bind:value={contexte} disabled={occupe} placeholder="Kubernetes, Prometheus…" />
    </label>
    <label class="large">
      Cookies du navigateur
      <span class="note">
        pour les seules vidéos qui exigent une session (limite d'âge, vérification anti-robot). Lus pour
        youtube.com uniquement, jamais écrits ni journalisés.
      </span>
      <select bind:value={navigateur} disabled={occupe}>
        <option value="">Aucun</option>
        {#each NAVIGATEURS as n (n)}
          <option value={n}>{n.charAt(0).toUpperCase() + n.slice(1)}</option>
        {/each}
      </select>
    </label>
  </section>
{/if}

{#if occupe}
  <section class="progression" aria-live="polite">
    <div class="barre" role="progressbar" aria-valuemin="0" aria-valuemax="100"
         aria-valuenow={telechargement ? Math.floor((telechargement.received * 100) / Math.max(telechargement.total, 1)) : (pourcent ?? undefined)}>
      {#if telechargement && telechargement.total > 0}
        <div class="jauge" style="width: {(telechargement.received * 100) / telechargement.total}%"></div>
      {:else if pourcent !== null}
        <div class="jauge" style="width: {pourcent}%"></div>
      {:else}
        <div class="jauge indeterminee"></div>
      {/if}
    </div>
    <div class="ligne">
      <p>
        {annulation ? "Annulation…" : etape}
        {#if telechargement && telechargement.total > 0}
          · {Math.floor((telechargement.received * 100) / telechargement.total)} % ({taille(telechargement.received)} / {taille(telechargement.total)})
        {:else if pourcent !== null}
          · {pourcent} %
          {#if position !== null && dureeMedia}· {horodatage(position)} / {horodatage(dureeMedia)}{/if}
          {#if vitesse !== null}· {vitesse.toFixed(1).replace(".", ",")}× temps réel{/if}
        {/if}
      </p>
      <button class="annuler" onclick={annuler} disabled={annulation}>Annuler</button>
    </div>
  </section>
{/if}

{#each notices as n, i (i)}
  <p class="notice">{n}</p>
{/each}

{#if erreur}
  <Erreur {erreur} />
{/if}
{#if annonce}
  <p class="notice">{annonce}</p>
{/if}

{#if segments.length || resultat}
  <section class="sortie">
    {#if resultat}
      <p class="resume">{resume}</p>
    {/if}
    <!-- Focalisable pour défiler au clavier : une zone déroulante doit
         l'être (WCAG 2.1.1), ce que la règle générique de Svelte ignore. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="segments" bind:this={zone} onscroll={surDefilement} tabindex="0" role="log" aria-label="Transcription">
      {#each segments as s, i (i)}
        <p><span class="temps">{horodatage(s.start)}</span>{s.text}</p>
      {/each}
      {#if resultat && !resultat.segments.length}
        <p class="note">Aucune parole détectée.</p>
      {/if}
    </div>
    {#if !suivre && occupe}
      <button class="suivi" onclick={reprendreSuivi}>↓ Suivre la transcription</button>
    {/if}
    {#if resultat && resultat.segments.length}
      <div class="actions">
        <button onclick={copier}>Copier</button>
        <span class="separateur">Exporter</span>
        {#each ["txt", "srt", "vtt", "json"] as f (f)}
          <button class="secondaire" onclick={() => exporter(f)}
                  title={f === "json" && !resultat.words ? "Sans horodatage au mot : activez-le dans les options avancées" : undefined}>
            .{f}
          </button>
        {/each}
      </div>
    {/if}
  </section>
{/if}
