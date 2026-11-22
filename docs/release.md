# Releases and code signing

Every push to `main` that passes CI builds the Windows installer on GitHub Actions
(`.github/workflows/release.yml`). The installer is kept as a workflow artifact for 30 days.
When the `version` in the root `Cargo.toml` hasn't been released yet, it's also published as
GitHub Release `v<version>`. **To release, bump the version.** Every other push is a test build.

Signing uses **Azure Artifact Signing** (formerly Trusted Signing). It signs `openpresenter.exe`,
`pp6tool.exe`, `libmpv-2.dll`, the installer and the uninstaller. Microsoft's certificates are
trusted by Windows, so Smart App Control and SmartScreen accept the files. Signing is off until
the repository variable `SIGNING_ENABLED` is `true`; until then the workflow builds the same
unsigned installer as `packaging/build-installer.sh`.

Cost: GitHub-hosted runners are free for public repositories. Artifact Signing Basic is
$9.99/month (5,000 signatures; each build uses 5).

## One-time setup

### 1. GitHub

- Repository settings → Environments → **New environment** `release`. Optional: under
  *Deployment branches*, allow `main` only.
- The first time, upload libmpv as a release asset (it's too big for git):

  ```sh
  gh release create deps-libmpv-0.42.0-38 packaging/vendor/libmpv-2.dll \
    --title "Build dependency: libmpv 0.42.0-38 (shinchiro)" \
    --notes "Used by the release workflow, pinned by SHA-256. Not an OpenPresenter release." --latest=false
  ```

  To update libmpv: upload a new asset under a new tag, then change `MPV_RELEASE` and
  `MPV_SHA256` in `release.yml`.

### 2. Azure: signing account and identity validation

Identity validation takes Microsoft **one to several business days**. Individual developers
must be in the USA or Canada.

1. Create an Azure account with a **pay-as-you-go** subscription (free and trial subscriptions
   aren't accepted).
2. Portal → *Subscriptions* → your subscription → *Resource providers* → register
   **Microsoft.CodeSigning**.
3. Create a resource group (e.g. `openpresenter-signing`), then an **Artifact Signing account**
   in it: pick a region (e.g. East US) and the **Basic** tier. Note the account name and the
   endpoint for the region (East US: `https://eus.codesigning.azure.net/`).
4. On your user account, add the role **Artifact Signing Identity Verifier** on the signing
   account (Access control (IAM)).
5. Signing account → *Identity validations* → **New** → *Public* → *Individual*. Follow the
   identity check (Microsoft Authenticator / ID document). Wait for **Completed**.
6. Signing account → *Certificate profiles* → **Create** → *Public Trust*, pick the
   completed validation. Note the profile name.

The certificate shows your validated name as publisher. To sign as the church instead, do the
validation as an *Organization* (the organization must be verifiable and usually needs a
few years of history).

### 3. Azure: let the workflow sign (OIDC, no stored passwords)

1. Microsoft Entra ID → *App registrations* → **New registration** `openpresenter-github`.
   Note the **Application (client) ID** and **Directory (tenant) ID**.
2. The app → *Certificates & secrets* → *Federated credentials* → **Add** → *GitHub Actions
   deploying Azure resources*: organization `<your GitHub user>`, repository `<repo>`,
   entity **Environment**, name `release`. That gives the subject
   `repo:<user>/<repo>:environment:release`: only jobs in the `release` environment can sign.
3. Signing account → *Access control (IAM)* → add role **Artifact Signing Certificate Profile
   Signer** for the app `openpresenter-github`.
4. GitHub → repository settings → *Secrets and variables* → *Actions*:
   - Secrets: `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`
   - Variables: `SIGNING_ENDPOINT` (e.g. `https://eus.codesigning.azure.net/`),
     `SIGNING_ACCOUNT`, `SIGNING_PROFILE`, and last `SIGNING_ENABLED` = `true`.
5. Actions → *Release* → **Run workflow** to check. The log lists each signed file with its
   publisher, and the run fails if any signature isn't valid.

## Checking a signed installer on Windows

Right-click the `.exe` → *Properties* → *Digital Signatures*, or in PowerShell:
`Get-AuthenticodeSignature .\OpenPresenter-Setup-….exe`.

## Local builds

`packaging/build-installer.sh` (Linux, Wine) still builds an unsigned installer for testing; see
[windows-setup.md](windows-setup.md).
