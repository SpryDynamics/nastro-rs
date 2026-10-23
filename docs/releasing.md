# Releasing

Releases are built by the `nastro-rs-release` pipeline (`azure-pipelines-release.yml`).
CI (`azure-pipelines.yml`) is separate, never signs anything, and runs on every push.

## What a release produces

| File | Contents |
| --- | --- |
| `nastro-<ver>-windows-x86_64.zip` | `nastro.exe`, `nastro-tap.exe` (**signed + timestamped**), license files, `REQUIREMENTS.txt` |
| `nastro-<ver>-linux-x86_64.tar.gz` | `nastro`, `nastro-tap`, license files, `REQUIREMENTS.txt` |
| `SHA256SUMS` | checksum of each archive (`sha256sum -c` compatible) |

Every archive includes `LICENSE`, `NOTICE.md` and `THIRD_PARTY_NOTICES.md`, so
redistribution terms travel with the binaries. `eng/package.ps1` fails if any of them,
or a binary, is missing.

FFmpeg is **not** bundled (see `NOTICE.md`). The Linux build links PulseAudio's client
library dynamically (LGPL-2.1, compliant via dynamic linking).

## Cutting a release

1. Bump `version` under `[workspace.package]` in `Cargo.toml` and move the
   `## [Unreleased]` entries in `CHANGELOG.md` under the new version.
2. `./eng/generate-notices.ps1` if dependencies changed, then commit (signed, `-s`).
3. Tag and push: `git tag -s v<version> -m "v<version>" && git push origin v<version>`.
4. The pipeline runs on the tag. It **fails** if the tag doesn't equal the `Cargo.toml`
   version.
5. Download the `release` artifact and spot-check it (below) before telling anyone.

You can also run `nastro-rs-release` by hand from `main` (Run pipeline) to get signed,
packaged artifacts without a tag. Untick *Sign the Windows binaries* to skip signing.

## Verifying a release artifact yourself

```powershell
az pipelines runs artifact download --artifact-name release --path rel --run-id <id> `
  --organization https://dev.azure.com/sprydynamics --project oss-tools
Get-Content rel\SHA256SUMS               # compare with Get-FileHash on each archive
Expand-Archive rel\nastro-*-windows-x86_64.zip rel\win
signtool verify /pa /v rel\win\nastro-*\nastro.exe   # issued to "Spry Dynamics. LLC", timestamped
```

Artifact Signing certificates are valid for **3 days**. The signature stays valid
afterwards only because it is RFC 3161 timestamped, which the pipeline always does.

## How signing is wired (and what it can reach)

- The pipeline signs through `az-artifact-sign` (pinned `0.1.0`, which pins `sign`
  `0.9.1-beta.26475.3`) using the Azure DevOps service connection
  **`nastro-artifact-signing`**.
- That connection uses **workload identity federation: there is no secret to leak or
  rotate.** It maps to the Entra app `nastro-rs-release-signing`
  (`c4736953-2452-493e-89e3-0a7337f26f16`), which has a single federated credential bound
  to that one service connection.
- The app's **only** Azure permission is *Artifact Signing Certificate Profile Signer* on
  the single certificate profile `SpryDynamicsAppSignPublic`
  (account `sd-artifact-signing`, endpoint `https://wus2.codesigning.azure.net`). It has no
  subscription- or resource-group-level role.
- Two guard rails on the service connection: only the `nastro-rs-release` pipeline is
  authorized to use it, and a **branch-control check** allows only `refs/heads/main` and
  `refs/tags/v*`. A branch or PR that edits the YAML cannot sign anything.
- `.az-sign.json` in the repo root holds the account and profile **names** (not secrets).

### Recreating the identity (e.g. in another tenant)

```powershell
az ad app create --display-name nastro-rs-release-signing --sign-in-audience AzureADMyOrg
az ad sp create --id <appId>
az role assignment create --assignee-object-id <spObjectId> --assignee-principal-type ServicePrincipal `
  --role "Artifact Signing Certificate Profile Signer" --scope <certificate-profile-resource-id>
# Create an ADO "Azure Resource Manager" service connection with workload identity
# federation (manual), then add a federated credential using the issuer and subject
# ADO shows for it:
az ad app federated-credential create --id <appObjectId> --parameters cred.json
```

## Not done

- No GitHub release or mirror yet; ADO is the only home, and the `release` pipeline
  artifact is the distribution point.
- No ARM/aarch64 builds and no macOS (out of scope for this port).
- No installer (MSI/winget/deb); the archives are portable.
