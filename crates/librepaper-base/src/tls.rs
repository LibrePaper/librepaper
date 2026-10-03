/// Select the TLS provider used by LibrePaper's rustls clients.
///
/// Reqwest, SQLx, and object_store's AWS backend use AWS-LC. The WebSocket
/// client needs the same provider installed explicitly when rustls features
/// have been unified. An embedding application may install its own provider
/// first; that choice is left intact.
pub fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Another thread may install a provider after the check. In that
        // race its provider is already the process-wide choice, so ignore
        // the failed install and let rustls use that provider.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}
