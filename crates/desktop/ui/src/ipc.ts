// Couche d'accès au backend — SPEC §4.2.
//
// Les types miroitent ceux de `crates/desktop/src/{commands,relais,erreur}.rs`.
// Toute divergence se paierait à l'exécution, sans que le compilateur
// n'avertisse : c'est la seule frontière du projet que Rust ne vérifie pas.

import { Channel, invoke } from "@tauri-apps/api/core";

export interface SidecarInfo {
  version: string | null;
  origin: string | null;
}

export interface Langue {
  code: string;
  /** Nom anglais, fourni par Whisper. */
  name: string;
}

export interface BackendInfo {
  version: string;
  backend: string;
  gpu: boolean;
  threads: number;
  ytdlp: SidecarInfo;
  ffmpeg: SidecarInfo;
  languages: Langue[];
}

export interface ModelEntry {
  alias: string;
  file: string;
  size: number;
  installed: boolean;
  translates: boolean;
}

export interface ModelsInfo {
  auto: string | null;
  whisper: ModelEntry[];
  vad: ModelEntry;
  usedBytes: number;
  dir: string;
}

export interface VideoInfo {
  videoId: string;
  title: string;
  channel: string | null;
  durationS: number | null;
  language: string | null;
  /** Langues des sous-titres rédigés. */
  subtitles: string[];
  /** Langue de la piste auto-générée d'origine. */
  autoCaptions: string | null;
}

export interface TranscribeArgs {
  url: string;
  model: string;
  lang: string | null;
  translate: boolean;
  initialPrompt: string | null;
  wordTimestamps: boolean;
  vad: boolean;
  preferSubs: boolean;
  cookiesFromBrowser: string | null;
}

export interface Segment {
  start: number;
  end: number;
  text: string;
}

export type Origine = "cache" | "subtitles" | "autoSubtitles" | "inference";

export interface Transcription {
  origin: Origine;
  title: string;
  channel: string | null;
  durationS: number | null;
  language: string | null;
  speed: number | null;
  words: boolean;
  segments: Segment[];
}

export type Phase = "cache" | "loading" | "probe" | "extraction";

/** Messages du relais, livrés par lots au plus dix fois par seconde. */
export type Message =
  | { type: "phase"; phase: Phase }
  | { type: "download"; item: string; received: number; total: number }
  | { type: "video"; title: string; channel: string | null; durationS: number | null }
  | { type: "subtitles"; lang: string; auto: boolean; translation: boolean }
  | { type: "notice"; message: string }
  | { type: "transcribing"; mediaS: number }
  | { type: "progress"; percent: number }
  | { type: "segments"; segments: Segment[] };

/** Erreur du backend, avec le code de sortie contractuel (SPEC SF-07). */
export interface IpcError {
  code: number;
  message: string;
  conseil: string | null;
  detail: string | null;
}

/** Code d'une opération annulée à la demande de l'utilisateur. */
export const CODE_ANNULE = 130;

export function isIpcError(e: unknown): e is IpcError {
  return typeof e === "object" && e !== null && "message" in e && "code" in e;
}

/** Toute exception, rendue sous la forme d'une erreur affichable. */
export function enErreur(e: unknown): IpcError {
  if (isIpcError(e)) return e;
  return { code: 3, message: "Erreur interne de l'application.", conseil: null, detail: String(e) };
}

function canal(recevoir: (m: Message) => void): Channel<Message[]> {
  const c = new Channel<Message[]>();
  c.onmessage = (lot) => lot.forEach(recevoir);
  return c;
}

export const backendInfo = () => invoke<BackendInfo>("backend_info");
export const listModels = () => invoke<ModelsInfo>("list_models");

export const probeUrl = (url: string, cookiesFromBrowser: string | null) =>
  invoke<VideoInfo>("probe_url", { url, cookiesFromBrowser });

export const transcribe = (args: TranscribeArgs, recevoir: (m: Message) => void) =>
  invoke<Transcription>("transcribe", { args, canal: canal(recevoir) });

export const cancel = () => invoke<boolean>("cancel");

/** Rend le chemin écrit, ou `null` si l'utilisateur a renoncé. */
export const exportAs = (format: string) => invoke<string | null>("export", { format });
export const copyText = () => invoke<void>("copy_text");

export const downloadModel = (alias: string, recevoir: (m: Message) => void) =>
  invoke<ModelsInfo>("download_model", { alias, canal: canal(recevoir) });

export const removeModel = (alias: string) => invoke<ModelsInfo>("remove_model", { alias });

export const updateExtractor = (recevoir: (m: Message) => void) =>
  invoke<SidecarInfo>("update_extractor", { canal: canal(recevoir) });
