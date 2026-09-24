# Licences des composants tiers

Scripta est distribué sous [GPLv3](LICENSE). Ce document recense les
composants tiers et leurs conditions.

> **Portée actuelle.** Depuis la v0.1.1, la CLI est distribuée en binaire.
> Ses archives n'embarquent **ni yt-dlp ni ffmpeg**, que l'utilisateur installe
> lui-même. Les installeurs de l'application de bureau, eux, **les
> embarquent** : leurs obligations sont décrites ci-dessous, et chaque
> installeur en joint les textes dans son répertoire `licences/`.
>
> Les bibliothèques compilées dans les binaires sont redistribuées elles aussi.
> Chaque archive et chaque installeur joignent donc `LICENCES-DEPENDANCES.md`,
> l'inventaire complet de leurs licences, textes intégraux compris, généré à la
> release par `cargo about` (voir `about.toml`).

---

## Composants invoqués à l'exécution

| Composant | Licence | CLI | Application de bureau |
|---|---|---|---|
| [yt-dlp](https://github.com/yt-dlp/yt-dlp) | The Unlicense ; ses exécutables embarquent Python (PSF-2.0) et d'autres bibliothèques | installé par l'utilisateur | embarqué |
| [FFmpeg](https://ffmpeg.org) | LGPL-2.1+, pour la build minimale de Scripta | installé par l'utilisateur | embarqué |

Versions et empreintes épinglées : `scripts/sidecars/versions.env`.

**yt-dlp** est placé par The Unlicense dans le domaine public. Ses exécutables
autonomes embarquent toutefois un interpréteur Python et des bibliothèques
sous leurs propres licences — PSF-2.0, MIT, BSD… —, dont le projet publie la
notice, `THIRD_PARTY_LICENSES.txt`. Chaque installeur la joint, avec la
licence de yt-dlp.

**FFmpeg** : l'application de bureau embarque une build minimale, compilée par
`scripts/sidecars/build-ffmpeg.sh` sans composant GPL ni non libre ; elle
relève donc de la LGPL 2.1 ou ultérieure. Chaque installeur joint le texte de
la licence et une fiche de construction : version, empreinte de l'archive
source, options de configuration. L'archive source elle-même accompagne chaque
release.

ffmpeg est invoqué comme programme distinct, jamais lié à Scripta :
l'utilisateur peut le remplacer par sa propre build. Une copie déposée dans le
répertoire utilisateur de Scripta prime sur la copie embarquée
([ADR-004](docs/SPEC.md#adr-004--emplacement-des-sidecars-mis-à-jour)), sans
toucher à l'installation.

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
| [whisper-rs](https://codeberg.org/tazz4843/whisper-rs), whisper-rs-sys | The Unlicense |
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) et ggml, dont whisper-rs-sys embarque les sources | MIT |
| [clap](https://github.com/clap-rs/clap) | MIT ou Apache-2.0 |
| [serde](https://serde.rs), serde_json | MIT ou Apache-2.0 |
| [ureq](https://github.com/algesten/ureq), rustls | MIT ou Apache-2.0 |
| [sha2](https://github.com/RustCrypto/hashes) | MIT ou Apache-2.0 |
| [directories](https://github.com/dirs-dev/directories-rs) | MIT ou Apache-2.0 |
| [thiserror](https://github.com/dtolnay/thiserror) | MIT ou Apache-2.0 |
| [url](https://github.com/servo/rust-url) | MIT ou Apache-2.0 |
| [ctrlc](https://github.com/Detegr/rust-ctrlc) | MIT ou Apache-2.0 |

Toutes sont permissives et compatibles avec la GPLv3. Parmi les dépendances
transitives figurent aussi une bibliothèque sous MPL-2.0 (`option-ext`) et les
certificats racines de `webpki-roots`, sous CDLA-Permissive-2.0 : compatibles
elles aussi, leurs textes figurent dans l'inventaire.

L'inventaire exhaustif, dépendances transitives comprises, s'obtient par
binaire — la CLI, ou l'application de bureau et son graphe plus large :

```bash
cargo install cargo-about --locked --features cli
cargo about generate --locked --manifest-path crates/cli/Cargo.toml -c about.toml about.hbs -o LICENCES-DEPENDANCES.md
cargo about generate --locked --manifest-path crates/desktop/Cargo.toml -c about.toml about.hbs -o crates/desktop/LICENCES-DEPENDANCES.md
```

L'application de bureau ajoute Tauri et ses dépendances, sous les mêmes
licences, plus la Boost Software License (BSL-1.0) du presse-papiers sous
Windows, compatible GPLv3 elle aussi.

`about.toml` fixe la liste des licences acceptées : une dépendance sous une
autre licence fait échouer la génération, donc la release.

---

## Incompatibilité connue

La **GPLv3 est incompatible avec les conditions de l'App Store d'Apple**. La
distribution macOS se fera exclusivement par `.dmg` signé et notarisé, hors
App Store — voir [SPEC §1.3](docs/SPEC.md#13-licence-et-conformité).
