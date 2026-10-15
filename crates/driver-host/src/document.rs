use crate::protocol::*;
use futures_util::TryStreamExt;
use mongodb::{
    Client,
    bson::{Document, doc},
    options::{ClientOptions, Credential, ServerAddress, Tls, TlsOptions},
};
use scylla::{
    client::{
        PoolSize, execution_profile::ExecutionProfile, session::Session,
        session_builder::SessionBuilder,
    },
    response::{PagingState, PagingStateResponse},
    statement::unprepared::Statement,
};
use serde_json::Value;
use std::{collections::VecDeque, sync::Arc, time::Duration};

pub struct Http {
    client: reqwest::Client,
    base: reqwest::Url,
    user: String,
    password: zeroize::Zeroizing<String>,
}
impl Http {
    pub fn new(p: &Value, password: &str) -> Result<Self> {
        let host = string(p, "host");
        if host.is_empty() || host.chars().any(|c| "/?#@\\".contains(c) || c.is_control()) {
            return Err("drivers.invalid_target");
        }
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.into()
        };
        let scheme = if string(p, "tls") == "Disabled" {
            "http"
        } else {
            "https"
        };
        let base =
            reqwest::Url::parse(&format!("{scheme}://{host}:{}", p["port"])).map_err(|_| FAILED)?;
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(
                p["timeout_seconds"].as_u64().unwrap_or(10).clamp(1, 60),
            ))
            .timeout(Duration::from_secs(execution_seconds(p)));
        if string(p, "tls") == "Required" {
            builder = builder.danger_accept_invalid_certs(true);
        }
        if !string(p, "ca").is_empty() {
            let pem = std::fs::read(string(p, "ca")).map_err(|_| "drivers.tls_failed")?;
            for cert in
                reqwest::Certificate::from_pem_bundle(&pem).map_err(|_| "drivers.tls_failed")?
            {
                builder = builder.add_root_certificate(cert);
            }
        }
        if !string(p, "client_cert").is_empty() {
            let mut pem =
                std::fs::read(string(p, "client_cert")).map_err(|_| "drivers.tls_failed")?;
            pem.extend(std::fs::read(string(p, "client_key")).map_err(|_| "drivers.tls_failed")?);
            builder = builder
                .identity(reqwest::Identity::from_pem(&pem).map_err(|_| "drivers.tls_failed")?);
        }
        Ok(Self {
            client: builder.build().map_err(|_| FAILED)?,
            base,
            user: string(p, "user").into(),
            password: zeroize::Zeroizing::new(password.into()),
        })
    }
    pub async fn request(
        &self,
        segments: &[&str],
        body: Option<String>,
        content_type: &str,
    ) -> Result<Value> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| FAILED)?
            .clear()
            .extend(segments);
        if segments == ["_cat", "indices"] {
            url.set_query(Some("format=json&h=index&s=index"));
        }
        let mut request = if let Some(body) = body {
            self.client
                .post(url)
                .header("Content-Type", content_type)
                .body(body)
        } else {
            self.client.get(url)
        };
        request = request.basic_auth(&self.user, Some(self.password.as_str()));
        let mut response = request
            .send()
            .await
            .map_err(|_| FAILED)?
            .error_for_status()
            .map_err(|_| FAILED)?;
        if response.content_length().is_some_and(|n| n > LIMIT as u64) {
            return Err(TOO_LARGE);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| FAILED)? {
            if bytes.len() + chunk.len() > LIMIT {
                return Err(TOO_LARGE);
            }
            bytes.extend(chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| FAILED)
    }
    pub async fn sql(&self, db: &str, sql: &str, maximum: usize) -> Result<Rows> {
        let segments = if db.is_empty() {
            vec!["rest", "sql"]
        } else {
            vec!["rest", "sql", db]
        };
        let v = self
            .request(&segments, Some(sql.into()), "text/plain; charset=utf-8")
            .await?;
        if v["code"].as_i64().unwrap_or(0) != 0 {
            return Err(FAILED);
        }
        let mut rows = Rows::new(
            v["column_meta"]
                .as_array()
                .ok_or(FAILED)?
                .iter()
                .map(|c| c[0].as_str().unwrap_or("").into())
                .collect(),
        )?;
        for row in v["data"].as_array().ok_or(FAILED)?.iter().take(maximum) {
            rows.push(
                row.as_array()
                    .ok_or(FAILED)?
                    .iter()
                    .map(cell)
                    .collect::<Result<Vec<_>>>()?,
            )?;
        }
        Ok(rows)
    }
}
pub fn mongo_auth_source(p: &Value) -> &str {
    let source = string(p, "mongo_auth_source").trim();
    if source.is_empty() { "admin" } else { source }
}
pub fn mongo_ping_database(p: &Value) -> &str {
    let database = string(p, "database");
    if database.is_empty() {
        mongo_auth_source(p)
    } else {
        database
    }
}
pub fn mongo_error(error: mongodb::error::Error) -> &'static str {
    use mongodb::error::ErrorKind;
    match error.kind.as_ref() {
        ErrorKind::Authentication { .. } => "mongo.error.authentication",
        ErrorKind::Command(command) if command.code == 18 => "mongo.error.authentication",
        ErrorKind::Command(command) if command.code == 13 => "mongo.error.permission",
        ErrorKind::Io(_) | ErrorKind::ServerSelection { .. } => "mongo.error.network",
        _ => FAILED,
    }
}
pub async fn mongo(p: &Value, password: &str) -> Result<Client> {
    if !string(p, "client_cert").is_empty() {
        return Err("drivers.tls_options_unsupported");
    }
    let mut options = ClientOptions::default();
    options.hosts = vec![ServerAddress::Tcp {
        host: string(p, "host").into(),
        port: Some(p["port"].as_u64().ok_or(FAILED)? as u16),
    }];
    options.max_pool_size = Some(1);
    options.min_pool_size = Some(0);
    options.retry_writes = Some(false);
    options.retry_reads = Some(false);
    let timeout = Duration::from_secs(p["timeout_seconds"].as_u64().unwrap_or(10).clamp(1, 60));
    options.connect_timeout = Some(timeout);
    options.server_selection_timeout = Some(timeout);
    if !string(p, "user").is_empty() {
        options.credential = Some(
            Credential::builder()
                .username(string(p, "user").to_owned())
                .password(password.to_owned())
                .source(mongo_auth_source(p).to_owned())
                .build(),
        );
    }
    if string(p, "tls") != "Disabled" {
        options.tls = Some(Tls::Enabled(
            TlsOptions::builder()
                .allow_invalid_certificates(string(p, "tls") == "Required")
                .ca_file_path(if string(p, "ca").is_empty() {
                    None
                } else {
                    Some(std::path::PathBuf::from(string(p, "ca")))
                })
                .build(),
        ));
    }
    let client = Client::with_options(options).map_err(mongo_error)?;
    client
        .database(mongo_ping_database(p))
        .run_command(doc! {"ping":1})
        .await
        .map_err(mongo_error)?;
    Ok(client)
}
pub async fn mongo_page(
    c: &Client,
    db: &str,
    name: &str,
    size: usize,
    offset: u64,
) -> Result<turbodbn_core::mongo::Page> {
    let started = std::time::Instant::now();
    let mut cursor = c
        .database(db)
        .collection::<Document>(name)
        .find(doc! {})
        .skip(offset)
        .limit((size + 1) as i64)
        .batch_size(1)
        .await
        .map_err(mongo_error)?;
    let mut documents = Vec::new();
    let mut bytes = 0;
    let mut more = false;
    while let Some(doc) = cursor.try_next().await.map_err(mongo_error)? {
        if documents.len() == size {
            more = true;
            break;
        }
        let source = mongodb::bson::Bson::Document(doc)
            .into_canonical_extjson()
            .to_string();
        if source.len() > turbodbn_core::mongo::DOCUMENT_BYTES {
            return Err(TOO_LARGE);
        }
        bytes += source.len();
        if bytes > turbodbn_core::mongo::PAGE_BYTES {
            return Err(TOO_LARGE);
        }
        documents.push(source);
    }
    Ok(turbodbn_core::mongo::Page {
        documents,
        offset: offset as usize,
        more,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}
pub async fn mongo_write(
    c: &Client,
    db: &str,
    change: &turbodbn_core::mongo::Mutation,
) -> Result<bool> {
    let (old, new) = turbodbn_core::mongo::validate_mutation(change)?;
    let mut collections = c
        .database(db)
        .list_collections()
        .filter(doc! {"name":&change.collection})
        .await
        .map_err(mongo_error)?;
    let collection = collections
        .try_next()
        .await
        .map_err(mongo_error)?
        .ok_or("mongo.document.denied")?;
    if collection.collection_type != mongodb::results::CollectionType::Collection
        || collection.info.read_only
    {
        return Err("mongo.document.denied");
    }
    let old = mongodb::bson::Bson::try_from(old).map_err(|_| "mongo.document.invalid")?;
    let new = mongodb::bson::Bson::try_from(new).map_err(|_| "mongo.document.invalid")?;
    let (Some(old), Some(new)) = (old.as_document(), new.as_document()) else {
        return Err("mongo.document.invalid");
    };
    // A round-trip validates canonical BSON wrappers and immutable identity after decoding.
    if !change.clone_document && (old.get("_id").is_none() || old.get("_id") != new.get("_id")) {
        return Err("mongo.document.id_immutable");
    }
    let target = c.database(db).collection::<Document>(&change.collection);
    if change.clone_document {
        if new.contains_key("_id") {
            return Err("mongo.document.id_immutable");
        }
        target.insert_one(new.clone()).await.map_err(mongo_error)?;
        Ok(true)
    } else {
        let filter = doc! {"_id":old.get("_id").unwrap().clone(),"$expr":{"$eq":["$$ROOT",{"$literal":old.clone()}]}};
        let result = target
            .replace_one(filter, new.clone())
            .collation(
                mongodb::options::Collation::builder()
                    .locale("simple")
                    .build(),
            )
            .await
            .map_err(mongo_error)?;
        if result.matched_count != 1 {
            return Err("mongo.document.conflict");
        }
        Ok(false)
    }
}
pub struct Cassandra {
    session: Session,
    pages: VecDeque<(String, u64, usize, PagingState)>,
}
impl Cassandra {
    pub async fn query_bounded(&self, sql: &str, maximum: usize) -> Result<Rows> {
        let mut state = PagingState::start();
        let mut result = Rows::default();
        loop {
            let (page, next) = self
                .query(sql, maximum - result.values.len(), state)
                .await?;
            if result.columns.is_empty() {
                result = Rows::new(page.columns)?;
            }
            let empty = page.values.is_empty();
            for row in page.values {
                result.push(row)?;
            }
            if result.values.len() >= maximum {
                break;
            }
            match next {
                Some(s) if !empty => state = s,
                Some(_) => return Err(FAILED),
                None => break,
            }
        }
        Ok(result)
    }
    pub async fn connect(p: &Value, password: &str) -> Result<Self> {
        let host = string(p, "host");
        let address = if host.contains(':') {
            format!("[{host}]:{}", p["port"])
        } else {
            format!("{host}:{}", p["port"])
        };
        let profile = ExecutionProfile::builder()
            .request_timeout(Some(Duration::from_secs(execution_seconds(p))))
            .build()
            .into_handle();
        let mut b = SessionBuilder::new()
            .default_execution_profile_handle(profile)
            .known_node(address)
            .pool_size(PoolSize::PerHost(std::num::NonZeroUsize::new(1).unwrap()))
            .connection_timeout(Duration::from_secs(
                p["timeout_seconds"].as_u64().unwrap_or(10).clamp(1, 60),
            ));
        if !string(p, "user").is_empty() {
            b = b.user(string(p, "user"), password);
        }
        if string(p, "tls") != "Disabled" {
            b = b.tls_context(Some(tls(p)?));
        }
        Ok(Self {
            session: b.build().await.map_err(|_| FAILED)?,
            pages: VecDeque::new(),
        })
    }
    pub async fn query(
        &self,
        sql: &str,
        size: usize,
        state: PagingState,
    ) -> Result<(Rows, Option<PagingState>)> {
        let mut statement = Statement::new(sql);
        statement.set_page_size(size as i32);
        let (result, next) = self
            .session
            .query_single_page(statement, &[], state)
            .await
            .map_err(|_| FAILED)?;
        let result = result.into_rows_result().map_err(|_| FAILED)?;
        if result.rows_bytes_size() > LIMIT {
            return Err(TOO_LARGE);
        }
        let mut rows = Rows::new(
            result
                .column_specs()
                .iter()
                .map(|c| c.name().to_owned())
                .collect(),
        )?;
        for row in result.rows::<scylla::value::Row>().map_err(|_| FAILED)? {
            rows.push(
                row.map_err(|_| FAILED)?
                    .columns
                    .into_iter()
                    .map(|v| {
                        v.map(|v| match v {
                            scylla::value::CqlValue::Text(s)
                            | scylla::value::CqlValue::Ascii(s) => s,
                            other => other.to_string(),
                        })
                    })
                    .collect(),
            )?;
        }
        Ok((
            rows,
            match next {
                PagingStateResponse::HasMorePages { state } => Some(state),
                PagingStateResponse::NoMorePages => None,
            },
        ))
    }
    pub async fn page(&mut self, sql: &str, size: usize, offset: u64) -> Result<(Rows, bool)> {
        let state = if offset == 0 {
            PagingState::start()
        } else {
            self.pages
                .iter()
                .find(|(s, o, n, _)| s == sql && *o == offset && *n == size)
                .map(|(_, _, _, s)| s.clone())
                .ok_or("drivers.offset_unsupported")?
        };
        let (rows, next) = self.query(sql, size, state).await?;
        let more = next.is_some();
        if let Some(state) = next {
            if self.pages.len() >= 64 {
                self.pages.pop_front();
            }
            self.pages
                .push_back((sql.into(), offset + size as u64, size, state));
        }
        Ok((rows, more))
    }
}
fn tls(p: &Value) -> Result<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    if !string(p, "ca").is_empty() {
        let bytes = std::fs::read(string(p, "ca")).map_err(|_| "drivers.tls_failed")?;
        for cert in rustls_pemfile::certs(&mut bytes.as_slice()) {
            roots
                .add(cert.map_err(|_| "drivers.tls_failed")?)
                .map_err(|_| "drivers.tls_failed")?;
        }
    }
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|_| "drivers.tls_failed")?
        .with_root_certificates(roots);
    let mut config = if string(p, "client_cert").is_empty() {
        builder.with_no_client_auth()
    } else {
        let pem = std::fs::read(string(p, "client_cert")).map_err(|_| "drivers.tls_failed")?;
        let key = std::fs::read(string(p, "client_key")).map_err(|_| "drivers.tls_failed")?;
        builder
            .with_client_auth_cert(
                rustls_pemfile::certs(&mut pem.as_slice())
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|_| "drivers.tls_failed")?,
                rustls_pemfile::private_key(&mut key.as_slice())
                    .map_err(|_| "drivers.tls_failed")?
                    .ok_or("drivers.tls_failed")?,
            )
            .map_err(|_| "drivers.tls_failed")?
    };
    if string(p, "tls") == "Required" {
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(EncryptedOnly(provider)));
    }
    Ok(Arc::new(config))
}
#[derive(Debug)]
struct EncryptedOnly(Arc<rustls::crypto::CryptoProvider>);
impl rustls::client::danger::ServerCertVerifier for EncryptedOnly {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        s: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, s, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        s: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, s, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod mongo_configuration_tests {
    use super::{mongo_auth_source, mongo_ping_database};
    #[tokio::test]
    async fn wire_fixture_preserves_bson_and_guards_document_writes() {
        use crate::{adapter::Adapter, mongo_fixture::Fixture};
        use serde_json::{Value, json};
        use turbodbn_core::mongo::{Mutation, Page, project};
        let fixture = Fixture::start();
        let profile = json!({"driver_id":"mongodb","host":"127.0.0.1","port":fixture.port,"timeout_seconds":3,"tls":"Disabled","database":"fixture","user":"","mongo_auth_source":"fixture","mongo_read_only":false});
        let mut adapter = Adapter::connect(profile.clone(), "").await.unwrap();
        let page=adapter.operation(&json!({"Workbench":{"database":"fixture","action":{"GridPage":{"object":{"name":"sample"},"options":{"page_size":2,"offset":0}}}}})).await.unwrap();
        let page: Page = serde_json::from_value(page["Workbench"]["MongoPage"].clone()).unwrap();
        assert_eq!(page.documents.len(), 2);
        assert!(page.more);
        let projection = project(&page, &[]).unwrap();
        assert_eq!(
            projection.documents[0]["long"]["$numberLong"],
            i64::MAX.to_string()
        );
        assert!(projection.types.contains(&"Int32".into()));
        assert!(projection.types.contains(&"Array".into()));
        let original = page.documents[0].clone();
        let mut next: Value = serde_json::from_str(&original).unwrap();
        next["r"] = json!({"$numberInt":"99"});
        let mut mutation = Mutation {
            collection: "sample".into(),
            original,
            replacement: next.to_string(),
            clone_document: false,
            confirmed: false,
        };
        let operation =
            |m: &Mutation| json!({"Workbench":{"database":"fixture","action":{"MongoWrite":m}}});
        assert_eq!(
            adapter.operation(&operation(&mutation)).await.unwrap_err(),
            "mongo.document.denied"
        );
        assert_eq!(fixture.state.lock().unwrap().writes, 0);
        mutation.confirmed = true;
        adapter.operation(&operation(&mutation)).await.unwrap();
        assert_eq!(fixture.state.lock().unwrap().writes, 1);
        assert_eq!(
            adapter.operation(&operation(&mutation)).await.unwrap_err(),
            "mongo.document.conflict"
        );
        assert_eq!(fixture.state.lock().unwrap().writes, 1);
        next.as_object_mut().unwrap().remove("_id");
        mutation.clone_document = true;
        mutation.replacement = next.to_string();
        adapter.operation(&operation(&mutation)).await.unwrap();
        assert_eq!(fixture.state.lock().unwrap().writes, 2);
        fixture.state.lock().unwrap().deny = true;
        assert_eq!(
            adapter.operation(&operation(&mutation)).await.unwrap_err(),
            "mongo.error.permission"
        );
        assert_eq!(fixture.state.lock().unwrap().writes, 2);
        fixture.state.lock().unwrap().view = true;
        assert_eq!(
            adapter.operation(&operation(&mutation)).await.unwrap_err(),
            "mongo.document.denied"
        );
        let mut profile = profile;
        profile["mongo_read_only"] = json!(true);
        let mut readonly = Adapter::connect(profile, "").await.unwrap();
        assert_eq!(
            readonly.operation(&operation(&mutation)).await.unwrap_err(),
            "mongo.document.denied"
        );
        assert!(adapter.operation(&json!({"Workbench":{"database":"fixture","action":{"Execute":{"sql":"DELETE"}}}})).await.is_err());
    }
    use serde_json::json;
    #[test]
    fn authentication_source_and_target_database_are_independent() {
        let profile = json!({"mongo_auth_source":"account_auth","database":"business"});
        assert_eq!(mongo_auth_source(&profile), "account_auth");
        assert_eq!(mongo_ping_database(&profile), "business");
        assert_eq!(
            mongo_ping_database(&json!({"mongo_auth_source":"account_auth"})),
            "account_auth"
        );
        assert_eq!(mongo_auth_source(&json!({"database":"business"})), "admin");
        assert_eq!(mongo_auth_source(&json!({"mongo_auth_source":""})), "admin");
    }
}
