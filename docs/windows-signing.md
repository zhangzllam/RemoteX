# Windows signing

RemoteX has two independent signatures:

1. **Updater signature** verifies that an installer accepted by the in-app
   updater was produced with the RemoteX minisign key. It is mandatory for
   updater artifacts.
2. **Authenticode** identifies the Windows publisher and helps Windows and
   SmartScreen establish trust. It is optional until the maintainer supplies a
   suitable code-signing certificate.

The release workflow supports Authenticode without storing a certificate in the
repository. Configure these GitHub Actions secrets:

- `WINDOWS_CERTIFICATE_BASE64`: base64-encoded PFX.
- `WINDOWS_CERTIFICATE_PASSWORD`: PFX export password.
- `WINDOWS_TIMESTAMP_URL`: the RFC 3161 timestamp service provided by the
  certificate issuer; when omitted, the workflow uses Sectigo's public service.

The workflow imports the PFX into the ephemeral current-user certificate store,
passes only its thumbprint to a generated Tauri configuration, signs with
SHA-256 and timestamping, deletes the temporary PFX, and verifies the installer
with `Get-AuthenticodeSignature`. Without the secrets, the same workflow builds
an unsigned installer and still produces the mandatory updater signature.

Follow the certificate issuer's current instructions for EV and modern OV
certificates. Tauri's official [Windows code-signing guide](https://v2.tauri.app/distribute/sign/windows/)
documents `certificateThumbprint`, `digestAlgorithm`, `timestampUrl`, custom
sign commands, and Windows trust behavior.

Never commit a PFX, password, updater private key, or exported secret. The
updater public key in `tauri.conf.json` is public by design.

