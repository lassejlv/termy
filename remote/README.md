# TermyRemoteKit

What the Termy iPhone app (`ios/`) and `termy remote` (`next/Sources/TermyRemoteAgent`)
share to reach terminal sessions through the relay (`crates/relay`):

- `SecureChannel`: the end-to-end handshake and transport. The client sends
  its ephemeral and static X25519 keys; the host answers with its ephemeral
  key and a sealed confirmation. Keys come from HKDF-SHA256 over
  DH(e,e), DH(e_client, s_host) and DH(s_client, e_host), salted with the
  hash of all four public keys, so only the real host can answer and only the
  real phone can send. Transport is ChaCha20-Poly1305 with a 64-bit counter
  nonce per direction; replays are refused.
- `TrustStore`: keys pinned per device. The phone pins a computer after the
  user compares fingerprints; the computer asks before trusting a new phone
  (`termy remote serve` prompts, `--trust-new` skips the prompt).
- `Messages`: what travels inside the channel. The host keeps the terminal
  (libtermy in the session host); the phone receives style runs per changed
  row, at most one frame every 33 ms, and sends logical keys the host encodes
  for the program's keyboard mode.
- `Relay`: the account API and the relay socket.

```sh
just test-remote
```
