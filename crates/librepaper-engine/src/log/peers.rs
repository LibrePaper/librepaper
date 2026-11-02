//! Which Loro peers wrote an update.
//!
//! A Loro update carries the operations it adds, tagged with the PeerID of
//! whoever made them. Reading that tag is how the server learns who typed a
//! character without decoding the document.

use loro::LoroDoc;

/// The Loro peers that wrote operations in one update blob. Empty if the blob does not decode.
///
/// A peer is included when its counter in the blob's end version vector is
/// greater than in its start version vector, so a peer that only appears
/// as a dependency of the blob's operations is left out.
pub fn peers_in_update(bytes: &[u8]) -> Vec<u64> {
    peers_where(bytes, |start, end| end > start)
}

/// The peers this update starts: they write operations here and none before it.
///
/// A new browser session always starts its PeerID at counter 0. A modified
/// client can instead reuse a PeerID from before authorship was recorded and
/// send one small edit under it; that peer continues past 0 in this update,
/// so it is left out and its older text is not claimed for the sender. Only
/// the browser path uses this. Nothing is refused: an unrecorded peer stays
/// "Unknown author".
pub fn peers_started_in_update(bytes: &[u8]) -> Vec<u64> {
    peers_where(bytes, |start, end| start == 0 && end > start)
}

/// The peers in one update blob whose start and end counters satisfy `keep`.
/// Empty if the blob does not decode. Sorted and deduplicated.
fn peers_where(bytes: &[u8], keep: impl Fn(i32, i32) -> bool) -> Vec<u64> {
    let Ok(meta) = LoroDoc::decode_import_blob_meta(bytes, true) else {
        return Vec::new();
    };
    let mut peers: Vec<u64> = meta
        .partial_end_vv
        .iter()
        .filter(|(peer, end)| {
            let start = meta.partial_start_vv.get(*peer).copied().unwrap_or(0);
            keep(start, **end)
        })
        .map(|(peer, _)| *peer)
        .collect();
    peers.sort_unstable();
    peers.dedup();
    peers
}

#[cfg(test)]
mod tests {
    use loro::{ExportMode, LoroDoc};

    use super::{peers_in_update, peers_started_in_update};

    #[test]
    fn reports_each_peer_that_wrote_in_the_update() {
        let first = LoroDoc::new();
        first.set_peer_id(1).unwrap();
        first.get_text("text").insert(0, "hello").unwrap();
        first.commit();
        let second = LoroDoc::new();
        second.set_peer_id(2).unwrap();
        second.get_text("text").insert(0, "world").unwrap();
        second.commit();

        let from_first = first.export(ExportMode::all_updates()).unwrap();
        assert_eq!(peers_in_update(&from_first), vec![1]);

        // Import the first update into the second doc and export everything
        // from the second doc's point of view: now both peers are in the blob.
        second.import(&from_first).unwrap();
        let combined = second.export(ExportMode::all_updates()).unwrap();
        assert_eq!(peers_in_update(&combined), vec![1, 2]);
    }

    #[test]
    fn started_peers_include_a_fresh_session() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        doc.get_text("text").insert(0, "hello").unwrap();
        doc.commit();

        let update = doc.export(ExportMode::all_updates()).unwrap();
        assert_eq!(peers_started_in_update(&update), vec![1]);
    }

    #[test]
    fn continuing_a_peer_does_not_start_it() {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        doc.get_text("text").insert(0, "hello").unwrap();
        doc.commit();
        let before = doc.oplog_vv();
        doc.get_text("text").insert(5, " world").unwrap();
        doc.commit();

        let update = doc.export(ExportMode::updates(&before)).unwrap();
        assert!(peers_started_in_update(&update).is_empty());
        assert_eq!(peers_in_update(&update), vec![1]);
    }

    #[test]
    fn a_new_peer_is_started_while_a_continuing_peer_is_not() {
        // Peer 1 writes "hello" and then "!". Peer 2 imports both and writes "world".
        let first = LoroDoc::new();
        first.set_peer_id(1).unwrap();
        first.get_text("text").insert(0, "hello").unwrap();
        first.commit();
        let base = first.oplog_vv();
        first.get_text("text").insert(5, "!").unwrap();
        first.commit();

        let second = LoroDoc::new();
        second.set_peer_id(2).unwrap();
        second
            .import(&first.export(ExportMode::all_updates()).unwrap())
            .unwrap();
        second.get_text("text").insert(0, "world").unwrap();
        second.commit();

        // Peer 1 continues past its end at `base`, so the blob has peer 1 as
        // a continuing writer and peer 2 as a new one.
        let combined = second.export(ExportMode::updates(&base)).unwrap();
        assert_eq!(peers_in_update(&combined), vec![1, 2]);
        assert_eq!(peers_started_in_update(&combined), vec![2]);
    }

    #[test]
    fn undecodable_bytes_name_no_peers() {
        assert!(peers_in_update(b"not a loro update").is_empty());
    }
}
