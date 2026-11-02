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
    let Ok(meta) = LoroDoc::decode_import_blob_meta(bytes, true) else {
        return Vec::new();
    };
    let mut peers: Vec<u64> = meta
        .partial_end_vv
        .iter()
        .filter(|(peer, end)| {
            let start = meta.partial_start_vv.get(*peer).copied().unwrap_or(0);
            **end > start
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

    use super::peers_in_update;

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
    fn undecodable_bytes_name_no_peers() {
        assert!(peers_in_update(b"not a loro update").is_empty());
    }
}
