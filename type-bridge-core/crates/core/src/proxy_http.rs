//! Blocking HTTP client for the standalone server's `/v3/capabilities` and
//! `/v3/staged` routes.
//!
//! The client follows no redirects and ignores ambient proxy variables, like
//! the TypeDB version probe: a redirect or an intermediary could otherwise
//! substitute the executor whose advertisement the caller trusts. TLS uses the
//! same rustls roots as the version probe.

use std::fmt;
use std::io::Read as _;
use std::time::Duration;

use crate::version::{
    RetainedCustomRootCa, TlsConfigurationError, native_root_store, plaintext_agent,
    tls_agent_with_handshake_timeout,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

const MAX_ENDPOINT_BYTES: usize = 4 * 1024;
const MAX_HEADERS: usize = 32;
const MAX_HEADER_BYTES: usize = 8 * 1024;
// Set by the client itself; a caller value would contradict the request.
const RESERVED_HEADERS: [&str; 5] = [
    "content-type",
    "content-length",
    "host",
    "transfer-encoding",
    "connection",
];

/// Trust for one proxy endpoint.
#[derive(Clone, Debug)]
pub enum ProxyTrust {
    /// Plaintext HTTP; the endpoint must use `http://`.
    Plaintext,
    /// TLS with the operating system's trust roots; the endpoint must use
    /// `https://`.
    NativeRoots,
    /// TLS trusting only one retained custom root bundle; the endpoint must
    /// use `https://`.
    CustomRoot(RetainedCustomRootCa),
}

/// Why a proxy request could not be made or answered.
#[derive(Debug)]
#[non_exhaustive]
pub enum ProxyHttpError {
    /// The reasons of this and the next variant are static text without path
    /// separators, so they can serve as SDK diagnostic messages.
    ///
    /// The endpoint is not one `http://` or `https://` base URL matching the
    /// configured trust.
    InvalidEndpoint(&'static str),
    /// A caller-supplied header is malformed or reserved.
    InvalidHeader(&'static str),
    /// The TLS trust could not be built.
    Tls(TlsConfigurationError),
    /// The request did not complete (connection, TLS handshake, timeout, or
    /// body read).
    Transport(String),
    /// The route answered with a status other than 200.
    Status {
        /// The route that was called.
        route: &'static str,
        /// The HTTP status code.
        status: u16,
    },
    /// The response body exceeds the caller's byte bound.
    ResponseTooLarge {
        /// The bound in bytes.
        limit: usize,
    },
}

impl ProxyHttpError {
    /// Return the stable machine-readable diagnostic code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidEndpoint(_) => "proxy_endpoint_invalid",
            Self::InvalidHeader(_) => "proxy_header_invalid",
            Self::Tls(error) => error.code(),
            Self::Transport(_) => "proxy_transport_failed",
            Self::Status { .. } => "proxy_http_status",
            Self::ResponseTooLarge { .. } => "proxy_response_too_large",
        }
    }
}

impl fmt::Display for ProxyHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint(reason) => write!(f, "proxy endpoint is invalid: {reason}"),
            Self::InvalidHeader(reason) => write!(f, "proxy request header is invalid: {reason}"),
            Self::Tls(error) => error.fmt(f),
            Self::Transport(message) => write!(f, "proxy request failed: {message}"),
            Self::Status { route, status } => {
                write!(f, "proxy route {route} answered HTTP status {status}")
            }
            Self::ResponseTooLarge { limit } => {
                write!(f, "proxy response exceeds its {limit}-byte bound")
            }
        }
    }
}

impl std::error::Error for ProxyHttpError {}

impl From<TlsConfigurationError> for ProxyHttpError {
    fn from(error: TlsConfigurationError) -> Self {
        Self::Tls(error)
    }
}

/// Exact HTTP status and body from the opt-in raw route.
#[derive(Debug)]
pub struct RawHttpResponse {
    /// HTTP response status, including refusals and provider errors.
    pub status: u16,
    /// Unmodified response bytes.
    pub body: Vec<u8>,
}

/// One validated proxy endpoint with its trust and request headers.
pub struct ProxyHttpClient {
    agent: ureq::Agent,
    base: String,
    headers: Vec<(String, String)>,
}

impl fmt::Debug for ProxyHttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Header values are commonly credentials.
        f.debug_struct("ProxyHttpClient")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl ProxyHttpClient {
    /// Validate the endpoint and headers and build the client.
    ///
    /// `endpoint` is the server's base URL, such as `https://proxy:8443` or
    /// `http://localhost:8080/typebridge`; the routes are appended to it.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyHttpError::InvalidEndpoint`] when the endpoint is not an
    /// `http://` or `https://` URL without query, fragment, or user info, or
    /// when its scheme contradicts `trust`;
    /// [`ProxyHttpError::InvalidHeader`] for a malformed or reserved header;
    /// [`ProxyHttpError::Tls`] when the trust roots cannot be loaded.
    pub fn new(
        endpoint: &str,
        trust: &ProxyTrust,
        headers: &[(String, String)],
    ) -> Result<Self, ProxyHttpError> {
        let base = validate_endpoint(endpoint, trust)?;
        validate_headers(headers)?;
        let agent = match trust {
            ProxyTrust::Plaintext => plaintext_agent(),
            ProxyTrust::NativeRoots => {
                tls_agent_with_handshake_timeout(native_root_store()?, Some(CONNECT_TIMEOUT))?
            }
            ProxyTrust::CustomRoot(roots) => {
                tls_agent_with_handshake_timeout(roots.root_store(), Some(CONNECT_TIMEOUT))?
            }
        };
        Ok(Self {
            agent,
            base,
            headers: headers.to_vec(),
        })
    }

    /// Fetch the exact staged (`/v3/capabilities`) advertisement bytes.
    ///
    /// # Errors
    ///
    /// Returns transport, non-200 status, or response-size errors.
    pub fn staged_capabilities(
        &self,
        timeout: Option<Duration>,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ProxyHttpError> {
        const ROUTE: &str = "/v3/capabilities";
        let request = self.request(
            "GET",
            ROUTE,
            Some(timeout.map_or(CONNECT_TIMEOUT, |timeout| timeout.min(CONNECT_TIMEOUT))),
        );
        read_response(ROUTE, request.call(), max_bytes)
    }

    /// Send one canonical staged request to `/v3/staged` and return the
    /// exact reply bytes.
    ///
    /// # Errors
    ///
    /// Returns transport, non-200 status, or response-size errors.
    pub fn exchange_staged(
        &self,
        request: &[u8],
        timeout: Option<Duration>,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ProxyHttpError> {
        const ROUTE: &str = "/v3/staged";
        let call = self
            .request("POST", ROUTE, timeout)
            .set("content-type", "application/json");
        read_response(ROUTE, call.send_bytes(request), max_bytes)
    }

    /// Post unchecked TypeQL to `/v3/raw`, retaining error bodies and status.
    /// No execution timeout or answer-byte ceiling is added by the client.
    /// Endpoint trust, headers and redirect refusal are shared with staged calls.
    ///
    /// # Errors
    /// Returns connection, TLS or body-read failures.
    pub fn exchange_raw(&self, request: &[u8]) -> Result<RawHttpResponse, ProxyHttpError> {
        let response = match self
            .request("POST", "/v3/raw", None)
            .set("content-type", "application/json")
            .send_bytes(request)
        {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(error)) => {
                return Err(ProxyHttpError::Transport(error.to_string()));
            }
        };
        let status = response.status();
        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|error| ProxyHttpError::Transport(error.to_string()))?;
        Ok(RawHttpResponse { status, body })
    }

    fn request(&self, method: &str, route: &str, timeout: Option<Duration>) -> ureq::Request {
        let mut request = self.agent.request(method, &format!("{}{route}", self.base));
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        for (name, value) in &self.headers {
            request = request.set(name, value);
        }
        request
    }
}

fn read_response(
    route: &'static str,
    response: Result<ureq::Response, ureq::Error>,
    max_bytes: usize,
) -> Result<Vec<u8>, ProxyHttpError> {
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(status, _)) => {
            return Err(ProxyHttpError::Status { route, status });
        }
        Err(ureq::Error::Transport(error)) => {
            return Err(ProxyHttpError::Transport(error.to_string()));
        }
    };
    if response.status() != 200 {
        return Err(ProxyHttpError::Status {
            route,
            status: response.status(),
        });
    }
    let mut body = Vec::new();
    response
        .into_reader()
        .take(
            u64::try_from(max_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut body)
        .map_err(|error| ProxyHttpError::Transport(error.to_string()))?;
    if body.len() > max_bytes {
        return Err(ProxyHttpError::ResponseTooLarge { limit: max_bytes });
    }
    Ok(body)
}

fn validate_endpoint(endpoint: &str, trust: &ProxyTrust) -> Result<String, ProxyHttpError> {
    if endpoint.len() > MAX_ENDPOINT_BYTES {
        return Err(ProxyHttpError::InvalidEndpoint(
            "the endpoint exceeds 4096 bytes",
        ));
    }
    let tls = !matches!(trust, ProxyTrust::Plaintext);
    let rest = if let Some(rest) = endpoint.strip_prefix("https://") {
        if !tls {
            return Err(ProxyHttpError::InvalidEndpoint(
                "an https endpoint requires TLS",
            ));
        }
        rest
    } else if let Some(rest) = endpoint.strip_prefix("http://") {
        if tls {
            return Err(ProxyHttpError::InvalidEndpoint(
                "TLS requires an https endpoint",
            ));
        }
        rest
    } else {
        return Err(ProxyHttpError::InvalidEndpoint(
            "the endpoint must use the http or https scheme",
        ));
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Err(ProxyHttpError::InvalidEndpoint("the endpoint has no host"));
    }
    if authority.contains('@') {
        return Err(ProxyHttpError::InvalidEndpoint(
            "credentials belong in a header, not the endpoint",
        ));
    }
    if rest.contains(['?', '#']) {
        return Err(ProxyHttpError::InvalidEndpoint(
            "the endpoint must not carry a query or fragment",
        ));
    }
    if rest
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(ProxyHttpError::InvalidEndpoint(
            "the endpoint contains whitespace or control characters",
        ));
    }
    Ok(endpoint.trim_end_matches('/').to_owned())
}

fn validate_headers(headers: &[(String, String)]) -> Result<(), ProxyHttpError> {
    if headers.len() > MAX_HEADERS {
        return Err(ProxyHttpError::InvalidHeader("more than 32 headers"));
    }
    for (name, value) in headers {
        if name.len() + value.len() > MAX_HEADER_BYTES {
            return Err(ProxyHttpError::InvalidHeader("a header exceeds 8192 bytes"));
        }
        // RFC 9110 token characters.
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return Err(ProxyHttpError::InvalidHeader(
                "a header name is not an HTTP token",
            ));
        }
        if RESERVED_HEADERS
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
        {
            return Err(ProxyHttpError::InvalidHeader(
                "a header name is set by the client itself",
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte == b'\t' || (b' '..=b'~').contains(&byte))
        {
            return Err(ProxyHttpError::InvalidHeader(
                "a header value contains control or non-ASCII characters",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    fn header(name: &str, value: &str) -> (String, String) {
        (name.to_owned(), value.to_owned())
    }

    #[test]
    fn endpoint_scheme_must_match_trust() {
        let cases = [
            ("https://proxy:8443", ProxyTrust::Plaintext),
            ("http://proxy:8080", ProxyTrust::NativeRoots),
            ("proxy:8080", ProxyTrust::Plaintext),
            ("http://", ProxyTrust::Plaintext),
            ("http://user:pass@proxy:8080", ProxyTrust::Plaintext),
            ("http://proxy:8080/?x=1", ProxyTrust::Plaintext),
            ("http://proxy:8080/a b", ProxyTrust::Plaintext),
        ];
        for (endpoint, trust) in cases {
            let error = ProxyHttpClient::new(endpoint, &trust, &[]).unwrap_err();
            assert_eq!(error.code(), "proxy_endpoint_invalid", "{endpoint}");
        }
        let client =
            ProxyHttpClient::new("http://proxy:8080/base/", &ProxyTrust::Plaintext, &[]).unwrap();
        assert_eq!(client.base, "http://proxy:8080/base");
    }

    #[test]
    fn headers_must_be_tokens_and_not_reserved() {
        for headers in [
            vec![header("Content-Type", "text/plain")],
            vec![header("Bad Name", "x")],
            vec![header("Authorization", "Bearer a\r\nInjected: 1")],
            vec![header("", "x")],
        ] {
            let error = ProxyHttpClient::new("http://proxy:8080", &ProxyTrust::Plaintext, &headers)
                .unwrap_err();
            assert_eq!(error.code(), "proxy_header_invalid");
        }
        let client = ProxyHttpClient::new(
            "http://proxy:8080",
            &ProxyTrust::Plaintext,
            &[header("Authorization", "Bearer secret")],
        )
        .unwrap();
        assert!(!format!("{client:?}").contains("secret"));
    }

    /// Serve one HTTP response and return the request head and body it got.
    fn serve_once(response: &'static [u8]) -> (String, thread::JoinHandle<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let mut stream = stream;
            stream.write_all(response).unwrap();
            (head, body)
        });
        (endpoint, server)
    }

    #[test]
    fn exchange_posts_the_exact_bytes_with_caller_headers() {
        let (endpoint, server) = serve_once(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nreply");
        let client = ProxyHttpClient::new(
            &format!("{endpoint}/"),
            &ProxyTrust::Plaintext,
            &[header("Authorization", "Bearer token")],
        )
        .unwrap();
        let reply = client
            .exchange_staged(b"{\"a\":1}", Some(Duration::from_secs(5)), 5)
            .unwrap();
        assert_eq!(reply, b"reply");
        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("POST /v3/staged HTTP/1.1\r\n"), "{head}");
        let head = head.to_ascii_lowercase();
        assert!(head.contains("authorization: bearer token\r\n"));
        assert!(head.contains("content-type: application/json\r\n"));
        assert_eq!(body, b"{\"a\":1}");
    }

    #[test]
    fn raw_exchange_retains_success_refusal_and_redirect_without_rewriting() {
        for (response, status, body) in [
            (&b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nreply"[..], 200, &b"reply"[..]),
            (&b"HTTP/1.1 403 Forbidden\r\ncontent-length: 24\r\n\r\n{\"error\":\"raw_disabled\"}"[..], 403, &b"{\"error\":\"raw_disabled\"}"[..]),
            (&b"HTTP/1.1 302 Found\r\nlocation: http://elsewhere/\r\ncontent-length: 0\r\n\r\n"[..], 302, &b""[..]),
        ] {
            let (endpoint, server) = serve_once(response);
            let client = ProxyHttpClient::new(
                &endpoint, &ProxyTrust::Plaintext, &[header("Authorization", "Bearer raw")],
            ).unwrap();
            let reply = client.exchange_raw(b"{\"query\":\"define entity raw;\"}").unwrap();
            assert_eq!(reply.status, status);
            assert_eq!(reply.body, body);
            let (head, request) = server.join().unwrap();
            assert!(head.starts_with("POST /v3/raw HTTP/1.1\r\n"));
            assert!(head.to_ascii_lowercase().contains("authorization: bearer raw\r\n"));
            assert_eq!(request, b"{\"query\":\"define entity raw;\"}");
        }
    }

    #[test]
    fn oversized_and_non_200_responses_are_refused() {
        let (endpoint, server) = serve_once(b"HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\nreply!");
        let client = ProxyHttpClient::new(&endpoint, &ProxyTrust::Plaintext, &[]).unwrap();
        let error = client
            .staged_capabilities(Some(Duration::from_secs(5)), 5)
            .unwrap_err();
        assert_eq!(error.code(), "proxy_response_too_large");
        assert!(
            server
                .join()
                .unwrap()
                .0
                .starts_with("GET /v3/capabilities ")
        );

        for response in [
            &b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n"[..],
            &b"HTTP/1.1 302 Found\r\nlocation: http://elsewhere/\r\ncontent-length: 0\r\n\r\n"[..],
        ] {
            let response: &'static [u8] = Box::leak(response.to_vec().into_boxed_slice());
            let (endpoint, server) = serve_once(response);
            let client = ProxyHttpClient::new(&endpoint, &ProxyTrust::Plaintext, &[]).unwrap();
            let error = client
                .exchange_staged(b"{}", Some(Duration::from_secs(5)), 64)
                .unwrap_err();
            assert_eq!(error.code(), "proxy_http_status");
            server.join().unwrap();
        }
    }
    #[test]
    fn unbounded_execution_still_times_out_a_stalled_tls_handshake() {
        let listener = (41690..41700)
            .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = std::sync::mpsc::channel();
        let peer = thread::spawn(move || {
            let (_socket, _) = listener.accept().unwrap();
            let _ = stopped.recv_timeout(Duration::from_secs(5));
        });
        let mut roots = rustls::RootCertStore::empty();
        let pem = include_bytes!("../../../../tests/fixtures/tls/root-ca.pem");
        for certificate in rustls_pemfile::certs(&mut &pem[..]) {
            roots.add(certificate.unwrap()).unwrap();
        }
        let client = ProxyHttpClient {
            agent: tls_agent_with_handshake_timeout(roots, Some(Duration::from_millis(50)))
                .unwrap(),
            base: format!("https://{address}"),
            headers: Vec::new(),
        };
        let started = std::time::Instant::now();
        let result = client.exchange_staged(b"{}", None, 1024);
        let elapsed = started.elapsed();
        stop.send(()).unwrap();
        peer.join().unwrap();
        assert!(matches!(result, Err(ProxyHttpError::Transport(_))));
        assert!(
            elapsed < Duration::from_secs(2),
            "stalled TLS took {elapsed:?}"
        );
    }
}
