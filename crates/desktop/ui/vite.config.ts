import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Port figé : `devUrl` de tauri.conf.json le référence, et un port mobile
// ferait échouer le lancement en mode développement.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2021", sourcemap: false },
});
