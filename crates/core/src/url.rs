//! Validation d'URL et reconstruction canonique — SPEC SF-01, §5.1.
//!
//! Principe de sécurité : l'entrée utilisateur n'atteint **jamais** un sidecar.
//! Seul un identifiant vidéo validé contre `^[A-Za-z0-9_-]{11}$` en est extrait,
//! à partir duquel une URL canonique est reconstruite. Toute injection
//! d'argument ou de paramètre de requête est neutralisée par construction.

use crate::error::{Result, ScriptaError};

/// Hôtes acceptés, en correspondance exacte (jamais un suffixe : `youtube.com.evil.tld`
/// doit être rejeté).
const ALLOWED_HOSTS: &[&str] = &[
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "youtu.be",
];

/// URL validée et reconstruite. Seul ce type est accepté par les fonctions qui
/// invoquent un sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalUrl {
    video_id: String,
    /// Vrai si l'entrée référençait une playlist, dont le paramètre a été écarté.
    had_playlist: bool,
}

impl CanonicalUrl {
    pub fn video_id(&self) -> &str {
        &self.video_id
    }

    /// Vrai si l'URL d'origine portait un paramètre `list=`, ignoré en v1.
    /// L'appelant doit en avertir l'utilisateur (SF-01).
    pub fn had_playlist(&self) -> bool {
        self.had_playlist
    }

    /// URL canonique transmise aux sidecars.
    pub fn as_str(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.video_id)
    }
}

impl std::fmt::Display for CanonicalUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_str())
    }
}

/// Valide une URL YouTube et en extrait la forme canonique.
pub fn parse(input: &str) -> Result<CanonicalUrl> {
    let invalid = || ScriptaError::InvalidUrl(input.to_string());

    // Analyse structurelle, jamais par expression régulière : une regex sur une
    // URL laisse systématiquement passer des cas limites.
    let parsed = url::Url::parse(input.trim()).map_err(|_| invalid())?;

    if parsed.scheme() != "https" {
        return Err(invalid());
    }

    let host = parsed.host_str().ok_or_else(invalid)?.to_lowercase();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        return Err(invalid());
    }

    // Une URL portant des identifiants est refusée : elle n'a aucune raison
    // d'exister ici et fuiterait vers le sidecar.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid());
    }

    let had_playlist = parsed.query_pairs().any(|(k, _)| k == "list");
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();

    let candidate: String = if host == "youtu.be" {
        // https://youtu.be/<ID>
        segments.first().ok_or_else(invalid)?.to_string()
    } else {
        match segments.as_slice() {
            // https://www.youtube.com/watch?v=<ID>
            ["watch"] => parsed
                .query_pairs()
                .find(|(k, _)| k == "v")
                .map(|(_, v)| v.into_owned())
                .ok_or_else(invalid)?,
            // /shorts/<ID>, /embed/<ID>, /live/<ID>, /v/<ID>
            [kind, id, ..] if matches!(*kind, "shorts" | "embed" | "live" | "v") => id.to_string(),
            _ => return Err(invalid()),
        }
    };

    if !is_valid_video_id(&candidate) {
        return Err(invalid());
    }

    Ok(CanonicalUrl {
        video_id: candidate,
        had_playlist,
    })
}

/// `^[A-Za-z0-9_-]{11}$` — vérifié sur les octets, l'alphabet étant ASCII.
fn is_valid_video_id(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "dQw4w9WgXcQ";

    #[test]
    fn accepte_les_formes_usuelles() {
        for input in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?v=dQw4w9WgXcQ",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ",
            "  https://www.youtube.com/watch?v=dQw4w9WgXcQ  ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
        ] {
            let got = parse(input).unwrap_or_else(|_| panic!("rejeté à tort : {input}"));
            assert_eq!(got.video_id(), ID);
        }
    }

    #[test]
    fn canonicalise_en_ecartant_les_parametres() {
        // Le point central de SF-01 : ce qui part vers le sidecar est reconstruit,
        // pas recopié. Les paramètres parasites disparaissent.
        let u = parse("https://youtu.be/dQw4w9WgXcQ?t=120&feature=share").unwrap();
        assert_eq!(u.as_str(), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    }

    #[test]
    fn signale_les_playlists_sans_les_traiter() {
        let u = parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLabc123").unwrap();
        assert!(u.had_playlist());
        assert_eq!(u.as_str(), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    }

    #[test]
    fn rejette_les_schemas_non_https() {
        for input in [
            "http://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://youtube.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert!(parse(input).is_err(), "accepté à tort : {input}");
        }
    }

    #[test]
    fn rejette_les_hotes_ressemblants() {
        // Correspondance exacte, jamais par suffixe ni par sous-chaîne.
        for input in [
            "https://youtube.com.evil.tld/watch?v=dQw4w9WgXcQ",
            "https://evil.tld/youtube.com/watch?v=dQw4w9WgXcQ",
            "https://notyoutube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be.evil.tld/dQw4w9WgXcQ",
            "https://vimeo.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert!(parse(input).is_err(), "accepté à tort : {input}");
        }
    }

    #[test]
    fn rejette_les_identifiants_malformes() {
        for input in [
            "https://www.youtube.com/watch?v=",
            "https://www.youtube.com/watch?v=tropcourt",
            "https://www.youtube.com/watch?v=beaucoup_trop_long_12345",
            "https://www.youtube.com/watch?v=abc%20def123",
            "https://www.youtube.com/watch?v=dQw4w9WgXc$",
            "https://www.youtube.com/watch",
            "https://www.youtube.com/",
            "https://youtu.be/",
        ] {
            assert!(parse(input).is_err(), "accepté à tort : {input}");
        }
    }

    #[test]
    fn neutralise_les_tentatives_d_injection() {
        // Même si l'un de ces cas franchissait la validation, la reconstruction
        // canonique garantit qu'aucun de ces fragments n'atteint le sidecar.
        for input in [
            "https://www.youtube.com/watch?v=--exec-before-dl",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ; rm -rf /",
            "https://www.youtube.com/watch?v=$(whoami)abc",
            "https://www.youtube.com/watch?v=`id`12345",
            "https://www.youtube.com/watch?v=-oevil.sh",
        ] {
            match parse(input) {
                Err(_) => {}
                Ok(u) => {
                    let s = u.as_str();
                    assert!(
                        !s.contains(['$', '`', ';', ' ']) && !s.contains("--"),
                        "fragment dangereux conservé : {s}"
                    );
                }
            }
        }
    }

    #[test]
    fn rejette_les_identifiants_dans_l_url() {
        assert!(parse("https://user:pass@www.youtube.com/watch?v=dQw4w9WgXcQ").is_err());
    }
}
