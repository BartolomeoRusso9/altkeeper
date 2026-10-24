use super::middleware::WasmProxyMiddleware;
use plist::Dictionary;
use plist_macro::plist_to_xml_string;
use plist_macro::pretty_print_dictionary;
#[cfg(not(feature = "wasm"))]
use reqwest::Certificate;
use reqwest::{
    ClientBuilder,
    header::{HeaderMap, HeaderValue},
};
use reqwest_middleware::ClientBuilder as MwClientBuilder;
use rootcause::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tracing::debug;

use crate::{SideloadError, anisette::AnisetteClientInfo, util::plist::PlistDataExtract};

#[cfg(not(feature = "wasm"))]
const APPLE_ROOT: &[u8] = include_bytes!("./apple_root.der");
const URL_BAG: &str = "https://gsa.apple.com/grandslam/GsService2/lookup";

/// Con ALTKEEPER_DEBUG (o la vecchia ALTREFRESH_DEBUG) impostata, stampa su stderr come risponde Apple a ogni richiesta:
/// stato, versione HTTP, intestazioni e, se è un errore, il corpo. Mai il corpo di una richiesta
/// (ha la prova della password e i dati anisette) né quello di una risposta riuscita.
fn gs_debug() -> bool {
    std::env::var_os("ALTKEEPER_DEBUG").is_some() || std::env::var_os("ALTREFRESH_DEBUG").is_some()
}

/// Numero d'ordine delle richieste a GrandSlam, per riconoscerle nel debug.
static GS_SEQ: AtomicUsize = AtomicUsize::new(0);

pub struct GrandSlam {
    pub client: reqwest_middleware::ClientWithMiddleware,
    pub client_info: AnisetteClientInfo,
    url_bag: Dictionary,
}

impl GrandSlam {
    /// Create a new GrandSlam instance
    ///
    /// # Arguments
    /// - `client`: The reqwest client to use for requests
    pub async fn new(
        client_info: AnisetteClientInfo,
        debug: bool,
        proxy_url: Option<String>,
    ) -> Result<Self, Report> {
        let client =
            Self::build_reqwest_client(debug, proxy_url).context("Failed to build HTTP client")?;
        let base_headers = Self::base_headers(&client_info, false)?;
        let url_bag = Self::fetch_url_bag(&client, base_headers).await?;
        Ok(Self {
            client,
            client_info,
            url_bag,
        })
    }

    /// Fetch the URL bag from GrandSlam and cache it
    pub async fn fetch_url_bag(
        client: &reqwest_middleware::ClientWithMiddleware,
        base_headers: HeaderMap,
    ) -> Result<Dictionary, Report> {
        debug!("Fetching URL bag from GrandSlam");
        let n = GS_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let started = Instant::now();
        let resp = client
            .get(URL_BAG)
            .headers(base_headers)
            .send()
            .await
            .context("Failed to fetch URL Bag")?;
        if gs_debug() {
            eprintln!(
                "[gs #{n}] GET {URL_BAG} -> {} {:?} remote={:?} in {:?}",
                resp.status(),
                resp.version(),
                resp.remote_addr(),
                started.elapsed()
            );
        }
        let resp = resp
            .text()
            .await
            .context("Failed to read URL Bag response text")?;

        let dict: Dictionary =
            plist::from_bytes(resp.as_bytes()).context("Failed to parse URL Bag plist")?;
        let urls = dict
            .get("urls")
            .and_then(|v| v.as_dictionary())
            .cloned()
            .ok_or_else(|| report!("URL Bag plist missing 'urls' dictionary"))?;

        Ok(urls)
    }

    pub fn get_url(&self, key: &str) -> Result<String, Report> {
        let url = self
            .url_bag
            .get_string(key)
            .context("Unable to find key in URL bag")?;
        Ok(url)
    }

    pub fn get(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        let builder = self
            .client
            .get(url)
            .headers(Self::base_headers(&self.client_info, false)?);

        Ok(builder)
    }

    pub fn get_sms(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        let builder = self
            .client
            .get(url)
            .headers(Self::base_headers(&self.client_info, true)?);

        Ok(builder)
    }

    pub fn put_sms(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        let builder = self
            .client
            .put(url)
            .headers(Self::base_headers(&self.client_info, true)?);

        Ok(builder)
    }

    pub fn post(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        let builder = self
            .client
            .post(url)
            .headers(Self::base_headers(&self.client_info, false)?);

        Ok(builder)
    }

    pub fn post_sms(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        let builder = self
            .client
            .post(url)
            .headers(Self::base_headers(&self.client_info, true)?);

        Ok(builder)
    }

    pub fn patch(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        let builder = self
            .client
            .patch(url)
            .headers(Self::base_headers(&self.client_info, false)?);

        Ok(builder)
    }

    pub async fn plist_request(
        &self,
        url: &str,
        body: &Dictionary,
        additional_headers: Option<HeaderMap>,
    ) -> Result<Dictionary, Report> {
        let debug_on = gs_debug();
        let n = GS_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let started = Instant::now();
        let request_xml = plist_to_xml_string(body);
        if debug_on {
            let extra: Vec<String> = additional_headers
                .iter()
                .flat_map(|h| h.iter().map(|(k, v)| format!("{k}: {v:?}")))
                .collect();
            eprintln!(
                "[gs #{n}] POST {url} body={} B client-info={:?} ua={:?} extra-headers={extra:?}",
                request_xml.len(),
                self.client_info.client_info,
                self.client_info.user_agent
            );
        }

        let sent = self
            .post(url)?
            .headers(additional_headers.unwrap_or_else(reqwest::header::HeaderMap::new))
            .body(request_xml)
            .send()
            .await;
        if debug_on && let Err(e) = &sent {
            eprintln!("[gs #{n}] send error after {:?}: {e:?}", started.elapsed());
        }
        let resp = sent.context("Failed to send grandslam request")?;

        if debug_on {
            eprintln!(
                "[gs #{n}] -> {} {:?} remote={:?} in {:?}",
                resp.status(),
                resp.version(),
                resp.remote_addr(),
                started.elapsed()
            );
            for (k, v) in resp.headers() {
                eprintln!("[gs #{n}]   {k}: {v:?}");
            }
        }

        if let Some(e) = resp.error_for_status_ref().err() {
            if debug_on {
                let text = resp.text().await.unwrap_or_default();
                let head: String = text.chars().take(800).collect();
                eprintln!("[gs #{n}] error body ({} B): {head}", text.len());
            }
            Err::<(), _>(e).context("Received error response from grandslam")?;
            unreachable!("error_for_status_ref gave an error");
        }
        let resp = resp
            .text()
            .await
            .context("Failed to read grandslam response as text")?;

        let dict: Dictionary = plist::from_bytes(resp.as_bytes())
            .context("Failed to parse grandslam response plist")
            .attach_with(|| resp.clone())?;

        let response_plist = dict
            .get("Response")
            .and_then(|v| v.as_dictionary())
            .cloned()
            .ok_or_else(|| {
                report!("grandslam response missing 'Response'")
                    .attach(pretty_print_dictionary(&dict))
            })?;

        if debug_on {
            // Solo "Status" (codice e messaggio d'errore di Apple): il resto ha token e chiavi.
            eprintln!(
                "[gs #{n}] ok, {} B, Status={:?}",
                resp.len(),
                response_plist.get("Status")
            );
        }

        Ok(response_plist)
    }

    fn base_headers(
        client_info: &AnisetteClientInfo,
        sms: bool,
    ) -> Result<reqwest::header::HeaderMap, Report> {
        let mut headers = reqwest::header::HeaderMap::new();
        if !sms {
            headers.insert("Content-Type", HeaderValue::from_static("text/x-xml-plist"));
            headers.insert("Accept", HeaderValue::from_static("text/x-xml-plist"));
        } else {
            headers.insert("Content-Type", HeaderValue::from_static("application/json"));
            headers.insert("Accept", HeaderValue::from_static("application/json"));
        }
        headers.insert(
            "X-Mme-Client-Info",
            HeaderValue::from_str(&client_info.client_info)?,
        );
        headers.insert(
            "User-Agent",
            HeaderValue::from_str(&client_info.user_agent)?,
        );
// removed fake xcode headers

        Ok(headers)
    }

    /// Build a reqwest client with the Apple root certificate
    ///
    /// # Arguments
    /// - `debug`: DANGER, If true, accept invalid certificates and enable verbose connection logging
    /// # Errors
    /// Returns an error if the reqwest client cannot be built
    pub fn build_reqwest_client(
        debug: bool,
        proxy_url: Option<String>,
    ) -> Result<reqwest_middleware::ClientWithMiddleware, Report> {
        #[cfg(not(feature = "wasm"))]
        let cert = Certificate::from_der(APPLE_ROOT)?;
        // Il bordo di Apple lascia passare solo ~2 richieste per connessione e rifiuta il resto
        // (AltSign: sessione nuova e una sola connessione per ogni richiesta). Quindi: HTTP/1.1,
        // niente riuso e "Connection: close" su tutte.
        #[cfg(not(feature = "wasm"))]
        let mut close = HeaderMap::new();
        #[cfg(not(feature = "wasm"))]
        close.insert("Connection", HeaderValue::from_static("close"));
        #[cfg(not(feature = "wasm"))]
        let client = ClientBuilder::new()
            .add_root_certificate(cert)
            .http1_only()
            .http1_title_case_headers()
            .pool_max_idle_per_host(0)
            .default_headers(close)
            .danger_accept_invalid_certs(debug)
            .connection_verbose(debug);
        // Su macOS parla con Apple con il TLS di sistema (Security.framework), come i client Apple.
        #[cfg(all(not(feature = "wasm"), target_os = "macos"))]
        let client = client.tls_backend_native();
        #[cfg(not(feature = "wasm"))]
        let client = client.build()?;
        #[cfg(feature = "wasm")]
        let client = ClientBuilder::new().build()?;

        let builder = MwClientBuilder::new(client);
        let builder = if let Some(proxy_url) = proxy_url {
            builder.with(WasmProxyMiddleware::new(proxy_url))
        } else {
            builder
        };
        Ok(builder.build())
    }
}

pub trait GrandSlamErrorChecker {
    fn check_grandslam_error(self) -> Result<Dictionary, Report<SideloadError>>;
}

impl GrandSlamErrorChecker for Dictionary {
    fn check_grandslam_error(self) -> Result<Self, Report<SideloadError>> {
        let result = match self.get("Status") {
            Some(plist::Value::Dictionary(d)) => d,
            _ => &self,
        };

        if result.get_signed_integer("ec").unwrap_or(0) != 0 {
            bail!(SideloadError::AuthWithMessage(
                result.get_signed_integer("ec").unwrap_or(-1),
                result.get_str("em").unwrap_or("Unknown error").to_string(),
            ))
        }

        Ok(self)
    }
}
