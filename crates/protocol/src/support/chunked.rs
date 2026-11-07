//! A sequence held as sealed immutable chunks plus one owned tail.

use crate::compact::{Entry, Timed};
use std::sync::Arc;

/// Chunk capacity for logs: measured, a power of two, and output-invisible.
pub const CHUNK: usize = 8;

/// Sealed chunks at canonical absolute extents plus the owned tail; `K` must be a power of two.
#[derive(Debug, Clone)]
pub struct ChunkSeq<T, const K: usize = CHUNK> {
    // Every sealed chunk holds exactly `K` entries.
    sealed: Vec<Arc<[T]>>,
    base_chunk: usize,
    tail: Vec<T>,
    start: u64,
}

impl<T: Clone, const K: usize> ChunkSeq<T, K> {
    pub const K: usize = K;

    pub fn new() -> Self {
        ChunkSeq {
            sealed: Vec::new(),
            base_chunk: 0,
            tail: Vec::new(),
            start: 0,
        }
    }

    /// Build from a flat sequence starting at logical 0; same extents as incremental append.
    pub fn from_vec(entries: Vec<T>) -> Self {
        let mut seq = ChunkSeq::new();
        seq.extend(entries);
        seq
    }

    pub fn shared(entries: Vec<T>) -> Arc<Self> {
        Arc::new(Self::from_vec(entries))
    }

    /// Logical position one past the last entry.
    pub fn end(&self) -> u64 {
        ((self.base_chunk + self.sealed.len()) * K + self.tail.len()) as u64
    }

    pub fn len(&self) -> usize {
        (self.end() - self.start) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn start(&self) -> u64 {
        self.start
    }

    /// Entries physically held, including the dead intra-chunk remainder; instrumentation only.
    pub fn retained_len(&self) -> usize {
        self.sealed.len() * K + self.tail.len()
    }

    pub fn chunk_ptrs(&self) -> impl Iterator<Item = *const T> + '_ {
        self.sealed.iter().map(|c| c.as_ptr())
    }

    pub fn sealed_count(&self) -> usize {
        self.sealed.len()
    }

    /// Leading sealed chunks both hold by the same allocation; 0 unless their frames line up.
    pub fn shared_prefix_chunks(&self, other: &Self) -> usize {
        if self.start != other.start || self.base_chunk != other.base_chunk {
            return 0;
        }
        self.sealed
            .iter()
            .zip(other.sealed.iter())
            .take_while(|(a, b)| Arc::ptr_eq(a, b))
            .count()
    }

    pub fn position_from(&self, from: u64, pred: impl Fn(&T) -> bool) -> Option<u64> {
        let from = from.max(self.start);
        let first = ((from / K as u64) as usize).saturating_sub(self.base_chunk);
        for (i, chunk) in self.sealed.iter().enumerate().skip(first) {
            let chunk_start = ((self.base_chunk + i) * K) as u64;
            let skip = from.saturating_sub(chunk_start).min(K as u64) as usize;
            if let Some(off) = chunk[skip..].iter().position(&pred) {
                return Some(chunk_start + (skip + off) as u64);
            }
        }
        let tail_base = ((self.base_chunk + self.sealed.len()) * K) as u64;
        let skip = from.saturating_sub(tail_base).min(self.tail.len() as u64) as usize;
        self.tail[skip..]
            .iter()
            .position(&pred)
            .map(|off| tail_base + (skip + off) as u64)
    }

    /// Walk `[from, upto)` chunk by chunk, passing each entry with its absolute position.
    pub fn for_each_range(&self, from: u64, upto: u64, mut f: impl FnMut(u64, &T)) {
        let from = from.max(self.start);
        if upto <= from {
            return;
        }
        let first = ((from / K as u64) as usize).saturating_sub(self.base_chunk);
        for (i, chunk) in self.sealed.iter().enumerate().skip(first) {
            let chunk_start = ((self.base_chunk + i) * K) as u64;
            if chunk_start >= upto {
                return;
            }
            let lo = from.saturating_sub(chunk_start).min(K as u64) as usize;
            let hi = (upto - chunk_start).min(K as u64) as usize;
            for (j, e) in chunk[lo..hi].iter().enumerate() {
                f(chunk_start + (lo + j) as u64, e);
            }
        }
        let tail_base = ((self.base_chunk + self.sealed.len()) * K) as u64;
        if upto > tail_base {
            let lo = from.saturating_sub(tail_base).min(self.tail.len() as u64) as usize;
            let hi = (upto - tail_base).min(self.tail.len() as u64) as usize;
            for (j, e) in self.tail[lo..hi].iter().enumerate() {
                f(tail_base + (lo + j) as u64, e);
            }
        }
    }

    pub fn eq_slice_from(&self, from: u64, other: &[T]) -> bool
    where
        T: PartialEq,
    {
        let mut at = 0usize;
        let mut equal = true;
        self.for_each_range(from, from + other.len() as u64, |_, e| {
            if equal && (at >= other.len() || other[at] != *e) {
                equal = false;
            }
            at += 1;
        });
        equal && at == other.len()
    }

    /// Strict equality over the frame and every retained entry, as interning requires.
    /// `Eq` makes a shared chunk equal to itself, so it is not compared entry by entry.
    pub fn represents_same_as(&self, other: &Self) -> bool
    where
        T: Eq,
    {
        self.start == other.start
            && self.base_chunk == other.base_chunk
            && self.tail == other.tail
            && self.sealed.len() == other.sealed.len()
            && self
                .sealed
                .iter()
                .zip(other.sealed.iter())
                .all(|(a, b)| Arc::ptr_eq(a, b) || a[..] == b[..])
    }

    /// Live entries covered by the first `chunks` sealed chunks, in relative units.
    pub fn live_len_of_sealed_prefix(&self, chunks: usize) -> usize {
        debug_assert!(chunks <= self.sealed.len(), "more chunks than are sealed");
        (((self.base_chunk + chunks) * K) as u64).saturating_sub(self.start) as usize
    }

    fn seal_full_tail(&mut self) {
        while self.tail.len() >= K {
            let rest = self.tail.split_off(K);
            let full = std::mem::replace(&mut self.tail, rest);
            self.sealed.push(Arc::from(full.into_boxed_slice()));
        }
    }

    pub fn push(&mut self, entry: T) {
        self.tail.push(entry);
        self.seal_full_tail();
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = T>) {
        for e in entries {
            self.tail.push(e);
        }
        self.seal_full_tail();
    }

    /// Adopt `other`'s sealed chunks by handle and copy its tail; `self` must be empty.
    pub fn adopt(&mut self, other: &Self) {
        debug_assert!(
            self.sealed.is_empty() && self.tail.is_empty() && self.start == 0,
            "adopt seeds a fresh sequence"
        );
        self.sealed = other.sealed.clone();
        self.base_chunk = other.base_chunk;
        self.tail = other.tail.clone();
        self.start = other.start;
        self.debug_assert_frame();
    }

    /// The entry at an absolute position, or `None` past the end; panics below `start`.
    pub fn get(&self, at: u64) -> Option<&T> {
        assert!(
            at >= self.start,
            "logical index {at} is below the dropped prefix (start {})",
            self.start
        );
        if at >= self.end() {
            return None;
        }
        let chunk = (at as usize) / K;
        if chunk < self.base_chunk + self.sealed.len() {
            Some(&self.sealed[chunk - self.base_chunk][(at as usize) % K])
        } else {
            let tail_base = (self.base_chunk + self.sealed.len()) * K;
            Some(&self.tail[at as usize - tail_base])
        }
    }

    pub fn iter_from(&self, from: u64) -> impl Iterator<Item = &T> + '_ {
        let from = from.max(self.start);
        let tail_base = ((self.base_chunk + self.sealed.len()) * K) as u64;
        let sealed = self.sealed.iter().enumerate().flat_map(move |(i, chunk)| {
            let chunk_start = ((self.base_chunk + i) * K) as u64;
            let skip = from.saturating_sub(chunk_start).min(K as u64) as usize;
            chunk[skip..].iter()
        });
        let tail =
            self.tail[from.saturating_sub(tail_base).min(self.tail.len() as u64) as usize..].iter();
        sealed.chain(tail)
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        self.iter_from(self.start)
    }

    /// The entry at relative position `rel` of the live window.
    pub fn at(&self, rel: usize) -> Option<&T> {
        self.get(self.start + rel as u64)
    }

    pub fn iter_at(&self, rel: usize) -> impl Iterator<Item = &T> + '_ {
        self.iter_from(self.start + rel as u64)
    }

    pub fn drop_front(&mut self, n: usize) {
        self.drop_below(self.start + n as u64);
    }

    /// Mutable access to the live entry at `rel`, copying its chunk first if shared.
    pub fn make_mut_at(&mut self, rel: usize) -> &mut T
    where
        T: WriteInPlace,
    {
        let at = self.start + rel as u64;
        assert!(at < self.end(), "write past the end at {at}");
        let chunk = (at as usize) / K;
        let sealed_end = self.base_chunk + self.sealed.len();
        if chunk < sealed_end {
            let i = chunk - self.base_chunk;
            // `Arc<[T]>` has no `make_mut`, so copy-on-write is spelled out.
            if Arc::strong_count(&self.sealed[i]) > 1 || Arc::weak_count(&self.sealed[i]) > 0 {
                let owned: Vec<T> = self.sealed[i].to_vec();
                self.sealed[i] = Arc::from(owned.into_boxed_slice());
            }
            let slot = (at as usize) % K;
            Arc::get_mut(&mut self.sealed[i])
                .expect("just made unique")
                .get_mut(slot)
                .expect("slot is inside the chunk")
        } else {
            let tail_base = sealed_end * K;
            &mut self.tail[at as usize - tail_base]
        }
    }

    /// Release everything below `upto` (clamped to the end) without re-chunking survivors.
    pub fn drop_below(&mut self, upto: u64) {
        let upto = upto.min(self.end()).max(self.start);
        if upto == self.start {
            return;
        }
        self.start = upto;
        let keep_from = (upto as usize) / K;
        if keep_from > self.base_chunk {
            let drop = (keep_from - self.base_chunk).min(self.sealed.len());
            self.sealed.drain(..drop);
            self.base_chunk += drop;
        }
        self.debug_assert_frame();
    }

    // Invariant `start >= base_chunk * K`, which `live_len_of_sealed_prefix` relies on.
    fn debug_assert_frame(&self) {
        debug_assert!(
            self.start >= (self.base_chunk * K) as u64,
            "live window starts at {} below its own first chunk at {}",
            self.start,
            self.base_chunk * K
        );
    }

    /// Materialise the live entries; cold paths and tests only.
    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }
}

/// Element types a sequence may overwrite in place; executed commands are not one.
pub trait WriteInPlace {}

impl WriteInPlace for Timed {}

#[cfg(test)]
impl WriteInPlace for u64 {}

/// A sealed chunk and its absolute index; holding the handle keeps the allocation from reuse.
#[derive(Debug, Clone)]
pub struct SealedMark<T> {
    chunk: usize,
    end: u64,
    handle: Arc<[T]>,
}

impl<T> SealedMark<T> {
    /// One past the chunk's last position.
    pub fn end(&self) -> u64 {
        self.end
    }
}

// Entry is not `WriteInPlace`, so a sealed chunk of an Entry sequence is never
// replaced or written: holders gain it only together with every chunk before it
// (clone, adopt) and lose chunks only from the front.
impl<const K: usize> ChunkSeq<Entry, K> {
    pub fn last_sealed(&self) -> Option<SealedMark<Entry>> {
        let handle = self.sealed.last()?;
        let chunk = self.base_chunk + self.sealed.len() - 1;
        Some(SealedMark {
            chunk,
            end: ((chunk + 1) * K) as u64,
            handle: Arc::clone(handle),
        })
    }

    /// True iff this holds the mark's allocation at its index; then it agrees with
    /// every other holder on each position below `mark.end()` that both retain.
    pub fn holds(&self, mark: &SealedMark<Entry>) -> bool {
        mark.chunk
            .checked_sub(self.base_chunk)
            .and_then(|i| self.sealed.get(i))
            .is_some_and(|c| Arc::ptr_eq(c, &mark.handle))
    }
}

impl<T: Clone, const K: usize> Default for ChunkSeq<T, K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone, const K: usize> From<Vec<T>> for ChunkSeq<T, K> {
    fn from(entries: Vec<T>) -> Self {
        ChunkSeq::from_vec(entries)
    }
}

impl<T: Clone, const K: usize> FromIterator<T> for ChunkSeq<T, K> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut seq = ChunkSeq::new();
        seq.extend(iter);
        seq
    }
}

impl<T: Clone + PartialEq, const K: usize> PartialEq for ChunkSeq<T, K> {
    /// Live content only, ignoring layout and `start`.
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<T: Clone + Eq, const K: usize> Eq for ChunkSeq<T, K> {}

impl<T: Clone + PartialEq, const K: usize> PartialEq<Vec<T>> for ChunkSeq<T, K> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<T: Clone + PartialEq, const K: usize, const N: usize> PartialEq<[T; N]> for ChunkSeq<T, K> {
    fn eq(&self, other: &[T; N]) -> bool {
        self.len() == N && self.iter().eq(other.iter())
    }
}

impl<T: Clone + PartialEq, const K: usize> PartialEq<Vec<T>> for &ChunkSeq<T, K> {
    fn eq(&self, other: &Vec<T>) -> bool {
        (**self).eq(other)
    }
}

impl<T: Clone + PartialEq, const K: usize, const N: usize> PartialEq<[T; N]> for &ChunkSeq<T, K> {
    fn eq(&self, other: &[T; N]) -> bool {
        (**self).eq(other)
    }
}

#[cfg(test)]
mod tests;
