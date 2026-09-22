# Licences des composants tiers

Scripta est distribué sous [GPLv3](LICENSE). Ce document recense les
composants tiers et leurs conditions.

> **Portée actuelle.** Scripta **n'embarque encore aucun binaire tiers** :
> `yt-dlp` et `ffmpeg` sont invoqués comme programmes externes, installés
> séparément par l'utilisateur. Les obligations attachées à leur
> *redistribution* ne s'appliqueront donc qu'à partir de l'empaquetage
> (Jalon 4), où ce fichier devra accompagner les artefacts et être complété
> des textes de licence intégraux.

---

## Composants invoqués à l'exécution

| Composant | Licence | Redistribué |
|---|---|---|
| [yt-dlp](https://github.com/yt-dlp/yt-dlp) | The Unlicense | non |
| [FFmpeg](https://ffmpeg.org) | LGPL-2.1+ ou GPL-2.0+ selon la build | non |

**The Unlicense** place `yt-dlp` dans le domaine public : aucune obligation.

**FFmpeg** est fourni par l'utilisateur, et la licence dépend de la build
retenue. Une build `--enable-gpl` est « GPLv2 ou ultérieure », donc compatible
GPLv3. Une build LGPL l'est également. À l'empaquetage, la build embarquée
devra être identifiée précisément et son texte de licence joint, la LGPL
imposant en outre de permettre le remplacement de la bibliothèque.

---

## Modèles

| Ressource | Licence | Provenance |
|---|---|---|
| Modèles Whisper GGML | MIT | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) |
| VAD Silero | MIT | [ggml-org/whisper-vad](https://huggingface.co/ggml-org/whisper-vad) |

Ces fichiers sont **téléchargés par l'utilisateur** au premier usage, jamais
redistribués avec Scripta.

---

## Dépendances Rust compilées

Liées statiquement dans le binaire, donc redistribuées.

| Crate | Licence |
|---|---|
| [whisper-rs](https://github.com/tazz4843/whisper-rs) | MIT |
| whisper-rs-sys, incluant [whisper.cpp](https://github.com/ggml-org/whisper.cpp) et ggml | MIT |
| [clap](https://github.com/clap-rs/clap) | MIT ou Apache-2.0 |
| [serde](https://serde.rs), serde_json | MIT ou Apache-2.0 |
| [ureq](https://github.com/algesten/ureq), rustls | MIT ou Apache-2.0 |
| [sha2](https://github.com/RustCrypto/hashes) | MIT ou Apache-2.0 |
| [directories](https://github.com/dirs-dev/directories-rs) | MIT ou Apache-2.0 |
| [thiserror](https://github.com/dtolnay/thiserror) | MIT ou Apache-2.0 |
| [url](https://github.com/servo/rust-url) | MIT ou Apache-2.0 |
| [ctrlc](https://github.com/Detegr/rust-ctrlc) | MIT ou Apache-2.0 |

Toutes sont permissives et compatibles avec la GPLv3.

L'inventaire exhaustif, dépendances transitives comprises, s'obtient par :

```bash
cargo install cargo-about && cargo about generate about.hbs
```

---

## Incompatibilité connue

La **GPLv3 est incompatible avec les conditions de l'App Store d'Apple**. La
distribution macOS se fera exclusivement par `.dmg` signé et notarisé, hors
App Store — voir [SPEC §1.3](docs/SPEC.md#13-licence-et-conformité).
