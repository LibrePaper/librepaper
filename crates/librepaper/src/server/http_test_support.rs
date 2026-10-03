//! Shared PostgreSQL-backed fixture for HTTP history tests.

use std::sync::Arc;

use librepaper_engine::log::Registry;
use crate::room::Rooms;
use crate::server::Server;
use librepaper_engine::storage::blob::FsStore;
use librepaper_engine::storage::postgres::{NewAccount, PostgresCatalog, WriterLease};
use librepaper_engine::storage::store::{DocumentInput, MutationActor, Store};
use librepaper_base::auth::{GithubApp, Policy};
use librepaper_base::config::Configuration;

pub(super) struct Deployment {
    pub(super) server: Server,
    pub(super) catalog: Arc<PostgresCatalog>,
    pub(super) slug: String,
    pub(super) owner_id: uuid::Uuid,
    pub(super) owner_session_generation: String,
    pub(super) _writer: WriterLease,
    _objects: tempfile::TempDir,
}

/// Build a small real server around a single owner and document. Each test
/// supplies its own document shape and policies so its authorization case is
/// visible where the fixture is used.
pub(super) async fn deployment(
    input: DocumentInput,
    figures: Vec<(String, Vec<u8>)>,
    publishers: &str,
    commenters: &str,
) -> Option<Deployment> {
    let catalog = librepaper_engine::testing::catalog().await?;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("owner".into()),
            handle: "owner".into(),
            display_name: "Owner".into(),
            email: None,
        })
        .await
        .unwrap();
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn librepaper_engine::storage::blob::BlobStore> =
        Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "deployment".into(),
    );
    let rooms = Rooms::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        registry.clone(),
    );
    let (worker, background) = librepaper_engine::storage::worker::Worker::new(
        catalog.clone(),
        blobs.clone(),
        registry.clone(),
        config.clone(),
    );
    tokio::spawn(worker.run());
    let store = Store::open_with_catalog(
        blobs.clone(),
        config.clone(),
        catalog.clone(),
        registry.clone(),
    );
    let actor = MutationActor {
        account_id: owner.id.to_string(),
        owner_key: "owner".into(),
        session_generation: owner.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        policy_comment: true,
        automation: false,
        unowned_publisher: false,
    };
    let slug = input.slug.clone();
    store
        .put_directory_as_actor(input, figures, actor)
        .await
        .unwrap();
    // `provider_configured` needs a client id to allow the GitHub identity's
    // edit ceiling. Device tokens do not call out to GitHub.
    let app = GithubApp {
        client_id: "test-client".into(),
        ..GithubApp::default()
    };
    let server = Server::new(
        store,
        rooms,
        background,
        std::collections::HashMap::new(),
        app,
        vec![0u8; 32],
        config,
        Policy::parse_publishers(publishers).unwrap(),
        Policy::parse(commenters),
    );
    Some(Deployment {
        server,
        catalog,
        slug,
        owner_id: owner.id,
        owner_session_generation: owner.session_generation.to_string(),
        _writer: writer,
        _objects: objects,
    })
}

pub(super) fn owner_bearer(deployment: &Deployment) -> axum::http::HeaderMap {
    use axum::http::{HeaderMap, HeaderValue};
    use librepaper_base::auth::{sign_device, Identity, PROVIDER_GITHUB};

    let identity = Identity {
        provider: PROVIDER_GITHUB.into(),
        id: deployment.owner_id.to_string(),
        handle: "owner".into(),
        name: "Owner".into(),
        picture: String::new(),
        session_generation: deployment.owner_session_generation.clone(),
    };
    let token = sign_device(
        &[0u8; 32],
        &identity,
        librepaper_base::util::now_unix() + 3600,
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers
}
