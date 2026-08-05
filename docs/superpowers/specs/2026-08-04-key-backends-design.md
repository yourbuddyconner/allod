# Key backends: file, macOS Keychain, YubiKey — and the passkey boundary

Date: 2026-08-04
Status: approved design. File + Keychain implement in milestone 4
(foundation sub-project); YubiKey PIV implements later behind the same
interface. Companion to the freehold milestone-4 design
(`freehold: docs/specs/2026-08-04-governed-review-surface-design.md`).

## Goal

Signing keys stop living as cleartext YAML inside governed repositories.
A `KeyBackend` abstraction gives the allod CLI and freehold a common way
to resolve and use signing keys, with three backends: a file backend
relocated out of the repo, a macOS Keychain backend gated by the
platform's unlock (Touch ID where the platform grants it), and a YubiKey
PIV backend where the secret never exists on the host at all.

## Constraints that shape everything here

- Allod signatures are raw ed25519 over a payload string (changeset hash,
  decision payload, envelope payload). Verification reads the public key
  from the graph's KeyRecord list (§6.2). Any backend that produces a
  standard ed25519 signature is drop-in; nothing about the graph, the
  wire format, or verification changes.
- **FIDO2/WebAuthn passkeys cannot do this.** A WebAuthn authenticator
  signs only its own assertion envelope (authenticatorData ‖
  clientDataHash), never caller-chosen bytes. Apple's Secure Enclave
  additionally signs only P-256. Supporting them means a protocol
  extension, not a backend — see "The passkey boundary" below.
- YubiKeys CAN do raw ed25519: PIV slots on firmware 5.7+, or the
  OpenPGP applet. FIDO2 mode on the same physical key is not usable for
  this.
- wasm never holds backend secrets. Today the wasm graph signs with file
  keys the host passes in; with keychain or hardware backends the secret
  must not enter the sandbox, so signing becomes host-side (two-phase
  seam below).

## The KeyBackend interface

Rust (new module `allod-core::keys` or a small `allod-keys` crate — the
implementation plan decides placement):

```rust
pub trait KeyBackend {
    /// A principal's signing capability for a graph, or why not.
    fn resolve(&self, graph_id: &str, principal: &str) -> Result<KeyHandle, String>;
    /// Raw ed25519 signature over the payload.
    fn sign(&self, handle: &KeyHandle, payload: &str) -> Result<String, String>;
    /// The public half, hex — for enrollment and display.
    fn public(&self, handle: &KeyHandle) -> Result<String, String>;
}
```

Resolution order is configuration, not hardcoding: a graph-level setting
(default `[keychain, file]` on macOS, `[file]` elsewhere) tried in order;
the first backend that resolves wins. `KeyHandle` is backend-tagged and
opaque to callers. Every current `Graph::load_key` call site moves to the
abstraction; `Keypair` remains the file backend's internal type.

## Backends

### file

- Location: `~/.local/share/allod/keys/<graph-id>/<principal>.yaml`
  (XDG_DATA_HOME respected), `graph-id` read from the graph's
  `graph.yaml`. Same YAML shape as today.
- `.allod/keys/` stays as a read fallback so existing graphs keep
  working; new keys are always created at the XDG path; a `allod key
  migrate` command moves a repo-local key out and reports what it did.
- `allod init` writes the `.gitignore` entry for `.allod/keys/`
  regardless of backend — belt and suspenders for the fallback era.

### keychain (macOS)

- Storage: a generic-password item, service `allod`, account
  `<graph-id>/<principal>`, value = the ed25519 secret (32 bytes,
  base64). Created by `allod key migrate --to keychain` or at key
  generation with `--backend keychain`.
- Access control: the item is created with the strictest access control
  the calling context can obtain (Rust `security-framework`,
  `SecAccessControl` with biometry-or-passcode). Honesty note: unsigned
  developer binaries may get a keychain password prompt rather than a
  Touch ID sheet — the platform decides based on code signature and
  entitlements. The design promises "the platform's unlock gate," not
  unconditionally Touch ID.
- Signing: retrieval (which triggers the prompt), then in-process ed25519
  sign, then the secret is zeroized. The secret exists in process memory
  only for the duration of the sign call.
- freehold's TypeScript mirror uses the same item (service/account
  convention shared), retrieved via the Security framework through its
  node binding or the `security` CLI, signed with Node's built-in
  ed25519 (`crypto.sign(null, payload, keyObject)`). Parity with the
  Rust backend is a tested property (same item, same signature).

### yubikey-piv (deferred implementation, designed now)

- Requires YubiKey firmware 5.7+ (ed25519 in PIV). Key generated
  on-device in a PIV slot (default 9c, digital signature, PIN-per-use
  policy; touch policy configurable at enrollment). The secret never
  exists outside the device.
- Rust implementation over the `yubikey` crate (pcsc). `resolve` finds
  the device + slot by serial recorded in the graph-level key config;
  `sign` prompts for PIN (and touch, per policy).
- Enrollment: `allod key enroll --backend yubikey-piv --as <principal>`
  generates on-device, reads back the public key, and emits the §6.2 key
  rotation changeset (below).
- The OpenPGP applet is a fallback route for pre-5.7 firmware; same
  trait, more moving parts (gpg-agent); only if demand appears.
- Failure UX: device absent → resolve fails with "insert YubiKey serial
  <n>"; wrong PIN counts surfaced; freehold shows governance actions
  disabled-with-reason exactly as for a missing file key.

## The host-signing seam (wasm and freehold)

The wasm graph currently signs internally with key material the host
hands it. That path remains for the file backend. For keychain and
hardware backends, signing is two-phase:

1. wasm exposes payload builders — pure functions with no key access:
   `decision_payload(record)` (exists in Rust today), and the changeset
   preimage builder for commits. The host asks wasm for the exact bytes
   to sign.
2. The host signs via its KeyBackend (prompting as the backend requires)
   and hands the signature back to a wasm assembler that attaches it and
   proceeds (admission, notes append, whatever the flow is).

Parity between "wasm-internal file signing" and "host-side backend
signing" is a tested invariant: same inputs, byte-identical records.
freehold's approve button therefore triggers the biometric or PIN prompt
at decision time, which is exactly the ceremony a signed approval should
have.

## Key lifecycle in the graph

Backends change where secrets live; the graph's model doesn't change:

- Enrollment of an additional key (e.g. adding a YubiKey next to a file
  key) is a key rotation changeset updating the principal's KeyRecord
  list — a new active record, signed by an existing active key (§6.2).
  Multiple active keys per principal are already legal; file and hardware
  keys coexist during migration.
- Revocation is a KeyRecord status update, same mechanics.
- Verification everywhere (CLI verify, CI, freehold) is untouched: public
  keys in graph state, raw ed25519, no backend awareness.

## The passkey boundary (future, protocol-level)

True passkeys — iCloud-synced WebAuthn credentials, Secure Enclave,
security keys in FIDO2 mode — require the graph to verify WebAuthn
assertion envelopes instead of raw ed25519. That is a §6 protocol
extension, sketched here so the door is visibly open and visibly not
walked through:

- KeyRecord gains a `sig_type` (`ed25519` default, `webauthn-es256` /
  `webauthn-ed25519` extension) and stores the credential public key.
- A signature of type webauthn is the assertion triple
  (authenticatorData, clientDataJSON, signature) with the payload hash
  bound as the challenge; verifiers check the envelope, the RP ID, and
  the inner signature.
- Cost: every verifier updates (allod verify, CI, freehold, future
  federated peers), Appendix H gains vectors for the new type, and the
  spec's signature sections take the extension. Roughly a milestone of
  its own, and only worth it when a user population without CLI access
  needs to sign — the cloud multi-user freehold. Not scheduled.

## Testing

- file: fully automated (unit + CLI integration, XDG path override via
  env for hermetic tests; fallback-read and migrate covered).
- keychain: automated where CI permits (macOS runners can create
  non-biometric items and sign); the biometric ACL path is a manual
  checklist (create with ACL, observe prompt, sign, revoke).
- yubikey-piv: hardware-in-the-loop manual checklist; CI compiles the
  backend behind a feature flag.
- Parity: one vector suite — payload → signature verify — run against
  every backend available in the environment, plus the wasm-internal vs
  host-signed record equality test.

## Out of scope

- WebAuthn/passkey signature profile (sketched above, not scheduled).
- Windows/Linux platform keystores (DPAPI, libsecret) — same trait,
  future backends.
- Threshold or multi-signature schemes (§4.6 signature thresholds exist
  at the policy layer and are unaffected).
