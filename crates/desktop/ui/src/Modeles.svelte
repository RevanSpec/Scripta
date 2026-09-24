<script lang="ts">
  import Erreur from "./Erreur.svelte";
  import {
    cancel, downloadModel, enErreur, removeModel, updateExtractor, CODE_ANNULE,
    type BackendInfo, type IpcError, type Message, type ModelEntry, type ModelsInfo,
  } from "./ipc";
  import { taille } from "./format";

  interface Props {
    modeles: ModelsInfo | null;
    info: BackendInfo | null;
    occupe: boolean;
  }
  let { modeles = $bindable(), info = $bindable(), occupe = $bindable() }: Props = $props();

  /** Élément en cours de téléchargement : un modèle, ou `yt-dlp`. */
  let enCours = $state<string | null>(null);
  let progression = $state<{ received: number; total: number } | null>(null);
  let aConfirmer = $state<string | null>(null);
  let erreur = $state<IpcError | null>(null);
  let message = $state<string | null>(null);

  function recevoir(m: Message) {
    if (m.type === "download") progression = { received: m.received, total: m.total };
  }

  async function tache(nom: string, travail: () => Promise<void>) {
    if (occupe) return;
    occupe = true;
    enCours = nom;
    progression = null;
    erreur = null;
    message = null;
    try {
      await travail();
    } catch (e) {
      const err = enErreur(e);
      if (err.code === CODE_ANNULE) message = "Téléchargement annulé ; il reprendra où il s'est arrêté.";
      else erreur = err;
    } finally {
      occupe = false;
      enCours = null;
      progression = null;
    }
  }

  function telecharger(m: ModelEntry) {
    tache(m.alias, async () => {
      modeles = await downloadModel(m.alias, recevoir);
      message = `Modèle ${m.alias} installé et vérifié.`;
    });
  }

  /** Suppression en deux temps : un clic arme, le second confirme. */
  function supprimer(m: ModelEntry) {
    if (aConfirmer !== m.alias) {
      aConfirmer = m.alias;
      setTimeout(() => {
        if (aConfirmer === m.alias) aConfirmer = null;
      }, 4000);
      return;
    }
    aConfirmer = null;
    tache(m.alias, async () => {
      modeles = await removeModel(m.alias);
      message = `Modèle ${m.alias} supprimé.`;
    });
  }

  function mettreAJour() {
    tache("yt-dlp", async () => {
      const ytdlp = await updateExtractor(recevoir);
      if (info) info = { ...info, ytdlp };
      message = `yt-dlp ${ytdlp.version ?? ""} installé et vérifié.`;
    });
  }

  const pourcent = $derived(
    progression && progression.total > 0 ? Math.floor((progression.received * 100) / progression.total) : null,
  );
</script>

{#snippet ligne(m: ModelEntry, role: string | null)}
  <tr>
    <td>
      <strong>{m.alias}</strong>
      {#if role}<span class="note">{role}</span>{/if}
    </td>
    <td class="nombre">{taille(m.size)}</td>
    <td>
      {#if enCours === m.alias && progression}
        <div class="barre petite"><div class="jauge" style="width: {pourcent ?? 0}%"></div></div>
      {:else if m.installed}
        installé
      {:else}
        <span class="note">—</span>
      {/if}
    </td>
    <td class="actions-ligne">
      {#if enCours === m.alias}
        {#if progression}
          <span class="note">{pourcent ?? 0} %</span>
          <button class="annuler" onclick={() => cancel()}>Annuler</button>
        {:else}
          <span class="note">…</span>
        {/if}
      {:else if m.installed}
        <button class="secondaire" class:danger={aConfirmer === m.alias} onclick={() => supprimer(m)} disabled={occupe}>
          {aConfirmer === m.alias ? "Confirmer" : "Supprimer"}
        </button>
      {:else}
        <button class="secondaire" onclick={() => telecharger(m)} disabled={occupe}>Télécharger</button>
      {/if}
    </td>
  </tr>
{/snippet}

<section class="bloc">
  <h2>Modèles de transcription</h2>
  <p class="note">
    Téléchargés au premier usage et vérifiés par empreinte SHA-256. Les plus gros transcrivent mieux, et plus lentement.
  </p>
  {#if modeles}
    <table>
      <tbody>
        {#each modeles.whisper as m (m.alias)}
          {@render ligne(m, m.alias === modeles.auto ? "choix automatique" : m.translates ? null : "ne traduit pas")}
        {/each}
      </tbody>
    </table>
    <h2>Détection d'activité vocale</h2>
    <table>
      <tbody>
        {@render ligne(modeles.vad, "Silero")}
      </tbody>
    </table>
    <p class="note">
      Espace occupé : {taille(modeles.usedBytes)} · <span class="chemin">{modeles.dir}</span>
    </p>
  {:else}
    <p class="note">Lecture du catalogue…</p>
  {/if}
</section>

<section class="bloc">
  <h2>Extracteurs</h2>
  <p class="note">YouTube change régulièrement : quand une vidéo refuse de s'extraire, mettre yt-dlp à jour suffit le plus souvent.</p>
  <table>
    <tbody>
      <tr>
        <td><strong>yt-dlp</strong></td>
        <td>
          {#if info}
            {info.ytdlp.version ?? "introuvable"}
            {#if info.ytdlp.version && info.ytdlp.origin}<span class="note">{info.ytdlp.origin}</span>{/if}
          {:else}
            <span class="note">…</span>
          {/if}
        </td>
        <td class="actions-ligne">
          {#if enCours === "yt-dlp"}
            <span class="note">{pourcent !== null ? `${pourcent} %` : "…"}</span>
          {:else}
            <button class="secondaire" onclick={mettreAJour} disabled={occupe}>Mettre à jour</button>
          {/if}
        </td>
      </tr>
      <tr>
        <td><strong>ffmpeg</strong></td>
        <td>
          {#if info}
            {info.ffmpeg.version ?? "introuvable — voir le README, section Installation"}
            {#if info.ffmpeg.version && info.ffmpeg.origin}<span class="note">{info.ffmpeg.origin}</span>{/if}
          {:else}
            <span class="note">…</span>
          {/if}
        </td>
        <td></td>
      </tr>
    </tbody>
  </table>
</section>

{#if erreur}
  <Erreur {erreur} />
{/if}
{#if message}
  <p class="notice">{message}</p>
{/if}
