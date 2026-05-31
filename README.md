# opera-proxy-rs

A Rust HTTP forward proxy that routes traffic through Opera / SurfEasy VPN endpoints. It registers with the SurfEasy API, discovers upstream proxy servers for a chosen country, and exposes a local proxy your browser or tools can use.

## Quick start

### From source

Requires a recent Rust toolchain and build tools (`cmake`, `clang`, `perl` on Linux — see CI).

```bash
cargo build --release
./target/release/opera-proxy-rs
```

By default the proxy listens on `127.0.0.1:18080`.

### Docker

A pre-built image is published as [`1337kavin/opera-proxy-rs:latest`](https://hub.docker.com/r/1337kavin/opera-proxy-rs).

```bash
docker run --rm -p 18080:18080 1337kavin/opera-proxy-rs:latest
```

The container binds to `0.0.0.0:18080`. Map the port as needed.

## Using the proxy

Point any HTTP or HTTPS client at the local listener. No username or password are required on the local side; authentication to SurfEasy is handled internally.

**Environment variables (common tools):**

```bash
export http_proxy=http://127.0.0.1:18080
export https_proxy=http://127.0.0.1:18080
export all_proxy=http://127.0.0.1:18080
```

**curl:**

```bash
curl -x http://127.0.0.1:18080 https://example.com
```

**Browser:** set the system or browser HTTP/HTTPS proxy to `127.0.0.1` port `18080`.

- Plain HTTP requests are forwarded directly.
- HTTPS uses `CONNECT` tunneling through the upstream SurfEasy endpoint.

## Configuration

All options are CLI flags. Run `opera-proxy-rs --help` for the full list.

| Flag | Default | Description |
|------|---------|-------------|
| `--bind-address` | `127.0.0.1:18080` | Address and port to listen on |
| `--country` | `EU` | SurfEasy country / region code for endpoint discovery |
| `--refresh` | `4h` | How often to refresh login, device password, and endpoints |
| `--timeout` | `30s` | Upstream request and connect timeout |
| `--api-login` | `se0316` | SurfEasy API client type |
| `--api-password` | *(built-in default)* | SurfEasy API client secret |
| `--rotation-mode` | `round-robin` | Upstream selection: `round-robin` or `random` |
| `--fake-sni` | — | Override TLS SNI / host sent to upstream |
| `--override-proxy-address` | — | Skip discovery; use a fixed upstream `host[:port]` |
| `--server-name-override` | — | TLS server name when using `--override-proxy-address` |

Durations accept [humantime](https://docs.rs/humantime) syntax (`30s`, `4h`, `1d`, etc.).

**Example — US endpoints, random rotation, listen on all interfaces:**

```bash
opera-proxy-rs \
  --bind-address 0.0.0.0:18080 \
  --country US \
  --rotation-mode random
```

**Example — fixed upstream (debugging):**

```bash
opera-proxy-rs \
  --override-proxy-address 203.0.113.10:443 \
  --server-name-override us0.sec-tunnel.com
```

When `--override-proxy-address` is set, the background task still refreshes API credentials but does not replace the endpoint pool.

## How it works

1. On startup, the proxy registers or logs in to the SurfEasy API and discovers available proxy endpoints for `--country`.
2. Incoming client connections are accepted on `--bind-address`.
3. Each request picks an upstream endpoint (`--rotation-mode`) and connects using SurfEasy device credentials.
4. A background loop periodically refreshes login, device password, and (unless overridden) the endpoint list.

Logging uses [`tracing`](https://docs.rs/tracing); set `RUST_LOG=info` (or `debug`) for more detail:

```bash
RUST_LOG=info opera-proxy-rs
```

## Development

```bash
cargo build --locked
cargo test --locked
```

## License

MIT — see [LICENSE](LICENSE).
