//! Canonical spill file: one committed stream per run plus per-node forget offsets per T-window.

use crate::smr::observe::canonical::CanonicalOrder;
use protocol::compact::Entry;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Off under `SIM_SPILL=off` or `SIM_RUNLOG=off`; read once.
pub(in crate::smr) fn spill_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("SIM_SPILL").as_deref() != Ok("off")
            && std::env::var("SIM_RUNLOG").as_deref() != Ok("off")
    })
}

pub(in crate::smr) const NO_SPILL_DIAGNOSIS: &str = "diagnosis unavailable: this run was configured with SIM_SPILL=off, so no \
     canonical stream was written to read the divergence back from. Re-run the \
     same commit and seed without SIM_SPILL=off for the exact report.";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SpillHeader {
    pub commit: String,
    pub dirty: bool,
    pub seed: u64,
    pub n: usize,
    pub t_window: u64,
    pub proto: String,
}

/// One T-window boundary; `appended` is the committed suffix it added.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SpillBoundary {
    pub w: u64,
    pub round: usize,
    pub appended: Vec<Entry>,
    pub offsets: Vec<u64>,
    pub lens: Vec<u64>,
    pub release: u64,
    pub digest: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SpillTrailer {
    /// Set once the run stopped being one canonical stream; boundary records stop there.
    pub diverged: bool,
    pub boundaries: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Record {
    Header(SpillHeader),
    Boundary(SpillBoundary),
    Trailer(SpillTrailer),
}

/// A parsed spill; `trailer` is `None` when the run did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spill {
    pub header: SpillHeader,
    pub boundaries: Vec<SpillBoundary>,
    pub trailer: Option<SpillTrailer>,
}

impl Spill {
    /// Parse JSONL; filesystem-free so the browser shares this reader.
    pub fn parse(text: &str) -> Result<Spill, String> {
        let mut header = None;
        let mut boundaries = Vec::new();
        let mut trailer = None;
        for (i, line) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let record: Record =
                serde_json::from_str(line).map_err(|e| format!("spill line {}: {e}", i + 1))?;
            match record {
                Record::Header(h) if header.is_none() && boundaries.is_empty() => {
                    header = Some(h);
                }
                Record::Header(_) => return Err(format!("spill line {}: second header", i + 1)),
                Record::Boundary(b) => {
                    if trailer.is_some() {
                        return Err(format!("spill line {}: boundary after trailer", i + 1));
                    }
                    boundaries.push(b);
                }
                Record::Trailer(t) => trailer = Some(t),
            }
        }
        Ok(Spill {
            header: header.ok_or("spill has no header record")?,
            boundaries,
            trailer,
        })
    }

    /// The canonical committed stream through boundary `b`, inclusive.
    pub fn canonical_through(&self, b: usize) -> Vec<Entry> {
        self.boundaries[..=b.min(self.boundaries.len().saturating_sub(1))]
            .iter()
            .flat_map(|r| r.appended.iter().cloned())
            .collect()
    }

    pub fn node_view(&self, b: usize, node: usize) -> Result<Vec<Entry>, String> {
        let boundary = self
            .boundaries
            .get(b)
            .ok_or_else(|| format!("no boundary {b} in this spill"))?;
        let len = *boundary
            .lens
            .get(node)
            .ok_or_else(|| format!("no node {node} in this spill"))? as usize;
        let canonical = self.canonical_through(b);
        if len > canonical.len() {
            return Err(format!(
                "boundary {b}: node {node} executed {len} commands but the \
                 canonical stream only reaches {}",
                canonical.len()
            ));
        }
        Ok(canonical[..len].to_vec())
    }

    /// Recompute the digest chain and compare it with every boundary's recorded digest.
    pub fn verify(&self) -> Result<(), String> {
        let mut chain = CanonicalOrder::new();
        for (b, boundary) in self.boundaries.iter().enumerate() {
            chain.extend_from_slice(&boundary.appended);
            let recomputed = format!("{:016x}", chain.digest_at(chain.len()));
            if recomputed != boundary.digest {
                return Err(format!(
                    "boundary {b} (round {}): the reconstructed stream digests to \
                     {recomputed}, the file records {}",
                    boundary.round, boundary.digest
                ));
            }
        }
        Ok(())
    }
}

pub fn read_spill(path: &Path) -> Result<Spill, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read spill {}: {e}", path.display()))?;
    Spill::parse(&text)
}

/// Cold path, run on a driver-bug abort: where a node's executed sequence leaves the spill.
pub(in crate::smr) fn diagnose(
    path: Option<&Path>,
    node: usize,
    round: usize,
    view: protocol::shared_state::ExecutedView<'_>,
) -> String {
    let Some(path) = path else {
        return NO_SPILL_DIAGNOSIS.to_string();
    };
    let spill = match read_spill(path) {
        Ok(s) => s,
        Err(e) => return format!("diagnosis unavailable: {e}"),
    };
    if let Err(e) = spill.verify() {
        return format!("the spill does not verify, so it cannot answer for the run: {e}");
    }
    let canonical = spill.canonical_through(spill.boundaries.len().saturating_sub(1));
    let offset = view.offset() as usize;
    for (i, entry) in view.iter_from(view.offset()).enumerate() {
        let at = offset + i;
        match canonical.get(at) {
            Some(canon) if canon == entry => {}
            Some(canon) => {
                return format!(
                    "node {node}, round {round}: first difference at committed position \
                     {at} — the node holds {entry:?}, the canonical stream holds {canon:?}"
                );
            }
            None => {
                return format!(
                    "node {node}, round {round}: the node holds {entry:?} at committed \
                     position {at}, past the end of the canonical stream ({})",
                    canonical.len()
                );
            }
        }
    }
    format!(
        "node {node}, round {round}: the node's retained tail [{offset}, {}) agrees with \
         the canonical stream entry for entry, so the divergence lies below the node's \
         own forget line, where nothing retains it",
        view.logical_len()
    )
}

pub(in crate::smr) struct SpillWriter {
    path: PathBuf,
    file: std::fs::File,
    written: u64,
    boundaries: usize,
    diverged: bool,
}

impl SpillWriter {
    /// `tag` names the full scenario, so runs that differ only in a parameter keep distinct files.
    pub(in crate::smr) fn create(header: SpillHeader, tag: Option<&str>) -> Result<Self, String> {
        let dir = crate::runlog::default_log_path()
            .parent()
            .map(|p| p.join("spills"))
            .ok_or("no ledger directory to place the spill beside")?;
        let tag = tag.map_or_else(|| crate::runlog::params_tag(&header), str::to_string);
        Self::create_in(&dir, header, &tag)
    }

    fn create_in(dir: &Path, header: SpillHeader, tag: &str) -> Result<Self, String> {
        let stem = crate::runlog::tagged_artifact_stem("smr-", header.n, header.seed, tag);
        let (mut file, path) = crate::runlog::create_new_unique(dir, &stem, "jsonl")?;
        write_record(&mut file, &Record::Header(header))?;
        Ok(SpillWriter {
            path,
            file,
            written: 0,
            boundaries: 0,
            diverged: false,
        })
    }

    pub(in crate::smr) fn path(&self) -> &Path {
        &self.path
    }

    pub(in crate::smr) fn mark_diverged(&mut self) {
        self.diverged = true;
    }

    /// `canonical` must already hold this boundary's suffix, and nothing may have forgotten yet.
    pub(in crate::smr) fn boundary(
        &mut self,
        canonical: &CanonicalOrder,
        w: u64,
        round: usize,
        offsets: Vec<u64>,
        lens: Vec<u64>,
        release: u64,
    ) -> Result<(), String> {
        if self.diverged {
            return Ok(());
        }
        let reach = canonical.len();
        if self.written < canonical.start() {
            return Err(format!(
                "boundary at round {round}: the canonical order was released to \
                 {} but the spill has only written {} — the stream would have a \
                 hole in it",
                canonical.start(),
                self.written
            ));
        }
        let appended = canonical.slice(self.written, reach).to_vec();
        let record = SpillBoundary {
            w,
            round,
            appended,
            offsets,
            lens,
            release,
            digest: format!("{:016x}", canonical.digest_at(reach)),
        };
        write_record(&mut self.file, &Record::Boundary(record))?;
        self.file.flush().map_err(|e| format!("spill flush: {e}"))?;
        self.written = reach;
        self.boundaries += 1;
        Ok(())
    }

    pub(in crate::smr) fn finish(&mut self) -> Result<(), String> {
        write_record(
            &mut self.file,
            &Record::Trailer(SpillTrailer {
                diverged: self.diverged,
                boundaries: self.boundaries,
            }),
        )?;
        self.file.flush().map_err(|e| format!("spill flush: {e}"))
    }
}

fn write_record(file: &mut std::fs::File, record: &Record) -> Result<(), String> {
    writeln!(
        file,
        "{}",
        serde_json::to_string(record).expect("spill record serializes")
    )
    .map_err(|e| format!("spill write: {e}"))
}

#[cfg(test)]
mod tests;
