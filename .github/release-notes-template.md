## Verifying release artifacts

All binaries are signed using [Sigstore cosign](https://docs.sigstore.dev/about/overview/). You can verify any binary was built by the official Commit-Boost CI pipeline from this release's commit.

Install cosign: https://docs.sigstore.dev/cosign/system_config/installation/

```bash
# Set the release version and your target architecture
# Architecture options: darwin_arm64, linux_arm64, linux_x86-64
export VERSION=vX.Y.Z
export ARCH=linux_x86-64

# Download the binary tarball and its signature
curl -L \
  -o "commit-boost-$VERSION-$ARCH.tar.gz" \
  "https://github.com/Commit-Boost/commit-boost-client/releases/download/$VERSION/commit-boost-$VERSION-$ARCH.tar.gz"

curl -L \
  -o "commit-boost-$VERSION-$ARCH.tar.gz.sigstore.json" \
  "https://github.com/Commit-Boost/commit-boost-client/releases/download/$VERSION/commit-boost-$VERSION-$ARCH.tar.gz.sigstore.json"

# Verify the binary was signed by the official CI pipeline
cosign verify-blob \
  "commit-boost-$VERSION-$ARCH.tar.gz" \
  --bundle "commit-boost-$VERSION-$ARCH.tar.gz.sigstore.json" \
  --certificate-oidc-issuer="https://token.actions.githubusercontent.com" \
  --certificate-identity="https://github.com/Commit-Boost/commit-boost-client/.github/workflows/release.yml@refs/tags/$VERSION"
```

A successful verification prints `Verified OK`. If the binary was modified after being built by CI, this command will fail.

The `.sigstore.json` bundle for each binary is attached to this release alongside the binary itself.
