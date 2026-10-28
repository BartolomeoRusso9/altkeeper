use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use omnisette::{
    adi_proxy::{
        ADIProxy, ADIProxyAnisetteProvider, Identifier, AKD_USER_AGENT, CLIENT_INFO_HEADER, DS_ID,
        IDENTIFIER_LENGTH,
    },
    anisette_headers_provider::AnisetteHeadersProvider,
    store_services_core::StoreServicesCoreADIProxy,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::time::timeout;
use uuid::Uuid;

const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

type Provider = ADIProxyAnisetteProvider<StoreServicesCoreADIProxy<'static>>;

struct Engine {
    libraries: PathBuf,
    provisioning_root: PathBuf,
    serial: tokio::sync::Mutex<()>,
    sequence: std::sync::atomic::AtomicU64,
}

struct ProviderSession {
    provider: Provider,
    provisioning_dir: TemporaryProvisioningDir,
}

struct TemporaryProvisioningDir(PathBuf);

impl Drop for TemporaryProvisioningDir {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            if error.kind() != std::io::ErrorKind::NotFound {
                crate::log::line(&format!(
                    "[anisette-local] Could not remove temporary provisioning state: {error}"
                ));
            }
        }
    }
}

impl Engine {
    fn new(libraries: PathBuf, data_dir: &Path) -> Result<Self, String> {
        let architecture = match std::env::consts::ARCH {
            "x86_64" => "x86_64",
            "x86" => "x86",
            "aarch64" => "arm64-v8a",
            "arm" => "armeabi-v7a",
            other => return Err(format!("Unsupported host architecture: {other}")),
        };
        let native_dir = libraries.join("lib").join(architecture);
        for name in ["libstoreservicescore.so", "libCoreADI.so"] {
            if !native_dir.join(name).is_file() {
                return Err(format!(
                    "Missing native Anisette library: {}",
                    native_dir.join(name).display()
                ));
            }
        }

        let provisioning_root = data_dir.join("state").join("anisette-provisioning");
        std::fs::create_dir_all(&provisioning_root)
            .map_err(|error| format!("Cannot create Anisette state directory: {error}"))?;
        crate::fsperm::restrict_dir(&provisioning_root)
            .map_err(|error| format!("Cannot protect Anisette state directory: {error}"))?;

        Ok(Self {
            libraries,
            provisioning_root,
            serial: tokio::sync::Mutex::new(()),
            sequence: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn provider(
        &self,
        identifier: Identifier,
        adi_pb: Option<&[u8]>,
    ) -> Result<ProviderSession, String> {
        let identity = Uuid::from_bytes(identifier);
        let sequence = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let provisioning_dir = TemporaryProvisioningDir(self.provisioning_root.join(format!(
            "{}-{}-{sequence}",
            identity,
            std::process::id()
        )));
        std::fs::create_dir_all(&provisioning_dir.0)
            .map_err(|error| format!("Cannot create provisioning state: {error}"))?;
        crate::fsperm::restrict_dir(&provisioning_dir.0)
            .map_err(|error| format!("Cannot protect provisioning state: {error}"))?;
        if let Some(adi_pb) = adi_pb {
            crate::fsperm::write_private(&provisioning_dir.0.join("adi.pb"), adi_pb)
                .map_err(|error| format!("Cannot save provisioning data: {error}"))?;
        }

        let mut proxy = StoreServicesCoreADIProxy::with_custom_provisioning_path(
            &self.libraries,
            &provisioning_dir.0,
        )
        .map_err(|error| format!("Cannot load native Anisette libraries: {error}"))?;
        proxy
            .set_device_identifier(identity.to_string().to_uppercase())
            .map_err(|error| format!("Cannot configure Anisette device identity: {error}"))?;
        proxy.set_local_user_uuid(hex::encode_upper(Sha256::digest(identifier)));
        let provider = ADIProxyAnisetteProvider::without_identifier(proxy)
            .map_err(|error| format!("Cannot create Anisette provider: {error}"))?;
        Ok(ProviderSession {
            provider,
            provisioning_dir,
        })
    }
}

pub async fn serve(
    bind: std::net::SocketAddr,
    libraries: PathBuf,
    data_dir: PathBuf,
) -> Result<(), String> {
    let engine = Arc::new(Engine::new(libraries, &data_dir)?);
    let router = Router::new()
        .route("/v3/client_info", get(client_info))
        .route(
            "/v3/get_headers",
            post(get_headers).layer(DefaultBodyLimit::max(MAX_PAYLOAD_BYTES)),
        )
        .route("/v3/provisioning_session", get(provisioning_session))
        .with_state(engine);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|error| format!("Cannot listen for local Anisette on {bind}: {error}"))?;
    crate::log::line(&format!(
        "[anisette-local] V3 server listening at http://{bind}. This protocol has no authentication or TLS; restrict network access."
    ));
    axum::serve(listener, router)
        .await
        .map_err(|error| format!("Local Anisette server failed: {error}"))
}

fn client_info_body() -> Value {
    json!({
        "client_info": CLIENT_INFO_HEADER,
        "user_agent": AKD_USER_AGENT,
    })
}

async fn client_info() -> Json<Value> {
    Json(client_info_body())
}

#[derive(Deserialize)]
struct HeadersRequest {
    identifier: String,
    adi_pb: String,
}

async fn get_headers(State(engine): State<Arc<Engine>>, body: Bytes) -> (StatusCode, Json<Value>) {
    let request: HeadersRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return get_headers_error(
                StatusCode::BAD_REQUEST,
                &format!("Invalid request: {error}"),
            )
        }
    };
    let identifier = match BASE64.decode(request.identifier) {
        Ok(bytes) if bytes.len() == IDENTIFIER_LENGTH => {
            let mut identifier = [0; IDENTIFIER_LENGTH];
            identifier.copy_from_slice(&bytes);
            identifier
        }
        Ok(_) => return get_headers_error(StatusCode::BAD_REQUEST, "Invalid identifier length."),
        Err(error) => {
            return get_headers_error(
                StatusCode::BAD_REQUEST,
                &format!("Invalid identifier: {error}"),
            )
        }
    };
    let adi_pb = match BASE64.decode(request.adi_pb) {
        Ok(bytes) if !bytes.is_empty() && bytes.len() <= MAX_PAYLOAD_BYTES => bytes,
        Ok(_) => {
            return get_headers_error(StatusCode::BAD_REQUEST, "Invalid provisioning data size.")
        }
        Err(error) => {
            return get_headers_error(
                StatusCode::BAD_REQUEST,
                &format!("Invalid provisioning data: {error}"),
            )
        }
    };

    let _serial = match engine.serial.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            return get_headers_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "The local Anisette engine is busy.",
            )
        }
    };
    let result = async {
        let mut provider = engine.provider(identifier, Some(&adi_pb))?;
        let headers = generate_headers(&mut provider)
            .await
            .map_err(|error| format!("Could not generate Anisette headers: {error}"))?;
        Ok::<HashMap<String, String>, String>(headers)
    }
    .await;

    match result {
        Ok(headers) => {
            let mut response = serde_json::Map::new();
            response.insert("result".into(), json!("Headers"));
            for name in ["X-Apple-I-MD-M", "X-Apple-I-MD", "X-Apple-I-MD-RINFO"] {
                let Some(value) = headers.get(name) else {
                    return get_headers_error(
                        StatusCode::BAD_GATEWAY,
                        "The native engine omitted a required Anisette header.",
                    );
                };
                response.insert(name.into(), json!(value));
            }
            (StatusCode::OK, Json(Value::Object(response)))
        }
        Err(error) => {
            crate::log::line(&format!(
                "[anisette-local] Header generation failed: {error}"
            ));
            get_headers_error(StatusCode::BAD_GATEWAY, "The local Anisette engine failed.")
        }
    }
}

async fn generate_headers(
    provider: &mut ProviderSession,
) -> Result<HashMap<String, String>, String> {
    let headers = provider
        .provider
        .get_anisette_headers(true)
        .await
        .map_err(|error| error.to_string())?;
    Ok(provider.provider.normalize_headers(headers))
}

fn get_headers_error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({
            "result": "GetHeadersError",
            "message": message,
        })),
    )
}

async fn provisioning_session(
    State(engine): State<Arc<Engine>>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade
        .max_message_size(MAX_PAYLOAD_BYTES)
        .max_frame_size(MAX_PAYLOAD_BYTES)
        .on_upgrade(move |socket| handle_provisioning(socket, engine))
}

#[derive(Deserialize)]
struct IdentifierRequest {
    identifier: String,
}

#[derive(Deserialize)]
struct StartProvisioningRequest {
    spim: String,
}

#[derive(Deserialize)]
struct EndProvisioningRequest {
    ptm: String,
    tk: String,
}

#[derive(Clone, Copy)]
enum SessionStep {
    Identifier,
    Start,
    End,
}

async fn handle_provisioning(mut socket: WebSocket, engine: Arc<Engine>) {
    let _serial = match engine.serial.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            let _ = send_ws(
                &mut socket,
                json!({
                    "result": "StartProvisioningError",
                    "message": "The local Anisette engine is busy.",
                }),
            )
            .await;
            return;
        }
    };
    let mut provider: Option<ProviderSession> = None;
    let mut session = 0;
    let mut step = SessionStep::Identifier;

    if send_ws(&mut socket, json!({"result": "GiveIdentifier"}))
        .await
        .is_err()
    {
        return;
    }

    loop {
        let message = match timeout(SESSION_IDLE_TIMEOUT, socket.recv()).await {
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(error))) => {
                crate::log::line(&format!(
                    "[anisette-local] Provisioning WebSocket failed: {error}"
                ));
                break;
            }
            Ok(None) => break,
            Err(_) => {
                let _ = send_ws(&mut socket, json!({"result": "Timeout"})).await;
                break;
            }
        };

        match message {
            Message::Text(text) => {
                let outcome = match step {
                    SessionStep::Identifier => {
                        let input = match serde_json::from_str::<IdentifierRequest>(text.as_str()) {
                            Ok(input) => input,
                            Err(error) => {
                                send_ws_error(&mut socket, "InvalidIdentifier", error.to_string())
                                    .await;
                                break;
                            }
                        };
                        let identifier = match BASE64.decode(input.identifier) {
                            Ok(bytes) if bytes.len() == IDENTIFIER_LENGTH => {
                                let mut identifier = [0; IDENTIFIER_LENGTH];
                                identifier.copy_from_slice(&bytes);
                                identifier
                            }
                            Ok(_) => {
                                send_ws_error(
                                    &mut socket,
                                    "InvalidIdentifier",
                                    "Identifier has an invalid length.".into(),
                                )
                                .await;
                                break;
                            }
                            Err(error) => {
                                send_ws_error(&mut socket, "InvalidIdentifier", error.to_string())
                                    .await;
                                break;
                            }
                        };
                        match engine.provider(identifier, None) {
                            Ok(instance) => {
                                provider = Some(instance);
                                step = SessionStep::Start;
                                json!({"result": "GiveStartProvisioningData"})
                            }
                            Err(error) => {
                                crate::log::line(&format!(
                                    "[anisette-local] Provider initialization failed: {error}"
                                ));
                                send_ws_error(
                                    &mut socket,
                                    "InvalidIdentifier",
                                    "Could not initialize the local Anisette engine.".into(),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    SessionStep::Start => {
                        let input =
                            match serde_json::from_str::<StartProvisioningRequest>(text.as_str()) {
                                Ok(input) => input,
                                Err(error) => {
                                    send_ws_error(
                                        &mut socket,
                                        "StartProvisioningError",
                                        error.to_string(),
                                    )
                                    .await;
                                    break;
                                }
                            };
                        let spim = match decode_payload(&input.spim) {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                send_ws_error(&mut socket, "StartProvisioningError", error).await;
                                break;
                            }
                        };
                        let Some(provider) = provider.as_mut() else {
                            send_ws_error(
                                &mut socket,
                                "StartProvisioningError",
                                "Provisioning state was lost.".into(),
                            )
                            .await;
                            break;
                        };
                        match provider
                            .provider
                            .adi_proxy()
                            .start_provisioning(DS_ID, &spim)
                        {
                            Ok(result) => {
                                session = result.session;
                                step = SessionStep::End;
                                json!({
                                    "result": "GiveEndProvisioningData",
                                    "cpim": BASE64.encode(result.cpim),
                                })
                            }
                            Err(error) => {
                                crate::log::line(&format!(
                                    "[anisette-local] Provisioning start failed: {error}"
                                ));
                                send_ws_error(
                                    &mut socket,
                                    "StartProvisioningError",
                                    "Native provisioning could not start.".into(),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    SessionStep::End => {
                        let input =
                            match serde_json::from_str::<EndProvisioningRequest>(text.as_str()) {
                                Ok(input) => input,
                                Err(error) => {
                                    send_ws_error(
                                        &mut socket,
                                        "EndProvisioningError",
                                        error.to_string(),
                                    )
                                    .await;
                                    break;
                                }
                            };
                        let ptm = match decode_payload(&input.ptm) {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                send_ws_error(&mut socket, "EndProvisioningError", error).await;
                                break;
                            }
                        };
                        let tk = match decode_payload(&input.tk) {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                send_ws_error(&mut socket, "EndProvisioningError", error).await;
                                break;
                            }
                        };
                        let Some(provider) = provider.as_mut() else {
                            send_ws_error(
                                &mut socket,
                                "EndProvisioningError",
                                "Provisioning state was lost.".into(),
                            )
                            .await;
                            break;
                        };
                        if let Err(error) = provider
                            .provider
                            .adi_proxy()
                            .end_provisioning(session, &ptm, &tk)
                        {
                            crate::log::line(&format!(
                                "[anisette-local] Provisioning completion failed: {error}"
                            ));
                            send_ws_error(
                                &mut socket,
                                "EndProvisioningError",
                                "Native provisioning could not finish.".into(),
                            )
                            .await;
                            break;
                        }
                        match std::fs::read(provider.provisioning_dir.0.join("adi.pb")) {
                            Ok(adi_pb) => {
                                session = 0;
                                json!({
                                    "result": "ProvisioningSuccess",
                                    "adi_pb": BASE64.encode(adi_pb),
                                })
                            }
                            Err(error) => {
                                crate::log::line(&format!(
                                    "[anisette-local] Could not read adi.pb: {error}"
                                ));
                                send_ws_error(
                                    &mut socket,
                                    "EndProvisioningError",
                                    "Could not read the completed provisioning data.".into(),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                };

                if send_ws(&mut socket, outcome).await.is_err() {
                    break;
                }
            }
            Message::Ping(payload) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {
                let _ = send_ws(&mut socket, json!({"result": "TextOnly"})).await;
                break;
            }
        }
    }

    if session != 0 {
        if let Some(provider) = provider.as_mut() {
            if let Err(error) = provider
                .provider
                .adi_proxy()
                .destroy_provisioning_session(session)
            {
                crate::log::line(&format!(
                    "[anisette-local] Could not destroy provisioning session: {error}"
                ));
            }
        }
    }
}

fn decode_payload(payload: &str) -> Result<Vec<u8>, String> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err("Provisioning payload is too large.".into());
    }
    BASE64
        .decode(payload)
        .map_err(|error| format!("Invalid base64 provisioning payload: {error}"))
}

async fn send_ws(socket: &mut WebSocket, value: Value) -> Result<(), String> {
    let text = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| error.to_string())
}

async fn send_ws_error(socket: &mut WebSocket, result: &str, message: String) {
    if let Err(error) = send_ws(
        socket,
        json!({
            "result": result,
            "message": message,
        }),
    )
    .await
    {
        crate::log::line(&format!(
            "[anisette-local] Could not send provisioning error: {error}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_info_matches_v3_shape() {
        let value = client_info_body();
        assert!(value["client_info"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(value["user_agent"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[test]
    fn rejects_oversized_provisioning_payloads() {
        assert!(decode_payload(&"A".repeat(MAX_PAYLOAD_BYTES + 1)).is_err());
    }
}
