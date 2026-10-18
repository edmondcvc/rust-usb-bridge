use anyhow::{bail, Context, Result};
use bridge_common::PairingBundle;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRecord {
    pub name: String,
    pub certificate_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedRecord {
    pub vendor: u16,
    pub product: u16,
    pub serial: Option<String>,
    pub path: String,
    pub original_driver: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub server_id: String,
    pub listen: String,
    pub server_names: Vec<String>,
    pub pair_code: String,
    #[serde(default)]
    pub invitations: BTreeMap<String, InvitationRecord>,
    #[serde(default)]
    pub clients: BTreeMap<String, ClientRecord>,
    #[serde(default)]
    pub shared: BTreeMap<String, SharedRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvitationRecord {
    pub expires_at: u64,
    pub issued_by: String,
}

pub fn require_root(mock: bool) -> Result<()> {
    #[cfg(target_os = "linux")]
    if !mock {
        let status = fs::read_to_string("/proc/self/status")?;
        let effective = status
            .lines()
            .find(|line| line.starts_with("Uid:"))
            .and_then(|line| line.split_whitespace().nth(2));
        if effective != Some("0") {
            bail!("hardware server commands require root; run with sudo");
        }
    }
    #[cfg(not(target_os = "linux"))]
    if !mock {
        bail!("hardware server is Linux-only; --mock is required on this platform");
    }
    let _ = mock;
    Ok(())
}

pub fn secret_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let started = std::time::Instant::now();
    let destination = match path.file_name().and_then(|name| name.to_str()) {
        Some("config.json") => "main_config",
        Some("pair-code.txt") => "pair_code_cache",
        _ => "identity_file",
    };
    let span = tracing::debug_span!("state_write", destination);
    let _entered = span.enter();
    tracing::debug!("state file write started");
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    tracing::debug!(
        elapsed_ms = started.elapsed().as_millis(),
        "state file sync started"
    );
    file.sync_all()?;
    tracing::debug!(
        elapsed_ms = started.elapsed().as_millis(),
        "state file sync completed"
    );
    drop(file);
    // rename replaces the existing file on Unix and Windows. Never remove the
    // committed identity/configuration first and leave a missing-file window.
    fs::rename(&temporary, path)?;
    tracing::debug!(
        elapsed_ms = started.elapsed().as_millis(),
        "state file replacement completed"
    );
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            "state directory sync started"
        );
        fs::File::open(parent)?.sync_all()?;
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            "state directory sync completed"
        );
    }
    tracing::debug!(
        elapsed_ms = started.elapsed().as_millis(),
        "state file write completed"
    );
    Ok(())
}

pub fn load(state_dir: &Path) -> Result<Config> {
    let config: Config = serde_json::from_slice(
        &fs::read(state_dir.join("config.json")).context("initialize server state first")?,
    )?;
    if config.version != 1 {
        bail!("unsupported config version {}", config.version);
    }
    Ok(config)
}
pub fn save(state_dir: &Path, config: &Config) -> Result<()> {
    secret_write(
        &state_dir.join("config.json"),
        &serde_json::to_vec_pretty(config)?,
    )?;
    if let Err(error) = secret_write(
        &state_dir.join("pair-code.txt"),
        format!("{}\n", config.pair_code).as_bytes(),
    ) {
        // Authentication and the pair-code CLI read config.json. This file is
        // only an auxiliary display cache; its failure cannot undo that commit.
        tracing::warn!(%error, "pair-code cache could not be updated; use the pair-code command");
    }
    Ok(())
}

fn validity(params: &mut CertificateParams) {
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(3650);
}

pub fn init(state_dir: &Path, names: Vec<String>) -> Result<Config> {
    if state_dir.join("config.json").exists() {
        bail!("already initialized; refusing to overwrite identities");
    }
    if state_dir.exists() {
        if fs::symlink_metadata(state_dir)?.file_type().is_symlink()
            || fs::read_dir(state_dir)?.next().is_some()
        {
            bail!("initial state directory must be empty and must not be a symlink");
        }
    }
    fs::create_dir_all(state_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(state_dir, fs::Permissions::from_mode(0o700))?;
    }
    let names = if names.is_empty() {
        vec![
            "usb-bridge".into(),
            "localhost".into(),
        ]
    } else {
        names
    };
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    validity(&mut params);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "Rust USB Bridge local CA");
    let ca_key = KeyPair::generate()?;
    let ca = params.self_signed(&ca_key)?;
    let mut server_params = CertificateParams::new(names.clone())?;
    validity(&mut server_params);
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    server_params
        .distinguished_name
        .push(DnType::CommonName, "Rust USB Bridge server");
    let server_key = KeyPair::generate()?;
    let server = server_params.signed_by(&server_key, &ca, &ca_key)?;
    secret_write(&state_dir.join("ca.pem"), ca.pem().as_bytes())?;
    secret_write(
        &state_dir.join("ca-key.pem"),
        ca_key.serialize_pem().as_bytes(),
    )?;
    secret_write(&state_dir.join("server.pem"), server.pem().as_bytes())?;
    secret_write(
        &state_dir.join("server-key.pem"),
        server_key.serialize_pem().as_bytes(),
    )?;
    secret_write(
        &state_dir.join("fingerprint.txt"),
        format!("SHA256:{}\n", hex::encode(Sha256::digest(ca.der()))).as_bytes(),
    )?;
    let config = Config {
        version: 1,
        server_id: uuid::Uuid::new_v4().to_string(),
        listen: "0.0.0.0:3241".into(),
        server_names: names,
        pair_code: uuid::Uuid::new_v4().to_string(),
        invitations: BTreeMap::new(),
        clients: BTreeMap::new(),
        shared: BTreeMap::new(),
    };
    save(state_dir, &config)?;
    Ok(config)
}

pub fn issue_client(state_dir: &Path, name: &str) -> Result<(PairingBundle, ClientRecord)> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        bail!("client name must contain 1 to 80 printable characters");
    }
    let client_id = uuid::Uuid::new_v4().to_string();
    let ca_pem = fs::read_to_string(state_dir.join("ca.pem"))?;
    let ca_key = KeyPair::from_pem(&fs::read_to_string(state_dir.join("ca-key.pem"))?)?;
    let ca = CertificateParams::from_ca_cert_pem(&ca_pem)?.self_signed(&ca_key)?;
    let mut params = CertificateParams::new(vec![client_id.clone()])?;
    validity(&mut params);
    params.distinguished_name.push(DnType::CommonName, name);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let key = KeyPair::generate()?;
    let cert = params.signed_by(&key, &ca, &ca_key)?;
    let record = ClientRecord {
        name: name.to_string(),
        certificate_sha256: hex::encode(Sha256::digest(cert.der())),
    };
    Ok((
        PairingBundle {
            client_id,
            ca_cert_pem: ca_pem,
            client_cert_pem: cert.pem(),
            client_key_pem: key.serialize_pem(),
        },
        record,
    ))
}

pub fn tls_config(state_dir: &Path) -> Result<rustls::ServerConfig> {
    use rustls::{server::WebPkiClientVerifier, RootCertStore};
    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut std::io::Cursor::new(fs::read(
        state_dir.join("ca.pem"),
    )?)) {
        roots.add(cert?)?;
    }
    let verifier = WebPkiClientVerifier::builder_with_provider(
        std::sync::Arc::new(roots),
        std::sync::Arc::new(rustls::crypto::ring::default_provider()),
    )
    .allow_unauthenticated()
    .build()?;
    let certs = rustls_pemfile::certs(&mut std::io::Cursor::new(fs::read(
        state_dir.join("server.pem"),
    )?))
    .collect::<std::io::Result<Vec<_>>>()?;
    let key = rustls_pemfile::private_key(&mut std::io::Cursor::new(fs::read(
        state_dir.join("server-key.pem"),
    )?))?
    .context("server key missing")?;
    Ok(
        rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_config() -> Config {
        Config {
            version: 1,
            server_id: "config-test-server".into(),
            listen: "127.0.0.1:0".into(),
            server_names: vec!["localhost".into()],
            pair_code: "test-code-before".into(),
            invitations: BTreeMap::new(),
            clients: BTreeMap::new(),
            shared: BTreeMap::new(),
        }
    }
    #[test]
    fn auxiliary_cache_failure_does_not_reject_committed_main_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = test_config();
        save(dir.path(), &config).unwrap();
        let cache = dir.path().join("pair-code.txt");
        fs::remove_file(&cache).unwrap();
        fs::create_dir(&cache).unwrap();
        config.pair_code = "test-code-after".into();
        config.shared.insert(
            "1-2".into(),
            SharedRecord {
                vendor: 0xf000,
                product: 2,
                serial: Some("persistent-record".into()),
                path: "/isolated/mock/sys/devices/1-2".into(),
                original_driver: Some("usb".into()),
            },
        );
        save(dir.path(), &config).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.pair_code, "test-code-after");
        assert_eq!(
            serde_json::to_value(loaded).unwrap(),
            serde_json::to_value(config).unwrap()
        );
        assert!(cache.is_dir());
    }
    #[test]
    fn main_config_failure_does_not_update_auxiliary_cache() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config();
        save(dir.path(), &config).unwrap();
        let main = dir.path().join("config.json");
        let backup = dir.path().join("saved-config.json");
        let committed = fs::read(&main).unwrap();
        fs::rename(&main, &backup).unwrap();
        fs::create_dir(&main).unwrap();
        let mut candidate = config;
        candidate.pair_code = "test-code-uncommitted".into();
        assert!(save(dir.path(), &candidate).is_err());
        assert_eq!(fs::read(&backup).unwrap(), committed);
        assert_eq!(
            fs::read_to_string(dir.path().join("pair-code.txt")).unwrap(),
            "test-code-before\n"
        );
        fs::remove_dir(&main).unwrap();
        fs::rename(&backup, &main).unwrap();
        assert_eq!(load(dir.path()).unwrap().pair_code, "test-code-before");
    }
    #[test]
    fn secret_write_replaces_existing_file_without_partial_reader_values() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Barrier,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let first = vec![b'A'; 8192];
        let second = vec![b'B'; 8192];
        secret_write(&path, &first).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(Barrier::new(2));
        let reader = {
            let path = path.clone();
            let stopped = stopped.clone();
            let ready = ready.clone();
            let first = first.clone();
            let second = second.clone();
            std::thread::spawn(move || {
                assert_eq!(fs::read(&path).unwrap(), first);
                ready.wait();
                let mut reads = 1;
                while !stopped.load(Ordering::Acquire) {
                    let value = fs::read(&path).expect("committed file must remain present");
                    assert!(
                        value == first || value == second,
                        "reader observed partial contents"
                    );
                    reads += 1;
                }
                reads
            })
        };
        ready.wait();
        for n in 0..16 {
            secret_write(&path, if n % 2 == 0 { &second } else { &first }).unwrap();
        }
        stopped.store(true, Ordering::Release);
        assert!(reader.join().unwrap() > 0);
        assert_eq!(fs::read(&path).unwrap(), first);
    }
    #[test]
    fn identity_is_persistent_and_reinit_refused() {
        let dir = tempfile::tempdir().unwrap();
        let first = init(dir.path(), vec![]).unwrap();
        assert_eq!(load(dir.path()).unwrap().server_id, first.server_id);
        assert!(init(dir.path(), vec![]).is_err());
        let (bundle, record) = issue_client(dir.path(), "测试电脑").unwrap();
        assert!(bundle.client_cert_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(record.certificate_sha256.len(), 64);
        assert!(tls_config(dir.path()).is_ok());
    }
}
