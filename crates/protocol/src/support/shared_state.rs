use crate::chunked::{ChunkSeq, SealedMark};
use crate::compact::{AUTO_CLIENT_BASE, ClientId, Entry};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Chunk capacity for the executed state; measured, and must be a power of two.
pub const K_STATE: usize = 8;

pub type ExecSeq = ChunkSeq<Entry, K_STATE>;

/// Auto-lane slots a wire state may name; the lane is a dense vector, so this caps what one frame can make a receiver allocate (128 MiB).
pub const MAX_WIRE_AUTO_SLOTS: u32 = 1 << 24;

/// Whether the replicated state machine is execute-once (cf. Raft client sessions, Ongaro 2014 §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatedCommit {
    /// Every committed entry executes, as the boxes print.
    #[default]
    Execute,
    /// Execute-once: an entry with sn(x) ≤ sn(c) leaves S_i unchanged; commits must come in sn order.
    Skip,
}

/// An execute-once commit of client `client`'s `sn` while S_i holds only up to `committed_sn` < sn − 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfSnOrder {
    pub client: ClientId,
    pub sn: u64,
    pub committed_sn: u64,
}

/// The shared state S_i: executed commands in commit order plus each client's committed sn.
#[derive(Debug, Default)]
pub struct SharedState {
    executed: ExecSeq,
    sn_manual: BTreeMap<ClientId, u64>,
    // slot = id − base; 0 is absence, since a committed sn is always ≥ 1.
    sn_auto: Vec<u64>,
    sn_auto_present: u32,
}

impl SharedState {
    pub fn from_entries(executed: Vec<Entry>, sn: BTreeMap<ClientId, u64>) -> Self {
        let mut state = SharedState {
            executed: ExecSeq::from_vec(executed),
            sn_manual: BTreeMap::new(),
            sn_auto: Vec::new(),
            sn_auto_present: 0,
        };
        for (client, sn) in sn {
            state.sn_insert(client, sn);
        }
        state
    }

    pub fn sn_get(&self, client: ClientId) -> Option<u64> {
        if client < AUTO_CLIENT_BASE {
            self.sn_manual.get(&client).copied()
        } else {
            match self.sn_auto.get((client - AUTO_CLIENT_BASE) as usize) {
                Some(0) | None => None,
                Some(&sn) => Some(sn),
            }
        }
    }

    pub fn sn_insert(&mut self, client: ClientId, sn: u64) {
        if client < AUTO_CLIENT_BASE {
            self.sn_manual.insert(client, sn);
        } else {
            assert!(sn != 0, "committed sn 0 is absence in the auto lane");
            let slot = (client - AUTO_CLIENT_BASE) as usize;
            if slot >= self.sn_auto.len() {
                self.sn_auto.resize(slot + 1, 0);
            }
            if self.sn_auto[slot] == 0 {
                self.sn_auto_present += 1;
            }
            self.sn_auto[slot] = sn;
        }
    }

    pub fn sn_len(&self) -> usize {
        self.sn_manual.len() + self.sn_auto_present as usize
    }

    pub fn sn_iter(&self) -> impl Iterator<Item = (ClientId, u64)> + '_ {
        self.sn_manual.iter().map(|(&c, &sn)| (c, sn)).chain(
            self.sn_auto
                .iter()
                .enumerate()
                .filter(|&(_, &sn)| sn != 0)
                .map(|(i, &sn)| (AUTO_CLIENT_BASE + i as ClientId, sn)),
        )
    }

    pub fn logical_len(&self) -> u64 {
        self.executed.end()
    }

    pub fn executed_offset(&self) -> u64 {
        self.executed.start()
    }

    pub fn executed(&self) -> ExecutedView<'_> {
        ExecutedView(&self.executed)
    }

    pub fn untruncated(&self) -> &ExecSeq {
        assert!(
            self.executed.start() == 0
                && self.executed.retained_len() as u64 == self.executed.end(),
            "the whole executed sequence was asked of a truncated state \
             (offset {}, retained {} of {})",
            self.executed.start(),
            self.executed.retained_len(),
            self.executed.end()
        );
        &self.executed
    }

    /// §4 p. 25: forget committed commands; `upto` is clamped to this server's own length.
    pub fn forget_committed_prefix(&mut self, upto: u64) {
        if self.forget_is_noop(upto) {
            return;
        }
        self.executed.drop_below(upto.min(self.executed.end()));
    }

    pub fn forget_is_noop(&self, upto: u64) -> bool {
        upto.min(self.executed.end()) <= self.executed.start()
    }

    /// Entries physically held; instrumentation only, no protocol decision may read it.
    pub fn retained_len(&self) -> usize {
        self.executed.retained_len()
    }

    pub fn executed_chunk_ptrs(&self) -> impl Iterator<Item = *const Entry> + '_ {
        self.executed.chunk_ptrs()
    }

    pub fn executed_sealed_count(&self) -> usize {
        self.executed.sealed_count()
    }

    pub(crate) fn push_executed(&mut self, entry: Entry) {
        self.executed.push(entry);
    }

    /// Executes one committed entry: a command extends S and sets sn(c); ⊥ sets sn(c) only; x_d is a no-op.
    pub(crate) fn execute(&mut self, entry: &Entry) {
        match entry {
            Entry::Cmd(x) => {
                self.push_executed(Entry::Cmd(*x));
                // DETERMINIZED: [DET-9] sn(c) := sn(x); the box's "increments" agrees when commits come in sn order
                self.sn_insert(x.client, x.sn);
            }
            Entry::Null { client, sn } => {
                self.sn_insert(*client, *sn);
            }
            Entry::Nop(_) => {}
        }
    }

    /// Whether `entries`, committed in order under [`RepeatedCommit::Skip`], keep every client's
    /// commits in sn order; the first entry that skips an sn is the error.
    pub(crate) fn check_sn_order<'a>(
        &self,
        entries: impl IntoIterator<Item = &'a Entry>,
    ) -> Result<(), OutOfSnOrder> {
        let mut advanced: BTreeMap<ClientId, u64> = BTreeMap::new();
        for (client, sn) in entries.into_iter().filter_map(Entry::client_sn) {
            let done = advanced
                .get(&client)
                .copied()
                .or_else(|| self.sn_get(client))
                .unwrap_or(0);
            if sn <= done {
                continue;
            }
            if sn != done + 1 {
                return Err(OutOfSnOrder {
                    client,
                    sn,
                    committed_sn: done,
                });
            }
            advanced.insert(client, sn);
        }
        Ok(())
    }

    /// Commits one entry; returns false iff the execute-once state machine left it out.
    /// Under Skip the caller has passed [`Self::check_sn_order`] for the sequence this entry is in.
    pub(crate) fn commit(&mut self, entry: &Entry, repeated: RepeatedCommit) -> bool {
        if repeated == RepeatedCommit::Skip
            && let Some((client, sn)) = entry.client_sn()
        {
            // sn(c) survives forgetting; the executed list does not.
            let done = self.sn_get(client).unwrap_or(0);
            // EXTENSION: [EXT-3] opt-in execute-once state machine: sn(x) ≤ sn(c) leaves S_i unchanged; needs sn-ordered commits
            if sn <= done {
                return false;
            }
            assert!(
                sn == done + 1,
                "{entry:?} committed out of sn order (sn(c) = {done}): execute-once would later drop the skipped sn"
            );
        }
        self.execute(entry);
        true
    }

    /// Representation equality that answers false, never panics, on diverged forget lines.
    pub fn content_eq(&self, other: &Self) -> bool {
        self.executed.represents_same_as(&other.executed)
            && self.sn_len() == other.sn_len()
            && self.sn_iter().eq(other.sn_iter())
    }
}

/// A borrowed executed sequence in logical indices; entries below `offset` are gone.
#[derive(Debug, Clone, Copy)]
pub struct ExecutedView<'a>(&'a ExecSeq);

impl<'a> ExecutedView<'a> {
    pub fn of(seq: &'a ExecSeq) -> Self {
        ExecutedView(seq)
    }

    pub fn offset(&self) -> u64 {
        self.0.start()
    }

    pub fn logical_len(&self) -> u64 {
        self.0.end()
    }

    pub fn to_vec(&self) -> Vec<Entry> {
        self.0.to_vec()
    }

    pub fn iter_from(self, from: u64) -> impl Iterator<Item = &'a Entry> {
        self.0.iter_from(from)
    }

    /// The entry at a logical index; None past the end, panics below the offset.
    pub fn get(self, i: u64) -> Option<&'a Entry> {
        assert!(
            i >= self.0.start(),
            "logical index {i} is below the forgotten prefix (offset {})",
            self.0.start()
        );
        self.0.get(i)
    }

    pub fn position_from(self, entry: &Entry, from: u64) -> Option<u64> {
        self.0.position_from(from, |e| e == entry)
    }

    pub fn for_each_range(self, from: u64, upto: u64, f: impl FnMut(u64, &Entry)) {
        self.0.for_each_range(from, upto, f);
    }

    pub fn eq_slice_from(self, from: u64, other: &[Entry]) -> bool {
        self.0.eq_slice_from(from, other)
    }

    pub fn last_sealed(self) -> Option<SealedMark<Entry>> {
        self.0.last_sealed()
    }

    pub fn holds(self, mark: &SealedMark<Entry>) -> bool {
        self.0.holds(mark)
    }
}

/// Copy-on-write points that may deep-copy a shared state.
pub const CLONE_SITE_COMPACT_COW_APPEND: usize = 0;
pub const CLONE_SITE_COMPACT_COW_FORGET: usize = 1;
pub const CLONE_SITE_RECOVERY_COW_EXECUTE: usize = 2;
pub const CLONE_SITE_RECOVERY_COW_FORGET: usize = 3;
pub const CLONE_SITES: usize = 4;

static STATE_CLONES: AtomicU64 = AtomicU64::new(0);
static STATE_CLONE_ENTRIES: AtomicU64 = AtomicU64::new(0);
static STATE_CLONE_SN: AtomicU64 = AtomicU64::new(0);
static STATE_CLONE_HANDLES: AtomicU64 = AtomicU64::new(0);
static SITE_HITS: [AtomicU64; CLONE_SITES] = [const { AtomicU64::new(0) }; CLONE_SITES];

pub fn note_state_clone_site(site: usize) {
    SITE_HITS[site].fetch_add(1, Ordering::Relaxed);
}

pub fn state_clone_stats() -> (u64, u64, u64, u64, [u64; CLONE_SITES]) {
    let mut sites = [0u64; CLONE_SITES];
    for (i, s) in sites.iter_mut().enumerate() {
        *s = SITE_HITS[i].load(Ordering::Relaxed);
    }
    (
        STATE_CLONES.load(Ordering::Relaxed),
        STATE_CLONE_ENTRIES.load(Ordering::Relaxed),
        STATE_CLONE_SN.load(Ordering::Relaxed),
        STATE_CLONE_HANDLES.load(Ordering::Relaxed),
        sites,
    )
}

impl Clone for SharedState {
    fn clone(&self) -> Self {
        STATE_CLONES.fetch_add(1, Ordering::Relaxed);
        STATE_CLONE_ENTRIES.fetch_add(self.executed.retained_len() as u64, Ordering::Relaxed);
        STATE_CLONE_SN.fetch_add(
            (self.sn_manual.len() + self.sn_auto.len()) as u64,
            Ordering::Relaxed,
        );
        STATE_CLONE_HANDLES.fetch_add(self.executed.sealed_count() as u64, Ordering::Relaxed);
        SharedState {
            executed: self.executed.clone(),
            sn_manual: self.sn_manual.clone(),
            sn_auto: self.sn_auto.clone(),
            sn_auto_present: self.sn_auto_present,
        }
    }
}

// Deterministic under parallel stepping: the stage-(d) inbox pins donor refcounts ≥ 2.
pub(crate) fn cow_state(state: &mut Arc<SharedState>, site: usize) -> &mut SharedState {
    if Arc::strong_count(state) > 1 {
        note_state_clone_site(site);
    }
    Arc::make_mut(state)
}

// Asserts on equal-length states with different offsets; cross-node code uses `content_eq`.
impl PartialEq for SharedState {
    fn eq(&self, other: &Self) -> bool {
        if self.executed.end() != other.executed.end() {
            return false;
        }
        assert!(
            self.executed.start() == other.executed.start(),
            "shared states of equal length disagree on what was forgotten \
             (offsets {} and {}) — one round truncates with one frontier",
            self.executed.start(),
            other.executed.start()
        );
        self.executed == other.executed
            && self.sn_len() == other.sn_len()
            && self.sn_iter().eq(other.sn_iter())
    }
}

impl Eq for SharedState {}

// The wire form is the materialized sequence: bincode is not self-describing. It holds the whole
// executed list, so a state-carrying reply grows with the history and meets a frame cap in long runs.
#[cfg(feature = "serde")]
impl serde::Serialize for SharedState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        struct SnMap<'a>(&'a SharedState);
        impl serde::Serialize for SnMap<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                let mut m = serializer.serialize_map(Some(self.0.sn_len()))?;
                for (client, sn) in self.0.sn_iter() {
                    m.serialize_entry(&client, &sn)?;
                }
                m.end()
            }
        }
        let mut st = serializer.serialize_struct("SharedState", 2)?;
        st.serialize_field("executed", &self.untruncated().to_vec())?;
        st.serialize_field("sn", &SnMap(self))?;
        st.end()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for SharedState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(rename = "SharedState")]
        struct Wire {
            executed: Vec<Entry>,
            sn: BTreeMap<ClientId, u64>,
        }
        let wire = Wire::deserialize(deserializer)?;
        if let Some((client, _)) = wire
            .sn
            .iter()
            .find(|&(&c, &sn)| c >= AUTO_CLIENT_BASE && sn == 0)
        {
            return Err(serde::de::Error::custom(format!(
                "committed sn 0 for auto-lane client {client} is absence, not a value"
            )));
        }
        if let Some((&client, _)) = wire.sn.last_key_value()
            && client >= AUTO_CLIENT_BASE + MAX_WIRE_AUTO_SLOTS
        {
            return Err(serde::de::Error::custom(format!(
                "auto-lane client {client} is past the {MAX_WIRE_AUTO_SLOTS}-slot wire limit"
            )));
        }
        Ok(SharedState::from_entries(wire.executed, wire.sn))
    }
}

#[cfg(test)]
mod tests;
