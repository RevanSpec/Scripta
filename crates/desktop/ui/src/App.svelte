<script lang="ts">
  import { onMount } from "svelte";
  import Transcription from "./Transcription.svelte";
  import Modeles from "./Modeles.svelte";
  import { backendInfo, listModels, enErreur, type BackendInfo, type ModelsInfo } from "./ipc";

  let info = $state<BackendInfo | null>(null);
  let modeles = $state<ModelsInfo | null>(null);
  let erreurInit = $state<string | null>(null);
  let vue = $state<"transcription" | "modeles">("transcription");
  // Une seule tâche longue à la fois, quel que soit l'onglet qui l'a lancée.
  let occupe = $state(false);

  onMount(() => {
    // Séparément : `backend_info` interroge les sidecars, ce qui prend une
    // seconde ou plus, et la liste des modèles n'a pas à l'attendre.
    listModels()
      .then((m) => (modeles = m))
      .catch((e) => (erreurInit = enErreur(e).message));
    backendInfo()
      .then((i) => (info = i))
      .catch((e) => (erreurInit = enErreur(e).message));
  });

  const manquants = $derived(
    info
      ? [
          info.ytdlp.version ? null : "yt-dlp",
          info.ffmpeg.version ? null : "ffmpeg",
        ].filter((n) => n !== null)
      : [],
  );
</script>

<div class="app">
  <header>
    <h1>Scripta</h1>
    <nav>
      <button class="onglet" class:actif={vue === "transcription"} onclick={() => (vue = "transcription")}>
        Transcription
      </button>
      <button class="onglet" class:actif={vue === "modeles"} onclick={() => (vue = "modeles")}>
        Modèles et outils
      </button>
    </nav>
    {#if info}
      <p class="etat" title="Accélération fixée à la compilation (ADR-001)">
        {info.backend === "cpu" ? "CPU" : info.backend.toUpperCase()} · {info.threads} threads
      </p>
    {/if}
  </header>

  {#if erreurInit}
    <p class="bandeau erreur-texte">{erreurInit}</p>
  {/if}
  {#if manquants.length}
    <p class="bandeau">
      {manquants.join(" et ")}
      {manquants.length > 1 ? "sont introuvables" : "est introuvable"} : la transcription échouera.
      <button class="lien" onclick={() => (vue = "modeles")}>Voir les outils</button>
    </p>
  {/if}

  <!-- Masqués plutôt que démontés : changer d'onglet ne doit interrompre
       ni une transcription ni un téléchargement. -->
  <main hidden={vue !== "transcription"}>
    <Transcription {info} {modeles} bind:occupe />
  </main>
  <main hidden={vue !== "modeles"}>
    <Modeles bind:modeles bind:info bind:occupe />
  </main>
</div>
