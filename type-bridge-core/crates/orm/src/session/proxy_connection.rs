//! Built-in HTTP transport for a proxy connection.
//!
//! Every binding reaches the standalone server's `/v3/capabilities` and
//! `/v3/staged` routes through this one transport. Callers with their own
//! route or authentication keep supplying an exchange instead.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_core_lib::proxy_http::{ProxyHttpClient, ProxyHttpError, ProxyTrust};
use type_bridge_core_lib::version::RetainedCustomRootCa;

use super::backend::AnswerCancellation;
use super::tls_policy::{TlsPolicy, TlsPolicyKind};
use crate::query_execution_limits::QueryExecutionDeadline;

/// Where a proxy connection sends its requests: the server's base URL, its
/// TLS policy, and request headers such as `Authorization`.
#[derive(Clone)]
pub struct ProxyEndpoint {
    url: String,
    tls: TlsPolicy,
    headers: Vec<(String, String)>,
}

impl ProxyEndpoint {
    /// A plaintext endpoint, such as `http://localhost:8080`. The routes are
    /// appended to `url`.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            tls: TlsPolicy::disabled(),
            headers: Vec::new(),
        }
    }

    /// Set the TLS policy; TLS requires an `https://` URL.
    #[must_use]
    pub fn tls(mut self, tls: TlsPolicy) -> Self {
        self.tls = tls;
        self
    }

    /// Add one header sent with every request.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

impl fmt::Debug for ProxyEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProxyEndpoint([REDACTED])")
    }
}

/// One opened proxy endpoint: validated URL and headers, snapshotted trust.
#[derive(Clone)]
pub struct ProxyHttpTransport {
    client: Arc<ProxyHttpClient>,
}

impl fmt::Debug for ProxyHttpTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProxyHttpTransport([REDACTED])")
    }
}

impl ProxyHttpTransport {
    /// Validate the endpoint and load its trust roots. A custom root bundle
    /// is read once, here.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input diagnostic for a malformed endpoint or header,
    /// or a TLS configuration diagnostic when the roots cannot be loaded.
    pub fn open(endpoint: &ProxyEndpoint) -> Result<Self, SdkExecutionDiagnostic> {
        let trust = match &endpoint.tls.kind {
            TlsPolicyKind::Disabled => ProxyTrust::Plaintext,
            TlsPolicyKind::NativeRoots => ProxyTrust::NativeRoots,
            TlsPolicyKind::CustomRoot(path) => {
                ProxyTrust::CustomRoot(RetainedCustomRootCa::load_configured_alias(path).map_err(
                    |error| lower(ProxyHttpError::Tls(error), SdkProviderOperation::Connect),
                )?)
            }
        };
        let client = ProxyHttpClient::new(&endpoint.url, &trust, &endpoint.headers)
            .map_err(|error| lower(error, SdkProviderOperation::Connect))?;
        Ok(Self {
            client: Arc::new(client),
        })
    }

    /// Fetch the executor's staged (`query-remote/v3`) advertisement.
    ///
    /// # Errors
    ///
    /// Returns cancellation, deadline, transport, or non-200 status errors.
    pub async fn staged_capabilities(
        &self,
        deadline: QueryExecutionDeadline,
        cancellation: &AnswerCancellation,
    ) -> Result<Vec<u8>, SdkExecutionDiagnostic> {
        let client = Arc::clone(&self.client);
        run(
            deadline,
            cancellation,
            SdkProviderOperation::Connect,
            move |timeout| client.staged_capabilities(timeout, MAX_REMOTE_ENVELOPE_BYTES),
        )
        .await
    }

    /// Send one canonical staged request to `/v3/staged`.
    ///
    /// # Errors
    ///
    /// Returns cancellation, deadline, size, or provider errors for `write`.
    pub async fn exchange_staged(
        &self,
        request: Vec<u8>,
        max_bytes: usize,
        deadline: QueryExecutionDeadline,
        cancellation: &AnswerCancellation,
    ) -> Result<Vec<u8>, SdkExecutionDiagnostic> {
        let client = Arc::clone(&self.client);
        run(
            deadline,
            cancellation,
            SdkProviderOperation::Write,
            move |timeout| client.exchange_staged(&request, timeout, max_bytes),
        )
        .await
    }
}

const BLOCKING_GRACE: Duration = Duration::from_secs(1);

async fn run(
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
    operation: SdkProviderOperation,
    call: impl FnOnce(Option<Duration>) -> Result<Vec<u8>, ProxyHttpError> + Send + 'static,
) -> Result<Vec<u8>, SdkExecutionDiagnostic> {
    bounded_exchange(deadline, cancellation, async move {
        // The blocking worker gets a grace period; the invocation deadline wins.
        let timeout = deadline
            .remaining_milliseconds()
            .map(|remaining| Duration::from_millis(remaining).saturating_add(BLOCKING_GRACE));
        match tokio::task::spawn_blocking(move || call(timeout)).await {
            Ok(result) => result.map_err(|error| lower(error, operation)),
            Err(_) => Err(SdkExecutionDiagnostic::internal_failure()),
        }
    })
    .await
}

pub(crate) async fn bounded_exchange<E: From<SdkExecutionDiagnostic>>(
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
    exchange: impl std::future::Future<Output = Result<Vec<u8>, E>>,
) -> Result<Vec<u8>, E> {
    deadline.check(cancellation).map_err(E::from)?;
    let result = tokio::select! {
        biased;
        () = cancellation.cancelled() => None,
        () = deadline.elapsed() => None,
        result = exchange => Some(result),
    };
    deadline.check(cancellation).map_err(E::from)?;
    result.unwrap_or_else(|| Err(SdkExecutionDiagnostic::internal_failure().into()))
}

fn lower(error: ProxyHttpError, operation: SdkProviderOperation) -> SdkExecutionDiagnostic {
    let code = error.code();
    match error {
        ProxyHttpError::InvalidEndpoint(reason) | ProxyHttpError::InvalidHeader(reason) => {
            SdkExecutionDiagnostic::invalid_input(static_code(code), static_message(reason))
        }
        ProxyHttpError::Tls(_) if code == "tls_custom_root_ca_too_large" => {
            SdkExecutionDiagnostic::resource_limit(
                static_code(code),
                static_message("The proxy TLS policy is invalid"),
            )
        }
        ProxyHttpError::Tls(_) => SdkExecutionDiagnostic::invalid_input(
            static_code(code),
            static_message("The proxy TLS policy is invalid"),
        ),
        ProxyHttpError::ResponseTooLarge { .. } => SdkExecutionDiagnostic::resource_limit(
            static_code(code),
            static_message("The proxy reply exceeds its byte bound"),
        ),
        // Redacted like every provider failure: the cause may name the
        // endpoint or echo a header.
        _ => SdkExecutionDiagnostic::provider_failure(operation),
    }
}

fn static_code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static proxy diagnostic code is canonical")
}

fn static_message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static proxy diagnostic message is valid")
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::query_execution_limits::QueryExecutionResourceLimits;

    fn deadline(milliseconds: u64) -> QueryExecutionDeadline {
        QueryExecutionDeadline::for_limits(QueryExecutionResourceLimits {
            timeout_milliseconds: crate::QueryTimeout::from_milliseconds(milliseconds),
            ..QueryExecutionResourceLimits::default()
        })
    }

    #[test]
    fn malformed_endpoints_and_headers_are_invalid_input() {
        for endpoint in [
            ProxyEndpoint::new("https://proxy:8443"),
            ProxyEndpoint::new("http://proxy:8080").tls(TlsPolicy::native_roots()),
            ProxyEndpoint::new("http://proxy:8080").header("Content-Length", "1"),
        ] {
            let error = ProxyHttpTransport::open(&endpoint).unwrap_err();
            assert!(
                matches!(
                    error.code().as_str(),
                    "proxy_endpoint_invalid" | "proxy_header_invalid"
                ),
                "{error:?}"
            );
        }
        let endpoint = ProxyEndpoint::new("http://proxy:8080").header("Authorization", "secret");
        assert!(!format!("{endpoint:?}").contains("secret"));
    }

    #[tokio::test]
    async fn cancellation_and_deadline_end_a_silent_exchange() {
        // Accepts the connection and never answers.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = ProxyEndpoint::new(format!("http://{}", listener.local_addr().unwrap()));
        let transport = ProxyHttpTransport::open(&endpoint).unwrap();

        let error = transport
            .exchange_staged(
                b"{}".to_vec(),
                64,
                deadline(50),
                &AnswerCancellation::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "transaction_deadline_exceeded");

        let cancellation = AnswerCancellation::default();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        });
        let error = transport
            .staged_capabilities(deadline(30_000), &cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "provider_cancelled");
        drop(listener);
    }

    #[tokio::test]
    async fn unreachable_proxy_is_a_redacted_provider_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let transport =
            ProxyHttpTransport::open(&ProxyEndpoint::new(format!("http://{address}"))).unwrap();
        let error = transport
            .staged_capabilities(deadline(30_000), &AnswerCancellation::default())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "provider_operation_failed");
        assert!(!format!("{error:?}").contains(&address.port().to_string()));
    }
}
