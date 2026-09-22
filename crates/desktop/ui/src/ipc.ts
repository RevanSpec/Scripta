// Couche d'accès au backend — SPEC §4.2.
//
// Les types miroitent ceux de `crates/desktop/src/commands.rs`. Toute
// divergence se paierait à l'exécution, sans que le compilateur n'avertisse :
// c'est la seule frontière du projet que Rust ne vérifie pas.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface BackendInfo {
  backend: string;
  gpu: boolean;
  threads: number;
  ytdlp: string | null;
  ffmpeg: string | null;
}

export interface ModelEntry {
  alias: string;
  size_mb: number;
  installed: boolean;
}

export interface VideoInfo {
  video_id: string;
  title: string;
  channel: string | null;
  duration_s: number | null;
  language: string | null;
  subtitle_langs: string[];
}

export interface TranscribeArgs {
  url: string;
  model: string;
  lang: string | null;
  translate: boolean;
  initial_prompt: string | null;
  word_timestamps: boolean;
  prefer_subs: boolean;
  cookies_from_browser: string | null;
}

/** Étapes rapportées par le backend, dans l'ordre nominal. */
export type Phase =
  | "cache"
  | "probe"
  | "subtitles"
  | "model"
  | "download"
  | "transcribe"
  | "extractor";

export interface Progress {
  phase: Phase;
  percent: number | null;
  detail: string | null;
}

/** Erreur du backend, avec le code de sortie contractuel (SPEC SF-07). */
export interface IpcError {
  message: string;
  code: number;
}

export function isIpcError(e: unknown): e is IpcError {
  return typeof e === "object" && e !== null && "message" in e && "code" in e;
}

/**
 * Message d'accompagnement pour les causes actionnables.
 *
 * Le code est contractuel : l'interface s'appuie dessus plutôt que d'analyser
 * un texte, qui changerait au premier ajustement de formulation.
 */
export function conseilPour(code: number): string | null {
  switch (code) {
    case 12:
      return "Cette vidéo exige une session authentifiée. Renseignez un navigateur dans les options avancées.";
    case 13:
      return "Les diffusions en direct ne sont pas prises en charge : leur durée n'est pas bornée.";
    case 14:
      return "Vidéo trop longue pour la limite en vigueur.";
    case 21:
      return "yt-dlp ou ffmpeg est introuvable. Consultez le README, section Installation.";
    case 30:
      return "Le modèle n'a pas pu être obtenu. Vérifiez votre connexion.";
    default:
      return null;
  }
}

export const backendInfo = () => invoke<BackendInfo>("backend_info");
export const listModels = () => invoke<ModelEntry[]>("list_models");
export const probeUrl = (url: string) => invoke<VideoInfo>("probe_url", { url });
export const transcribe = (args: TranscribeArgs) => invoke<string>("transcribe", { args });
export const cancel = () => invoke<boolean>("cancel");
export const updateExtractor = () => invoke<string>("update_extractor");

export const renderAs = (format: string, maxLineWidth?: number, maxLineCount?: number) =>
  invoke<string>("render_as", { format, maxLineWidth, maxLineCount });

export const saveOutput = (path: string, contents: string) =>
  invoke<void>("save_output", { path, contents });

export const onProgress = (f: (p: Progress) => void) =>
  listen<Progress>("scripta://progress", (e) => f(e.payload));
