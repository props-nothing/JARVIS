//! The discovery cache: answers keyed by everything that could make one answer wrong for another.
//!
//! The tool fabric requires that a discovery cache key include "server configuration identity,
//! authenticated principal, workspace/account, protocol version, capability set, and list-result
//! version/TTL", and that "invocation revalidates current ownership and permission". The two
//! halves of that sentence are the two things this module does: it makes a **complete key the only
//! construction** (see [`super::registry::DiscoveryCacheKey`]), and it makes a cached answer
//! **expire** rather than merely age, so the second half has something to revalidate against.
//!
//! A cache here is not an optimization. It is where the contract's scoping rule is enforced in
//! practice: the lookup is a lookup on the *whole* key, so there is no code path that could return
//! one scope's answer for another's. The failure this prevents has a name in this project — the
//! idempotency key that named three of five dimensions and read as complete — and the remedy is
//! the same: make the partial key unrepresentable rather than remembered.

use std::collections::BTreeMap;

use crate::time::{IsoDate, UtcTimestamp};

use super::definition::ToolDefinition;
use super::registry::{DiscoveryCacheKey, RegisteredTool};

/// How long a discovery answer stays usable, in days.
///
/// A bound rather than an open-ended entry: a cached catalog that never expires keeps offering a
/// tool that the server removed, and a caller has no signal that anything changed. One day is
/// short enough that a removed tool stops being offered within a working session and long enough
/// that a busy daemon does not re-handshake on every turn.
pub const DISCOVERY_TTL_DAYS: i64 = 1;

/// The greatest number of cache entries one cache holds.
///
/// The key space is a product of five scopes, so it is larger than it looks: an unbounded cache
/// would grow with the number of distinct principals and protocol versions rather than with the
/// number of servers. Bounded here so the growth is capped, and the eviction is defined below
/// rather than left to whichever entry happens to be last.
pub const MAX_DISCOVERY_ENTRIES: usize = 512;

/// The definitions discovered for one scope, with the revision they were computed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredCatalog {
    /// Where the answer came from. The cache's `get` returns this so a caller can report which
    /// revision it was served without a second lookup.
    pub key: DiscoveryCacheKey,
    /// Which server answered.
    pub server: super::registry::ServerConfigId,
    /// The definitions, ordered by identity so the answer is stable across calls.
    pub tools: Vec<ToolDefinition>,
    /// When the entry was stored.
    pub stored_at: UtcTimestamp,
    /// The last day it may be used without revalidating. Inclusive, matching the model
    /// gateway's evidence rule — a day named as the last valid day is valid on that day.
    pub revalidate_by: IsoDate,
}

impl DiscoveredCatalog {
    /// Returns whether the answer is still usable on `today`.
    #[must_use]
    pub fn is_fresh_on(&self, today: IsoDate) -> bool {
        self.revalidate_by.is_no_earlier_than(today)
    }

    /// Returns the capabilities this answer offers, as a set.
    ///
    /// Derived from the tools rather than stored, so it cannot disagree with them.
    #[must_use]
    pub fn capabilities(&self) -> std::collections::BTreeSet<super::identity::ToolCapability> {
        self.tools
            .iter()
            .map(|tool| tool.identity.capability.clone())
            .collect()
    }
}

/// Builds the answer for one scope from a set of registered tools.
///
/// **The filter is the scope, and it is applied before anything is returned** rather than applied
/// by the caller afterwards. The tool fabric's selection rule puts grants and availability first,
/// and this is the availability half: a tool registered by a *different* server is not part of this
/// server's answer, and offering it would tell one server's principal that another server exists.
///
/// Takes the definitions it may offer rather than reaching into the registry, so the filter is a
/// pure function of its inputs and can be tested without a registry.
#[must_use]
pub fn catalog_for(
    key: DiscoveryCacheKey,
    offered: impl IntoIterator<Item = RegisteredTool>,
    stored_at: UtcTimestamp,
    revalidate_by: IsoDate,
) -> DiscoveredCatalog {
    let mut tools: Vec<ToolDefinition> = offered
        .into_iter()
        .filter(|registered| registered.server == key.scope.server)
        .map(|registered| registered.definition)
        .collect();
    // Ordered by identity: `RegisteredTool`s arrive in identity order from the registry, but this
    // does not rely on that, because a caller may pass anything and the answer's shape is a
    // contract rather than an accident of the input.
    tools.sort_by(|left, right| left.identity.cmp(&right.identity));
    DiscoveredCatalog {
        server: key.scope.server.clone(),
        key,
        tools,
        stored_at,
        revalidate_by,
    }
}

/// A cache of discovery answers, keyed by every dimension that could make one wrong for another.
#[derive(Debug, Default)]
pub struct DiscoveryCache {
    entries: BTreeMap<DiscoveryCacheKey, DiscoveredCatalog>,
    /// The number of lookups that found nothing, so an operator can see whether the scoping is
    /// working rather than inferring it from latency. A count rather than a log line, because a
    /// cache miss on a correct key is ordinary and must not be reported as a fault.
    misses: u64,
    /// The number of lookups that found a **stale** entry. Separate from `misses` because the two
    /// are different facts: a miss means no answer was computed for this scope, while a stale hit
    /// means one was and it can no longer be used — the case the contract's "stale discovery cache
    /// and source replacement" test is about.
    stale: u64,
}

impl DiscoveryCache {
    /// Builds an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns how many entries are cached.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the miss and stale counts.
    #[must_use]
    pub fn counters(&self) -> (u64, u64) {
        (self.misses, self.stale)
    }

    /// Stores an answer.
    ///
    /// The entry's own key is the map key, so a caller cannot store an answer under a key that
    /// disagrees with it — which would be the way to reintroduce the scoping defect through the
    /// back door while every type still looked right.
    pub fn store(&mut self, catalog: DiscoveredCatalog) {
        // Evict when full, oldest first, before inserting. Defined here rather than left to the
        // map's iteration order: an undefined eviction is one an operator cannot reason about, and
        // it would also make a test that asserted a specific survivor pass or fail by accident.
        while self.entries.len() >= MAX_DISCOVERY_ENTRIES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, catalog)| catalog.stored_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.entries.insert(catalog.key.clone(), catalog);
    }

    /// Returns a fresh answer for `key` on `today`, or `None`.
    ///
    /// A stale entry is **left in place** and counted, rather than removed and reported as a miss.
    /// The difference is what a caller can act on: "no answer was ever computed" and "the answer
    /// you have is out of date" lead to different next steps, and a cache that collapsed them
    /// would make a correctly-expiring entry look like a scope that was never asked about.
    pub fn get_fresh(
        &mut self,
        key: &DiscoveryCacheKey,
        today: IsoDate,
    ) -> Option<&DiscoveredCatalog> {
        // The freshness check and the counter update need the entry, and the borrow has to end
        // before the returned reference is produced, so the decision is taken first.
        let verdict = match self.entries.get(key) {
            None => Verdict::Missing,
            Some(catalog) if catalog.is_fresh_on(today) => Verdict::Fresh,
            Some(_) => Verdict::Stale,
        };
        match verdict {
            Verdict::Missing => {
                self.misses += 1;
                None
            }
            Verdict::Stale => {
                self.stale += 1;
                None
            }
            Verdict::Fresh => self.entries.get(key),
        }
    }

    /// Removes every entry for one server, returning how many were removed.
    ///
    /// **The mechanism for "source replacement".** When a server's configuration changes or it is
    /// re-added, every answer computed from it is wrong — not merely old — because the tools it
    /// offers may all be gone. Dropping them by server rather than by key means a caller does not
    /// have to enumerate the principals and protocol versions that happened to query it, which is
    /// the enumeration a caller would get wrong.
    pub fn invalidate_server(&mut self, server: &super::registry::ServerConfigId) -> usize {
        let before = self.entries.len();
        self.entries.retain(|key, _| &key.scope.server != server);
        before - self.entries.len()
    }

    /// Removes every entry whose list version is not `current` for that key's server.
    ///
    /// The version half of "stale discovery cache": a server that reports a new list version has
    /// redefined its catalog, so an answer at the old version describes tools that may no longer
    /// exist. Keyed on the server because list versions are the server's own sequence and are not
    /// comparable across servers.
    pub fn invalidate_below_version(
        &mut self,
        server: &super::registry::ServerConfigId,
        current: &super::registry::ListVersion,
    ) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|key, _| &key.scope.server != server || &key.list_version == current);
        before - self.entries.len()
    }

    /// Removes every entry, returning how many were removed.
    pub fn clear(&mut self) -> usize {
        let removed = self.entries.len();
        self.entries.clear();
        removed
    }
}

/// The outcome of a cache lookup, computed before the borrow is returned.
enum Verdict {
    Missing,
    Stale,
    Fresh,
}
