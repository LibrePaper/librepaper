/// Select the TLS provider used by LibrePaper's rustls clients.
///
/// Several dependencies enable different rustls crypto providers through
/// Cargo feature unification. In that case rustls cannot infer a provider
/// when a client config is built, so select ring before starting a client
/// connection. An embedding application may install its own provider first;
/// that choice is left intact.
pub(crate) fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Another thread may install a provider after the check. In that
        // race its provider is already the process-wide choice, so ignore
        // the failed install and let rustls use that provider.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}
