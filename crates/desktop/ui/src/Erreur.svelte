<script lang="ts">
  import type { IpcError } from "./ipc";

  let { erreur }: { erreur: IpcError } = $props();
</script>

<!-- Le message et le conseil d'abord ; le détail technique, replié, pour qui
     en a l'usage ou doit signaler un problème (SPEC SF-07, tâche 3.10). -->
<section class="erreur" role="alert">
  <p>{erreur.message}</p>
  {#if erreur.conseil}<p class="conseil">{erreur.conseil}</p>{/if}
  {#if erreur.detail}
    <details>
      <summary>Détail technique</summary>
      <pre>{erreur.detail}</pre>
    </details>
  {/if}
</section>
