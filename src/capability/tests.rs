use super::*;

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::net::TcpListener;
use std::sync::Mutex;
use std::thread;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Verifier};
use serde_json::{json, Value};
use tempfile::TempDir;

const CAPABILITY_ID: &str =
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const WORKLOAD_ID: &str = "brama-test-workload";
const TEST_KEY: [u8; 32] = [0x2a; 32];

fn write_key(directory: &TempDir, name: &str, mode: u32) -> PathBuf {
    let path = directory.path().join(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&path)
        .expect("create test signing key");
    file.write_all(&TEST_KEY).expect("write test signing key");
    path
}

fn redeem_against<F>(serve: F) -> Result<Secret, CapabilityError>
where
    F: FnOnce(Vec<u8>) -> Vec<u8> + Send + 'static,
{
    let directory = tempfile::tempdir().expect("create isolated test directory");
    let key_path = write_key(&directory, "workload.key", 0o600);
    let socket_path = directory.path().join("skarbiec.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind fake broker socket");

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one redemption");
        let mut request = Vec::new();
        stream
            .read_to_end(&mut request)
            .expect("read request through client write EOF");
        let response = serve(request);
        stream.write_all(&response).expect("write broker response");
    });

    let client = CapabilityClient::new(socket_path, WORKLOAD_ID.to_owned(), &key_path)
        .expect("construct capability client");
    let capability = CapabilityRef::provider(CAPABILITY_ID, "provider:openrouter")
        .expect("construct bound capability");
    let result = client.redeem(&capability);
    server.join().expect("fake broker completed");
    result
}

#[test]
fn accepts_only_the_three_exact_brama_binding_tuples() {
    let cases = [
        (
            CapabilityRef::provider(CAPABILITY_ID, "provider:openrouter"),
            Purpose::ProviderAuthenticate,
            "brama.provider.authenticate",
            "provider:openrouter",
        ),
        (
            CapabilityRef::supabase(CAPABILITY_ID, "supabase:primary"),
            Purpose::SupabaseConnect,
            "brama.supabase.connect",
            "supabase:primary",
        ),
        (
            CapabilityRef::request_sign(CAPABILITY_ID, "agent:writer"),
            Purpose::RequestSign,
            "brama.request.sign",
            "agent:writer",
        ),
    ];

    for (binding, expected_purpose, expected_purpose_name, expected_resource) in cases {
        let binding = binding.expect("canonical tuple must be accepted");
        assert_eq!(binding.id, CAPABILITY_ID);
        assert_eq!(binding.target, TARGET);
        assert_eq!(binding.purpose(), expected_purpose);
        assert_eq!(binding.purpose().as_str(), expected_purpose_name);
        assert_eq!(binding.resource(), expected_resource);
    }
}

#[test]
fn rejects_mistargeted_non_concrete_or_non_opaque_bindings() {
    let cases = [
        CapabilityRef::new(
            CAPABILITY_ID,
            "most",
            Purpose::ProviderAuthenticate,
            "provider:openrouter",
        ),
        CapabilityRef::provider(CAPABILITY_ID, "openrouter"),
        CapabilityRef::supabase(CAPABILITY_ID, "provider:primary"),
        CapabilityRef::request_sign(CAPABILITY_ID, "agent:*"),
        CapabilityRef::request_sign(CAPABILITY_ID, "agent:writer?"),
        CapabilityRef::provider(
            "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
            "provider:openrouter",
        ),
        CapabilityRef::provider("not-an-opaque-capability-id", "provider:openrouter"),
    ];

    for result in cases {
        assert!(matches!(result, Err(CapabilityError::InvalidBinding)));
    }
}

#[test]
fn signing_key_must_be_an_owner_only_regular_file_not_a_symlink() {
    let directory = tempfile::tempdir().expect("create isolated test directory");
    let socket = directory.path().join("unused.sock");

    let owner_only = write_key(&directory, "owner.key", 0o600);
    CapabilityClient::new(socket.clone(), WORKLOAD_ID.to_owned(), &owner_only)
        .expect("owner-only regular key must be accepted");

    let group_readable = write_key(&directory, "group-readable.key", 0o600);
    fs::set_permissions(&group_readable, fs::Permissions::from_mode(0o640))
        .expect("make test key group-readable");
    assert!(matches!(
        CapabilityClient::new(socket.clone(), WORKLOAD_ID.to_owned(), &group_readable),
        Err(CapabilityError::InvalidConfiguration)
    ));

    let empty = write_key(&directory, "empty.key", 0o600);
    fs::write(&empty, []).expect("truncate test key");
    assert!(matches!(
        CapabilityClient::new(socket.clone(), WORKLOAD_ID.to_owned(), &empty),
        Err(CapabilityError::InvalidConfiguration)
    ));

    let oversized = write_key(&directory, "oversized.key", 0o600);
    fs::write(&oversized, vec![0_u8; MAX_KEY_BYTES as usize + 1])
        .expect("write oversized test key");
    assert!(matches!(
        CapabilityClient::new(socket.clone(), WORKLOAD_ID.to_owned(), &oversized),
        Err(CapabilityError::InvalidConfiguration)
    ));

    assert!(matches!(
        CapabilityClient::new(socket.clone(), WORKLOAD_ID.to_owned(), directory.path()),
        Err(CapabilityError::InvalidConfiguration)
    ));
    let link = directory.path().join("linked.key");
    symlink(&owner_only, &link).expect("create signing-key symlink");
    assert!(matches!(
        CapabilityClient::new(socket, WORKLOAD_ID.to_owned(), &link),
        Err(CapabilityError::InvalidConfiguration)
    ));
}

#[test]
fn debug_output_redacts_capability_ids_keys_paths_and_secret_bytes() {
    let directory = tempfile::tempdir().expect("create isolated test directory");
    let key_path = write_key(&directory, "sensitive-owner-key", 0o600);
    let socket_path = directory.path().join("sensitive-broker.sock");
    let client = CapabilityClient::new(socket_path.clone(), WORKLOAD_ID.to_owned(), &key_path)
        .expect("construct capability client");
    let capability = CapabilityRef::provider(CAPABILITY_ID, "provider:openrouter")
        .expect("construct bound capability");
    let secret_bytes = b"debug-must-never-expose-this-secret";
    let secret = Secret(Zeroizing::new(secret_bytes.to_vec()));

    let rendered = format!("{client:?} {capability:?} {secret:?}");

    assert!(!rendered.contains(CAPABILITY_ID));
    assert!(!rendered.contains(secret_bytes.escape_ascii().to_string().as_str()));
    assert!(!rendered.contains(key_path.to_string_lossy().as_ref()));
    assert!(!rendered.contains(socket_path.to_string_lossy().as_ref()));
    assert!(rendered.contains("[opaque]"));
    assert!(rendered.contains("[redacted]"));
}

#[test]
fn redeem_sends_canonical_signed_request_and_accepts_only_exact_framed_secret() {
    let expected_secret = vec![0x00, 0x73, 0xff, 0x0a];
    let response_secret = expected_secret.clone();
    let secret = redeem_against(move |request| {
        assert_eq!(request.last(), Some(&b'\n'));
        assert_eq!(request.iter().filter(|byte| **byte == b'\n').count(), 1);

        let request: Value = serde_json::from_slice(&request[..request.len() - 1])
            .expect("request is one JSON control line");
        let fields = request.as_object().expect("request is a JSON object");
        assert_eq!(fields.len(), 5);
        assert_eq!(fields.get("version"), Some(&json!(WIRE_VERSION)));
        assert_eq!(fields.get("capability_id"), Some(&json!(CAPABILITY_ID)));
        assert_eq!(fields.get("workload_id"), Some(&json!(WORKLOAD_ID)));

        let nonce = fields
            .get("nonce")
            .and_then(Value::as_str)
            .expect("nonce is a string");
        assert_eq!(nonce.len(), 64);
        assert!(nonce.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));

        let proof = fields
            .get("proof")
            .and_then(Value::as_str)
            .expect("proof is a string");
        assert_eq!(proof.len(), 86);
        assert!(!proof.contains('='));
        let proof = URL_SAFE_NO_PAD.decode(proof).expect("proof is unpadded base64url");
        let signature = Signature::from_slice(&proof).expect("proof is an Ed25519 signature");
        let signing_key = SigningKey::from_bytes(&TEST_KEY);
        let mut signed = Vec::new();
        signed.extend_from_slice(b"SKARBIEC-WORKLOAD-PROOF\0v1\0");
        signed.extend_from_slice(CAPABILITY_ID.as_bytes());
        signed.push(0);
        signed.extend_from_slice(nonce.as_bytes());
        signed.push(0);
        signed.extend_from_slice(WORKLOAD_ID.as_bytes());
        signing_key
            .verifying_key()
            .verify(&signed, &signature)
            .expect("proof covers canonical domain and request identity");

        let mut response = format!(
            "{{\"version\":\"{}\",\"status\":\"ok\",\"secret_len\":{}}}\n",
            WIRE_VERSION,
            response_secret.len()
        )
        .into_bytes();
        response.extend_from_slice(&response_secret);
        response
    })
    .expect("canonical framed response must redeem");

    assert!(secret.expose() == expected_secret.as_slice());
}

#[test]
fn malformed_control_or_payload_fails_closed() {
    let cases = [
        b"{\"version\":\"skarbiec.redeem.v1\",\"status\":\"ok\",\"secret_len\":1,\"secret\":\"x\"}\nx".to_vec(),
        b"{\"version\":\"skarbiec.redeem.v2\",\"status\":\"ok\",\"secret_len\":1}\nx".to_vec(),
        b"{\"version\":\"skarbiec.redeem.v1\",\"status\":\"ok\",\"secret_len\":0}\n".to_vec(),
        b"{\"version\":\"skarbiec.redeem.v1\",\"status\":\"ok\",\"secret_len\":65537}\n".to_vec(),
        b"{\"version\":\"skarbiec.redeem.v1\",\"status\":\"ok\",\"secret_len\":3}\nxy".to_vec(),
        b"{\"version\":\"skarbiec.redeem.v1\",\"status\":\"ok\",\"secret_len\":2}\nxyz".to_vec(),
        b"{\"version\":\"skarbiec.redeem.v1\",\"status\":\"ok\",\"secret_len\":1}".to_vec(),
        [vec![b'x'; MAX_CONTROL_LINE + 1], b"\n".to_vec()].concat(),
        b"{not-json}\n".to_vec(),
        b"[]\n".to_vec(),
        b"{\"version\":\"skarbiec.redeem.v1\",\"status\":\"denied\"}\n".to_vec(),
        b"{\"version\":\"skarbiec.redeem.v1\",\"status\":\"ok\"}\n".to_vec(),
        b"{\"version\":\"skarbiec.redeem.v1\",\"status\":\"retry\",\"secret_len\":1}\nx".to_vec(),
    ];

    for response in cases {
        let result = redeem_against(move |_| response);
        assert!(matches!(result, Err(CapabilityError::RedemptionDenied)));
    }
}

static CAPABILITY_ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvRestore {
    fn set(entries: &[(&'static str, String)]) -> Self {
        let previous = entries
            .iter()
            .map(|(name, value)| {
                let old = std::env::var_os(name);
                std::env::set_var(name, value);
                (*name, old)
            })
            .collect();
        Self(previous)
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..) {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn gateway_selects_exact_agent_provider_and_subscription_capabilities_without_persistence() {
    let _environment = CAPABILITY_ENV_LOCK.lock().expect("lock capability test environment");
    let directory = tempfile::tempdir().expect("create isolated gateway test directory");
    let state_dir = directory.path().join("state");
    let key_path = write_key(&directory, "gateway-workload.key", 0o600);
    let socket_path = directory.path().join("gateway-broker.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind fake gateway broker");
    let agent_id = "1".repeat(64);
    let provider_id = "2".repeat(64);
    let subscription_id = "3".repeat(64);
    let agent_secret = b"agent-final-use-only".to_vec();
    let provider_secret = b"provider-final-use-only".to_vec();
    let subscription_secret = b"subscription-final-use-only".to_vec();
    let expected = [
        (agent_id.clone(), agent_secret.clone()),
        (provider_id.clone(), provider_secret.clone()),
        (subscription_id.clone(), subscription_secret.clone()),
    ];
    let server = thread::spawn(move || {
        for (expected_id, secret) in expected {
            let (mut stream, _) = listener.accept().expect("accept gateway redemption");
            let mut request = Vec::new();
            stream.read_to_end(&mut request).expect("read request through EOF");
            let control: Value = serde_json::from_slice(&request[..request.len() - 1])
                .expect("gateway request is JSON");
            assert_eq!(control.get("capability_id"), Some(&json!(expected_id)));
            let mut response = format!(
                "{{\"version\":\"{}\",\"status\":\"ok\",\"secret_len\":{}}}\n",
                WIRE_VERSION,
                secret.len()
            )
            .into_bytes();
            response.extend_from_slice(&secret);
            stream.write_all(&response).expect("write framed gateway secret");
        }
    });
    let _restore = EnvRestore::set(&[
        ("SKARBIEC_CAP_SOCKET", socket_path.to_string_lossy().into_owned()),
        ("SKARBIEC_WORKLOAD_ID", WORKLOAD_ID.to_owned()),
        ("SKARBIEC_WORKLOAD_SIGNING_KEY_FILE", key_path.to_string_lossy().into_owned()),
        (
            "BRAMA_REQUEST_SIGN_CAPABILITY_IDS",
            json!({"Agent / A": agent_id}).to_string(),
        ),
        (
            "BRAMA_PROVIDER_CAPABILITY_IDS",
            json!({"Open Router": provider_id, "Subscription / 7": subscription_id}).to_string(),
        ),
        ("BRAMA_STATE_DIR", state_dir.to_string_lossy().into_owned()),
    ]);

    let agent = crate::gateway::broker::get_agent_auth_secret("Agent / A")
        .await
        .expect("configured agent capability redeems");
    assert_eq!(agent.expose(), agent_secret);
    drop(agent);
    let provider = crate::gateway::broker::provider_credential("Open Router")
        .await
        .expect("configured provider capability redeems");
    assert_eq!(provider.expose(), provider_secret);
    drop(provider);
    let subscription = crate::gateway::broker::subscription_credential(
        "Subscription / 7",
        "Claude Code",
    )
    .await
    .expect("configured subscription capability redeems");
    assert_eq!(subscription.expose(), subscription_secret);
    drop(subscription);
    server.join().expect("fake gateway broker completed");

    assert!(crate::gateway::broker::get_agent_auth_secret("Agent / B").await.is_none());
    assert!(crate::gateway::broker::provider_credential("Other Router").await.is_none());
    assert!(crate::gateway::broker::subscription_credential("Subscription / 8", "Claude Code")
        .await
        .is_none());

    if state_dir.exists() {
        for entry in fs::read_dir(&state_dir).expect("read isolated Brama state") {
            let path = entry.expect("read state entry").path();
            if path.is_file() {
                let bytes = fs::read(path).expect("read state artifact");
                for secret in [&agent_secret, &provider_secret, &subscription_secret] {
                    assert!(!bytes.windows(secret.len()).any(|window| window == secret.as_slice()));
                }
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn reauth_rejects_plaintext_without_returning_or_persisting_it() {
    let _environment = CAPABILITY_ENV_LOCK.lock().expect("lock capability test environment");
    let directory = tempfile::tempdir().expect("create isolated reauth state");
    let state_dir = directory.path().join("state");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake Weles reauth server");
    let address = listener.local_addr().expect("read fake server address");
    let plaintext = "reauth-plaintext-must-not-persist";
    let response_body = json!({"ok": true, "credential": plaintext}).to_string();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept reauth request");
        let mut request = [0_u8; 8192];
        let _ = stream.read(&mut request).expect("read reauth request");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        stream.write_all(response.as_bytes()).expect("write reauth response");
    });
    let _restore = EnvRestore::set(&[
        ("WELES_BRAMA_REAUTH_URL", format!("http://{address}/reauth")),
        ("WELES_REAUTH_TIMEOUT_MS", "5000".to_owned()),
        ("BRAMA_STATE_DIR", state_dir.to_string_lossy().into_owned()),
    ]);

    let error = crate::subscription_dispatch::reauth::reauth_provider(
        "agent-a",
        "claude_code",
        "subscription-a",
        "claude-opus",
        "401 authentication failed",
    )
    .await
    .expect_err("plaintext reauth response must fail closed");
    server.join().expect("fake reauth server completed");

    assert_eq!(error, "Weles reauth returned forbidden plaintext credential");
    assert!(!error.contains(plaintext));
    if state_dir.exists() {
        for entry in fs::read_dir(&state_dir).expect("read isolated reauth state") {
            let path = entry.expect("read reauth state entry").path();
            if path.is_file() {
                let bytes = fs::read(path).expect("read reauth artifact");
                assert!(!bytes.windows(plaintext.len()).any(|window| window == plaintext.as_bytes()));
            }
        }
    }
}
