//! Provider trait and registry implementation.
//!
//! See module-level docs in [`super`](crate::mention) for the Phase D
//! design rationale and the FFI/thread-safety contract.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

/// A host-implemented provider of custom @-mention kinds.
///
/// Examples: `agent`, `tool`, `skill`, `image`, `mcp_resource`.
///
/// Implementors must be **cheap to clone behind an `Arc`** — the registry
/// hands out `Arc<dyn MentionProvider>` clones on every `get`. Methods
/// are synchronous and `&self`; for I/O, spawn a worker and return
/// results. The trait has no generic parameters, so `Arc<dyn MentionProvider>`
/// is FFI-safe (no monomorphization, no `Self: Sized`).
pub trait MentionProvider: Send + Sync {
    /// The @-kind this provider serves, e.g. `"agent"`, `"tool"`, `"skill"`,
    /// `"image"`, `"mcp_resource"`. Must be unique within a registry.
    /// Returned as `&'static str` because the kind is part of the type's
    /// identity — it cannot change at runtime.
    fn kind(&self) -> &'static str;

    /// Best-effort completion suggestions for a partial key. The provider
    /// decides how to score and rank — the host just displays the top
    /// `limit` results. Returning an empty `Vec` means "no suggestions",
    /// which is distinct from an error.
    fn suggest(&self, query: &str, limit: usize) -> Vec<ExternalMentionCandidate>;

    /// Resolve a fully-typed key into a concrete payload. Returning
    /// `Err(ProviderError::NotFound(_))` tells the host the key is not
    /// recognized by this provider.
    fn resolve(&self, key: &str) -> Result<ExternalResolveResult, ProviderError>;

    /// Whether this provider performs only local computation. Override
    /// to return `false` for providers that fetch over the network
    /// (the *oh-my-pi* invariant: a single `resolve` may not perform
    /// network I/O without explicit user consent).
    fn is_local(&self) -> bool {
        true
    }
}

/// A single completion suggestion returned from [`MentionProvider::suggest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalMentionCandidate {
    /// The unique key the host should pass back to
    /// [`MentionProvider::resolve`] when this candidate is selected.
    pub key: String,
    /// Human-readable display name shown in the completion popup.
    pub display: String,
    /// Optional one-line description / subtitle.
    pub description: Option<String>,
}

/// The concrete payload returned from [`MentionProvider::resolve`].
///
/// Each variant carries the data needed by the host to *act on* the
/// mention (e.g. spawn an agent, call a tool, embed an image).
/// `Other(String)` is an escape hatch for host-defined kinds that don't
/// fit the built-in variants; hosts may serialize it as opaque JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalResolveResult {
    /// Spawn hint for an agent. The host decides how to interpret it
    /// (subprocess, RPC, in-process task, etc.).
    Agent { spawn_hint: String },
    /// A callable tool / function signature (typically a JSON Schema or
    /// language-specific signature string).
    Tool { signature: String },
    /// A skill body (markdown / natural-language instructions) and the
    /// on-disk location the body was loaded from.
    Skill { body: String, location: PathBuf },
    /// An image, inlined as base64 with its MIME type.
    Image { base64: String, mime: String },
    /// A resource exposed by an MCP server.
    McpResource { uri: String, content: String },
    /// Opaque fallback for kinds that don't fit the variants above.
    /// Hosts should preserve the string verbatim and surface it to the
    /// model as raw text.
    Other(String),
}

/// Errors a provider can return from [`MentionProvider::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The key is not recognized by this provider. The host may try a
    /// different provider, fall back to a file lookup, or surface a
    /// "not found" error to the user.
    NotFound(String),
    /// A local I/O error (file read failed, permission denied, etc.).
    /// The wrapped `String` is a human-readable diagnostic.
    IoError(String),
    /// A network error. Providers that ever return this should override
    /// [`MentionProvider::is_local`] to return `false`.
    NetworkError(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::NotFound(k) => write!(f, "not found: {k}"),
            ProviderError::IoError(m) => write!(f, "io error: {m}"),
            ProviderError::NetworkError(m) => write!(f, "network error: {m}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// A registry of `MentionProvider`s keyed by [`MentionProvider::kind`].
///
/// The registry is `!Sync`-in-itself but the trait object inside is
/// `Send + Sync`, so `Arc<ProviderRegistry>` is safe to share across
/// threads. Lookups are O(1) average (hash map).
pub struct ProviderRegistry {
    providers: HashMap<&'static str, Arc<dyn MentionProvider>>,
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider. The provider's `kind()` becomes the lookup
    /// key. Re-registering the same kind replaces the previous entry —
    /// useful for hot-reload scenarios in long-lived host processes.
    pub fn register(&mut self, provider: Arc<dyn MentionProvider>) {
        self.providers.insert(provider.kind(), provider);
    }

    /// Look up a provider by kind. Returns `None` if no provider is
    /// registered for `kind`.
    pub fn get(&self, kind: &str) -> Option<Arc<dyn MentionProvider>> {
        self.providers.get(kind).cloned()
    }

    /// All registered kinds, in arbitrary order. The returned slice
    /// borrows from the registry, so it lives at most as long as `self`.
    pub fn list_kinds(&self) -> Vec<&'static str> {
        self.providers.keys().copied().collect()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A minimal provider used by most tests. Always local by default;
    /// `network()` flips `is_local` to `false` for the oh-my-pi guard.
    struct StubProvider {
        kind: &'static str,
        candidates: Vec<ExternalMentionCandidate>,
        local: bool,
        resolve: Box<dyn Fn(&str) -> Result<ExternalResolveResult, ProviderError> + Send + Sync>,
    }

    impl StubProvider {
        fn new(kind: &'static str, candidates: Vec<ExternalMentionCandidate>) -> Self {
            Self {
                kind,
                candidates,
                local: true,
                resolve: Box::new(|key| Err(ProviderError::NotFound(key.to_string()))),
            }
        }

        fn with_resolver(
            mut self,
            f: impl Fn(&str) -> Result<ExternalResolveResult, ProviderError> + Send + Sync + 'static,
        ) -> Self {
            self.resolve = Box::new(f);
            self
        }

        fn network(mut self) -> Self {
            self.local = false;
            self
        }
    }

    impl MentionProvider for StubProvider {
        fn kind(&self) -> &'static str {
            self.kind
        }

        fn suggest(&self, query: &str, limit: usize) -> Vec<ExternalMentionCandidate> {
            self.candidates
                .iter()
                .filter(|c| query.is_empty() || c.key.contains(query) || c.display.contains(query))
                .take(limit)
                .cloned()
                .collect()
        }

        fn resolve(&self, key: &str) -> Result<ExternalResolveResult, ProviderError> {
            (self.resolve)(key)
        }

        fn is_local(&self) -> bool {
            self.local
        }
    }

    // ----- tests -----

    #[test]
    fn empty_registry_has_no_kinds() {
        let r = ProviderRegistry::new();
        assert!(r.list_kinds().is_empty());
        assert!(r.get("anything").is_none());
    }

    #[test]
    fn default_registry_matches_new() {
        let r = ProviderRegistry::default();
        assert!(r.list_kinds().is_empty());
    }

    #[test]
    fn register_and_lookup_by_kind() {
        let mut r = ProviderRegistry::new();
        let p = Arc::new(StubProvider::new("agent", vec![]));
        r.register(p);
        let got = r.get("agent").expect("agent should be registered");
        assert_eq!(got.kind(), "agent");
    }

    #[test]
    fn list_kinds_returns_all_registered() {
        let mut r = ProviderRegistry::new();
        r.register(Arc::new(StubProvider::new("agent", vec![])));
        r.register(Arc::new(StubProvider::new("tool", vec![])));
        r.register(Arc::new(StubProvider::new("skill", vec![])));
        let mut kinds = r.list_kinds();
        kinds.sort_unstable();
        assert_eq!(kinds, vec!["agent", "skill", "tool"]);
    }

    #[test]
    fn multiple_providers_of_different_kinds_coexist() {
        let mut r = ProviderRegistry::new();
        r.register(Arc::new(StubProvider::new("agent", vec![])));
        r.register(Arc::new(StubProvider::new("tool", vec![])));
        assert_eq!(r.get("agent").unwrap().kind(), "agent");
        assert_eq!(r.get("tool").unwrap().kind(), "tool");
        assert!(r.get("skill").is_none());
    }

    #[test]
    fn reregister_replaces_existing_provider() {
        let mut r = ProviderRegistry::new();
        r.register(Arc::new(StubProvider::new("agent", vec![])));
        r.register(Arc::new(StubProvider::new(
            "agent",
            vec![ExternalMentionCandidate {
                key: "v2".into(),
                display: "v2".into(),
                description: None,
            }],
        )));
        // Re-registering the same kind replaces, not duplicates.
        assert_eq!(r.list_kinds().len(), 1);
        let p = r.get("agent").unwrap();
        let cands = p.suggest("", 10);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].key, "v2");
    }

    #[test]
    fn suggest_returns_candidates_matching_query() {
        let cands = vec![
            ExternalMentionCandidate {
                key: "reviewer".into(),
                display: "Reviewer".into(),
                description: Some("Reviews code".into()),
            },
            ExternalMentionCandidate {
                key: "writer".into(),
                display: "Writer".into(),
                description: Some("Writes docs".into()),
            },
        ];
        let p = StubProvider::new("agent", cands);
        // Query "rev" should match only the reviewer.
        let got = p.suggest("rev", 10);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "reviewer");
    }

    #[test]
    fn suggest_respects_limit() {
        let cands: Vec<ExternalMentionCandidate> = (0..5)
            .map(|i| ExternalMentionCandidate {
                key: format!("k{i}"),
                display: format!("d{i}"),
                description: None,
            })
            .collect();
        let p = StubProvider::new("tool", cands);
        let got = p.suggest("", 2);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn suggest_empty_query_returns_all_capped_by_limit() {
        let cands: Vec<ExternalMentionCandidate> = (0..3)
            .map(|i| ExternalMentionCandidate {
                key: format!("k{i}"),
                display: format!("d{i}"),
                description: None,
            })
            .collect();
        let p = StubProvider::new("tool", cands);
        let got = p.suggest("", 100);
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn resolve_returns_ok_for_known_key() {
        let p = StubProvider::new("agent", vec![]).with_resolver(|key| {
            Ok(ExternalResolveResult::Agent {
                spawn_hint: format!("spawn {key}"),
            })
        });
        let got = p.resolve("reviewer").expect("resolve should succeed");
        match got {
            ExternalResolveResult::Agent { spawn_hint } => {
                assert_eq!(spawn_hint, "spawn reviewer");
            }
            _ => panic!("expected Agent variant"),
        }
    }

    #[test]
    fn resolve_returns_not_found_for_unknown_key() {
        let p = StubProvider::new("tool", vec![]);
        let err = p.resolve("nope").unwrap_err();
        assert_eq!(err, ProviderError::NotFound("nope".into()));
    }

    #[test]
    fn is_local_defaults_to_true() {
        let p = StubProvider::new("agent", vec![]);
        assert!(p.is_local());
    }

    #[test]
    fn is_local_can_be_overridden_to_false() {
        let p = StubProvider::new("skill", vec![]).network();
        assert!(!p.is_local());
    }

    #[test]
    fn registry_get_returns_arc_clones() {
        let mut r = ProviderRegistry::new();
        let p: Arc<dyn MentionProvider> = Arc::new(StubProvider::new("agent", vec![]));
        r.register(Arc::clone(&p));
        let a = r.get("agent").unwrap();
        let b = r.get("agent").unwrap();
        // Arc::ptr_eq compares the underlying pointer.
        assert!(Arc::ptr_eq(&a, &b));
        assert!(Arc::ptr_eq(&a, &p));
    }

    #[test]
    fn trait_object_is_send_and_sync() {
        // This is a *compile-time* check: the function will not type-check
        // if `dyn MentionProvider` is not `Send + Sync`.
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn MentionProvider>();
    }

    #[test]
    fn arc_dyn_provider_is_send_and_sync() {
        // Belt-and-braces: the *Arc* wrapping the trait object must also
        // be `Send + Sync` for the registry to be shareable across threads.
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<Arc<dyn MentionProvider>>();
    }

    #[test]
    fn provider_with_shared_state_is_send_sync() {
        // A provider that owns a Mutex is still `Send + Sync` because the
        // Mutex itself is. This guards against accidental `Rc` / `*mut`
        // usage slipping into providers.
        struct CountingProvider {
            count: Mutex<u32>,
        }
        impl MentionProvider for CountingProvider {
            fn kind(&self) -> &'static str {
                "counter"
            }
            fn suggest(&self, _query: &str, _limit: usize) -> Vec<ExternalMentionCandidate> {
                *self.count.lock().unwrap() += 1;
                vec![]
            }
            fn resolve(&self, _key: &str) -> Result<ExternalResolveResult, ProviderError> {
                Err(ProviderError::NotFound("n/a".into()))
            }
        }
        let p: Arc<dyn MentionProvider> = Arc::new(CountingProvider {
            count: Mutex::new(0),
        });
        // Use it from the type system to confirm it satisfies the bounds.
        let mut r = ProviderRegistry::new();
        r.register(p);
        assert_eq!(r.get("counter").unwrap().kind(), "counter");
    }

    #[test]
    fn provider_error_display_is_readable() {
        let e = ProviderError::NotFound("k".into());
        assert_eq!(e.to_string(), "not found: k");
        let e = ProviderError::IoError("read failed".into());
        assert_eq!(e.to_string(), "io error: read failed");
        let e = ProviderError::NetworkError("dns".into());
        assert_eq!(e.to_string(), "network error: dns");
    }

    #[test]
    fn external_resolve_result_variants_are_constructable() {
        // Smoke-test every variant to make sure the public surface
        // stays usable. Phase C serializes these through the C ABI JSON
        // bridge, so any future change here is a breaking change.
        let _ = ExternalResolveResult::Agent {
            spawn_hint: "x".into(),
        };
        let _ = ExternalResolveResult::Tool {
            signature: "fn()".into(),
        };
        let _ = ExternalResolveResult::Skill {
            body: "body".into(),
            location: PathBuf::from("/tmp/skill.md"),
        };
        let _ = ExternalResolveResult::Image {
            base64: "AAA=".into(),
            mime: "image/png".into(),
        };
        let _ = ExternalResolveResult::McpResource {
            uri: "fs://x".into(),
            content: "{}".into(),
        };
        let _ = ExternalResolveResult::Other("opaque".into());
    }
}
