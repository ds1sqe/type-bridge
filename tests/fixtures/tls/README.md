# Isolated TLS test PKI

These files are public, test-only credentials for the opt-in `./test.sh --tls`
lane. The server certificate is valid only for `localhost` and `127.0.0.1`.
Never deploy or reuse this key outside the isolated test stack.

The root signing key is deliberately not checked in. Regenerate all three
tracked PEM files together from the repository root with OpenSSL:

```bash
fixture_dir=tests/fixtures/tls
scratch="$(mktemp -d)"
trap 'rm -rf -- "$scratch"' EXIT
umask 077

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 \
  -out "$scratch/root-ca-key.pem"
openssl req -x509 -new -sha256 -days 3650 \
  -key "$scratch/root-ca-key.pem" \
  -out "$scratch/root-ca.pem" \
  -subj '/CN=type-bridge isolated TLS test root/O=type-bridge test fixtures' \
  -addext 'basicConstraints=critical,CA:TRUE,pathlen:0' \
  -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -addext 'subjectKeyIdentifier=hash'

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 \
  -out "$scratch/server-key.pem"
openssl req -new -sha256 \
  -key "$scratch/server-key.pem" \
  -out "$scratch/server.csr" \
  -subj '/CN=localhost/O=type-bridge test fixtures'
printf '%s\n' \
  '[server]' \
  'basicConstraints=critical,CA:FALSE' \
  'keyUsage=critical,digitalSignature,keyEncipherment' \
  'extendedKeyUsage=serverAuth' \
  'subjectAltName=DNS:localhost,IP:127.0.0.1' \
  'subjectKeyIdentifier=hash' \
  'authorityKeyIdentifier=keyid,issuer' |
  openssl x509 -req -sha256 -days 3650 \
    -in "$scratch/server.csr" \
    -CA "$scratch/root-ca.pem" \
    -CAkey "$scratch/root-ca-key.pem" \
    -CAcreateserial \
    -out "$scratch/server-cert.pem" \
    -extfile /dev/stdin \
    -extensions server

openssl verify -CAfile "$scratch/root-ca.pem" \
  -verify_hostname localhost "$scratch/server-cert.pem"
openssl verify -CAfile "$scratch/root-ca.pem" \
  -verify_ip 127.0.0.1 "$scratch/server-cert.pem"

install -m 0644 "$scratch/root-ca.pem" "$fixture_dir/root-ca.pem"
install -m 0644 "$scratch/server-cert.pem" "$fixture_dir/server-cert.pem"
install -m 0644 "$scratch/server-key.pem" "$fixture_dir/server-key.pem"
```

The tracked key is intentionally world-readable because it is a public test
credential and the unprivileged HAProxy container must read its bind mount.
The endpoint itself is published on host loopback only.

The compose sidecar joins the ordinary TypeDB network and terminates TLS for
both gRPC (`1729`) and version HTTP (`8000`). Its backend links remain local,
plaintext container traffic; only the dynamically published host endpoints
are used by the TLS tests.

CI runs the focused runtime target against TypeDB 3.8.3, 3.11.5, and 3.12.1.
For the `NativeRoots` proof on Linux, the test process scopes the standard
`SSL_CERT_FILE` native-store override to `root-ca.pem` after Cargo has fetched
the locked dependency graph; Cargo then stays offline. This makes native-root
coverage deterministic without installing this public test root globally.
