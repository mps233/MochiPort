# macOS Signing And Notarization

Codex Remote is distributed as a Developer ID signed and notarized `.dmg`.

## Local Signing

Prerequisites:

- Apple Developer Program membership.
- A `Developer ID Application` certificate installed in Keychain Access.
- An app-specific password for your Apple ID.

Check the signing identity:

```bash
security find-identity -v -p codesigning
```

Build the app:

```bash
./scripts/build-macos-app.sh
```

Sign, notarize, staple, and create the DMG:

```bash
export APPLE_CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export NOTARY_KEYCHAIN_PROFILE="arthas-notary"

./scripts/sign-notarize-macos-app.sh
```

If you do not already have a notarytool keychain profile, create one:

```bash
xcrun notarytool store-credentials "arthas-notary" \
  --apple-id "you@example.com" \
  --team-id "TEAMID" \
  --password "xxxx-xxxx-xxxx-xxxx"
```

You can also skip the profile and use environment variables directly:

```bash
export APPLE_TEAM_ID="TEAMID"
export APPLE_ID="you@example.com"
export APPLE_APP_SPECIFIC_PASSWORD="xxxx-xxxx-xxxx-xxxx"
```

For CI, prefer App Store Connect API keys:

```bash
export APPLE_API_KEY_ID="KEYID12345"
export APPLE_API_ISSUER_ID="00000000-0000-0000-0000-000000000000"
export APPLE_API_KEY_PATH="/path/to/AuthKey_KEYID12345.p8"
```

Output:

```text
target/dist/Codex Remote.dmg
```

The app does not install LaunchAgents, login items, or shell shims. The bundled daemon only starts after the user clicks `Start Local Service`.

## GitHub Actions Secrets

Export the `Developer ID Application` certificate as a `.p12` from Keychain Access, then base64 encode it:

```bash
base64 -i DeveloperIDApplication.p12 | pbcopy
```

Configure these repository secrets:

```text
APPLE_CERTIFICATE_P12_BASE64
APPLE_CERTIFICATE_PASSWORD
APPLE_CODESIGN_IDENTITY
APPLE_TEAM_ID
APPLE_API_KEY_ID
APPLE_API_ISSUER_ID
APPLE_API_KEY_P8
KEYCHAIN_PASSWORD
```

`APPLE_CODESIGN_IDENTITY` should match the Keychain identity exactly, for example:

```text
Developer ID Application: Your Name (TEAMID)
```

`APPLE_API_KEY_P8` is the full text content of the downloaded `.p8` private key.

## Release Flow

Manual run:

```text
Actions -> release-macos -> Run workflow
```

Tagged release:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The workflow builds `target/dist/Codex Remote.app`, imports the signing certificate into a temporary keychain, signs and notarizes the app, creates a signed/notarized DMG, uploads it as an artifact, and attaches it to the GitHub Release for tag builds.
