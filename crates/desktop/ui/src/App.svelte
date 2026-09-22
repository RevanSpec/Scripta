<script lang="ts">
  import { onMount } from "svelte";
  import { save } from "@tauri-apps/plugin-dialog";
  import {
    backendInfo, listModels, probeUrl, transcribe, cancel,
    renderAs, saveOutput, updateExtractor, onProgress,
    isIpcError, conseilPour,
    type BackendInfo, type ModelEntry, type VideoInfo, type Phase,
  } from "./ipc";

  let info = $state<BackendInfo | null>(null);
  let modeles = $state<ModelEntry[]>([]);
  let video = $state<VideoInfo | null>(null);

  let url = $state("");
  let modele = $state("auto");
  let langue = $state("");
  let traduire = $state(false);
  let contexte = $state("");
  let motsHorodates = $state(false);
  let sousTitres = $state(false);
  let navigateur = $state("");
  let avancees = $state(false);

  let occupe = $state(false);
  let phase = $state<Phase | null>(null);
  let pourcent = $state<number | null>(null);
  let detail = $state<string | null>(null);
  let sortie = $state("");
  let erreur = $state<{ message: string; conseil: string | null } | null>(null);

  const LIBELLES: Record<Phase, string> = {
    cache: "Trouvé en cache",
    probe: "Lecture des métadonnées",
    subtitles: "Sous-titres officiels",
    model: "Chargement du modèle",
    download: "Extraction audio",
    transcribe: "Transcription",
    extractor: "Mise à jour de l'extracteur",
  };

  onMount(async () => {
    info = await backendInfo();
    modeles = await listModels();
    await onProgress((p) => {
      phase = p.phase;
      pourcent = p.percent;
      detail = p.detail;
    });
  });

  function signaler(e: unknown) {
    if (isIpcError(e)) {
      erreur = { message: e.message, conseil: conseilPour(e.code) };
    } else {
      erreur = { message: String(e), conseil: null };
    }
  }

  async function verifier() {
    if (!url.trim()) return;
    erreur = null;
    video = null;
    try {
      video = await probeUrl(url.trim());
    } catch (e) {
      signaler(e);
    }
  }

  async function lancer() {
    if (!url.trim() || occupe) return;
    occupe = true;
    erreur = null;
    sortie = "";
    phase = null;
    pourcent = null;
    try {
      sortie = await transcribe({
        url: url.trim(),
        model: modele,
        lang: langue.trim() || null,
        translate: traduire,
        initial_prompt: contexte.trim() || null,
        word_timestamps: motsHorodates,
        prefer_subs: sousTitres,
        cookies_from_browser: navigateur.trim() || null,
      });
    } catch (e) {
      signaler(e);
    } finally {
      occupe = false;
      phase = null;
      pourcent = null;
    }
  }

  async function exporter(format: string) {
    try {
      const contenu = await renderAs(format);
      const nom = (video?.title ?? "transcription").replace(/[^\p{L}\p{N} _-]/gu, "").slice(0, 60);
      const chemin = await save({
        defaultPath: `${nom || "transcription"}.${format}`,
        filters: [{ name: format.toUpperCase(), extensions: [format] }],
      });
      if (chemin) await saveOutput(chemin, contenu);
    } catch (e) {
      signaler(e);
    }
  }

  async function majExtracteur() {
    occupe = true;
    erreur = null;
    try {
      const v = await updateExtractor();
      detail = `yt-dlp ${v}`;
      if (info) info = { ...info, ytdlp: v };
    } catch (e) {
      signaler(e);
    } finally {
      occupe = false;
      phase = null;
    }
  }

  function duree(s: number | null): string {
    if (s === null) return "durée inconnue";
    return s < 60 ? `${Math.round(s)} s` : `${Math.round(s / 60)} min`;
  }
</script>

<main>
  <header>
    <h1>Scripta</h1>
    {#if info}
      <p class="etat">
        {info.backend === "cpu" ? "CPU" : info.backend.toUpperCase()} · {info.threads} threads
        {#if info.ytdlp}· yt-dlp {info.ytdlp}{:else}· <span class="manque">yt-dlp absent</span>{/if}
        {#if !info.ffmpeg}· <span class="manque">ffmpeg absent</span>{/if}
      </p>
    {/if}
  </header>

  <section class="saisie">
    <input
      type="url"
      bind:value={url}
      onchange={verifier}
      placeholder="https://www.youtube.com/watch?v=..."
      disabled={occupe}
    />
    <button onclick={lancer} disabled={occupe || !url.trim()}>Transcrire</button>
  </section>

  {#if video}
    <p class="video">
      <strong>{video.title}</strong>
      {#if video.channel}— {video.channel}{/if}
      · {duree(video.duration_s)}
      {#if video.subtitle_langs.length}
        · sous-titres : {video.subtitle_langs.slice(0, 6).join(", ")}
      {/if}
    </p>
  {/if}

  <section class="reglages">
    <label>
      Modèle
      <select bind:value={modele} disabled={occupe}>
        <option value="auto">auto</option>
        {#each modeles as m (m.alias)}
          <option value={m.alias}>{m.alias} — {m.size_mb} Mo{m.installed ? "" : " (à télécharger)"}</option>
        {/each}
      </select>
    </label>
    <label>
      Langue
      <input bind:value={langue} placeholder="auto" size="6" disabled={occupe} />
    </label>
    <label class="case">
      <input type="checkbox" bind:checked={sousTitres} disabled={occupe} />
      Sous-titres officiels si disponibles
    </label>
    <button class="lien" onclick={() => (avancees = !avancees)}>
      {avancees ? "▾" : "▸"} Options avancées
    </button>
  </section>

  {#if avancees}
    <section class="avancees">
      <label class="case">
        <input type="checkbox" bind:checked={traduire} disabled={occupe} />
        Traduire vers l'anglais <span class="note">(pas avec turbo)</span>
      </label>
      <label class="case">
        <input type="checkbox" bind:checked={motsHorodates} disabled={occupe} />
        Horodatage au mot
      </label>
      <label class="large">
        Contexte <span class="note">noms propres et jargon, améliore les termes rares</span>
        <input bind:value={contexte} disabled={occupe} placeholder="Kubernetes, Prometheus…" />
      </label>
      <label class="large">
        Cookies du navigateur
        <span class="note">requis pour les vidéos restreintes ; jamais activé d'office</span>
        <input bind:value={navigateur} disabled={occupe} placeholder="firefox, chrome, edge…" />
      </label>
      <button onclick={majExtracteur} disabled={occupe}>Mettre à jour yt-dlp</button>
    </section>
  {/if}

  {#if occupe || phase}
    <section class="progression">
      <div class="barre">
        <div class="jauge" style="width: {pourcent ?? 0}%"></div>
      </div>
      <p>
        {phase ? LIBELLES[phase] : "Préparation"}
        {#if pourcent !== null}· {pourcent} %{/if}
        {#if detail}· {detail}{/if}
      </p>
      {#if occupe}
        <button class="annuler" onclick={cancel}>Annuler</button>
      {/if}
    </section>
  {/if}

  {#if erreur}
    <section class="erreur">
      <p>{erreur.message}</p>
      {#if erreur.conseil}<p class="conseil">{erreur.conseil}</p>{/if}
    </section>
  {/if}

  {#if sortie}
    <section class="sortie">
      <textarea readonly value={sortie}></textarea>
      <div class="actions">
        <button onclick={() => navigator.clipboard.writeText(sortie)}>Copier</button>
        <span class="separateur">Exporter</span>
        {#each ["txt", "srt", "vtt", "json"] as f (f)}
          <button onclick={() => exporter(f)}>.{f}</button>
        {/each}
      </div>
    </section>
  {/if}
</main>
