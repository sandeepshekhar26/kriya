//! Model identity resolution for `kriya-llm-proxy` (F1, doc 28 §F1): read a served model's
//! resolved digest from where it ALREADY lives — an Ollama registry manifest, or an operator-
//! precomputed file-hash cache — and **never** hash a multi-GB model weight file on the request
//! path. A miss in both is reported honestly as [`DigestSource::Unresolved`], never guessed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// Where a resolved digest came from — carried into the `kriya.model.identity` receipt so the
/// claim is exactly as strong as the evidence behind it (doc 24's honesty discipline: never
/// inflate a lookup into an attestation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestSource {
    /// Read straight from an Ollama registry manifest (`<models_dir>/manifests/...`) — the digest
    /// Ollama itself computed and recorded when it pulled the model.
    OllamaManifest,
    /// Looked up in the operator-populated file-hash cache, keyed by `(path, size, mtime)` — a
    /// sha256 computed OFF the request path (via the `hash-model` subcommand), never on it.
    FileSha256Cached,
    /// Neither a manifest nor a cache hit — honestly unresolved.
    Unresolved,
}

impl DigestSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DigestSource::OllamaManifest => "ollama-manifest",
            DigestSource::FileSha256Cached => "file-sha256-cached",
            DigestSource::Unresolved => "unresolved",
        }
    }
}

/// The result of resolving one served model name to an identity.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// sha256 hex of the model's weight layer/file, no `sha256:` prefix. `None` iff
    /// `source == Unresolved`.
    pub digest: Option<String>,
    pub source: DigestSource,
    /// The resolved layer/file's size in bytes, when known (metadata only — never implies the
    /// bytes were read on this path).
    pub size: Option<u64>,
}

impl ResolvedModel {
    fn unresolved() -> Self {
        ResolvedModel {
            digest: None,
            source: DigestSource::Unresolved,
            size: None,
        }
    }
}

// ─── Ollama manifest resolution ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OllamaManifestLayer {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    size: u64,
}

#[derive(Deserialize)]
struct OllamaManifestFile {
    #[serde(default)]
    layers: Vec<OllamaManifestLayer>,
}

/// The layer mediaType that carries the actual model weights (Ollama also stores separate
/// params/template/license/system layers) — the digest that identifies "which model," not which
/// prompt template ships alongside it.
const MODEL_LAYER_MEDIA_TYPE: &str = "application/vnd.ollama.image.model";

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// `~/.ollama/models` unless overridden by `OLLAMA_MODELS` — the same env var Ollama itself reads.
pub fn ollama_models_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OLLAMA_MODELS") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    home_dir()
        .map(|h| h.join(".ollama").join("models"))
        .unwrap_or_else(|| PathBuf::from(".ollama/models"))
}

/// Split an Ollama-style model reference (`"llama3.1:8b"`, `"llama3.1"`,
/// `"myregistry.example.com/user/model:tag"`) into `(registry, namespace, name, tag)` — the exact
/// path segments a manifest lives at under `manifests/`, applying Ollama's own defaults
/// (`registry.ollama.ai` / `library` / `latest`).
fn split_model_ref(model: &str) -> (String, String, String, String) {
    let (path_part, tag) = match model.rsplit_once(':') {
        Some((p, t)) if !t.is_empty() => (p.to_string(), t.to_string()),
        _ => (model.to_string(), "latest".to_string()),
    };
    let segments: Vec<&str> = path_part.split('/').filter(|s| !s.is_empty()).collect();
    match segments.len() {
        0 => (
            "registry.ollama.ai".to_string(),
            "library".to_string(),
            model.to_string(),
            tag,
        ),
        1 => (
            "registry.ollama.ai".to_string(),
            "library".to_string(),
            segments[0].to_string(),
            tag,
        ),
        2 => (
            "registry.ollama.ai".to_string(),
            segments[0].to_string(),
            segments[1].to_string(),
            tag,
        ),
        _ => (
            segments[0].to_string(),
            segments[1..segments.len() - 1].join("/"),
            segments[segments.len() - 1].to_string(),
            tag,
        ),
    }
}

/// Resolve `model`'s digest straight from its Ollama registry manifest — a small JSON file Ollama
/// already wrote when it pulled the model. Never touches the (possibly multi-GB) weight blob
/// itself; the manifest carries the blob's digest + size as metadata alongside it.
pub fn resolve_ollama_digest(models_dir: &Path, model: &str) -> Option<(String, u64)> {
    let (registry, namespace, name, tag) = split_model_ref(model);
    let manifest_path = models_dir
        .join("manifests")
        .join(registry)
        .join(namespace)
        .join(name)
        .join(tag);
    let text = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: OllamaManifestFile = serde_json::from_str(&text).ok()?;
    let layer = manifest
        .layers
        .iter()
        .find(|l| l.media_type == MODEL_LAYER_MEDIA_TYPE)?;
    let digest = layer
        .digest
        .strip_prefix("sha256:")
        .unwrap_or(&layer.digest)
        .to_string();
    Some((digest, layer.size))
}

// ─── File-hash cache (llama.cpp / raw local model files) ──────────────────────────────────────

/// One cached entry: the file's identity at hash time (`path`, `size`, `mtime`) plus the sha256
/// hex digest computed OFF the request path. A cache HIT is a pure metadata lookup (stat only); a
/// MISS is reported honestly as [`DigestSource::Unresolved`] rather than hashing inline.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct CacheEntry {
    path: String,
    size: u64,
    mtime_secs: u64,
    digest: String,
}

#[derive(Default, Deserialize, Serialize)]
struct CacheFile {
    #[serde(default)]
    entries: Vec<CacheEntry>,
}

/// Default cache path: `~/.kriya/llm-proxy/digest-cache.json`.
pub fn default_cache_path() -> PathBuf {
    home_dir()
        .map(|h| h.join(".kriya").join("llm-proxy").join("digest-cache.json"))
        .unwrap_or_else(|| std::env::temp_dir().join("kriya-llm-proxy-digest-cache.json"))
}

fn file_identity(path: &Path) -> std::io::Result<(u64, u64)> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok((meta.len(), mtime))
}

/// A pure metadata lookup — stats `path` (cheap) and checks the cache for a digest computed under
/// the EXACT same `(path, size, mtime)` identity. Never reads file content; a stale cache entry
/// (the file changed since) is silently treated as a miss.
fn lookup_cached_digest(cache_path: &Path, path: &Path) -> Option<String> {
    let (size, mtime) = file_identity(path).ok()?;
    let path_str = path.to_string_lossy().to_string();
    let text = std::fs::read_to_string(cache_path).ok()?;
    let cache: CacheFile = serde_json::from_str(&text).ok()?;
    cache
        .entries
        .into_iter()
        .find(|e| e.path == path_str && e.size == size && e.mtime_secs == mtime)
        .map(|e| e.digest)
}

/// Compute (real sha256, reading file content — the ONLY function in this module that does) and
/// cache the digest for `path`, keyed by its current `(size, mtime)`. Meant to be run by the
/// `kriya-llm-proxy hash-model <path>` subcommand, by the operator, AHEAD of time — never by the
/// proxy's request-serving path.
pub fn compute_and_cache_digest(cache_path: &Path, path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let (size, mtime) = file_identity(path)?;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let digest = hex::encode(hasher.finalize());

    let mut cache: CacheFile = std::fs::read_to_string(cache_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let path_str = path.to_string_lossy().to_string();
    cache.entries.retain(|e| e.path != path_str);
    cache.entries.push(CacheEntry {
        path: path_str,
        size,
        mtime_secs: mtime,
        digest: digest.clone(),
    });
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(cache_path, serde_json::to_string_pretty(&cache)?)?;
    Ok(digest)
}

/// Resolve a served model NAME to its identity: try the Ollama manifest first (the common local-
/// inference case), then an operator-supplied `model_paths` mapping (model name → local file path)
/// against the file-hash cache, else honestly [`DigestSource::Unresolved`] — never fabricated,
/// never computed inline.
pub fn resolve_model(
    models_dir: &Path,
    cache_path: &Path,
    model_paths: &HashMap<String, PathBuf>,
    model: &str,
) -> ResolvedModel {
    if let Some((digest, size)) = resolve_ollama_digest(models_dir, model) {
        return ResolvedModel {
            digest: Some(digest),
            source: DigestSource::OllamaManifest,
            size: Some(size),
        };
    }
    if let Some(path) = model_paths.get(model) {
        if let Some(digest) = lookup_cached_digest(cache_path, path) {
            let size = std::fs::metadata(path).ok().map(|m| m.len());
            return ResolvedModel {
                digest: Some(digest),
                source: DigestSource::FileSha256Cached,
                size,
            };
        }
    }
    ResolvedModel::unresolved()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kriya-llm-manifest-test-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn split_model_ref_applies_ollama_defaults() {
        assert_eq!(
            split_model_ref("llama3.1"),
            (
                "registry.ollama.ai".to_string(),
                "library".to_string(),
                "llama3.1".to_string(),
                "latest".to_string()
            )
        );
        assert_eq!(
            split_model_ref("llama3.1:8b"),
            (
                "registry.ollama.ai".to_string(),
                "library".to_string(),
                "llama3.1".to_string(),
                "8b".to_string()
            )
        );
        assert_eq!(
            split_model_ref("someuser/mymodel:latest"),
            (
                "registry.ollama.ai".to_string(),
                "someuser".to_string(),
                "mymodel".to_string(),
                "latest".to_string()
            )
        );
        assert_eq!(
            split_model_ref("registry.example.com/team/mymodel:v2"),
            (
                "registry.example.com".to_string(),
                "team".to_string(),
                "mymodel".to_string(),
                "v2".to_string()
            )
        );
    }

    #[test]
    fn resolve_ollama_digest_reads_the_model_layer_from_a_real_manifest_file() {
        let dir = tmp_dir("manifest");
        let manifest_dir = dir
            .join("manifests")
            .join("registry.ollama.ai")
            .join("library")
            .join("llama3.1");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(
            manifest_dir.join("8b"),
            serde_json::json!({
                "schemaVersion": 2,
                "config": {"mediaType": "application/vnd.docker.container.image.v1+json", "digest": "sha256:cfgcfgcfg", "size": 500},
                "layers": [
                    {"mediaType": "application/vnd.ollama.image.model", "digest": "sha256:abc123def456", "size": 4_700_000_000u64},
                    {"mediaType": "application/vnd.ollama.image.template", "digest": "sha256:tpltpltpl", "size": 200},
                ]
            })
            .to_string(),
        )
        .unwrap();

        let (digest, size) = resolve_ollama_digest(&dir, "llama3.1:8b").expect("manifest resolves");
        assert_eq!(digest, "abc123def456", "sha256: prefix stripped");
        assert_eq!(size, 4_700_000_000);

        // Never touches a nonexistent weight blob file — the manifest alone is enough.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_ollama_digest_is_none_when_no_manifest_exists() {
        let dir = tmp_dir("missing");
        assert!(resolve_ollama_digest(&dir, "nonexistent-model:latest").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_hash_cache_round_trips_and_never_rehashes_on_hit() {
        let dir = tmp_dir("cache");
        let cache_path = dir.join("digest-cache.json");
        let model_file = dir.join("model.gguf");
        std::fs::write(&model_file, b"pretend-multi-gb-of-weights").unwrap();

        let digest = compute_and_cache_digest(&cache_path, &model_file).unwrap();
        assert_eq!(digest.len(), 64, "sha256 hex");

        let hit = lookup_cached_digest(&cache_path, &model_file);
        assert_eq!(hit.as_deref(), Some(digest.as_str()));

        // Changing the file's content (and thus mtime/size in the general case) invalidates the
        // cache entry keyed by the OLD identity — simulate via a differently-sized file at a new
        // path (mtime granularity on some filesystems is coarse, so size is the reliable probe here).
        let other_file = dir.join("other.gguf");
        std::fs::write(&other_file, b"different weights, different size!!").unwrap();
        assert!(
            lookup_cached_digest(&cache_path, &other_file).is_none(),
            "an unhashed file must miss, never fabricate a digest"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_model_prefers_ollama_manifest_then_file_cache_then_honest_unresolved() {
        let dir = tmp_dir("resolve");
        let cache_path = dir.join("digest-cache.json");
        let models_dir = dir.join("ollama-models");
        std::fs::create_dir_all(&models_dir).unwrap();

        // Nothing resolvable at all → honestly Unresolved.
        let mut model_paths = HashMap::new();
        let r = resolve_model(&models_dir, &cache_path, &model_paths, "mystery-model");
        assert_eq!(r.source, DigestSource::Unresolved);
        assert!(r.digest.is_none());

        // A file-cache entry for a llama.cpp-style raw model file.
        let gguf = dir.join("weights.gguf");
        std::fs::write(&gguf, b"raw weights").unwrap();
        let expected = compute_and_cache_digest(&cache_path, &gguf).unwrap();
        model_paths.insert("local-gguf-model".to_string(), gguf.clone());
        let r2 = resolve_model(&models_dir, &cache_path, &model_paths, "local-gguf-model");
        assert_eq!(r2.source, DigestSource::FileSha256Cached);
        assert_eq!(r2.digest.as_deref(), Some(expected.as_str()));

        // An Ollama manifest takes priority when both would resolve for the same name.
        let manifest_dir = models_dir
            .join("manifests")
            .join("registry.ollama.ai")
            .join("library")
            .join("local-gguf-model");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(
            manifest_dir.join("latest"),
            serde_json::json!({"layers": [{"mediaType": "application/vnd.ollama.image.model", "digest": "sha256:fromollama", "size": 10}]}).to_string(),
        )
        .unwrap();
        let r3 = resolve_model(&models_dir, &cache_path, &model_paths, "local-gguf-model");
        assert_eq!(r3.source, DigestSource::OllamaManifest);
        assert_eq!(r3.digest.as_deref(), Some("fromollama"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
