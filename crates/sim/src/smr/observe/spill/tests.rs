use super::*;
use protocol::compact::ClientCommand;

fn cmd(op: u64) -> Entry {
    Entry::Cmd(ClientCommand {
        client: op as u32,
        sn: 1,
        op,
    })
}

fn boundary(w: u64, appended: &[Entry], lens: &[u64], chain: &mut CanonicalOrder) -> String {
    chain.extend_from_slice(appended);
    let record = SpillBoundary {
        w,
        round: (w * 10) as usize,
        appended: appended.to_vec(),
        offsets: lens.to_vec(),
        lens: lens.to_vec(),
        release: 0,
        digest: format!("{:016x}", chain.digest_at(chain.len())),
    };
    serde_json::to_string(&Record::Boundary(record)).expect("serializes")
}

fn file(lines: &[String]) -> String {
    let header = serde_json::to_string(&Record::Header(SpillHeader {
        commit: "deadbeef".into(),
        dirty: false,
        seed: 7,
        n: 2,
        t_window: 10,
        proto: "recovery".into(),
    }))
    .expect("serializes");
    std::iter::once(header)
        .chain(lines.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn two_boundaries() -> String {
    let mut chain = CanonicalOrder::new();
    let a = boundary(1, &[cmd(1), cmd(2)], &[2, 1], &mut chain);
    let b = boundary(2, &[cmd(3), cmd(4)], &[4, 3], &mut chain);
    let trailer = serde_json::to_string(&Record::Trailer(SpillTrailer {
        diverged: false,
        boundaries: 2,
    }))
    .expect("serializes");
    file(&[a, b, trailer])
}

#[test]
fn a_node_view_is_the_canonical_prefix_at_its_own_length() {
    let spill = Spill::parse(&two_boundaries()).expect("parses");
    assert_eq!(spill.node_view(0, 0).expect("view"), vec![cmd(1), cmd(2)]);
    assert_eq!(spill.node_view(0, 1).expect("view"), vec![cmd(1)]);
    assert_eq!(
        spill.node_view(1, 0).expect("view"),
        vec![cmd(1), cmd(2), cmd(3), cmd(4)]
    );
    assert_eq!(
        spill.node_view(1, 1).expect("view"),
        vec![cmd(1), cmd(2), cmd(3)]
    );
}

#[test]
fn a_clean_spill_verifies() {
    Spill::parse(&two_boundaries())
        .expect("parses")
        .verify()
        .expect("verifies");
}

#[test]
fn a_corrupted_entry_fails_the_digest() {
    let mut spill = Spill::parse(&two_boundaries()).expect("parses");
    spill.boundaries[1].appended[0] = cmd(99);
    let err = spill.verify().expect_err("corruption must be caught");
    assert!(err.contains("boundary 1"), "{err}");
}

#[test]
fn a_reordered_prefix_fails_the_digest() {
    let mut spill = Spill::parse(&two_boundaries()).expect("parses");
    spill.boundaries[0].appended.swap(0, 1);
    assert!(spill.verify().is_err(), "a reorder is a different history");
}

#[test]
fn a_corrupted_digest_field_fails_too() {
    let mut spill = Spill::parse(&two_boundaries()).expect("parses");
    spill.boundaries[0].digest = "0000000000000000".into();
    assert!(spill.verify().is_err());
}

#[test]
fn a_file_without_a_trailer_parses_and_says_so() {
    let mut chain = CanonicalOrder::new();
    let a = boundary(1, &[cmd(1)], &[1, 1], &mut chain);
    let spill = Spill::parse(&file(&[a])).expect("parses");
    assert!(spill.trailer.is_none());
    spill.verify().expect("what is there still verifies");
}

#[test]
fn a_headerless_file_is_refused() {
    let mut chain = CanonicalOrder::new();
    let a = boundary(1, &[cmd(1)], &[1, 1], &mut chain);
    assert!(Spill::parse(&a).is_err());
}

fn header() -> SpillHeader {
    SpillHeader {
        commit: "deadbeef".into(),
        dirty: false,
        seed: 7,
        n: 2,
        t_window: 10,
        proto: "recovery".into(),
    }
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spill-names-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn runs_differing_only_in_a_swept_parameter_get_distinct_spill_names() {
    let dir = scratch_dir("tags");
    let a = SpillWriter::create_in(&dir, header(), "aaaaaaaa").expect("creates");
    let b = SpillWriter::create_in(&dir, header(), "bbbbbbbb").expect("creates");
    assert_ne!(a.path(), b.path());
    assert!(
        a.path()
            .to_string_lossy()
            .ends_with("-smr-n2-seed7-aaaaaaaa.jsonl")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_identical_name_is_never_overwritten() {
    let dir = scratch_dir("collide");
    let mut first = SpillWriter::create_in(&dir, header(), "cccccccc").expect("creates");
    first.finish().expect("finishes");
    let before = std::fs::read_to_string(first.path()).expect("reads");
    let second = SpillWriter::create_in(&dir, header(), "cccccccc").expect("creates");
    assert_ne!(first.path(), second.path());
    assert_eq!(
        std::fs::read_to_string(first.path()).expect("reads"),
        before
    );
    let _ = std::fs::remove_dir_all(&dir);
}
