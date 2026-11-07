//! Wire types must round-trip under the serde feature — the networked node
//! ships them inside PullReply payloads.
#![cfg(feature = "serde")]

use protocol::compact::{ClientCommand, Entry, SharedState, Timed};
use std::collections::BTreeMap;

#[test]
fn compact_wire_types_roundtrip_json() {
    let cmd = ClientCommand {
        client: 3,
        sn: 2,
        op: 77,
    };
    let entries = vec![
        Entry::Cmd(cmd),
        Entry::Null { client: 4, sn: 1 },
        Entry::Nop(0),
    ];
    let timed = Timed {
        entry: entries[0].clone(),
        round: 9,
    };
    let state = SharedState::from_entries(entries.clone(), BTreeMap::from([(3u32, 2u64), (4, 1)]));

    let cmd2: ClientCommand = serde_json::from_str(&serde_json::to_string(&cmd).unwrap()).unwrap();
    assert_eq!(cmd2, cmd);
    let entries2: Vec<Entry> =
        serde_json::from_str(&serde_json::to_string(&entries).unwrap()).unwrap();
    assert_eq!(entries2, entries);
    let timed2: Timed = serde_json::from_str(&serde_json::to_string(&timed).unwrap()).unwrap();
    assert_eq!(timed2, timed);
    let state2: SharedState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert_eq!(state2, state);
}

#[test]
fn a_wire_state_cannot_demand_an_unbounded_auto_lane() {
    use protocol::compact::AUTO_CLIENT_BASE;
    use protocol::shared_state::MAX_WIRE_AUTO_SLOTS;

    let frame = |client: u32| format!(r#"{{"executed":[],"sn":{{"{client}":3}}}}"#);
    let last_ok = AUTO_CLIENT_BASE + MAX_WIRE_AUTO_SLOTS - 1;
    let state: SharedState = serde_json::from_str(&frame(AUTO_CLIENT_BASE + 4000)).unwrap();
    assert_eq!(state.sn_get(AUTO_CLIENT_BASE + 4000), Some(3));
    assert!(serde_json::from_str::<SharedState>(&frame(last_ok)).is_ok());
    for client in [last_ok + 1, u32::MAX] {
        let refused = serde_json::from_str::<SharedState>(&frame(client)).err();
        assert!(
            refused.is_some_and(|e| e.to_string().contains("auto-lane")),
            "client {client} was not refused"
        );
    }
}
